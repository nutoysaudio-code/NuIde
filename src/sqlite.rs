//! Visor de bases de datos SQLite, sin crates.
//!
//! NuIde no implementa el formato de SQLite (son páginas B-tree, freeblocks y
//! codificación variable: un proyecto en sí mismo). Lo que sí puede hacer —y es
//! lo que necesita un IDE— es **preguntarle** a un binario que ya existe en el
//! sistema y leerle la salida tabulada:
//!
//! 1. `sqlite3.exe` (viene, por ejemplo, con el SDK de Android) con
//!    `-separator "\t"` y cabeceras activas.
//! 2. Si no está, `python.exe` con el módulo `sqlite3` de su biblioteca estándar.
//!
//! Ambos devuelven **TSV**, que es el mismo estilo de contrato por líneas que usa
//! el despliegue ([`crate::deploy`]) y el host de SpASM. Todo lo público de este
//! módulo **bloquea** (levanta el proceso y espera): la IDE lo llama en un hilo,
//! nunca en el de la UI.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;

use crate::pty;

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

/// Qué binario se va a usar para leer la base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Sqlite3(PathBuf),
    Python(PathBuf),
}

impl Tool {
    /// `sqlite3.exe` en el PATH o en las ubicaciones típicas del SDK; si no,
    /// `python`/`py` (el módulo `sqlite3` viene en la biblioteca estándar).
    pub fn discover() -> Option<Tool> {
        #[cfg(windows)]
        let names = ["sqlite3.exe", "sqlite3"];
        #[cfg(not(windows))]
        let names = ["sqlite3"];
        if let Some(p) = pty::on_path(&names) {
            return Some(Tool::Sqlite3(PathBuf::from(p)));
        }
        // Rutas conocidas sin ensuciar el PATH del usuario.
        for extra in sqlite3_extra_paths() {
            if Path::new(&extra).is_file() {
                return Some(Tool::Sqlite3(PathBuf::from(extra)));
            }
        }
        // Python: el mismo descubrimiento del toolchain SpASM (respeta
        // NUIDE_PYTHON y el PATH).
        if let Some(py) = crate::spasm::Toolchain::discover().python {
            if py.is_file() {
                return Some(Tool::Python(py));
            }
        }
        None
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tool::Sqlite3(_) => "sqlite3",
            Tool::Python(_) => "python/sqlite3",
        }
    }
}

/// Ubicaciones donde suele haber un `sqlite3.exe` sin estar en el PATH.
fn sqlite3_extra_paths() -> Vec<String> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = Path::new(&local);
        out.push(
            base.join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join("sqlite3.exe")
                .display()
                .to_string(),
        );
    }
    out
}

/// Script de Python que imprime TSV (cabecera + filas) para una consulta.
const PY_TSV: &str = r#"
import sqlite3, sys
db, sql, limit = sys.argv[1], sys.argv[2], int(sys.argv[3])
con = sqlite3.connect(db)
con.text_factory = str
cur = con.execute(sql)
cols = [d[0] for d in (cur.description or [])]
print("\t".join(cols))
n = 0
for row in cur:
    if n >= limit: break
    print("\t".join("" if v is None else str(v).replace("\t", " ").replace("\n", " ") for v in row))
    n += 1
"#;

/// Resultado de una lectura pedida desde la UI (llega por canal, desde el hilo).
#[derive(Debug, Clone)]
pub struct Loaded {
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
pub fn spawn_load(tool: Tool, db: PathBuf, table: Option<String>, tx: Sender<Loaded>) {
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
            tables,
            table: pick,
            header,
            lines,
            note,
        });
    });
}

/// Ejecuta `sql` sobre `db` y devuelve el resultado tabulado.
pub fn query(tool: &Tool, db: &Path, sql: &str, limit: usize) -> Table {
    let mut table = Table::default();
    let out = match tool {
        Tool::Sqlite3(exe) => Command::new(exe)
            .arg("-separator")
            .arg("\t")
            .arg("-header")
            .arg(db)
            .arg(sql)
            .output(),
        Tool::Python(exe) => Command::new(exe)
            .arg("-c")
            .arg(PY_TSV)
            .arg(db)
            .arg(sql)
            .arg(limit.to_string())
            .output(),
    };
    match out {
        Ok(o) => {
            if !o.status.success() {
                table.error = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return table;
            }
            let text = String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n");
            let mut lines = text.lines().filter(|l| !l.is_empty());
            if let Some(header) = lines.next() {
                table.columns = header.split('\t').map(str::to_string).collect();
            }
            for line in lines.take(limit) {
                table
                    .rows
                    .push(line.split('\t').map(str::to_string).collect());
            }
            table
        }
        Err(e) => {
            table.error = format!("no se pudo correr {}: {e}", tool.label());
            table
        }
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
    fn el_descubrimiento_devuelve_un_binario_real_o_nada() {
        // No se puede exigir cuál: depende de la máquina. Lo que sí vale es que
        // si encuentra algo, el archivo existe.
        if let Some(tool) = Tool::discover() {
            let path = match &tool {
                Tool::Sqlite3(p) | Tool::Python(p) => p.clone(),
            };
            assert!(path.is_file(), "{} no existe", path.display());
            assert!(!tool.label().is_empty());
        }
    }

    #[test]
    fn una_consulta_sobre_una_base_rota_devuelve_error_sin_panic() {
        let dir = std::env::temp_dir().join(format!("nuide-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("no-es-sqlite.db");
        std::fs::write(&db, b"esto no es una base").unwrap();
        // Sin binario disponible, no hay nada que probar.
        let Some(tool) = Tool::discover() else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let t = query(&tool, &db, "SELECT 1", 10);
        // O devuelve algo o explica por qué no; nunca se queda mudo y vacío.
        assert!(!t.rows.is_empty() || !t.error.is_empty(), "{t:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
