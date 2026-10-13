//! Servidor **MCP mínimo de prueba** (stdio) para los tests de `mcp.rs`.
//!
//! No se publica: es el `.exe` de fixtures que `tests/mcp_e2e.rs` lanza como
//! proceso hijo para verificar el *handshake*, `tools/list` y `tools/call`.
//!
//! Herramientas:
//! - `echo { text }` → `eco: <text>`.
//! - `boom` → error (`isError: true`), para probar el camino de fallo.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "stub", "version": "0.0.1" }
            })),
            "tools/list" => Some(json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Devuelve el texto recibido",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string" } },
                            "required": ["text"]
                        }
                    },
                    {
                        "name": "boom",
                        "description": "Falla siempre",
                        "inputSchema": { "type": "object", "properties": {} }
                    }
                ]
            })),
            "tools/call" => {
                let tool = msg
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match tool {
                    "echo" => {
                        let text = msg
                            .pointer("/params/arguments/text")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        Some(json!({
                            "content": [{ "type": "text", "text": format!("eco: {text}") }]
                        }))
                    }
                    "boom" => Some(json!({
                        "content": [{ "type": "text", "text": "explota" }],
                        "isError": true
                    })),
                    _ => Some(json!({
                        "content": [{ "type": "text", "text": format!("? {tool}") }]
                    })),
                }
            }
            _ => None,
        };
        // Las notificaciones (sin `id`) se ignoran.
        if let (Some(id), Some(result)) = (id, result) {
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
}
