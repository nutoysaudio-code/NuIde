//! Visor de bases de datos SQLite, **nativo** (rusqlite).
//!
//! Antes este módulo le preguntaba a `sqlite3.exe` o a `python -m sqlite3` y les
//! leía TSV. Ahora que NuIde ya enlaza SQLite (para la persistencia de NuCoder),
//! lee la base **directo**, sin depender de binarios externos. La API pública es
//! la misma: todo lo que expone este módulo **bloquea** (abre y consulta), así la
//! IDE lo llama en un hilo, nunca en el de la UI.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Filas de un resultado: cabecera + celdas ya separadas.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Texto del error, si hubo (se muestra en el panel en vez de reventar).
    pub error: String,
}

impl Table {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Cuántas filas se piden por defecto en el visor.
    pub const DEFAULT_LIMIT: usize = 200;
}

/// Motor de lectura. Hoy siempre es el nativo; se mantiene el tipo para no tocar
/// el panel ni las firmas (y por si mañana se quiere un fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Native,
}

impl Tool {
    /// El motor nativo siempre está disponible.
    pub fn discover() -> Option<Tool> {
        Some(Tool::Native)
    }

    pub fn label(&self) -> &'static str {
        "nativo (rusqlite)"
    }
}

/// Resultado de una lectura pedida desde la UI (llega por canal, desde el hilo).
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Época de la lectura (para descartar resultados viejos).
    pub gen: u64,
    /// Tablas de la base (vacío si falló).
    pub tables: Vec<String>,
    /// Tabla consultada.
    pub table: String,
    /// Cabecera y filas ya formateadas.
    pub header: String,
    pub lines: Vec<String>,
    /// Aviso para el panel (error, "sin base", ...).
    pub note: String,
}

/// Pide en un hilo la lista de tablas y las filas de una tabla.
pub fn spawn_load(tool: Tool, db: PathBuf, table: Option<String>, gen: u64, tx: Sender<Loaded>) {
    std::thread::spawn(move || {
        let tables = tables(&tool, &db);
        let pick = table
            .clone()
            .or_else(|| tables.first().cloned())
            .unwrap_or_default();
        let (header, lines, note) = if pick.is_empty() {
            (
                String::new(),
                Vec::new(),
                "la base no tiene tablas".to_string(),
            )
        } else {
            let t = rows(&tool, &db, &pick, Table::DEFAULT_LIMIT);
            if !t.error.is_empty() {
                (String::new(), Vec::new(), t.error)
            } else {
                let header = t.columns.join("  |  ");
                let lines: Vec<String> = t
                    .rows
                    .iter()
                    .map(|r| r.join("  |  "))
                    .take(Table::DEFAULT_LIMIT)
                    .collect();
                let note = if lines.is_empty() {
                    format!("{pick}: 0 filas")
                } else {
                    format!("{pick}: {} fila(s)", lines.len())
                };
                (header, lines, note)
            }
        };
        let _ = tx.send(Loaded {
            gen,
            tables,
            table: pick,
            header,
            lines,
            note,
        });
    });
}

/// Ejecuta `sql` sobre `db` (sólo lectura) y devuelve el resultado tabulado.
pub fn query(_tool: &Tool, db: &Path, sql: &str, limit: usize) -> Table {
    let mut table = Table::default();
    let conn = match rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(e) => {
            table.error = e.to_string();
            return table;
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            table.error = e.to_string();
            return table;
        }
    };
    let ncols = stmt.column_count();
    table.columns = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = match stmt.query([]) {
        Ok(r) => r,
        Err(e) => {
            table.error = e.to_string();
            return table;
        }
    };
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let mut out = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    let v: rusqlite::types::Value = row
                        .get(i)
                        .unwrap_or(rusqlite::types::Value::Null);
                    out.push(value_to_string(&v));
                }
                table.rows.push(out);
                if table.rows.len() >= limit {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                table.error = e.to_string();
                break;
            }
        }
    }
    table
}

/// Texto de una celda (NULL → vacío, BLOB → resumen de tamaño).
fn value_to_string(v: &rusqlite::types::Value) -> String {
    use rusqlite::types::Value;
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

/// Tablas y vistas de la base.
pub fn tables(tool: &Tool, db: &Path) -> Vec<String> {
    let sql = "SELECT name FROM sqlite_master WHERE type IN ('table','view') \
               AND name NOT LIKE 'sqlite_%' ORDER BY name";
    let t = query(tool, db, sql, 500);
    t.rows.into_iter().flatten().collect()
}

/// Columnas de una tabla (`(nombre, tipo)`), vía PRAGMA.
pub fn columns(tool: &Tool, db: &Path, table: &str) -> Vec<(String, String)> {
    let t = query(
        tool,
        db,
        &format!("PRAGMA table_info({})", quote_ident(table)),
        200,
    );
    t.rows
        .iter()
        .filter_map(|r| Some((r.get(1)?.clone(), r.get(2)?.clone())))
        .collect()
}

/// Filas de una tabla.
pub fn rows(tool: &Tool, db: &Path, table: &str, limit: usize) -> Table {
    query(
        tool,
        db,
        &format!("SELECT * FROM {} LIMIT {}", quote_ident(table), limit),
        limit,
    )
}

/// Escapa un identificador para meterlo en SQL (`"mi tabla"`). Se usa con nombres
/// que vienen de la propia base, pero nunca se sabe.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `true` si el archivo parece una base SQLite (magic `SQLite format 3\0`).
/// Una base cifrada o creada por otra cosa no pasa, y el panel lo avisa.
pub fn looks_like_sqlite(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && bytes.starts_with(b"SQLite format 3\0")
}

/// Chequeo rápido de que el archivo es legible como base.
pub fn probe(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16];
    matches!(std::io::Read::read_exact(&mut f, &mut buf), Ok(())) && looks_like_sqlite(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_ident_escapa_las_comillas() {
        assert_eq!(quote_ident("clientes"), "\"clientes\"");
        assert_eq!(quote_ident("mi\"tabla"), "\"mi\"\"tabla\"");
    }

    #[test]
    fn el_magic_de_sqlite_se_detecta() {
        let mut pagina = vec![0u8; 100];
        pagina[..16].copy_from_slice(b"SQLite format 3\0");
        assert!(looks_like_sqlite(&pagina));
        assert!(!looks_like_sqlite(b"PK\x03\x04 algo zip"));
        assert!(!looks_like_sqlite(b"corto"));
    }

    #[test]
    fn table_se_pone_vacia_sin_filas() {
        let mut t = Table::default();
        assert!(t.is_empty());
        t.rows.push(vec!["1".into()]);
        assert!(!t.is_empty());
    }

    #[test]
    fn el_motor_nativo_esta_siempre() {
        let tool = Tool::discover().expect("nativo");
        assert!(!tool.label().is_empty());
    }

    #[test]
    fn lee_una_base_real_creada_con_rusqlite() {
        let dir = std::env::temp_dir().join(format!("nuide-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("demo.db");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE clientes(id INTEGER PRIMARY KEY, nombre TEXT);
                 INSERT INTO clientes(nombre) VALUES ('Ana'), ('Beto');",
            )
            .unwrap();
        }
        assert!(probe(&db));
        let tool = Tool::discover().unwrap();
        assert_eq!(tables(&tool, &db), vec!["clientes".to_string()]);
        let t = rows(&tool, &db, "clientes", 200);
        assert!(t.error.is_empty(), "{}", t.error);
        assert_eq!(t.columns, vec!["id".to_string(), "nombre".to_string()]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0][1], "Ana");
        let cols = columns(&tool, &db, "clientes");
        assert_eq!(cols[0].0, "id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn una_consulta_sobre_una_base_rota_devuelve_error_sin_panic() {
        let dir = std::env::temp_dir().join(format!("nuide-sqlite-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("no-es-sqlite.db");
        std::fs::write(&db, b"esto no es una base").unwrap();
        let tool = Tool::discover().unwrap();
        let t = query(&tool, &db, "SELECT 1", 10);
        // O devuelve algo o explica por qué no; nunca se queda mudo y vacío.
        assert!(!t.rows.is_empty() || !t.error.is_empty(), "{t:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
