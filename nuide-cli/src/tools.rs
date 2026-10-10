//! Herramientas del agente (estilo OpenCode): leer, escribir, editar, listar,
//! buscar por patrón, grep y ejecutar comandos. Todo con `std`, sin crates
//! nuevos. Cada herramienta devuelve texto (es lo que ve el modelo).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

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
    ]
}

/// Ejecuta una herramienta por nombre con argumentos JSON. Devuelve el texto de
/// resultado (incluye los errores como texto para que el modelo los vea).
pub fn execute(name: &str, args: &Value, cwd: &Path) -> String {
    let res = match name {
        "read_file" => read_file(args, cwd),
        "write_file" => write_file(args, cwd),
        "edit_file" => edit_file(args, cwd),
        "list_dir" => list_dir(args, cwd),
        "glob" => glob_tool(args, cwd),
        "grep" => grep_tool(args, cwd),
        "bash" => bash(args, cwd),
        other => Err(format!("herramienta desconocida: {other}")),
    };
    match res {
        Ok(s) => truncate(s),
        Err(e) => format!("error: {e}"),
    }
}

fn read_file(args: &Value, cwd: &Path) -> Result<String, String> {
    let path = resolve(cwd, str_arg(args, "path")?);
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

fn write_file(args: &Value, cwd: &Path) -> Result<String, String> {
    let path = resolve(cwd, str_arg(args, "path")?);
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

fn edit_file(args: &Value, cwd: &Path) -> Result<String, String> {
    let path = resolve(cwd, str_arg(args, "path")?);
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

fn list_dir(args: &Value, cwd: &Path) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(cwd, p))
        .unwrap_or_else(|| cwd.to_path_buf());
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

fn glob_tool(args: &Value, cwd: &Path) -> Result<String, String> {
    let pattern = str_arg(args, "pattern")?;
    let base = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(cwd, p))
        .unwrap_or_else(|| cwd.to_path_buf());
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

fn grep_tool(args: &Value, cwd: &Path) -> Result<String, String> {
    let needle = str_arg(args, "pattern")?;
    let base = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| resolve(cwd, p))
        .unwrap_or_else(|| cwd.to_path_buf());
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

fn bash(args: &Value, cwd: &Path) -> Result<String, String> {
    let command = str_arg(args, "command")?;
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let out = Command::new(shell)
        .arg(flag)
        .arg(command)
        .current_dir(cwd)
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
fn glob_match(pattern: &str, text: &str) -> bool {
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
        let dir = std::env::temp_dir().join(format!("nuide-tools-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let args = json!({ "path": "x.txt", "content": "uno\ndos\ntres\n" });
        write_file(&args, &dir).unwrap();
        let out = read_file(&json!({"path":"x.txt"}), &dir).unwrap();
        assert!(out.contains("2  dos"));
        let e = json!({"path":"x.txt","old_string":"dos","new_string":"DOS"});
        edit_file(&e, &dir).unwrap();
        let out = read_file(&json!({"path":"x.txt"}), &dir).unwrap();
        assert!(out.contains("DOS"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_falla_si_no_es_unico() {
        let dir = std::env::temp_dir().join(format!("nuide-tools2-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        write_file(&json!({"path":"y.txt","content":"a\na\n"}), &dir).unwrap();
        let r = edit_file(
            &json!({"path":"y.txt","old_string":"a","new_string":"b"}),
            &dir,
        );
        assert!(r.is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
