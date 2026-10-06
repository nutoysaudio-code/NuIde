//! Servidor web local del proyecto, con CORS.
//!
//! Módulo **opt-in**: se prende desde Preferencias (`web.enabled`) y sirve la
//! carpeta elegida del proyecto en `http://localhost:<puerto>/`. Sirve para ver
//! el sitio como lo vería un navegador antes de subirlo, y el CORS explícito es
//! justo lo que hace falta cuando la página llama a una API de otro origen.
//!
//! Sin crates: `std::net` + HTTP/1.1 a mano. El parseo y la respuesta son
//! funciones puras ([`parse_request`], [`respond`]) para poder probarlas sin
//! abrir un socket; el hilo sólo acepta conexiones y las atiende.
//!
//! Seguridad: sólo se sirve lo que cae **dentro** de la raíz (`sanitize` resuelve
//! `..` y rechaza escapes), y la contraseña de ningún sitio se toca acá.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Puerto por defecto del servidor local.
pub const DEFAULT_PORT: u16 = 8080;

/// Configuración del módulo (viene del config de NuIde, por proyecto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    pub enabled: bool,
    pub port: u16,
    /// Carpeta servida, relativa a la raíz del proyecto ("" = la raíz).
    pub root_sub: String,
    /// Valor de `Access-Control-Allow-Origin` (`*` o un origen concreto).
    pub cors_origin: String,
    /// `Access-Control-Allow-Methods`.
    pub cors_methods: String,
    /// `Access-Control-Allow-Headers`.
    pub cors_headers: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            root_sub: String::new(),
            cors_origin: "*".to_string(),
            cors_methods: "GET, HEAD, OPTIONS".to_string(),
            cors_headers: "Content-Type, Authorization, X-Requested-With".to_string(),
        }
    }
}

impl WebConfig {
    /// Clave del config indexada por proyecto (como el sitio de despliegue).
    fn key(project: &Path, field: &str) -> String {
        format!("web.{}.{field}", project.display())
    }

    pub fn load(project: &Path, cfg: &crate::config::Config) -> WebConfig {
        let get = |f: &str, def: &str| {
            cfg.get(&Self::key(project, f))
                .map(str::to_string)
                .unwrap_or_else(|| def.to_string())
        };
        WebConfig {
            enabled: matches!(
                cfg.get(&Self::key(project, "enabled")).map(str::trim),
                Some("1") | Some("true")
            ),
            port: cfg
                .get(&Self::key(project, "port"))
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(DEFAULT_PORT),
            root_sub: get("root", ""),
            cors_origin: get("cors", "*"),
            cors_methods: get("cors-methods", "GET, HEAD, OPTIONS"),
            cors_headers: get(
                "cors-headers",
                "Content-Type, Authorization, X-Requested-With",
            ),
        }
    }

    pub fn save(&self, project: &Path, cfg: &mut crate::config::Config) {
        let mut put = |f: &str, v: String| {
            let k = Self::key(project, f);
            cfg.set(&k, v);
        };
        put("enabled", if self.enabled { "1" } else { "0" }.to_string());
        put("port", self.port.to_string());
        put("root", self.root_sub.clone());
        put("cors", self.cors_origin.clone());
        put("cors-methods", self.cors_methods.clone());
        put("cors-headers", self.cors_headers.clone());
    }

    /// Raíz absoluta que se sirve.
    pub fn root(&self, project: &Path) -> PathBuf {
        let sub = self.root_sub.trim().replace('\\', "/");
        let sub = sub.trim_matches('/');
        if sub.is_empty() {
            project.to_path_buf()
        } else {
            project.join(sub.replace('/', std::path::MAIN_SEPARATOR_STR))
        }
    }

    /// URL para mostrar en la barra de estado.
    pub fn url(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }
}

/// Cabeceras de respuesta (nombre ya capitalizado como las manda el servidor).
pub type Headers = BTreeMap<String, String>;

/// Petición mínima que entendemos: método, camino y cabeceras sueltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Headers,
}

/// Respuesta ya serializada, lista para escribir en el socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    fn text(status: u16, reason: &'static str, body: &str) -> Response {
        let mut headers = Headers::new();
        headers.insert("Content-Type".into(), "text/plain; charset=utf-8".into());
        Response {
            status,
            reason,
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    /// Serializa `HTTP/1.1 ...` con `Content-Length` y `Connection: close`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason);
        for (k, v) in &self.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        out.push_str("Connection: close\r\n\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// Lee la petición. `None` si la línea inicial no es válida.
pub fn parse_request(stream: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    if stream.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let path = parts.next()?;
    if method.is_empty() || path.is_empty() {
        return None;
    }
    let mut headers = Headers::new();
    loop {
        let mut h = String::new();
        if stream.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some(Request {
        method,
        path: path.to_string(),
        headers,
    })
}

/// Quita la query (`/a.css?v=2` → `/a.css`) y decodifica `%XX` y `+`.
pub fn clean_path(raw: &str) -> String {
    let base = raw.split_once('?').map(|(a, _)| a).unwrap_or(raw);
    let mut out = String::with_capacity(base.len());
    let mut it = base.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '%' => {
                let hi = it.next();
                let lo = it.next();
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        if let Ok(v) = u8::from_str_radix(&format!("{h}{l}"), 16) {
                            out.push(v as char);
                        }
                    }
                    // Un `%` suelto al final se deja tal cual.
                    (Some(h), None) => {
                        out.push('%');
                        out.push(h);
                    }
                    (None, _) => out.push('%'),
                }
            }
            '+' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Resuelve `p` dentro de `root`, o `None` si escapa (path traversal).
/// Se hace **léxicamente** (sin tocar el disco) para poder probarlo y para no
/// seguir enlaces fuera de la raíz.
pub fn sanitize(root: &Path, p: &str) -> Option<PathBuf> {
    let decoded = clean_path(p);
    let trimmed = decoded.trim_start_matches('/');
    let mut acc = root.to_path_buf();
    let mut depth = 0usize;
    for part in trimmed.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                if depth == 0 {
                    return None; // quiso salir de la raíz
                }
                acc.pop();
                depth -= 1;
            }
            other => {
                // Un separador embebido o un nombre con unidad (`C:`) no valen.
                if other.contains('\\') || other.contains(':') {
                    return None;
                }
                acc.push(other);
                depth += 1;
            }
        }
    }
    // Raíz con componentes `..` ya resueltos.
    let normalized: PathBuf = acc
        .components()
        .filter(|c| !matches!(c, Component::ParentDir))
        .collect();
    Some(normalized)
}

/// Tipo MIME por extensión. Suficiente para un sitio web estático.
pub fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
}

/// Cabeceras CORS para la configuración dada.
pub fn cors_headers(cfg: &WebConfig) -> Headers {
    let mut h = Headers::new();
    h.insert(
        "Access-Control-Allow-Origin".into(),
        cfg.cors_origin.clone(),
    );
    h.insert(
        "Access-Control-Allow-Methods".into(),
        cfg.cors_methods.clone(),
    );
    h.insert(
        "Access-Control-Allow-Headers".into(),
        cfg.cors_headers.clone(),
    );
    // Con credenciales, `*` no sirve: el navegador lo rechaza.
    if cfg.cors_origin != "*" {
        h.insert("Access-Control-Allow-Credentials".into(), "true".into());
        h.insert("Vary".into(), "Origin".into());
    }
    h
}

/// Arma la respuesta para una petición. Puro: lee del disco pero no escribe.
pub fn respond(root: &Path, cfg: &WebConfig, req: &Request) -> Response {
    // Preflight: se contesta siempre con CORS, sin mirar el archivo.
    if req.method == "OPTIONS" {
        let mut headers = cors_headers(cfg);
        headers.insert("Access-Control-Max-Age".into(), "600".into());
        return Response {
            status: 204,
            reason: "No Content",
            headers,
            body: Vec::new(),
        };
    }
    if req.method != "GET" && req.method != "HEAD" {
        let mut r = Response::text(405, "Method Not Allowed", "método no permitido");
        r.headers.extend(cors_headers(cfg));
        return r;
    }

    let Some(target) = sanitize(root, &req.path) else {
        let mut r = Response::text(403, "Forbidden", "fuera de la raíz");
        r.headers.extend(cors_headers(cfg));
        return r;
    };

    // Directorio: se intenta `index.html` adentro.
    let file = if target.is_dir() {
        target.join("index.html")
    } else {
        target.clone()
    };
    let mut headers = cors_headers(cfg);
    match std::fs::read(&file) {
        Ok(body) => {
            headers.insert("Content-Type".into(), mime_for(&file).into());
            Response {
                status: 200,
                reason: "OK",
                headers,
                body: if req.method == "HEAD" {
                    Vec::new()
                } else {
                    body
                },
            }
        }
        Err(_) => {
            let mut r = Response::text(404, "Not Found", "no existe");
            r.headers.extend(headers);
            r
        }
    }
}

/// Lo que el servidor le avisa a la UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebEvent {
    /// Arrancó (o no) en tal puerto/URL.
    Started { url: String },
    /// Error para levantar el servidor.
    StartFailed { message: String },
    /// Se detuvo.
    Stopped,
    /// `MÉTODO /camino -> 200 (1234 B)`
    Served { line: String },
}

/// Un servidor en marcha: el flag para detenerlo y el handle del hilo.
pub struct Server {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    /// Levanta el servidor. El `bind` se hace acá para poder reportar el error
    /// (puerto ocupado) en lugar de perderlo en el hilo.
    pub fn start(cfg: WebConfig, root: PathBuf, tx: Sender<WebEvent>) -> Result<Server, String> {
        let listener = TcpListener::bind(("127.0.0.1", cfg.port))
            .map_err(|e| format!("puerto {}: {e}", cfg.port))?;
        // El listener se pone **no bloqueante** y se sondea con una pausa corta:
        // así el hilo puede mirar el flag de parada. Con `accept()` bloqueante,
        // `stop()` se colgaba para siempre esperando al hilo (lo mostró la
        // prueba end-to-end).
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("no blocking: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let url = cfg.url();
        // Se avisa antes de mover el canal al hilo.
        let _ = tx.send(WebEvent::Started { url });
        let handle = thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        let cfg = cfg.clone();
                        let root = root.clone();
                        let tx = tx.clone();
                        // Una conexión, una petición: HTTP/1.1 con `Connection: close`.
                        let _ = thread::spawn(move || serve_one(stream, &cfg, &root, &tx));
                    }
                    // Nada esperando: se vuelve a mirar el flag tras la pausa.
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
            let _ = tx.send(WebEvent::Stopped);
        });
        Ok(Server {
            stop,
            handle: Some(handle),
        })
    }

    /// Pide la parada y espera al hilo.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Atiende una conexión: parsea, responde y cierra.
fn serve_one(mut stream: TcpStream, cfg: &WebConfig, root: &Path, tx: &Sender<WebEvent>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let Some(req) = parse_request(&mut reader) else {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    };
    let response = respond(root, cfg, &req);
    let line = format!(
        "{} {} -> {} ({} B)",
        req.method,
        req.path,
        response.status,
        response.body.len()
    );
    let _ = stream.write_all(&response.to_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
    let _ = tx.send(WebEvent::Served { line });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(if cfg!(windows) { "C:/sitio" } else { "/sitio" }).to_path_buf()
    }

    fn cfg() -> WebConfig {
        WebConfig::default()
    }

    #[test]
    fn sanitize_mantiene_las_rutas_dentro_de_la_raiz() {
        assert_eq!(
            sanitize(&root(), "index.html"),
            Some(root().join("index.html"))
        );
        assert_eq!(
            sanitize(&root(), "/css/site.css"),
            Some(root().join("css").join("site.css"))
        );
        // Traversal: adentro y afuera.
        assert_eq!(sanitize(&root(), "/a/../b.txt"), Some(root().join("b.txt")));
        assert_eq!(sanitize(&root(), "/../../etc/passwd"), None);
        assert_eq!(sanitize(&root(), "/a/../../b"), None);
        // Rutas con unidad o separador embebido no se cuelan.
        assert_eq!(sanitize(&root(), "/C:/windows"), None);
        assert_eq!(sanitize(&root(), "/a\\b"), None);
    }

    #[test]
    fn clean_path_quita_la_query_y_decodifica() {
        assert_eq!(clean_path("/a.css?v=2"), "/a.css");
        assert_eq!(clean_path("/hola%20mundo"), "/hola mundo");
        assert_eq!(clean_path("/a+b"), "/a b");
    }

    #[test]
    fn las_respuestas_llevan_cors_y_content_type() {
        let r = respond(
            &root(),
            &cfg(),
            &Request {
                method: "GET".into(),
                path: "/x.json".into(),
                headers: Headers::new(),
            },
        );
        // El archivo no existe en la máquina de pruebas: 404, pero con CORS.
        assert_eq!(r.status, 404);
        assert_eq!(
            r.headers
                .get("Access-Control-Allow-Origin")
                .map(String::as_str),
            Some("*")
        );

        // Un archivo real del disco de pruebas.
        let dir = std::env::temp_dir().join(format!("nuide-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hoja.css"), "a{color:red}").unwrap();
        let r = respond(
            &dir,
            &cfg(),
            &Request {
                method: "GET".into(),
                path: "/hoja.css".into(),
                headers: Headers::new(),
            },
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            r.headers.get("Content-Type").map(String::as_str),
            Some("text/css; charset=utf-8")
        );
        assert_eq!(r.body, b"a{color:red}");

        // HEAD no manda cuerpo.
        let r = respond(
            &dir,
            &cfg(),
            &Request {
                method: "HEAD".into(),
                path: "/hoja.css".into(),
                headers: Headers::new(),
            },
        );
        assert!(r.body.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_preflight_se_responde_sin_mirar_el_archivo() {
        let r = respond(
            &root(),
            &cfg(),
            &Request {
                method: "OPTIONS".into(),
                path: "/cualquiera".into(),
                headers: Headers::new(),
            },
        );
        assert_eq!(r.status, 204);
        assert!(r.headers.contains_key("Access-Control-Allow-Methods"));
        assert!(r.headers.contains_key("Access-Control-Max-Age"));
    }

    #[test]
    fn cors_con_origen_concreto_permite_credenciales() {
        let mut c = cfg();
        c.cors_origin = "https://midominio.com".into();
        let h = cors_headers(&c);
        assert_eq!(
            h.get("Access-Control-Allow-Credentials")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(h.get("Vary").map(String::as_str), Some("Origin"));
        // Con `*` el navegador rechaza credenciales: no se manda la cabecera.
        let h = cors_headers(&cfg());
        assert!(!h.contains_key("Access-Control-Allow-Credentials"));
    }

    #[test]
    fn metodos_desconocidos_y_traversal_tienen_codigo_apropiado() {
        let r = respond(
            &root(),
            &cfg(),
            &Request {
                method: "POST".into(),
                path: "/".into(),
                headers: Headers::new(),
            },
        );
        assert_eq!(r.status, 405);
        let r = respond(
            &root(),
            &cfg(),
            &Request {
                method: "GET".into(),
                path: "/../secreto".into(),
                headers: Headers::new(),
            },
        );
        assert_eq!(r.status, 403);
    }

    #[test]
    fn la_serializacion_de_la_respuesta_es_http_valido() {
        let r = respond(
            &root(),
            &cfg(),
            &Request {
                method: "GET".into(),
                path: "/no-esta".into(),
                headers: Headers::new(),
            },
        );
        let text = String::from_utf8(r.to_bytes()).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"), "{text}");
        assert!(text.contains("Content-Length: 9\r\n"), "{text}");
        assert!(
            text.contains("Connection: close\r\n\r\nno existe"),
            "{text}"
        );
    }

    #[test]
    fn el_directorio_resuelve_a_index_html() {
        let dir = std::env::temp_dir().join(format!("nuide-web-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>h</h1>").unwrap();
        let r = respond(
            &dir,
            &cfg(),
            &Request {
                method: "GET".into(),
                path: "/".into(),
                headers: Headers::new(),
            },
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            r.headers.get("Content-Type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_request_lee_metodo_camino_y_cabeceras() {
        let raw = b"GET /a/b.css?v=1 HTTP/1.1\r\nHost: localhost\r\nOrigin: https://x.io\r\n\r\n";
        let mut cursor = std::io::Cursor::new(&raw[..]);
        let req = parse_request(&mut cursor).expect("petición válida");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/a/b.css?v=1");
        assert_eq!(
            req.headers.get("origin").map(String::as_str),
            Some("https://x.io")
        );
        assert_eq!(
            req.headers.get("host").map(String::as_str),
            Some("localhost")
        );

        // Basura: no es petición.
        let mut bad = std::io::Cursor::new(&b"\r\n"[..]);
        assert!(parse_request(&mut bad).is_none());
    }

    #[test]
    fn el_puerto_y_la_raiz_se_resuelven() {
        let mut c = cfg();
        assert_eq!(c.url(), "http://localhost:8080/");
        c.root_sub = "/build/".into();
        assert_eq!(
            c.root(Path::new("F:/proy")),
            PathBuf::from("F:/proy").join("build")
        );
        assert_eq!(cfg().root(Path::new("F:/proy")), PathBuf::from("F:/proy"));
    }
}
