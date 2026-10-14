//! **Persistencia de NuCoder en SQLite.**
//!
//! Todo lo que antes vivía en archivos JSON (`config.json`, `sessions/*.json`,
//! `cookbook/*.cookie`) pasa a una sola base: `~/.config/nucoder/nucoder.db`.
//!
//! Tablas:
//!
//! - `settings(key TEXT PRIMARY KEY, value TEXT)` — la config completa (proveedores,
//!   permisos, skills, estado de modelos, MCP…) serializada como JSON bajo la clave
//!   `config`; y `schema` para la versión.
//! - `sessions(project TEXT PRIMARY KEY, cwd, updated_at, messages)` — los **chats**,
//!   un renglón por proyecto, `messages` es el arreglo JSON.
//! - `cookbook(id TEXT PRIMARY KEY, cookie, updated_at)` — las recetas de NuCook.
//!
//! La primera vez, `migrate` importa desde los archivos viejos (que **no** se
//! borran: quedan como respaldo).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// Ruta de la base: `~/.config/nucoder/nucoder.db`.
pub fn db_path() -> PathBuf {
    crate::config::config_dir().join("nucoder.db")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Abre (creando si falta) la base por defecto y asegura el esquema.
pub fn open() -> rusqlite::Result<Connection> {
    let conn = open_at(&db_path())?;
    let _ = migrate(&conn);
    Ok(conn)
}

/// Abre una base en una ruta cualquiera (lo usan las pruebas).
pub fn open_at(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sessions (
             project    TEXT PRIMARY KEY,
             cwd        TEXT NOT NULL,
             updated_at INTEGER NOT NULL,
             messages   TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS cookbook (
             id         TEXT PRIMARY KEY,
             cookie     TEXT NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS ide_settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
    )?;
    Ok(conn)
}

// ---------------------------------------------------------------------------
// settings (clave/valor)
// ---------------------------------------------------------------------------

/// Lee un valor de `settings`.
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Escribe (o reemplaza) un valor en `settings`.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// chats (sesiones)
// ---------------------------------------------------------------------------

/// Un renglón del listado de sesiones.
pub struct SessionRow {
    pub project: String,
    pub cwd: String,
    pub messages: usize,
    pub updated_at: i64,
}

/// Guarda (o reemplaza) la conversación de un proyecto.
pub fn session_save(
    conn: &Connection,
    project: &str,
    cwd: &str,
    messages: &[Value],
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO sessions(project, cwd, updated_at, messages) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(project) DO UPDATE SET cwd = excluded.cwd,
                                            updated_at = excluded.updated_at,
                                            messages = excluded.messages",
        params![project, cwd, now(), json],
    )?;
    Ok(())
}

/// Carga la conversación de un proyecto (si existe y tiene mensajes).
pub fn session_load(conn: &Connection, project: &str) -> Option<Vec<Value>> {
    let raw: String = conn
        .query_row(
            "SELECT messages FROM sessions WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let msgs: Vec<Value> = serde_json::from_str(&raw).ok()?;
    (!msgs.is_empty()).then_some(msgs)
}

/// Borra la conversación de un proyecto.
pub fn session_remove(conn: &Connection, project: &str) {
    let _ = conn.execute("DELETE FROM sessions WHERE project = ?1", params![project]);
}

/// Lista las sesiones, la más reciente primero.
pub fn sessions_list(conn: &Connection) -> Vec<SessionRow> {
    let Ok(mut st) = conn.prepare("SELECT project, cwd, messages, updated_at FROM sessions") else {
        return Vec::new();
    };
    let rows = st.query_map([], |r| {
        let raw: String = r.get(2)?;
        let n = serde_json::from_str::<Vec<Value>>(&raw).map(|v| v.len()).unwrap_or(0);
        Ok(SessionRow {
            project: r.get(0)?,
            cwd: r.get(1)?,
            messages: n,
            updated_at: r.get(3)?,
        })
    });
    let mut out: Vec<SessionRow> = rows.into_iter().flatten().flatten().collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    out
}

// ---------------------------------------------------------------------------
// recetario (NuCook)
// ---------------------------------------------------------------------------

/// Guarda (o reemplaza) una receta.
pub fn cookie_save(conn: &Connection, id: &str, cookie: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO cookbook(id, cookie, updated_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET cookie = excluded.cookie,
                                        updated_at = excluded.updated_at",
        params![id, cookie, now()],
    )?;
    Ok(())
}

/// Lee una receta por id.
pub fn cookie_load(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT cookie FROM cookbook WHERE id = ?1",
        params![id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Todas las recetas (id, json).
pub fn cookies_list(conn: &Connection) -> Vec<(String, String)> {
    let Ok(mut st) = conn.prepare("SELECT id, cookie FROM cookbook ORDER BY id") else {
        return Vec::new();
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

// ---------------------------------------------------------------------------
// Migración desde los archivos viejos
// ---------------------------------------------------------------------------

/// Importa una sola vez `config.json`, `sessions/*.json` y `cookbook/*.cookie`.
/// Devuelve la cantidad de ítems importados. No borra los originales.
pub fn migrate(conn: &Connection) -> rusqlite::Result<usize> {
    if get_setting(conn, "schema").is_some() {
        return Ok(0);
    }
    let dir = crate::config::config_dir();
    let mut n = 0usize;

    // config.json → settings['config']
    let cfg = dir.join("config.json");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        let text = text.trim_start_matches('\u{feff}');
        if serde_json::from_str::<Value>(text).is_ok() {
            set_setting(conn, "config", text.trim())?;
            n += 1;
        }
    }

    // sessions/*.json → sessions
    if let Ok(rd) = std::fs::read_dir(dir.join("sessions")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}')) else {
                continue;
            };
            let cwd = v.get("cwd").and_then(Value::as_str).unwrap_or("");
            let msgs = v.get("messages").and_then(Value::as_array);
            let Some(msgs) = msgs else { continue };
            if msgs.is_empty() {
                continue;
            }
            let project = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if session_save(conn, &project, cwd, msgs).is_ok() {
                n += 1;
            }
        }
    }

    // cookbook/*.cookie → cookbook
    if let Ok(rd) = std::fs::read_dir(dir.join("cookbook")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("cookie") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if !id.is_empty() && cookie_save(conn, &id, &body).is_ok() {
                n += 1;
            }
        }
    }

    set_setting(conn, "schema", "1")?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// Ajustes de la IDE (misma base, tabla aparte)
// ---------------------------------------------------------------------------

/// Todos los ajustes de NuIde (`ide_settings`), ordenados por clave.
pub fn ide_all(conn: &Connection) -> Vec<(String, String)> {
    let Ok(mut st) = conn.prepare("SELECT key, value FROM ide_settings ORDER BY key") else {
        return Vec::new();
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

/// Reemplaza **todos** los ajustes de NuIde por el mapa dado.
pub fn ide_replace_all(conn: &Connection, pairs: &[(String, String)]) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ide_settings", [])?;
    let mut st = conn.prepare("INSERT INTO ide_settings(key, value) VALUES(?1, ?2)")?;
    for (k, v) in pairs {
        st.execute(params![k, v])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nucoder-db-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_file(&d);
        d
    }

    #[test]
    fn settings_roundtrip() {
        let p = tmp("set");
        let c = open_at(&p).unwrap();
        assert!(get_setting(&c, "config").is_none());
        set_setting(&c, "config", "{\"a\":1}").unwrap();
        set_setting(&c, "config", "{\"a\":2}").unwrap(); // reemplaza
        assert_eq!(get_setting(&c, "config").as_deref(), Some("{\"a\":2}"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sesiones_guardan_cargan_listan_y_borran() {
        let p = tmp("ses");
        let c = open_at(&p).unwrap();
        let msgs = vec![json!({"role":"user","content":"hola"})];
        session_save(&c, "proj-1", "C:/proj/1", &msgs).unwrap();
        assert_eq!(session_load(&c, "proj-1").unwrap().len(), 1);
        assert!(session_load(&c, "otro").is_none());
        let list = sessions_list(&c);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].messages, 1);
        session_remove(&c, "proj-1");
        assert!(session_load(&c, "proj-1").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn recetas_guardan_y_cargan() {        let p = tmp("cook");
        let c = open_at(&p).unwrap();
        cookie_save(&c, "demo", "{\"id\":\"demo\"}").unwrap();
        assert_eq!(cookie_load(&c, "demo").as_deref(), Some("{\"id\":\"demo\"}"));
        assert_eq!(cookies_list(&c).len(), 1);
        assert!(cookie_load(&c, "nope").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn ajustes_de_la_ide_se_reemplazan() {
        let p = tmp("ide");
        let c = open_at(&p).unwrap();
        assert!(ide_all(&c).is_empty());
        ide_replace_all(
            &c,
            &[
                ("project".into(), "F:\\proj".into()),
                ("show.left".into(), "1".into()),
            ],
        )
        .unwrap();
        assert_eq!(ide_all(&c).len(), 2);
        // Reemplazo total: borra lo que no esté.
        ide_replace_all(&c, &[("project".into(), "C:\\otro".into())]).unwrap();
        let all = ide_all(&c);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1, "C:\\otro");
        let _ = std::fs::remove_file(&p);
    }
}
