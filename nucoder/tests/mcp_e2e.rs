//! Test end-to-end del cliente MCP contra el servidor de prueba `mcp_stub`.
//!
//! Cargo expone la ruta del binario de fixtures en `CARGO_BIN_EXE_mcp_stub`.

use std::time::Duration;

use nucoder::mcp::{McpClient, McpServer};
use serde_json::json;

fn stub() -> McpServer {
    McpServer {
        id: "stub".to_string(),
        command: env!("CARGO_BIN_EXE_mcp_stub").to_string(),
        ..McpServer::default()
    }
}

#[test]
fn handshake_lista_herramientas_y_llama() {
    let mut client = McpClient::spawn(&stub()).expect("spawn del stub");
    // `serverInfo.name` del handshake.
    assert_eq!(client.server_name, "stub");
    // `tools/list`.
    let names: Vec<&str> = client.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"), "tools: {names:?}");
    assert!(names.contains(&"boom"));
    // `tools/call` con resultado de texto.
    let out = client
        .call("echo", &json!({ "text": "hola" }), Duration::from_secs(10))
        .expect("call echo");
    assert!(out.contains("eco: hola"), "salida: {out}");
    client.shutdown();
}

#[test]
fn una_herramienta_con_is_error_devuelve_error() {
    let mut client = McpClient::spawn(&stub()).expect("spawn del stub");
    let err = client
        .call("boom", &json!({}), Duration::from_secs(10))
        .expect_err("boom debe fallar");
    assert!(err.contains("explota"), "error: {err}");
    client.shutdown();
}

#[test]
fn comando_inexistente_falla_al_lanzar() {
    let bad = McpServer {
        id: "bad".to_string(),
        command: "no-existe-este-binario-xyz".to_string(),
        ..McpServer::default()
    };
    assert!(McpClient::spawn(&bad).is_err());
}

#[test]
fn el_hub_conecta_una_vez_expone_y_llama() {
    use nucoder::mcp::McpHub;

    let mut hub = McpHub::default();
    let srv = stub();
    let notices = hub.ensure(std::slice::from_ref(&srv));
    assert!(notices.iter().any(|n| n.contains("stub")), "{notices:?}");
    assert_eq!(hub.ids(), vec!["stub".to_string()]);

    let names = |hub: &McpHub, s: &McpServer| -> Vec<String> {
        hub.specs(std::slice::from_ref(s))
            .iter()
            .filter_map(|v| {
                v.pointer("/function/name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
            .collect()
    };
    assert!(names(&hub, &srv).contains(&"stub__echo".to_string()));

    // Segunda vez: mismo fingerprint ⇒ no relanza ni avisa (conexión persistente).
    assert!(hub.ensure(std::slice::from_ref(&srv)).is_empty());

    // Llamada a través del hub.
    let out = hub.call("stub", "echo", &json!({ "text": "x" })).unwrap();
    assert!(out.contains("eco: x"));

    // Filtro `tools`.
    let mut filtered = stub();
    filtered.tools = vec!["echo".to_string()];
    let mut hub2 = McpHub::default();
    hub2.ensure(std::slice::from_ref(&filtered));
    assert_eq!(names(&hub2, &filtered), vec!["stub__echo".to_string()]);
}
