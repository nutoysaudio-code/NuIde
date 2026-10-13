//! Bucle del agente: rutea, pide al modelo, ejecuta herramientas y **rota de
//! proveedor** cuando se agota la cuota o la key es inválida — sin perder la
//! conversación (que es sólo una lista de mensajes que se reenvía tal cual).
//!
//! Corre en un hilo; se comunica con la UI por un canal de `AgentEvent`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::caps::Cap;
use crate::client::{self, ClientError, StreamEvent};
use crate::config::Config;
use crate::image::Image;
use crate::knowledge::Knowledge;
use crate::mcp::McpHub;
use crate::skills::Registry;
use crate::tools::{self, Ctx};

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
    /// El agente pide aprobación para una acción sensible (borrar/mover/bash).
    /// La UI responde por `reply` (`true` = aprobar).
    Confirm {
        name: String,
        args: String,
        reply: Sender<bool>,
    },
    /// El agente presenta un **plan de trabajo** para aprobar (autopiloto).
    /// La UI responde por `reply` (`true` = aprobar; tras eso, si el autopiloto
    /// está activo, el resto del turno corre sin confirmaciones).
    Plan {
        goal: String,
        steps: Vec<String>,
        reply: Sender<bool>,
    },
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
    /// Pide confirmación antes de ejecutar acciones sensibles (TUI).
    pub confirm: bool,
    /// Confina las tools de archivos a `cwd` (+ KB).
    pub sandbox: bool,
    /// KB local para el prompt de sistema y `read_docs`.
    pub knowledge: Arc<Knowledge>,
    /// Skills descubiertos (catálogo en el prompt + tool `skill`).
    pub skills: Arc<Registry>,
    /// Restringe el router a modelos con esta capacidad (si hay).
    pub cap: Option<Cap>,
    /// El turno lleva imágenes: exige un modelo con visión.
    pub vision: bool,
    /// Fuerza un modelo por id (ignora el router; sin rotación).
    pub model_override: Option<String>,
    /// Tope aproximado de contexto (caracteres) para la compactación.
    pub context_chars: usize,
    /// Host de terminales de la IDE: si está, la tool `run_in_terminal` escribe
    /// el comando en una terminal real (visible y viva). En CLI queda `None`.
    pub terminal: Option<Sender<crate::tools::TerminalJob>>,
    /// Auto-aprobar (switch del panel): salta las confirmaciones.
    pub auto_approve: bool,
    /// Autopiloto: tras aprobar el plan, sigue hasta terminarlo sin confirmar.
    pub autopilot: bool,
    /// Permiso de internet (`web_fetch`/`web_search`).
    pub web: bool,
    /// Permiso de leer fuera del proyecto.
    pub read_outside: bool,
    /// Permiso de escribir en terminales de la IDE.
    pub terminal_allowed: bool,
    /// Host del cliente FTP de la IDE (para las tools `ftp_*`).
    pub ftp: Option<Sender<crate::tools::FtpJob>>,
    /// Permiso del cliente FTP.
    pub ftp_allowed: bool,
    /// Conexiones MCP **persistentes** entre turnos.
    pub mcp_hub: Arc<Mutex<McpHub>>,
}

/// Herramientas que piden confirmación (destructivas o que ejecutan shell).
fn is_sensitive(name: &str) -> bool {
    matches!(
        name,
        "delete_path" | "move_file" | "bash" | "run_in_terminal" | "ftp_delete" | "ftp_upload"
    )
}

/// Separa una herramienta MCP `<id>__<tool>` → `(id, tool)`.
fn split_mcp(name: &str, ids: &[String]) -> Option<(String, String)> {
    for id in ids {
        if let Some(tool) = name
            .strip_prefix(id.as_str())
            .and_then(|rest| rest.strip_prefix("__"))
        {
            return Some((id.clone(), tool.to_string()));
        }
    }
    None
}

/// Describe el criterio de ruteo para los mensajes de error.
fn route_criteria(free_only: bool, cap: Option<Cap>, vision: bool) -> String {
    let mut parts = Vec::new();
    if free_only {
        parts.push("gratis".to_string());
    }
    if let Some(c) = cap {
        parts.push(format!("cap {}", c.as_str()));
    }
    if vision {
        parts.push("visión".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(" + "))
    }
}

impl Agent {
    /// Corre hasta que el modelo deja de pedir herramientas (o falla).
    pub fn run(&self, mut conversation: Vec<Value>, tx: Sender<AgentEvent>) {
        let mut tools_spec = tools::tool_specs();
        // Servidores MCP: se (re)conectan por el **hub persistente** (no se
        // relanzan en cada turno) y sus herramientas se suman al modelo con
        // prefijo `<id>__<tool>`.
        let mcp_ids: Vec<String>;
        {
            let mut hub = self.mcp_hub.lock().unwrap_or_else(|e| e.into_inner());
            for n in hub.ensure(&self.config.mcp) {
                let _ = tx.send(AgentEvent::Notice(n));
            }
            mcp_ids = hub.ids();
            tools_spec.extend(hub.specs(&self.config.mcp));
        }
        let ctx = Ctx {
            cwd: &self.cwd,
            sandbox: self.sandbox,
            knowledge: Some(self.knowledge.as_ref()),
            skills: Some(self.skills.as_ref()),
            terminal: self.terminal.as_ref(),
            web: self.web,
            read_outside: self.read_outside,
            terminal_allowed: self.terminal_allowed,
            ftp: self.ftp.as_ref(),
            ftp_allowed: self.ftp_allowed,
            config: Some(self.config.as_ref()),
        };
        // Las imágenes de turnos previos se bajan ya (liberan RAM); sólo el
        // turno actual las conserva, y al terminar también se bajan.
        downgrade_old_images(&mut conversation);
        let mut excluded: Vec<usize> = Vec::new();
        // Con auto-aprobar, todas las acciones sensibles pasan sin preguntar.
        // Con autopiloto, se prende tras aprobar el plan.
        let mut auto = self.auto_approve;

        loop {
            if self.cancel.load(Ordering::Relaxed) {
                let _ = tx.send(AgentEvent::Notice("cancelado".into()));
                break;
            }
            let cand = match &self.model_override {
                Some(id) => self.config.find_candidate(id),
                None => {
                    self.config
                        .route(&self.task, &excluded, self.free_only, self.cap, self.vision)
                }
            };
            let Some(cand) = cand else {
                let msg = match &self.model_override {
                    Some(id) => format!("no existe el modelo `{id}` (mirá `nucoder models`)"),
                    None => {
                        if excluded.is_empty() {
                            let criterio = route_criteria(self.free_only, self.cap, self.vision);
                            format!("no hay modelos que cumplan el criterio{criterio}")
                        } else {
                            "todos los proveedores quedaron sin cuota".to_string()
                        }
                    }
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

            // Acota el contexto (descarta turnos viejos) si hace falta; no se
            // clona la conversación (las imágenes ya se degradaron in-place).
            trim_conversation(&mut conversation, self.context_chars);

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

                        // Plan de trabajo: siempre se pide aprobación explícita
                        // (aunque esté auto-aprobar). Tras aprobarlo, con
                        // autopiloto el resto del turno corre sin confirmar.
                        if name == "submit_plan" {
                            let goal = args
                                .get("goal")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let steps: Vec<String> = args
                                .get("steps")
                                .and_then(Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_string)
                                        .collect()
                                })
                                .unwrap_or_default();
                            let (rtx, rrx) = channel::<bool>();
                            let _ = tx.send(AgentEvent::Plan {
                                goal: goal.clone(),
                                steps: steps.clone(),
                                reply: rtx,
                            });
                            let ok = rrx.recv().unwrap_or(false);
                            let content = if ok {
                                if self.autopilot {
                                    auto = true;
                                }
                                format!(
                                    "Plan aprobado. Ejecutá los {} pasos en orden{}.",
                                    steps.len(),
                                    if self.autopilot {
                                        " (autopiloto: sin pedir confirmación)"
                                    } else {
                                        ""
                                    }
                                )
                            } else {
                                "El usuario rechazó el plan; pedí aclaraciones o proponé otro."
                                    .to_string()
                            };
                            conversation.push(json!({
                                "role": "tool",
                                "tool_call_id": id,
                                "content": content,
                            }));
                            continue;
                        }

                        // Acciones sensibles: pedir aprobación a la UI y esperar.
                        // Cargar un skill con permiso `ask` también lo es, y las
                        // herramientas de un server MCP con `confirm`.
                        let sensitive = is_sensitive(name)
                            || (name == "skill"
                                && args
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|id| self.skills.needs_confirmation(id)))
                            || self
                                .config
                                .mcp
                                .iter()
                                .any(|s| s.confirm && name.starts_with(&format!("{}__", s.id)));
                        if !auto && self.confirm && sensitive {
                            let (rtx, rrx) = channel::<bool>();
                            let _ = tx.send(AgentEvent::Confirm {
                                name: name.clone(),
                                args: arguments.clone(),
                                reply: rtx,
                            });
                            if !rrx.recv().unwrap_or(false) {
                                let _ = tx.send(AgentEvent::Notice(format!("✗ {name} rechazado")));
                                conversation.push(json!({
                                    "role": "tool",
                                    "tool_call_id": id,
                                    "content": "El usuario rechazó esta acción.",
                                }));
                                continue;
                            }
                        }
                        let _ = tx.send(AgentEvent::ToolStart {
                            name: name.clone(),
                            args: arguments.clone(),
                        });
                        // ¿Es una herramienta de un server MCP? (`<id>__<tool>`)
                        let out = match split_mcp(name, &mcp_ids) {
                            Some((sid, tool)) => {
                                let mut hub =
                                    self.mcp_hub.lock().unwrap_or_else(|e| e.into_inner());
                                match hub.call(&sid, &tool, &args) {
                                    Ok(t) => t,
                                    Err(e) => format!("error: {e}"),
                                }
                            }
                            None => tools::execute(name, &args, &ctx),
                        };
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
                Err(e) => {
                    // Modelo forzado: no se rota; se reporta y corta.
                    if self.model_override.is_some() {
                        let _ = tx.send(AgentEvent::Error(format!("{}: {e}", cand.label)));
                        break;
                    }
                    match e {
                        ClientError::Quota(m) => {
                            let _ = tx.send(AgentEvent::Notice(format!(
                                "⟳ {} sin cuota; rotando… ({m})",
                                cand.label
                            )));
                            excluded.push(cand.flat);
                        }
                        ClientError::Auth(m) => {
                            let _ = tx.send(AgentEvent::Notice(format!(
                                "⟳ {} no autorizado; rotando… ({m})",
                                cand.label
                            )));
                            excluded.push(cand.flat);
                        }
                        ClientError::Server(m) => {
                            let _ = tx.send(AgentEvent::Notice(format!(
                                "⟳ {} error del proveedor; rotando… ({m})",
                                cand.label
                            )));
                            excluded.push(cand.flat);
                        }
                        // 404: modelo no disponible para esta cuenta/endpoint.
                        ClientError::Http(404, m) => {
                            let _ = tx.send(AgentEvent::Notice(format!(
                                "⟳ {} no disponible; rotando… ({m})",
                                cand.label
                            )));
                            excluded.push(cand.flat);
                        }
                        other => {
                            let _ = tx.send(AgentEvent::Error(other.to_string()));
                            break;
                        }
                    }
                }
            }
        }

        // El turno terminó: los servidores MCP siguen vivos (hub persistente);
        // sólo se liberan las imágenes (no se reenvían nunca más).
        downgrade_images(&mut conversation);
        let _ = tx.send(AgentEvent::Done { conversation });
    }
}

/// ¿El mensaje trae partes `image_url`?
fn has_image_msg(m: &Value) -> bool {
    m.get("content")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .any(|p| p.get("type").and_then(Value::as_str) == Some("image_url"))
        })
        .unwrap_or(false)
}

/// Reemplaza las partes `image_url` de un mensaje por `[imagen]` (in-place).
fn downgrade_message(m: &mut Value) {
    if !has_image_msg(m) {
        return;
    }
    if let Some(arr) = m.get_mut("content").and_then(Value::as_array_mut) {
        for p in arr.iter_mut() {
            if p.get("type").and_then(Value::as_str) == Some("image_url") {
                *p = json!({ "type": "text", "text": "[imagen]" });
            }
        }
    }
}

/// Baja las imágenes de todos los turnos **menos el actual** (último user).
pub fn downgrade_old_images(messages: &mut [Value]) {
    let last = messages
        .iter()
        .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
    for (i, m) in messages.iter_mut().enumerate() {
        if Some(i) != last {
            downgrade_message(m);
        }
    }
}

/// Baja **todas** las imágenes (fin del turno): libera RAM, no se reenvían.
pub fn downgrade_images(messages: &mut [Value]) {
    for m in messages.iter_mut() {
        downgrade_message(m);
    }
}

/// Tamaño aproximado de la conversación (contenido + tool_calls).
fn context_chars(conv: &[Value]) -> usize {
    conv.iter()
        .map(|m| {
            let content = match m.get("content") {
                Some(Value::String(s)) => s.len(),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|p| {
                        p.get("text")
                            .and_then(Value::as_str)
                            .map(str::len)
                            .unwrap_or(0)
                    })
                    .sum(),
                _ => 0,
            };
            let calls = m
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|c| c.to_string().len()).sum())
                .unwrap_or(0);
            content + calls
        })
        .sum()
}

/// Si el contexto supera el tope, descarta los turnos (user→…) más viejos,
/// salteando el system y **sin** cortar un par assistant/tool. El turno actual
/// (último user) nunca se descarta.
pub fn trim_conversation(conv: &mut Vec<Value>, limit: usize) {
    if conv.len() <= 1 {
        return;
    }
    while context_chars(conv) > limit {
        let is_user = |m: &Value| m.get("role").and_then(Value::as_str) == Some("user");
        let Some(start) = conv
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, m)| is_user(m))
            .map(|(i, _)| i)
        else {
            break;
        };
        let next = conv
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, m)| is_user(m))
            .map(|(i, _)| i);
        match next {
            Some(end) => {
                conv.drain(start..end);
            }
            // Sólo queda el turno actual: no se puede recortar más.
            None => break,
        }
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

/// Mensaje de sistema; con `cache` usa un bloque cacheable (prompt caching de
/// proveedores que lo soportan: Anthropic/OpenRouter, etc.).
pub fn system_value(system: &str, cache: bool) -> Value {
    if cache {
        json!({
            "role": "system",
            "content": [{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" }
            }]
        })
    } else {
        json!({ "role": "system", "content": system })
    }
}

/// Arma la conversación inicial (system + primer mensaje de usuario, ya armados).
pub fn new_conversation_with(system: Value, user: Value) -> Vec<Value> {
    vec![system, user]
}

/// Mensaje `user`; con imágenes usa el formato multimodal de OpenAI.
pub fn user_message(prompt: &str, images: &[Image]) -> Value {
    if images.is_empty() {
        return json!({ "role": "user", "content": prompt });
    }
    let mut parts = vec![json!({ "type": "text", "text": prompt })];
    for im in images {
        parts.push(json!({ "type": "image_url", "image_url": { "url": im.data_url } }));
    }
    json!({ "role": "user", "content": parts })
}

/// Prompt de sistema del coder. Incluye el *primer* de la KB local y el
/// catálogo de **skills** (si los hay).
pub fn system_prompt(
    cwd: &std::path::Path,
    knowledge: &Knowledge,
    skills: &Registry,
    autopilot: bool,
) -> String {
    let platform = if cfg!(windows) {
        "Windows"
    } else {
        "Linux/macOS"
    };
    let mut s = format!(
        "Sos NuCoder, un agente de programación en la terminal.\n\
         Trabajás sobre el proyecto en: {}\n\
         Sistema: {platform}.\n\n\
         Usá las herramientas para inspeccionar y modificar archivos antes de responder. \
         Preferí `read_file`/`grep`/`glob` para entender el código y `edit_file`/`write_file` \
         para cambiarlo. Ejecutá tests con `bash` cuando sirva. Sé conciso: explicá qué hacés \
         y mostrá sólo los fragmentos relevantes. No inventes APIs: verificá en el código.\n\n\
         PLAN: para cualquier tarea de varios pasos, primero llamá a `submit_plan` con el \
         objetivo y los pasos; esperá la aprobación y recién después ejecutá los pasos en \
         orden. Para tareas triviales de un paso, respondé directo.",
        cwd.display()
    );
    if autopilot {
        s.push_str(
            "\n\nAUTOPILOTO: cuando el usuario apruebe el plan, ejecutá TODOS sus pasos en \
             orden sin detenerte a pedir confirmación, y avisá al terminar con un resumen.",
        );
    }
    s.push_str(&knowledge.primer());
    s.push_str(&crate::cook::catalog_prompt(skills));
    s
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

    fn img() -> Image {
        Image {
            label: "x".into(),
            mime: "image/png".into(),
            data_url: "data:image/png;base64,Zm9v".into(),
        }
    }

    #[test]
    fn solo_el_ultimo_turno_conserva_imagenes() {
        let mut conv = vec![
            json!({"role":"user","content":[
                {"type":"text","text":"mirá"},
                {"type":"image_url","image_url":{"url":"data:x"}}
            ]}),
            json!({"role":"assistant","content":"ok"}),
            json!({"role":"user","content":"seguí"}),
        ];
        downgrade_old_images(&mut conv);
        assert!(!has_image_msg(&conv[0]));
        assert_eq!(conv[0]["content"][1]["text"], "[imagen]");
        assert_eq!(conv[2]["content"], "seguí");
    }

    #[test]
    fn el_turno_actual_conserva_la_imagen() {
        let mut conv = vec![user_message("mirá", &[img()])];
        downgrade_old_images(&mut conv);
        assert!(has_image_msg(&conv[0]));
        // Al terminar el turno, se libera.
        downgrade_images(&mut conv);
        assert!(!has_image_msg(&conv[0]));
    }

    #[test]
    fn system_cacheable() {
        assert_eq!(system_value("hola", false)["content"], "hola");
        let c = system_value("hola", true);
        assert_eq!(c["content"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn recorta_contexto_por_turnos() {
        let mut conv = vec![json!({"role":"system","content":"sys"})];
        for i in 0..3 {
            conv.push(json!({"role":"user","content":"x".repeat(1000)}));
            conv.push(json!({"role":"assistant","content":format!("r{i}")}));
        }
        conv.push(json!({"role":"user","content":"actual"}));
        trim_conversation(&mut conv, 2000);
        // El system y el último user siguen.
        assert_eq!(conv[0]["role"], "system");
        assert_eq!(conv.last().unwrap()["content"], "actual");
        assert!(context_chars(&conv) <= 2000);
    }

    #[test]
    fn conversacion_inicial_texto() {
        let c = new_conversation_with(system_value("sys", false), user_message("hola", &[]));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0]["role"], "system");
        assert_eq!(c[1]["content"], "hola");
    }

    #[test]
    fn mensaje_con_imagenes_es_multimodal() {
        let img = Image {
            label: "x".into(),
            mime: "image/png".into(),
            data_url: "data:image/png;base64,Zm9v".into(),
        };
        let m = user_message("mirá", &[img]);
        assert_eq!(m["content"][0]["type"], "text");
        assert_eq!(m["content"][0]["text"], "mirá");
        assert_eq!(m["content"][1]["type"], "image_url");
        assert_eq!(
            m["content"][1]["image_url"]["url"],
            "data:image/png;base64,Zm9v"
        );
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

    #[test]
    fn separa_herramientas_mcp_por_prefijo() {
        let ids = vec!["rea".to_string(), "otro".to_string()];
        assert_eq!(
            split_mcp("rea__open_binary", &ids),
            Some(("rea".to_string(), "open_binary".to_string()))
        );
        assert_eq!(
            split_mcp("otro__x_y", &ids),
            Some(("otro".to_string(), "x_y".to_string()))
        );
        // Una herramienta propia no es MCP.
        assert!(split_mcp("read_file", &ids).is_none());
        // El orden de los ids no cambia el resultado aunque uno sea prefijo de otro.
        let ids = vec!["a".to_string(), "a_b".to_string()];
        assert_eq!(
            split_mcp("a_b__t", &ids),
            Some(("a_b".to_string(), "t".to_string()))
        );
    }
}
