//! Bucle del agente: rutea, pide al modelo, ejecuta herramientas y **rota de
//! proveedor** cuando se agota la cuota o la key es inválida — sin perder la
//! conversación (que es sólo una lista de mensajes que se reenvía tal cual).
//!
//! Corre en un hilo; se comunica con la UI por un canal de `AgentEvent`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::client::{self, ClientError, StreamEvent};
use crate::config::Config;
use crate::tools;

/// Eventos que el agente manda a la UI.
#[derive(Debug)]
pub enum AgentEvent {
    /// Se eligió un proveedor/modelo para el turno.
    Routed { label: String, free: bool },
    /// Texto del asistente (streaming).
    Delta(String),
    /// El asistente pidió una herramienta.
    ToolStart { name: String, args: String },
    /// Resultado de una herramienta.
    ToolResult { name: String, text: String },
    /// Aviso (rotación, info).
    Notice(String),
    /// Error fatal del turno.
    Error(String),
    /// Fin del turno; devuelve la conversación actualizada.
    Done { conversation: Vec<Value> },
}

/// Configuración de una corrida del agente.
pub struct Agent {
    pub config: Arc<Config>,
    pub cwd: PathBuf,
    pub task: String,
    pub free_only: bool,
    pub cancel: Arc<AtomicBool>,
}

impl Agent {
    /// Corre hasta que el modelo deja de pedir herramientas (o falla).
    pub fn run(&self, mut conversation: Vec<Value>, tx: Sender<AgentEvent>) {
        let tools_spec = tools::tool_specs();
        let mut excluded: Vec<usize> = Vec::new();

        loop {
            if self.cancel.load(Ordering::Relaxed) {
                let _ = tx.send(AgentEvent::Notice("cancelado".into()));
                break;
            }
            let Some(cand) = self.config.route(&self.task, &excluded, self.free_only) else {
                let msg = if excluded.is_empty() {
                    "sin proveedores configurados (cargá una API key gratuita)".to_string()
                } else {
                    "todos los proveedores quedaron sin cuota".to_string()
                };
                let _ = tx.send(AgentEvent::Error(msg));
                break;
            };
            let provider = &self.config.providers[cand.provider_idx];
            let model = &provider.models[cand.model_idx];
            let _ = tx.send(AgentEvent::Routed {
                label: cand.label.clone(),
                free: cand.free,
            });

            let mut on_event = |ev: StreamEvent| match ev {
                StreamEvent::Text(t) => {
                    let _ = tx.send(AgentEvent::Delta(t));
                }
                StreamEvent::ToolCallStarted { name } => {
                    let _ = tx.send(AgentEvent::Notice(format!("→ {name}")));
                }
            };

            match client::chat_stream(
                &provider.base_url,
                &provider.api_key,
                &model.id,
                &conversation,
                &tools_spec,
                &self.cancel,
                &mut on_event,
            ) {
                Ok(res) => {
                    let calls = normalize_calls(&res.tool_calls);
                    conversation.push(assistant_message(&res.content, &calls));
                    if calls.is_empty() {
                        break;
                    }
                    for (id, name, arguments) in &calls {
                        let args: Value =
                            serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                        let _ = tx.send(AgentEvent::ToolStart {
                            name: name.clone(),
                            args: arguments.clone(),
                        });
                        let out = tools::execute(name, &args, &self.cwd);
                        let _ = tx.send(AgentEvent::ToolResult {
                            name: name.clone(),
                            text: out.clone(),
                        });
                        conversation.push(json!({
                            "role": "tool",
                            "tool_call_id": id,
                            "content": out,
                        }));
                    }
                }
                Err(ClientError::Quota(m)) => {
                    let _ = tx.send(AgentEvent::Notice(format!(
                        "⟳ {} sin cuota; rotando… ({m})",
                        cand.label
                    )));
                    excluded.push(cand.flat);
                }
                Err(ClientError::Auth(m)) => {
                    let _ = tx.send(AgentEvent::Notice(format!(
                        "⟳ {} key inválida; rotando… ({m})",
                        cand.label
                    )));
                    excluded.push(cand.flat);
                }
                Err(ClientError::Server(m)) => {
                    let _ = tx.send(AgentEvent::Notice(format!(
                        "⟳ {} error del proveedor; rotando… ({m})",
                        cand.label
                    )));
                    excluded.push(cand.flat);
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::Error(e.to_string()));
                    break;
                }
            }
        }

        let _ = tx.send(AgentEvent::Done { conversation });
    }
}

/// Asegura que cada tool_call tenga id y nombre.
fn normalize_calls(calls: &[client::ToolCall]) -> Vec<(String, String, String)> {
    calls
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.name.is_empty())
        .map(|(i, c)| {
            let id = if c.id.is_empty() {
                format!("call_{i}")
            } else {
                c.id.clone()
            };
            (id, c.name.clone(), c.arguments.clone())
        })
        .collect()
}

fn assistant_message(content: &str, calls: &[(String, String, String)]) -> Value {
    let mut msg = json!({
        "role": "assistant",
        "content": if content.is_empty() { Value::Null } else { json!(content) },
    });
    if !calls.is_empty() {
        let arr: Vec<Value> = calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args },
                })
            })
            .collect();
        msg["tool_calls"] = Value::Array(arr);
    }
    msg
}

/// Arma la conversación inicial (system + primer mensaje del usuario).
pub fn new_conversation(system: &str, user: &str) -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ]
}

/// Prompt de sistema del coder.
pub fn system_prompt(cwd: &std::path::Path) -> String {
    let platform = if cfg!(windows) {
        "Windows"
    } else {
        "Linux/macOS"
    };
    format!(
        "Sos Nuide-CLI, un agente de programación en la terminal (estilo OpenCode).\n\
         Trabajás sobre el proyecto en: {}\n\
         Sistema: {platform}.\n\n\
         Usá las herramientas para inspeccionar y modificar archivos antes de responder. \
         Preferí `read_file`/`grep`/`glob` para entender el código y `edit_file`/`write_file` \
         para cambiarlo. Ejecutá tests con `bash` cuando sirva. Sé conciso: explicá qué hacés \
         y mostrá sólo los fragmentos relevantes. No inventes APIs: verificá en el código.",
        cwd.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mensaje_asistente_con_tools() {
        let calls = vec![("c1".into(), "read_file".into(), "{}".into())];
        let m = assistant_message("mirá", &calls);
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(m["content"], "mirá");
    }

    #[test]
    fn normaliza_ids_vacios() {
        let calls = vec![client::ToolCall {
            id: String::new(),
            name: "glob".into(),
            arguments: "{}".into(),
        }];
        let n = normalize_calls(&calls);
        assert_eq!(n[0].0, "call_0");
    }
}
