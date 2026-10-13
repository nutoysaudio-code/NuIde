//! Cliente **MCP** (Model Context Protocol) de NuCoder.
//!
//! Habla JSON-RPC 2.0 sobre **stdio** con un servidor MCP lanzado como proceso
//! hijo (`npx -y rea-agents@6.4.0 mcp`, etc.). Expone:
//!
//! 1. [`McpClient::spawn`] — lanza, hace `initialize` → `notifications/initialized`
//!    → `tools/list` y deja las herramientas listas.
//! 2. [`McpClient::call`] — `tools/call` y devuelve el texto.
//!
//! Sin crates nuevas: `serde_json` + `std::process` (vía [`crate::proc::hidden`]
//! para no abrir consola en Windows). Un hilo lee el `stdout` línea por línea
//! (MCP stdio usa **JSON por línea**) y lo manda por canal; las peticiones
//! esperan la respuesta con el `id` que les corresponde.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// Versión del protocolo que anunciamos (MCP 2024-11-05).
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// Tiempo máximo para `initialize`/`tools/list`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Tiempo máximo para una llamada a herramienta (REA puede tardar: decompilación).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Un servidor MCP configurado.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    /// Variables de entorno para el proceso (p. ej. `GHIDRA_INSTALL_DIR`).
    pub env: Vec<(String, String)>,
    /// Filtro glob de herramientas a exponer (vacío = todas). REA trae ~170:
    /// conviene listar las que se usan para no inflar el prompt.
    pub tools: Vec<String>,
    pub enabled: bool,
    /// Pedir confirmación antes de cada `tools/call` (los que ejecutan cosas).
    pub confirm: bool,
}

impl Default for McpServer {
    fn default() -> Self {
        McpServer {
            id: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            tools: Vec::new(),
            enabled: true,
            confirm: true,
        }
    }
}

impl McpServer {
    /// Construye desde la entrada de config (`{ "command": …, "args": [...] }`).
    pub fn from_value(id: &str, v: &Value) -> Option<Self> {
        let command = v.get("command").and_then(Value::as_str)?.trim().to_string();
        if command.is_empty() {
            return None;
        }
        let args = v
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let env = v
            .get("env")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let tools = v
            .get("tools")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Some(McpServer {
            id: id.to_string(),
            command,
            args,
            env,
            tools,
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            confirm: v.get("confirm").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    /// JSON para persistir en la config.
    pub fn to_value(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        json!({
            "command": self.command,
            "args": self.args,
            "env": env,
            "tools": self.tools,
            "enabled": self.enabled,
            "confirm": self.confirm,
        })
    }
}

/// Una herramienta expuesta por un servidor MCP.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Cliente de un servidor MCP por stdio.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: i64,
    /// Nombre reportado por el server (`serverInfo.name`), si lo dio.
    pub server_name: String,
    /// Herramientas descubiertas con `tools/list`.
    pub tools: Vec<McpTool>,
    /// Pedir confirmación antes de cada `tools/call`.
    pub confirm: bool,
}

impl McpClient {
    /// Lanza el server y hace el *handshake* + `tools/list`.
    pub fn spawn(server: &McpServer) -> Result<Self, String> {
        let mut cmd = crate::proc::hidden(&server.command);
        cmd.args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &server.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("no se pudo lanzar `{}`: {e}", server.command))?;
        let stdin = child.stdin.take().ok_or("el proceso no tiene stdin")?;
        let stdout = child.stdout.take().ok_or("el proceso no tiene stdout")?;
        let (tx, rx) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(t) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let mut client = McpClient {
            child,
            stdin,
            rx,
            next_id: 1,
            server_name: server.id.clone(),
            tools: Vec::new(),
            confirm: server.confirm,
        };
        client.handshake()?;
        Ok(client)
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        let line = format!("{msg}\n");
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("escribiendo al server MCP: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("vacando stdin del server MCP: {e}"))
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("timeout en `{method}`"));
            }
            match self.rx.recv_timeout(remaining) {
                Ok(v) => {
                    if v.get("id").and_then(Value::as_i64) == Some(id) {
                        if let Some(err) = v.get("error") {
                            return Err(format!("{method}: {err}"));
                        }
                        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // Otro mensaje (notificación o respuesta vieja): se ignora.
                }
                Err(_) => return Err(format!("timeout en `{method}`")),
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn handshake(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "nucoder", "version": env!("CARGO_PKG_VERSION") },
        });
        let res = self.request("initialize", params, HANDSHAKE_TIMEOUT)?;
        if let Some(name) = res.pointer("/serverInfo/name").and_then(Value::as_str) {
            self.server_name = name.to_string();
        }
        self.notify("notifications/initialized", json!({}))?;
        self.load_tools()
    }

    /// (Re)lee `tools/list`.
    pub fn load_tools(&mut self) -> Result<(), String> {
        let res = self.request("tools/list", json!({}), HANDSHAKE_TIMEOUT)?;
        let tools = res
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.tools = tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name").and_then(Value::as_str)?.to_string();
                let description = t
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let schema = t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                Some(McpTool {
                    name,
                    description,
                    schema,
                })
            })
            .collect();
        Ok(())
    }

    /// Llama a una herramienta y devuelve el texto del resultado.
    pub fn call(&mut self, tool: &str, args: &Value, timeout: Duration) -> Result<String, String> {
        let params = json!({ "name": tool, "arguments": args });
        let res = self.request("tools/call", params, timeout)?;
        let is_err = res.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let mut text = String::new();
        if let Some(arr) = res.get("content").and_then(Value::as_array) {
            for part in arr {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => push_text(&mut text, part.get("text")),
                    // Un recurso embebido puede traer texto.
                    Some("resource") => push_text(&mut text, part.pointer("/resource/text")),
                    _ => {}
                }
            }
        }
        if text.trim().is_empty() {
            text = res.to_string();
        }
        if is_err {
            Err(format!("tool `{tool}`: {}", text.trim()))
        } else {
            Ok(text)
        }
    }

    /// Termina el proceso.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn push_text(out: &mut String, v: Option<&Value>) {
    if let Some(s) = v.and_then(Value::as_str) {
        out.push_str(s);
        out.push('\n');
    }
}

/// Un cliente conectado y la huella de la config con la que se lanzó.
struct HubEntry {
    fingerprint: String,
    client: McpClient,
}

/// Mantiene **conexiones vivas** a los servidores MCP entre turnos: sólo
/// (re)conecta los que faltan o cambiaron y cierra los que se deshabilitaron.
#[derive(Default)]
pub struct McpHub {
    clients: HashMap<String, HubEntry>,
}

impl McpHub {
    /// Huella de config: si cambia, el server se relanza.
    fn fingerprint(s: &McpServer) -> String {
        let env: Vec<String> = s.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!(
            "{}\u{1}{}\u{1}{}\u{1}{}",
            s.command,
            s.args.join("\u{1}"),
            env.join("\u{1}"),
            s.tools.join("\u{1}")
        )
    }

    /// Asegura que los servers `enabled` estén conectados: lanza los que faltan o
    /// cambiaron y cierra los que ya no corresponden. Devuelve avisos para la UI.
    pub fn ensure(&mut self, servers: &[McpServer]) -> Vec<String> {
        let mut notices = Vec::new();
        let wanted: Vec<&McpServer> = servers
            .iter()
            .filter(|s| s.enabled && !s.command.trim().is_empty())
            .collect();
        let ids: Vec<&str> = wanted.iter().map(|s| s.id.as_str()).collect();
        // Cerrar los que ya no están habilitados / se borraron.
        let stale: Vec<String> = self
            .clients
            .keys()
            .filter(|id| !ids.contains(&id.as_str()))
            .cloned()
            .collect();
        for id in stale {
            if let Some(e) = self.clients.remove(&id) {
                notices.push(format!("mcp `{id}`: desconectado"));
                drop(e);
            }
        }
        for s in wanted {
            let fp = Self::fingerprint(s);
            let changed = self
                .clients
                .get(&s.id)
                .map(|e| e.fingerprint != fp)
                .unwrap_or(true);
            if !changed {
                continue;
            }
            if self.clients.remove(&s.id).is_some() {
                // El cliente viejo se apaga al soltarse (`Drop`).
            }
            match McpClient::spawn(s) {
                Ok(client) => {
                    notices.push(format!(
                        "mcp `{}`: {} herramienta(s)",
                        s.id,
                        client.tools.len()
                    ));
                    self.clients.insert(
                        s.id.clone(),
                        HubEntry {
                            fingerprint: fp,
                            client,
                        },
                    );
                }
                Err(e) => notices.push(format!("mcp `{}`: {e}", s.id)),
            }
        }
        notices
    }

    /// Ids de los servers conectados.
    pub fn ids(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Especificaciones de herramientas (con prefijo `<id>__`) para el modelo,
    /// aplicando el filtro `tools` de cada server.
    pub fn specs(&self, servers: &[McpServer]) -> Vec<Value> {
        let mut out = Vec::new();
        for s in servers {
            let Some(entry) = self.clients.get(&s.id) else {
                continue;
            };
            for t in &entry.client.tools {
                if !s.tools.is_empty()
                    && !s.tools.iter().any(|g| crate::tools::glob_match(g, &t.name))
                {
                    continue;
                }
                out.push(json!({
                    "type": "function",
                    "function": {
                        "name": format!("{}__{}", s.id, t.name),
                        "description": format!("[{}·mcp] {}", s.id, t.description),
                        "parameters": t.schema,
                    }
                }));
            }
        }
        out
    }

    /// Llama a una herramienta del server `server_id`.
    pub fn call(&mut self, server_id: &str, tool: &str, args: &Value) -> Result<String, String> {
        let entry = self
            .clients
            .get_mut(server_id)
            .ok_or_else(|| format!("el servidor mcp `{server_id}` no está conectado"))?;
        entry.client.call(tool, args, CALL_TIMEOUT)
    }

    /// Cierra todas las conexiones.
    pub fn shutdown(&mut self) {
        self.clients.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_servidor_de_config() {
        let v = json!({
            "command": "npx",
            "args": ["-y", "rea-agents@6.4.0", "mcp"],
            "env": { "GHIDRA_INSTALL_DIR": "C:/ghidra" },
            "tools": ["open_binary", "inspect_*"],
            "confirm": false,
        });
        let s = McpServer::from_value("rea", &v).unwrap();
        assert_eq!(s.id, "rea");
        assert_eq!(s.command, "npx");
        assert_eq!(s.args.len(), 3);
        assert_eq!(
            s.env,
            vec![("GHIDRA_INSTALL_DIR".to_string(), "C:/ghidra".to_string())]
        );
        assert_eq!(s.tools, vec!["open_binary", "inspect_*"]);
        assert!(s.enabled);
        assert!(!s.confirm);
        // Round-trip de persistencia.
        let back = McpServer::from_value("rea", &s.to_value()).unwrap();
        assert_eq!(back.command, "npx");
        assert_eq!(back.args, s.args);
        assert_eq!(back.tools, s.tools);
        assert!(!back.confirm);
    }

    #[test]
    fn servidor_sin_command_es_none() {
        assert!(McpServer::from_value("x", &json!({ "args": [] })).is_none());
        assert!(McpServer::from_value("x", &json!({ "command": "  " })).is_none());
    }
}
