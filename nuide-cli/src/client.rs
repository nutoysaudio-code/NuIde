//! Cliente HTTP de Nuide-CLI: habla con cualquier endpoint **compatible-OpenAI**
//! (`POST {base}/chat/completions`, streaming SSE) y con `GET {base}/models`.
//!
//! No agrega crates de red: usa `curl.exe` como proceso hijo (igual que el resto
//! del ecosistema). La API key se pasa por un archivo de configuración de curl
//! (`-K`), no por la línea de comandos, para no exponerla en la lista de
//! procesos.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Error del cliente. `Quota` y `Auth` disparan la rotación a otro proveedor.
#[derive(Debug)]
pub enum ClientError {
    /// Sin cuota / rate limit / crédito agotado (429, 402, "insufficient").
    Quota(String),
    /// Key inválida o sin permiso (401/403).
    Auth(String),
    /// Error transitorio del proveedor (5xx): conviene rotar.
    Server(String),
    /// Otro error HTTP.
    Http(u16, String),
    /// No se pudo hablar con el endpoint.
    Network(String),
    /// La respuesta no se pudo interpretar.
    Parse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Quota(m) => write!(f, "cuota agotada: {m}"),
            ClientError::Auth(m) => write!(f, "autenticación: {m}"),
            ClientError::Server(m) => write!(f, "error del proveedor: {m}"),
            ClientError::Http(c, m) => write!(f, "HTTP {c}: {m}"),
            ClientError::Network(m) => write!(f, "red: {m}"),
            ClientError::Parse(m) => write!(f, "respuesta inválida: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Una llamada a herramienta pedida por el modelo.
#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Resultado de un turno del asistente.
#[derive(Debug, Default)]
pub struct StreamResult {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

/// Eventos en vivo mientras llega el stream (para pintar en la TUI).
pub enum StreamEvent {
    Text(String),
    ToolCallStarted { name: String },
}

/// Un modelo anunciado por `GET /models`.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

/// Pide un turno al modelo con streaming. `on_event` se llama a medida que llega.
pub fn chat_stream(
    base_url: &str,
    api_key: &str,
    model: &str,
    messages: &[Value],
    tools: &[Value],
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<StreamResult, ClientError> {
    let url = endpoint(base_url, "chat/completions");
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    let body_file = write_temp("body", body.to_string().as_bytes());

    let mut headers: Vec<(String, String)> = vec![
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), "text/event-stream".into()),
    ];
    if !api_key.is_empty() {
        headers.push(("Authorization".into(), format!("Bearer {api_key}")));
    }

    let cfg = curl_config(&url, &headers, Some(&body_file));
    let cfg_file = write_temp("curlcfg", cfg.as_bytes());

    let result = run_stream(&cfg_file, cancel, on_event);

    let _ = std::fs::remove_file(&body_file);
    let _ = std::fs::remove_file(&cfg_file);
    result
}

/// `GET {base}/models` (para descubrir modelos; marca los gratuitos).
pub fn list_models(base_url: &str, api_key: &str) -> Result<Vec<ModelInfo>, ClientError> {
    let url = endpoint(base_url, "models");
    let mut headers: Vec<(String, String)> = vec![("Accept".into(), "application/json".into())];
    if !api_key.is_empty() {
        headers.push(("Authorization".into(), format!("Bearer {api_key}")));
    }
    let cfg = curl_config(&url, &headers, None);
    let cfg_file = write_temp("curlcfg", cfg.as_bytes());

    let out = Command::new(curl_bin())
        .arg("-K")
        .arg(&cfg_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let _ = std::fs::remove_file(&cfg_file);

    let out = out.map_err(|e| ClientError::Network(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (status, body) = split_status(&text);
    if status >= 400 {
        return Err(classify(status, &body));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| ClientError::Parse(format!("models: {e}")))?;
    let mut models = Vec::new();
    if let Some(arr) = v.get("data").and_then(Value::as_array) {
        for m in arr {
            let id = m
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            models.push(ModelInfo { id, name });
        }
    }
    Ok(models)
}

// ---------------------------------------------------------------------------
// Interno
// ---------------------------------------------------------------------------

fn run_stream(
    cfg_file: &std::path::Path,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<StreamResult, ClientError> {
    let mut child = Command::new(curl_bin())
        .arg("-K")
        .arg(cfg_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ClientError::Network(format!("no se pudo lanzar curl: {e}")))?;

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    let mut result = StreamResult::default();
    let mut status: u16 = 0;
    let mut err_body = String::new();
    let mut stream_error: Option<String> = None;
    let mut saw_sse = false;
    let mut line = Vec::new();

    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);

        if let Some(rest) = text.strip_prefix("@@HTTP") {
            status = rest.trim_end_matches("@@").trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(payload) = text.strip_prefix("data:") {
            saw_sse = true;
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            handle_chunk(payload, &mut result, &mut stream_error, on_event);
        } else if text.starts_with(':') || text.is_empty() {
            // comentario SSE o línea en blanco
        } else if !saw_sse {
            // cuerpo no-SSE (p. ej. un error JSON)
            err_body.push_str(text);
        }
    }

    let mut err = String::new();
    if let Some(mut se) = child.stderr.take() {
        let _ = se.read_to_string(&mut err);
    }
    let _ = child.wait();

    if cancel.load(Ordering::Relaxed) {
        return Err(ClientError::Network("cancelado".into()));
    }
    if status == 0 {
        let msg = if err.trim().is_empty() {
            "sin respuesta de curl".to_string()
        } else {
            err.trim().to_string()
        };
        return Err(ClientError::Network(msg));
    }
    if status >= 400 {
        let msg = if err_body.trim().is_empty() {
            err.trim().to_string()
        } else {
            err_body.trim().to_string()
        };
        return Err(classify(status, &msg));
    }
    if let Some(msg) = stream_error {
        if result.content.is_empty() && result.tool_calls.is_empty() {
            return Err(classify(200, &msg));
        }
    }
    Ok(result)
}

/// Interpreta un `data:` SSE y acumula texto y deltas de tool_calls.
fn handle_chunk(
    payload: &str,
    result: &mut StreamResult,
    stream_error: &mut Option<String>,
    on_event: &mut dyn FnMut(StreamEvent),
) {
    let Ok(v) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    if let Some(e) = v.get("error") {
        let msg = e
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| e.to_string());
        *stream_error = Some(msg);
        return;
    }
    let Some(choices) = v.get("choices").and_then(Value::as_array) else {
        return;
    };
    let Some(ch) = choices.first() else {
        return;
    };
    if let Some(fr) = ch.get("finish_reason").and_then(Value::as_str) {
        result.finish_reason = Some(fr.to_string());
    }
    let Some(delta) = ch.get("delta") else {
        return;
    };
    if let Some(c) = delta.get("content").and_then(Value::as_str) {
        if !c.is_empty() {
            result.content.push_str(c);
            on_event(StreamEvent::Text(c.to_string()));
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while result.tool_calls.len() <= index {
                result.tool_calls.push(ToolCall::default());
            }
            let slot = &mut result.tool_calls[index];
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    slot.id = id.to_string();
                }
            }
            if let Some(f) = tc.get("function") {
                if let Some(name) = f.get("name").and_then(Value::as_str) {
                    if !name.is_empty() && slot.name.is_empty() {
                        slot.name = name.to_string();
                        on_event(StreamEvent::ToolCallStarted {
                            name: name.to_string(),
                        });
                    }
                }
                if let Some(args) = f.get("arguments").and_then(Value::as_str) {
                    slot.arguments.push_str(args);
                }
            }
        }
    }
}

/// Clasifica un error HTTP (o un error embebido con código 200).
fn classify(status: u16, body: &str) -> ClientError {
    let l = body.to_lowercase();
    let quota = l.contains("insufficient")
        || l.contains("quota")
        || l.contains("rate limit")
        || l.contains("rate-limit")
        || l.contains("too many requests")
        || l.contains("credits")
        || l.contains("billing");
    if status == 429 || status == 402 || quota {
        return ClientError::Quota(body.to_string());
    }
    if status == 401 || status == 403 {
        return ClientError::Auth(body.to_string());
    }
    if (500..=599).contains(&status) {
        return ClientError::Server(body.to_string());
    }
    ClientError::Http(status, body.to_string())
}

/// Configuración de curl (`-K`). Sin `-i`; el código HTTP llega por `-w`.
fn curl_config(url: &str, headers: &[(String, String)], body_file: Option<&PathBuf>) -> String {
    let mut s = String::new();
    s.push_str("silent\nshow-error\nlocation\nno-buffer\n");
    s.push_str("request = \"POST\"\n");
    if body_file.is_none() {
        // `list_models` es un GET.
        s = s.replace("request = \"POST\"\n", "request = \"GET\"\n");
    }
    s.push_str(&format!("url = {}\n", quote(url)));
    for (k, v) in headers {
        s.push_str(&format!("header = {}\n", quote(&format!("{k}: {v}"))));
    }
    if let Some(path) = body_file {
        let fwd = path.to_string_lossy().replace('\\', "/");
        s.push_str(&format!("data-binary = {}\n", quote(&format!("@{fwd}"))));
    }
    s.push_str("max-time = 600\n");
    // El código HTTP sale al final, en su propia línea.
    s.push_str("write-out = \"\\n@@HTTP%{http_code}@@\"\n");
    s
}

/// Cadena al estilo curl-config (comillas + escapes).
fn quote(v: &str) -> String {
    let mut out = String::from("\"");
    for c in v.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), path)
}

fn curl_bin() -> String {
    std::env::var("NUIDE_CURL").unwrap_or_else(|_| "curl".to_string())
}

fn write_temp(tag: &str, bytes: &[u8]) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("nuide-cli-{tag}-{}-{nanos}", std::process::id()));
    if let Ok(mut f) = std::fs::File::create(&p) {
        let _ = f.write_all(bytes);
    }
    p
}

/// Separa el cuerpo del marcador `@@HTTP<code>@@` (para respuestas no-stream).
fn split_status(text: &str) -> (u16, String) {
    if let Some(pos) = text.rfind("@@HTTP") {
        let head = &text[..pos];
        let code = text[pos + 6..]
            .trim_end_matches("@@")
            .trim()
            .parse()
            .unwrap_or(0);
        return (code, head.trim().to_string());
    }
    // Sin marcador: asumimos cuerpo plano.
    (200, text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clasifica_errores() {
        assert!(matches!(classify(429, "slow down"), ClientError::Quota(_)));
        assert!(matches!(
            classify(400, "insufficient credits"),
            ClientError::Quota(_)
        ));
        assert!(matches!(classify(401, "bad key"), ClientError::Auth(_)));
        assert!(matches!(classify(500, "boom"), ClientError::Server(_)));
    }

    #[test]
    fn split_status_lee_marcador() {
        let (c, b) = split_status("{\"x\":1}\n@@HTTP200@@");
        assert_eq!(c, 200);
        assert_eq!(b, "{\"x\":1}");
    }

    #[test]
    fn acumula_tool_calls_del_stream() {
        let mut r = StreamResult::default();
        let mut err = None;
        let mut ev = |_e: StreamEvent| {};
        handle_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            &mut r,
            &mut err,
            &mut ev,
        );
        handle_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]}}]}"#,
            &mut r,
            &mut err,
            &mut ev,
        );
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "read_file");
        assert_eq!(r.tool_calls[0].id, "c1");
        assert_eq!(r.tool_calls[0].arguments, r#"{"path":"a"}"#);
    }
}
