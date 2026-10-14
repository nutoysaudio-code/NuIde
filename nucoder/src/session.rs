//! Persistencia de la conversación **por proyecto**.
//!
//! Guarda el historial (mensajes JSON) en `~/.config/nucoder/sessions/<clave>.json`,
//! donde la clave deriva del directorio del proyecto (`--cwd`). Las imágenes ya
//! vienen degradadas a `[imagen]`, así el archivo no se llena de base64.

use std::path::{Path, PathBuf};

use serde_json::Value;

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

/// Guarda la conversación en SQLite. Devuelve la ruta de la base.
pub fn save(cwd: &Path, messages: &[Value]) -> std::io::Result<PathBuf> {
    let conn = crate::db::open().map_err(std::io::Error::other)?;
    crate::db::session_save(&conn, &key_for(cwd), &cwd.to_string_lossy(), messages)
        .map_err(std::io::Error::other)?;
    Ok(crate::db::db_path())
}

/// Carga la conversación guardada (si existe y tiene mensajes).
pub fn load(cwd: &Path) -> Option<Vec<Value>> {
    let conn = crate::db::open().ok()?;
    crate::db::session_load(&conn, &key_for(cwd))
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
    let Ok(conn) = crate::db::open() else {
        return Vec::new();
    };
    crate::db::sessions_list(&conn)
        .into_iter()
        .filter(|r| r.messages > 0)
        .map(|r| SessionInfo {
            path: crate::db::db_path(),
            cwd: r.cwd,
            messages: r.messages,
        })
        .collect()
}

/// Borra la sesión del proyecto.
pub fn remove(cwd: &Path) {
    if let Ok(conn) = crate::db::open() {
        crate::db::session_remove(&conn, &key_for(cwd));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_carga_y_borra() {
        let cwd = std::env::temp_dir().join("nucoder-session-test");
        std::fs::create_dir_all(&cwd).unwrap();
        let msgs = vec![
            serde_json::json!({"role":"system","content":"sys"}),
            serde_json::json!({"role":"user","content":"hola"}),
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
