//! Buscar (y reemplazar) en todo el proyecto, en un hilo aparte.
//!
//! Reutiliza `collect_files` (que saltea generados y carpetas ocultas) y los
//! buscadores de línea del editor. El tope de coincidencias y de tamaño de
//! archivo evita que un proyecto enorme cuelgue el hilo.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use crate::editor::{build_regex, collect_files, find_in_line, find_in_line_regex};

/// Tope de coincidencias devueltas.
const MAX_HITS: usize = 500;
/// Tamaño máximo de archivo a escanear.
const MAX_FILE_BYTES: u64 = 2_000_000;

/// Una coincidencia en un archivo del proyecto.
pub struct SearchHit {
    pub path: PathBuf,
    /// Línea (0-based).
    pub line: usize,
    /// Columna del inicio de la coincidencia (en caracteres).
    pub col: usize,
    /// Texto completo de la línea.
    pub text: String,
}

/// Resultado de una búsqueda. `gen` permite descartar respuestas viejas.
pub struct SearchResult {
    pub gen: u64,
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
}

/// Lanza la búsqueda en un hilo.
pub fn spawn(
    root: PathBuf,
    query: String,
    case_sensitive: bool,
    regex: bool,
    gen: u64,
    tx: Sender<SearchResult>,
) {
    thread::spawn(move || {
        let result = run(&root, &query, case_sensitive, regex, gen);
        let _ = tx.send(result);
    });
}

fn run(root: &Path, query: &str, case_sensitive: bool, regex: bool, gen: u64) -> SearchResult {
    let mut hits = Vec::new();
    let mut truncated = false;
    if query.is_empty() {
        return SearchResult {
            gen,
            hits,
            truncated,
        };
    }
    let re = if regex {
        match build_regex(query, case_sensitive) {
            Some(re) => Some(re),
            None => {
                return SearchResult {
                    gen,
                    hits,
                    truncated,
                }
            }
        }
    } else {
        None
    };

    'outer: for path in collect_files(root) {
        match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_FILE_BYTES => continue,
            Ok(_) => {}
            Err(_) => continue,
        }
        // `read_to_string` falla con binarios (no son UTF-8 válido): se saltean.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let found = match &re {
                Some(re) => find_in_line_regex(line, re, false),
                None => find_in_line(line, query, case_sensitive, false),
            };
            if let Some(&(start, _)) = found.first() {
                hits.push(SearchHit {
                    path: path.clone(),
                    line: i,
                    col: start,
                    text: line.to_string(),
                });
                if hits.len() >= MAX_HITS {
                    truncated = true;
                    break 'outer;
                }
            }
        }
    }

    SearchResult {
        gen,
        hits,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nuide-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn encuentra_en_varios_archivos_y_saltea_generados() {
        let dir = temp_dir("run");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(
            dir.join("src").join("a.spasm"),
            "let total = 0\nlet x = 0\n",
        )
        .unwrap();
        std::fs::write(dir.join("src").join("b.spasm"), "print total\n").unwrap();
        std::fs::write(dir.join("target").join("c.spasm"), "total total\n").unwrap();

        let result = run(&dir, "total", false, false, 7);
        assert_eq!(result.gen, 7);
        assert_eq!(result.hits.len(), 2);
        assert!(result
            .hits
            .iter()
            .all(|h| h.path.file_name().unwrap() != "c.spasm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_y_mayusculas() {
        let dir = temp_dir("regex");
        std::fs::write(dir.join("a.txt"), "Foo\nfoo\nf00\n").unwrap();

        let literal = run(&dir, "foo", false, false, 0);
        assert_eq!(literal.hits.len(), 2); // Foo y foo (sin distinguir)
        let case = run(&dir, "foo", true, false, 0);
        assert_eq!(case.hits.len(), 1);
        let re = run(&dir, r"f\d+", false, true, 0);
        assert_eq!(re.hits.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_spawn_avisa_por_el_canal() {
        let dir = temp_dir("spawn");
        std::fs::write(dir.join("a.txt"), "hola mundo\n").unwrap();
        let (tx, rx) = channel();
        spawn(dir.clone(), "mundo".to_string(), false, false, 3, tx);
        let result = rx.recv().unwrap();
        assert_eq!(result.gen, 3);
        assert_eq!(result.hits.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
