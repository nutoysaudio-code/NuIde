//! Herramientas del agente: leer, escribir, editar, listar, buscar por patrón,
//! grep, ejecutar comandos, borrar/mover, leer la KB local y consultar internet.
//! Todo con `std` (más `curl.exe` para la red), sin crates nuevos. Cada
//! herramienta devuelve texto (es lo que ve el modelo).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use serde_json::{json, Value};

use crate::caps::Cap;
use crate::client;
use crate::cook;
use crate::knowledge::Knowledge;
use crate::skills::Registry;

/// Pedido del agente al **host** (la IDE) para correr un comando en una
/// **terminal real** —no capturada— para que quede visible y siga vivo (p. ej.
/// un servidor de desarrollo). El host responde por `reply` con un texto corto.
pub struct TerminalJob {
    pub command: String,
    /// Terminal destino sugerida por el modelo (opcional).
    pub name: Option<String>,
    /// El host contesta con el mensaje que verá el modelo.
    pub reply: Sender<String>,
}

/// Operación del **cliente FTP** de la IDE que el agente puede pedir.
#[derive(Debug, Clone)]
pub enum FtpOp {
    /// Abre el cliente y navega a `path`.
    Open(String),
    /// Lista un directorio remoto.
    List(String),
    /// Sube un archivo local a `remote`.
    Upload { local: String, remote: String },
    /// Baja `remote` a `local`.
    Download { remote: String, local: String },
    /// Borra un archivo remoto.
    Delete(String),
    /// Crea un directorio remoto.
    Mkdir(String),
}

/// Pedido del agente al **cliente FTP** del host (la IDE).
pub struct FtpJob {
    pub op: FtpOp,
    pub reply: Sender<String>,
}

/// Carpetas que se saltean al recorrer (ruido / generados).
const SKIP_DIRS: [&str; 10] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".idea",
    ".vscode",
    "releases",
];

const MAX_READ_LINES: usize = 600;
const MAX_OUTPUT: usize = 32_000;
/// Tiempo máximo de una petición de red (segundos).
const WEB_TIMEOUT_SECS: &str = "30";

/// Contexto de ejecución: raíz del proyecto, política de sandbox y KB local.
pub struct Ctx<'a> {
    pub cwd: &'a Path,
    /// Si es `true`, las tools de archivos quedan confinadas a `cwd` (lectura
    /// también en la KB). `bash` no se puede confinar y queda exento.
    pub sandbox: bool,
    /// KB local para `read_docs` (y permitida en modo sandbox).
    pub knowledge: Option<&'a Knowledge>,
    /// Skills descubiertos (para la tool `skill` y su lectura en sandbox).
    pub skills: Option<&'a Registry>,
    /// Host de terminales (la IDE). Si está, `run_in_terminal` escribe el
    /// comando en una terminal real; si falta (CLI), la tool no está disponible.
    pub terminal: Option<&'a Sender<TerminalJob>>,
    /// Permiso de internet (`web_fetch`/`web_search`).
    pub web: bool,
    /// Permiso de **leer** fuera del proyecto (además del sandbox).
    pub read_outside: bool,
    /// Permiso de escribir en terminales de la IDE (`run_in_terminal`).
    pub terminal_allowed: bool,
    /// Host del cliente FTP (la IDE). Si está, las tools `ftp_*` funcionan.
    pub ftp: Option<&'a Sender<FtpJob>>,
    /// Permiso del cliente FTP.
    pub ftp_allowed: bool,
    /// Config (para elegir proveedor/modelo por capacidad en las tools no-chat).
    pub config: Option<&'a crate::config::Config>,
}

impl<'a> Ctx<'a> {
    /// Contexto sin sandbox ni KB (para tests).
    #[cfg(test)]
    pub fn new(cwd: &'a Path) -> Self {
        Ctx {
            cwd,
            sandbox: false,
            knowledge: None,
            skills: None,
            terminal: None,
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        }
    }

    /// ¿La ruta cae dentro del espacio permitido? Lectura: `cwd` + KB; escritura:
    /// sólo `cwd`.
    fn allows(&self, path: &Path, write: bool) -> bool {
        if !self.sandbox {
            return true;
        }
        if within(path, self.cwd) {
            return true;
        }
        if !write {
            // Permiso explícito de leer fuera del proyecto.
            if self.read_outside {
                return true;
            }
            if let Some(kb) = self.knowledge {
                if kb.roots().iter().any(|r| within(path, r)) {
                    return true;
                }
            }
            // Los skills viven fuera del proyecto (`~/.claude`, `.agents`, …):
            // leerlos (cuerpo y recursos) es válido sin habilitar escritura.
            if let Some(sk) = self.skills {
                if sk.dirs().iter().any(|d| within(path, d)) {
                    return true;
                }
            }
        }
        false
    }
}

const SANDBOX_MSG: &str =
    "ruta fuera del espacio de trabajo (sandbox); reiniciá con --no-sandbox para permitirlo";

/// Permiso de internet apagado (pestaña Permisos → Internet).
const WEB_DISABLED: &str =
    "permiso de internet desactivado (Permisos → Internet); activalo para navegar/consultar";

/// Especificación de herramientas para la API OpenAI (`tools`).
pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Leer un archivo de texto. Devuelve el contenido con números de línea.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Ruta del archivo (relativa al proyecto)." },
                        "offset": { "type": "integer", "description": "Línea inicial (1-based)." },
                        "limit": { "type": "integer", "description": "Cantidad máxima de líneas." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Crear o sobrescribir un archivo con el contenido dado.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Reemplazar un fragmento exacto dentro de un archivo. Falla si old_string no es único (salvo replace_all).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old_string": { "type": "string", "description": "Texto exacto a buscar." },
                        "new_string": { "type": "string", "description": "Texto de reemplazo." },
                        "replace_all": { "type": "boolean", "description": "Reemplazar todas las apariciones." }
                    },
                    "required": ["path", "old_string", "new_string"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "delete_path",
                "description": "Borrar un archivo o directorio. Para directorios no vacíos usá recursive=true.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "recursive": { "type": "boolean", "description": "Borrar un directorio con todo su contenido." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "move_file",
                "description": "Mover o renombrar un archivo o directorio.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Origen." },
                        "to": { "type": "string", "description": "Destino." }
                    },
                    "required": ["from", "to"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "Listar el contenido de un directorio.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Directorio (por defecto, la raíz)." } }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "glob",
                "description": "Buscar archivos por patrón glob (acepta ** y *). Ej: src/**/*.rs",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string", "description": "Directorio base (por defecto, la raíz)." }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Buscar texto en los archivos del proyecto. Devuelve ruta:línea:contenido.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string", "description": "Directorio base (por defecto, la raíz)." },
                        "include": { "type": "string", "description": "Filtro glob de archivos, ej. *.rs" }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Ejecutar un comando de shell en la raíz del proyecto y devolver su salida.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "run_in_terminal",
                "description": "Escribir un comando en una terminal real de la IDE, que queda visible y sigue corriendo. Usalo para procesos largos o interactivos (servidores de desarrollo, watchers, `npm run dev`, `python -m http.server`, `cargo run`); devuelve enseguida. Para comandos puntuales cuya salida necesitás, usá `bash`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Comando a ejecutar en la terminal." },
                        "name": { "type": "string", "description": "Nombre de la terminal destino (opcional, p. ej. `build`)." }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "submit_plan",
                "description": "Presentar un PLAN de trabajo para que el usuario lo apruebe. Usalo al empezar cualquier tarea de varios pasos: primero el plan; tras la aprobación, ejecutá los pasos en orden (y si el autopiloto está activo, sin pedir confirmación en cada paso). Para tareas triviales de un paso, no lo uses.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "goal": { "type": "string", "description": "Objetivo de la tarea en una frase." },
                        "steps": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Pasos concretos y ordenados."
                        }
                    },
                    "required": ["goal", "steps"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_open",
                "description": "Abrir el cliente FTP de la IDE y navegar a un directorio remoto (queda visible). Path absoluto del servidor, p. ej. /public_html.",
                "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_list",
                "description": "Listar un directorio remoto del servidor FTP configurado.",
                "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_upload",
                "description": "Subir un archivo local a una ruta remota del servidor FTP.",
                "parameters": { "type": "object", "properties": {
                    "local": { "type": "string", "description": "Ruta local (relativa al proyecto)." },
                    "remote": { "type": "string", "description": "Ruta remota absoluta." } }, "required": ["local", "remote"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_download",
                "description": "Bajar un archivo remoto del servidor FTP a una ruta local.",
                "parameters": { "type": "object", "properties": {
                    "remote": { "type": "string" },
                    "local": { "type": "string" } }, "required": ["remote", "local"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_delete",
                "description": "Borrar un archivo remoto del servidor FTP.",
                "parameters": { "type": "object", "properties": { "remote": { "type": "string" } }, "required": ["remote"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ftp_mkdir",
                "description": "Crear un directorio remoto en el servidor FTP.",
                "parameters": { "type": "object", "properties": { "remote": { "type": "string" } }, "required": ["remote"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "image_generate",
                "description": "Generar una imagen a partir de una descripción de texto (endpoint /images/generations). La guarda en el proyecto (carpeta `generated/`) y devuelve la ruta. Necesita un proveedor con un modelo de imagen.",
                "parameters": { "type": "object", "properties": {
                    "prompt": { "type": "string", "description": "Descripción de la imagen." },
                    "size": { "type": "string", "description": "Tamaño, p. ej. 1024x1024 (default)." } },
                    "required": ["prompt"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "audio_transcribe",
                "description": "Transcribir un archivo de audio a texto (endpoint /audio/transcriptions). Necesita un proveedor con un modelo de audio.",
                "parameters": { "type": "object", "properties": { "path": { "type": "string", "description": "Ruta del audio (relativa al proyecto)." } }, "required": ["path"] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Descargar una URL (http/https) y devolver el texto. Para documentación o páginas puntuales.",
                "parameters": {
                    "type": "object",
                    "properties": { "url": { "type": "string" } },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Buscar en la web (DuckDuckGo) y devolver títulos y URLs.",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_docs",
                "description": "KB local del dominio (SpASM). Sin `name` lista los documentos; con `name` devuelve el contenido.",
                "parameters": {
                    "type": "object",
                    "properties": { "name": { "type": "string", "description": "Nombre del documento (ver el listado)." } }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "skill",
                "description": "Cargar un SKILL por su id (los que figuran en el catálogo SKILLS del prompt: skills de Claude/OpenCode/nativos). Devuelve la forma enseñable (la receta cocida) si existe, o el cuerpo original, más sus archivos de apoyo. Cargalo antes de ejecutar una tarea que el skill cubra y seguí sus pasos. Sin `id`, lista los disponibles.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Id exacto del skill (ver el catálogo SKILLS)." },
                        "format": { "type": "string", "description": "`auto` (default: receta cocida si existe, si no el original), `cooked` (forzar la receta) o `raw` (el cuerpo original)." }
                    }
                }
            }
        }),
    ]
}

/// Ejecuta una herramienta por nombre con argumentos JSON. Devuelve el texto de
/// resultado (incluye los errores como texto para que el modelo los vea).
pub fn execute(name: &str, args: &Value, ctx: &Ctx) -> String {
    let res = match name {
        "read_file" => read_file(args, ctx),
        "write_file" => write_file(args, ctx),
        "edit_file" => edit_file(args, ctx),
        "delete_path" => delete_path(args, ctx),
        "move_file" => move_file(args, ctx),
        "list_dir" => list_dir(args, ctx),
        "glob" => glob_tool(args, ctx),
        "grep" => grep_tool(args, ctx),
        "bash" => bash(args, ctx),
        "run_in_terminal" => run_in_terminal(args, ctx),
        "web_fetch" => {
            if ctx.web {
                web_fetch(args)
            } else {
                Err(WEB_DISABLED.into())
            }
        }
        "web_search" => {
            if ctx.web {
                web_search(args)
            } else {
                Err(WEB_DISABLED.into())
            }
        }
        "submit_plan" => Err("`submit_plan` la maneja el bucle del agente".into()),
        "ftp_open" => ftp_open(args, ctx),
        "ftp_list" => ftp_list(args, ctx),
        "ftp_upload" => ftp_upload(args, ctx),
        "ftp_download" => ftp_download(args, ctx),
        "ftp_delete" => ftp_delete(args, ctx),
        "ftp_mkdir" => ftp_mkdir(args, ctx),
        "image_generate" => image_generate(args, ctx),
        "audio_transcribe" => audio_transcribe(args, ctx),
        "read_docs" => read_docs(args, ctx),
        "skill" => read_skill(args, ctx),
        other => Err(format!("herramienta desconocida: {other}")),
    };
    match res {
        Ok(s) => truncate(s),
        Err(e) => format!("error: {e}"),
    }
}

fn read_file(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let path = resolve(ctx.cwd, str_arg(args, "path")?);
    if !ctx.allows(&path, false) {
        return Err(SANDBOX_MSG.into());
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(MAX_READ_LINES);
    let mut out = String::new();
    let mut shown = 0;
    for (i, line) in content.lines().enumerate() {
        let no = i + 1;
        if no < offset {
            continue;
        }
        if shown >= limit {
            out.push_str("[…truncado]\n");
            break;
        }
        out.push_str(&format!("{no:>5}  {line}\n"));
        shown += 1;
    }
    if out.is_empty() {
        out.push_str("(archivo vacío)\n");
    }
    Ok(out)
}

fn write_file(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let path = resolve(ctx.cwd, str_arg(args, "path")?);
    if !ctx.allows(&path, true) {
        return Err(SANDBOX_MSG.into());
    }
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or("falta `content`")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines = content.lines().count();
    Ok(format!("escrito {} ({lines} líneas)", path.display()))
}

fn edit_file(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let path = resolve(ctx.cwd, str_arg(args, "path")?);
    if !ctx.allows(&path, true) {
        return Err(SANDBOX_MSG.into());
    }
    let old = args
        .get("old_string")
        .and_then(Value::as_str)
        .ok_or("falta `old_string`")?;
    let new = args
        .get("new_string")
        .and_then(Value::as_str)
        .ok_or("falta `new_string`")?;
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let count = content.matches(old).count();
    if count == 0 {
        return Err(format!("`old_string` no aparece en {}", path.display()));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "`old_string` aparece {count} veces en {}; agregá más contexto o usá replace_all",
            path.display()
        ));
    }
    let updated = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    fs::write(&path, updated).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("editado {} ({count} reemplazo(s))", path.display()))
}

fn delete_path(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let path = resolve(ctx.cwd, str_arg(args, "path")?);
    if !ctx.allows(&path, true) {
        return Err(SANDBOX_MSG.into());
    }
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        if recursive {
            fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        } else {
            fs::remove_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    } else {
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(format!("borrado {}", path.display()))
}

fn move_file(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let from = resolve(ctx.cwd, str_arg(args, "from")?);
    let to = resolve(ctx.cwd, str_arg(args, "to")?);
    if !ctx.allows(&from, true) || !ctx.allows(&to, true) {
        return Err(SANDBOX_MSG.into());
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if fs::rename(&from, &to).is_err() {
        // Otro volumen: copiar y borrar (sólo archivos).
        if from.is_dir() {
            return Err(format!(
                "no se pudo mover el directorio {} (¿otro volumen?)",
                from.display()
            ));
        }
        fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        fs::remove_file(&from).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(format!("movido {} → {}", from.display(), to.display()))
}

fn list_dir(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(ctx.cwd, p))
        .unwrap_or_else(|| ctx.cwd.to_path_buf());
    if !ctx.allows(&path, false) {
        return Err(SANDBOX_MSG.into());
    }
    let mut entries: Vec<(String, bool, u64)> = Vec::new();
    let rd = fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for e in rd.flatten() {
        let meta = e.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        entries.push((e.file_name().to_string_lossy().to_string(), is_dir, size));
    }
    entries.sort_by_key(|e| (e.1, e.0.clone()));
    let mut out = format!("{}:\n", path.display());
    for (name, is_dir, size) in entries.iter().take(400) {
        if *is_dir {
            out.push_str(&format!("  {name}/\n"));
        } else {
            out.push_str(&format!("  {name}  ({size} bytes)\n"));
        }
    }
    Ok(out)
}

fn glob_tool(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let pattern = str_arg(args, "pattern")?;
    let base = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(ctx.cwd, p))
        .unwrap_or_else(|| ctx.cwd.to_path_buf());
    if !ctx.allows(&base, false) {
        return Err(SANDBOX_MSG.into());
    }
    let mut hits = Vec::new();
    walk_files(&base, &mut |p| {
        let rel = p
            .strip_prefix(&base)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        if glob_match(pattern, &rel) {
            hits.push(rel);
        }
    });
    hits.sort();
    if hits.is_empty() {
        return Ok("(sin coincidencias)\n".into());
    }
    Ok(hits.into_iter().take(300).collect::<Vec<_>>().join("\n") + "\n")
}

fn grep_tool(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let needle = str_arg(args, "pattern")?;
    let base = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(ctx.cwd, p))
        .unwrap_or_else(|| ctx.cwd.to_path_buf());
    if !ctx.allows(&base, false) {
        return Err(SANDBOX_MSG.into());
    }
    let include = args
        .get("include")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut out = String::new();
    let mut count = 0;
    walk_files(&base, &mut |p| {
        if count >= 200 {
            return;
        }
        if let Some(inc) = &include {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let full = p.to_string_lossy();
            if !glob_match(inc, &name) && !glob_match(inc, &full) {
                return;
            }
        }
        let Ok(text) = fs::read_to_string(p) else {
            return;
        };
        if text.len() > 2_000_000 {
            return;
        }
        for (i, line) in text.lines().enumerate() {
            if line.contains(needle) {
                let rel = p
                    .strip_prefix(&base)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push_str(&format!("{}:{}: {}\n", rel, i + 1, line.trim()));
                count += 1;
                if count >= 200 {
                    break;
                }
            }
        }
    });
    if out.is_empty() {
        return Ok("(sin coincidencias)\n".into());
    }
    Ok(out)
}

fn bash(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let command = str_arg(args, "command")?;
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let out = crate::proc::hidden(shell)
        .arg(flag)
        .arg(command)
        .current_dir(ctx.cwd)
        .output()
        .map_err(|e| e.to_string())?;
    let code = out.status.code().unwrap_or(-1);
    let mut text = String::new();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stdout.trim().is_empty() {
        text.push_str(stdout.trim_end());
        text.push('\n');
    }
    if !stderr.trim().is_empty() {
        text.push_str("[stderr] ");
        text.push_str(stderr.trim_end());
        text.push('\n');
    }
    text.push_str(&format!("[exit {code}]"));
    Ok(text)
}

/// Manda una operación al **cliente FTP** de la IDE y espera su respuesta.
fn ftp_job(op: FtpOp, ctx: &Ctx) -> Result<String, String> {
    let Some(tx) = ctx.ftp else {
        return Err("el cliente FTP no está disponible; abrí «Cliente FTP» en la IDE".into());
    };
    if !ctx.ftp_allowed {
        return Err("permiso de FTP desactivado (Permisos → FTP)".into());
    }
    let (rtx, rrx) = channel::<String>();
    tx.send(FtpJob { op, reply: rtx })
        .map_err(|_| "no se pudo enviar al cliente FTP de la IDE".to_string())?;
    Ok(rrx
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| "el cliente FTP no respondió a tiempo".to_string()))
}

fn ftp_open(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(FtpOp::Open(str_arg(args, "path")?.to_string()), ctx)
}
fn ftp_list(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(FtpOp::List(str_arg(args, "path")?.to_string()), ctx)
}
fn ftp_upload(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(
        FtpOp::Upload {
            local: str_arg(args, "local")?.to_string(),
            remote: str_arg(args, "remote")?.to_string(),
        },
        ctx,
    )
}
fn ftp_download(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(
        FtpOp::Download {
            remote: str_arg(args, "remote")?.to_string(),
            local: str_arg(args, "local")?.to_string(),
        },
        ctx,
    )
}
fn ftp_delete(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(FtpOp::Delete(str_arg(args, "remote")?.to_string()), ctx)
}
fn ftp_mkdir(args: &Value, ctx: &Ctx) -> Result<String, String> {
    ftp_job(FtpOp::Mkdir(str_arg(args, "remote")?.to_string()), ctx)
}

/// Elige el primer candidato con capacidad `cap` → `(base_url, api_key, modelo)`.
fn pick_with_cap(ctx: &Ctx, cap: Cap) -> Result<(String, String, String), String> {
    let Some(cfg) = ctx.config else {
        return Err("sin configuración de proveedores".into());
    };
    let cands = cfg.candidates(cfg.free_only, Some(cap), false);
    let c = cands.first().ok_or_else(|| {
        format!(
            "no hay un modelo con capacidad `{}`; configurá un proveedor (nucoder discover)",
            cap.as_str()
        )
    })?;
    let p = &cfg.providers[c.provider_idx];
    let m = &p.models[c.model_idx];
    Ok((p.base_url.clone(), p.api_key.clone(), m.id.clone()))
}

/// Genera una imagen y la guarda en `<proyecto>/generated/`.
fn image_generate(args: &Value, ctx: &Ctx) -> Result<String, String> {
    if !ctx.web {
        return Err(WEB_DISABLED.into());
    }
    let prompt = str_arg(args, "prompt")?;
    let size = args
        .get("size")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("1024x1024");
    let (base, key, model) = pick_with_cap(ctx, Cap::Image)?;
    let bytes =
        client::image_generate(&base, &key, &model, prompt, size).map_err(|e| e.to_string())?;
    let dir = ctx.cwd.join("generated");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("img-{}.png", stamp()));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(format!(
        "imagen generada con {model} ({} KB): {}",
        bytes.len() / 1024,
        path.display()
    ))
}

/// Transcribe un audio a texto.
fn audio_transcribe(args: &Value, ctx: &Ctx) -> Result<String, String> {
    if !ctx.web {
        return Err(WEB_DISABLED.into());
    }
    let path = resolve(ctx.cwd, str_arg(args, "path")?);
    if !ctx.allows(&path, false) {
        return Err(SANDBOX_MSG.into());
    }
    let (base, key, model) = pick_with_cap(ctx, Cap::Audio)?;
    let text = client::transcribe(&base, &key, &model, &path).map_err(|e| e.to_string())?;
    Ok(format!("[{model}] {text}"))
}

/// Marca temporal (ms) para nombrar los archivos generados.
fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Manda el comando al **host** (la IDE) para que lo escriba en una terminal
/// real. La respuesta del host es el texto que verá el modelo. Sin host (CLI),
/// no está disponible: se sugiere `bash`.
fn run_in_terminal(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let command = str_arg(args, "command")?.to_string();
    let name = args.get("name").and_then(Value::as_str).map(str::to_string);
    let Some(tx) = ctx.terminal else {
        return Err(
            "`run_in_terminal` sólo funciona dentro de la IDE; usá `bash` para este comando".into(),
        );
    };
    if !ctx.terminal_allowed {
        return Err("permiso de terminales desactivado (Permisos → Terminales); usá `bash`".into());
    }
    let (rtx, rrx) = channel::<String>();
    tx.send(TerminalJob {
        command,
        name,
        reply: rtx,
    })
    .map_err(|_| "no se pudo enviar el comando a la IDE".to_string())?;
    // El host lo procesa en su bucle de UI (milisegundos). Si tarda, seguimos
    // igual: el comando ya quedó encolado.
    Ok(rrx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| "comando enviado a la terminal de la IDE".to_string()))
}

fn web_fetch(args: &Value) -> Result<String, String> {
    let url = str_arg(args, "url")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("la url debe empezar con http:// o https://".into());
    }
    // Acotar el crudo antes de procesar (páginas enormes no aportan).
    let body = safe_head(&curl_get(url)?, 400_000).to_string();
    let text = if looks_html(&body) {
        html_to_text(&body)
    } else {
        body
    };
    Ok(format!("GET {url}\n{text}"))
}

fn web_search(args: &Value) -> Result<String, String> {
    let q = str_arg(args, "query")?;
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(q));
    let html = curl_get(&url)?;
    let hits = parse_ddg(&html);
    if hits.is_empty() {
        return Ok(format!("(sin resultados para «{q}»)\n{url}"));
    }
    let mut out = format!("resultados para «{q}»:\n");
    for (title, link) in hits.iter().take(10) {
        out.push_str(&format!("- {title}\n  {link}\n"));
    }
    Ok(out)
}

fn read_docs(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let Some(kb) = ctx.knowledge else {
        return Ok("(no hay base de conocimiento configurada)\n".into());
    };
    match args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        None => {
            let mut out = String::from("documentos disponibles:\n");
            for d in kb.docs() {
                out.push_str(&format!("  - {}\n", d.name));
            }
            if kb.is_empty() {
                out.push_str("  (vacía)\n");
            }
            Ok(out)
        }
        Some(name) => match kb.read(name) {
            Some((n, content)) => Ok(format!("--- {n} ---\n{content}")),
            None => Err(format!(
                "no existe el documento `{name}` (usá read_docs sin argumentos para listar)"
            )),
        },
    }
}

/// Carga un skill por id (los del catálogo SKILLS del prompt). Sin `id`, lista.
fn read_skill(args: &Value, ctx: &Ctx) -> Result<String, String> {
    let Some(reg) = ctx.skills else {
        return Ok("(no hay skills disponibles)\n".into());
    };
    match args
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        None => {
            let mut out = String::from("skills disponibles:\n");
            for sk in reg.advertised() {
                out.push_str(&format!("  - {}\n", sk.line()));
            }
            if reg.is_empty() {
                out.push_str("  (vacío)\n");
            }
            Ok(out)
        }
        Some(id) => {
            // `auto`: receta cocida (del recetario) si existe; si no, el original.
            let format = args.get("format").and_then(Value::as_str).unwrap_or("auto");
            match format {
                "raw" => reg.load(id),
                "cooked" => match cook::load_cookie(id) {
                    Some(t) => Ok(t.render_text()),
                    None => match reg.by_id(id) {
                        Some(sk) => cook::cook_skill(sk).map(|t| t.render_text()),
                        None => reg.load(id),
                    },
                },
                _ => match cook::load_cookie(id) {
                    Some(t) => Ok(t.render_text()),
                    None => reg.load(id),
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internet (curl.exe)
// ---------------------------------------------------------------------------

fn curl_get(url: &str) -> Result<String, String> {
    let out = crate::proc::curl()
        .args([
            "-sSL",
            "--max-time",
            WEB_TIMEOUT_SECS,
            "-A",
            "NuCoder/0.3 (+https://github.com/nutoysaudio-code/NuIde)",
            url,
        ])
        .output()
        .map_err(|e| format!("no se pudo ejecutar curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// ¿Parece HTML? (para decidir si conviene limpiarlo).
fn looks_html(s: &str) -> bool {
    let head = safe_head(s, 400).to_lowercase();
    head.contains("<!doctype html") || head.contains("<html") || head.contains("<body")
}

/// Prefijo de `s` de hasta `max` bytes, en un límite de carácter válido.
fn safe_head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

/// Quita `<script>`/`<style>`, los tags y normaliza el texto.
fn html_to_text(html: &str) -> String {
    let mut kept = String::with_capacity(html.len());
    let mut rest = html;
    // Recortar bloques de script/style (pueden contener `<` que rompe el strip).
    loop {
        let script = find_ci(rest, "<script");
        let style = find_ci(rest, "<style");
        let (pos, close) = match (script, style) {
            (Some(s), Some(t)) if s <= t => (s, "</script>"),
            (Some(_), Some(t)) => (t, "</style>"),
            (Some(s), None) => (s, "</script>"),
            (None, Some(t)) => (t, "</style>"),
            (None, None) => break,
        };
        kept.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        match find_ci(tail, close) {
            Some(end) => rest = &tail[end + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    kept.push_str(rest);
    let no_tags = strip_tags(&kept);
    decode_entities(&no_tags)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_lowercase().find(&needle.to_lowercase())
}

/// Extrae `(título, url)` de los resultados de DuckDuckGo (HTML).
fn parse_ddg(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("class=\"result__a\"") {
        let start = rest[..pos].rfind("<a ").unwrap_or(pos);
        let tag_end = rest[pos..]
            .find('>')
            .map(|i| pos + i + 1)
            .unwrap_or(rest.len());
        let tag = &rest[start..tag_end];
        let href = attr(tag, "href").unwrap_or_default();
        let text_end = rest[tag_end..]
            .find("</a>")
            .map(|i| tag_end + i)
            .unwrap_or(rest.len());
        let title = decode_entities(&strip_tags(&rest[tag_end..text_end]))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !href.is_empty() && !title.is_empty() {
            out.push((title, clean_ddg_url(&href)));
        }
        rest = &rest[(text_end + 4).min(rest.len())..];
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let pos = tag.find(&needle)? + needle.len();
    let rest = &tag[pos..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)? + 1;
        Some(rest[1..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

fn clean_ddg_url(href: &str) -> String {
    let h = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    if let Some(pos) = h.find("uddg=") {
        let v = &h[pos + 5..];
        let end = v.find('&').unwrap_or(v.len());
        return urldecode(&v[..end]);
    }
    h
}

fn urlencode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("falta `{key}`"))
}

/// Resuelve una ruta relativa contra `cwd` (o la usa tal cual si es absoluta).
pub fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// ¿`path` cae dentro de `root`? (canonicalizando de forma tolerante).
fn within(path: &Path, root: &Path) -> bool {
    canon_lenient(path).starts_with(canon_lenient(root))
}

/// Canonicaliza aunque la ruta no exista (usa el padre).
fn canon_lenient(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    if let (Some(parent), Some(name)) = (p.parent(), p.file_name()) {
        if let Ok(cp) = parent.canonicalize() {
            return cp.join(name);
        }
    }
    p.to_path_buf()
}

/// Recorre archivos bajo `base`, salteando `SKIP_DIRS` y entradas ocultas.
fn walk_files(base: &Path, f: &mut dyn FnMut(&Path)) {
    let rd = match fs::read_dir(base) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk_files(&path, f);
        } else {
            f(&path);
        }
    }
}

/// Coincidencia glob con `*`, `?` y `**`. `*` no cruza `/`; `**` sí.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    glob_bytes(pattern.as_bytes(), text.as_bytes())
}

fn glob_bytes(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        b'*' => {
            if p.len() >= 2 && p[1] == b'*' {
                let rest = if p.get(2) == Some(&b'/') {
                    &p[3..]
                } else {
                    &p[2..]
                };
                for i in 0..=s.len() {
                    if glob_bytes(rest, &s[i..]) {
                        return true;
                    }
                }
                false
            } else {
                let rest = &p[1..];
                let mut i = 0;
                loop {
                    if glob_bytes(rest, &s[i..]) {
                        return true;
                    }
                    if i >= s.len() || s[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
        }
        b'?' => !s.is_empty() && s[0] != b'/' && glob_bytes(&p[1..], &s[1..]),
        c => s.first() == Some(&c) && glob_bytes(&p[1..], &s[1..]),
    }
}

fn truncate(mut s: String) -> String {
    if s.len() <= MAX_OUTPUT {
        return s;
    }
    // Cortar en un límite de caracteres válido.
    let mut cut = MAX_OUTPUT;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("\n[…salida truncada]\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nuide-tools-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn glob_basico() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/main.rs"));
        assert!(glob_match("**/*.spasm", "spasm/router.spasm"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
    }

    #[test]
    fn read_y_edit_archivo() {
        let dir = temp_dir("re");
        let ctx = Ctx::new(&dir);
        let args = json!({ "path": "x.txt", "content": "uno\ndos\ntres\n" });
        write_file(&args, &ctx).unwrap();
        let out = read_file(&json!({"path":"x.txt"}), &ctx).unwrap();
        assert!(out.contains("2  dos"));
        let e = json!({"path":"x.txt","old_string":"dos","new_string":"DOS"});
        edit_file(&e, &ctx).unwrap();
        let out = read_file(&json!({"path":"x.txt"}), &ctx).unwrap();
        assert!(out.contains("DOS"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_falla_si_no_es_unico() {
        let dir = temp_dir("ed");
        let ctx = Ctx::new(&dir);
        write_file(&json!({"path":"y.txt","content":"a\na\n"}), &ctx).unwrap();
        let r = edit_file(
            &json!({"path":"y.txt","old_string":"a","new_string":"b"}),
            &ctx,
        );
        assert!(r.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn borra_y_mueve() {
        let dir = temp_dir("mv");
        let ctx = Ctx::new(&dir);
        write_file(&json!({"path":"a.txt","content":"hola"}), &ctx).unwrap();
        move_file(&json!({"from":"a.txt","to":"sub/b.txt"}), &ctx).unwrap();
        assert!(dir.join("sub/b.txt").is_file());
        assert!(!dir.join("a.txt").exists());
        delete_path(&json!({"path":"sub","recursive":true}), &ctx).unwrap();
        assert!(!dir.join("sub").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sandbox_confina_al_proyecto() {
        let dir = temp_dir("sb");
        let outside = temp_dir("sb-out");
        let ctx = Ctx {
            cwd: &dir,
            sandbox: true,
            knowledge: None,
            skills: None,
            terminal: None,
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        };
        // Dentro del proyecto: OK.
        assert!(write_file(&json!({"path":"in.txt","content":"x"}), &ctx).is_ok());
        // Fuera: bloqueado en escritura y lectura.
        let out = outside.join("o.txt");
        let res = write_file(&json!({"path": out.to_string_lossy(), "content":"x"}), &ctx);
        assert!(res.unwrap_err().contains("sandbox"));
        assert!(read_file(
            &json!({"path": outside.join("o.txt").to_string_lossy()}),
            &ctx
        )
        .is_err());
        // Sin sandbox: permitido.
        let free = Ctx::new(&dir);
        assert!(write_file(
            &json!({"path": out.to_string_lossy(), "content":"x"}),
            &free
        )
        .is_ok());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn run_in_terminal_manda_al_host_y_espera_respuesta() {
        let dir = temp_dir("term");
        let (tx, rx) = channel::<TerminalJob>();
        let ctx = Ctx {
            cwd: &dir,
            sandbox: false,
            knowledge: None,
            skills: None,
            terminal: Some(&tx),
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        };
        let handle = std::thread::spawn(move || {
            let job = rx.recv().unwrap();
            assert_eq!(job.command, "npm run dev");
            job.reply
                .send(format!("escrito en la terminal: {}", job.command))
                .unwrap();
        });
        let out = run_in_terminal(&json!({"command": "npm run dev"}), &ctx).unwrap();
        assert!(out.contains("npm run dev"), "respuesta inesperada: {out}");
        handle.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_in_terminal_sin_host_avisa() {
        let dir = temp_dir("term-none");
        let ctx = Ctx::new(&dir);
        assert!(run_in_terminal(&json!({"command": "echo hola"}), &ctx).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sin_permiso_de_internet_la_web_se_bloquea() {
        let dir = temp_dir("web-off");
        let mut ctx = Ctx::new(&dir);
        ctx.web = false;
        let out = execute("web_search", &json!({"query": "x"}), &ctx);
        assert!(out.contains("internet"), "respuesta: {out}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn permiso_de_leer_fuera_habilita_la_lectura() {
        let dir = temp_dir("ro-in");
        let outside = temp_dir("ro-out");
        let file = outside.join("a.txt");
        fs::write(&file, "hola").unwrap();
        let mut ctx = Ctx {
            cwd: &dir,
            sandbox: true,
            knowledge: None,
            skills: None,
            terminal: None,
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        };
        let path = file.to_string_lossy().to_string();
        // Sin permiso: bloqueado por sandbox.
        assert!(read_file(&json!({"path": path}), &ctx).is_err());
        // Con permiso: se lee.
        ctx.read_outside = true;
        assert!(read_file(&json!({"path": path}), &ctx).is_ok());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn html_a_texto_y_entidades() {
        let html = r#"<html><head><style>.x{color:red}</style></head>
            <body><h1>Título</h1><script>var a=1<2;</script><p>Hola &amp; adiós</p></body></html>"#;
        let txt = html_to_text(html);
        assert!(txt.contains("Título"));
        assert!(txt.contains("Hola & adiós"));
        assert!(!txt.contains("color:red"));
        assert!(!txt.contains("var a"));
    }

    #[test]
    fn parsea_resultados_de_ddg() {
        let html = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=1">Ejemplo <b>Uno</b></a>
            <a rel="nofollow" class="result__a" href="https://otro.test/b">Otro resultado</a>
        "#;
        let hits = parse_ddg(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "Ejemplo Uno");
        assert_eq!(hits[0].1, "https://example.com/a");
        assert_eq!(hits[1].1, "https://otro.test/b");
    }

    #[test]
    fn skill_tool_carga_por_id_y_lista_sin_id() {
        use crate::skills::{Registry, SkillSource};
        let dir = temp_dir("skill-tool");
        let root = dir.join("skills");
        let sk_dir = root.join("demo");
        fs::create_dir_all(&sk_dir).unwrap();
        fs::write(
            sk_dir.join("SKILL.md"),
            "---\nname: Demo\ndescription: sirve\n---\n\nPasos del demo.",
        )
        .unwrap();
        let reg = Registry::from_sources(vec![(SkillSource::OpenCode, root)]);
        let ctx = Ctx {
            cwd: &dir,
            sandbox: false,
            knowledge: None,
            skills: Some(&reg),
            terminal: None,
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        };
        // Sin id: lista.
        let list = execute("skill", &json!({}), &ctx);
        assert!(list.contains("demo"), "listado: {list}");
        // Con id: cuerpo sin frontmatter.
        let body = execute("skill", &json!({"id": "demo"}), &ctx);
        assert!(body.contains("Pasos del demo"));
        assert!(!body.contains("name: Demo"));
        // Id inexistente: error legible.
        let bad = execute("skill", &json!({"id": "noexiste"}), &ctx);
        assert!(bad.starts_with("error:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sandbox_permite_leer_recursos_del_skill() {
        use crate::skills::{Registry, SkillSource};
        let project = temp_dir("skill-sb");
        // El skill vive fuera del proyecto.
        let outside = temp_dir("skill-sb-out");
        let root = outside.join("skills");
        let sk_dir = root.join("demo");
        fs::create_dir_all(&sk_dir).unwrap();
        fs::write(sk_dir.join("SKILL.md"), "---\ndescription: x\n---\ncuerpo").unwrap();
        fs::create_dir_all(sk_dir.join("references")).unwrap();
        fs::write(sk_dir.join("references/ref.md"), "referencia").unwrap();
        let reg = Registry::from_sources(vec![(SkillSource::Claude, root)]);
        let ctx = Ctx {
            cwd: &project,
            sandbox: true,
            knowledge: None,
            skills: Some(&reg),
            terminal: None,
            web: true,
            read_outside: false,
            terminal_allowed: true,
            ftp: None,
            ftp_allowed: true,
            config: None,
        };
        let ref_path = sk_dir.join("references/ref.md");
        let out = read_file(&json!({ "path": ref_path.to_string_lossy() }), &ctx).unwrap();
        assert!(out.contains("referencia"));
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&outside);
    }
}
