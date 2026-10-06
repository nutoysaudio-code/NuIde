use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use nuide::webserver::{Server as WebServer, WebConfig, WebEvent};

fn get(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("conectar");
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Prueba de humo real: levanta el servidor, pide un archivo, un preflight CORS
/// y un intento de path traversal.
#[test]
#[ignore = "usa un puerto local; correr con --ignored"]
fn el_servidor_local_responde_con_cors_y_bloquea_el_traversal() {
    let dir = std::env::temp_dir().join(format!("nuide-web-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<h1>hola</h1>").unwrap();
    std::fs::write(dir.join("hoja.css"), "a{color:red}").unwrap();

    let port = 18_080 + (std::process::id() % 400) as u16;
    let cfg = WebConfig {
        enabled: true,
        port,
        root_sub: String::new(),
        cors_origin: "*".into(),
        cors_methods: "GET, HEAD, OPTIONS".into(),
        cors_headers: "Content-Type".into(),
    };
    let (tx, rx) = std::sync::mpsc::channel::<WebEvent>();
    let server = WebServer::start(cfg.clone(), PathBuf::from(&dir), tx).expect("arrancar");
    let addr = format!("127.0.0.1:{port}");
    std::thread::sleep(std::time::Duration::from_millis(250));

    // 1) Un archivo existente.
    let r = get(&addr, "GET /hoja.css HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("text/css"), "{r}");
    assert!(r.contains("Access-Control-Allow-Origin: *"), "{r}");
    assert!(r.contains("a{color:red}"), "{r}");

    // 2) Directorio -> index.html.
    let r = get(&addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.contains("<h1>hola</h1>"), "{r}");

    // 3) Preflight.
    let r = get(
        &addr,
        "OPTIONS /hoja.css HTTP/1.1\r\nHost: x\r\nOrigin: https://otro.io\r\n\r\n",
    );
    assert!(r.starts_with("HTTP/1.1 204"), "{r}");
    assert!(r.contains("Access-Control-Allow-Methods"), "{r}");

    // 4) Traversal: no debe asomar.
    let r = get(&addr, "GET /../secreto HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");

    // 5) 404 con CORS igual.
    let r = get(&addr, "GET /no-esta HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
    assert!(r.contains("Access-Control-Allow-Origin"), "{r}");

    // La UI recibió las líneas de servicio.
    server.stop();
    let events: Vec<WebEvent> = rx.try_iter().collect();
    assert!(
        events.iter().any(|e| matches!(e, WebEvent::Started { .. })),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, WebEvent::Served { .. })),
        "{events:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
