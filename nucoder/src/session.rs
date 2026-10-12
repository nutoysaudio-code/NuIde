//! Persistencia de la conversación **por proyecto**.
//!
//! Guarda el historial (mensajes JSON) en `~/.config/nucoder/sessions/<clave>.json`,
//! donde la clave deriva del directorio del proyecto (`--cwd`). Las imágenes ya
//! vienen degradadas a `[imagen]`, así el archivo no se llena de base64.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Ruta del archivo de sesión para un proyecto.
pub fn path(cwd: &Path) -> PathBuf {
    crate::config::config_dir()
        .join("sessions")
        .join(format!("{}.json", key_for(cwd)))
}

/// Clave estable (FNV-1a) + nombre del proyecto, para el archivo.
fn key_for(cwd: &Path) -> String {
    let s = cwd.to_string_lossy().to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(32)
        .collect();
    format!("{safe}-{h:016x}")
}

/// Guarda la conversación. Devuelve la ruta escrita.
pub fn save(cwd: &Path, messages: &[Value]) -> std::io::Result<PathBuf> {
    let p = path(cwd);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let doc = json!({
        "cwd": cwd.to_string_lossy(),
        "messages": messages,
    });
    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    std::fs::write(&p, text)?;
    Ok(p)
}

/// Carga la conversación guardada (si existe y tiene mensajes).
pub fn load(cwd: &Path) -> Option<Vec<Value>> {
    load_file(&path(cwd))
}

/// Carga los mensajes de un archivo de sesión cualquiera.
pub fn load_file(path: &Path) -> Option<Vec<Value>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let raw = raw.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(raw).ok()?;
    let msgs = v.get("messages").and_then(Value::as_array)?;
    (!msgs.is_empty()).then(|| msgs.clone())
}

/// Info de una sesión guardada (para `/sessions`).
pub struct SessionInfo {
    pub path: PathBuf,
    pub cwd: String,
    pub messages: usize,
}

/// Lista las sesiones guardadas, la más reciente primero.
pub fn list() -> Vec<SessionInfo> {
    let dir = crate::config::config_dir().join("sessions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(std::time::SystemTime, SessionInfo)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap_or_default();
        let raw = raw.trim_start_matches('\u{feff}');
        let Ok(v) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let cwd = v
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let messages = v
            .get("messages")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        if messages == 0 {
            continue;
        }
        let modified = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        out.push((
            modified,
            SessionInfo {
                path: p,
                cwd,
                messages,
            },
        ));
    }
    out.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    out.into_iter().map(|(_, i)| i).collect()
}

/// Borra la sesión del proyecto.
pub fn remove(cwd: &Path) {
    let _ = std::fs::remove_file(path(cwd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_carga_y_borra() {
        let cwd = std::env::temp_dir().join("nucoder-session-test");
        std::fs::create_dir_all(&cwd).unwrap();
        let msgs = vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"user","content":"hola"}),
        ];
        let p = save(&cwd, &msgs).unwrap();
        assert!(p.is_file());
        let back = load(&cwd).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1]["content"], "hola");
        remove(&cwd);
        assert!(load(&cwd).is_none());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn clave_estable_y_distinta_por_proyecto() {
        let a = key_for(Path::new("C:/proj/a"));
        let b = key_for(Path::new("C:/proj/b"));
        assert_ne!(a, b);
        assert_eq!(key_for(Path::new("C:/proj/a")), a);
    }
}
