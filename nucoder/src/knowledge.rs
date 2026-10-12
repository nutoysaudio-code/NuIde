//! Conocimiento local (KB) del agente: descubre documentos en las rutas
//! configuradas y los expone con la herramienta `read_docs` + un *primer* en el
//! prompt de sistema.
//!
//! Fuentes, en orden: config (`knowledge`), variable `NUCODER_KNOWLEDGE`
//! (rutas separadas por el separador del SO) y autodetección del ecosistema
//! SpASM (`SPASM_HOME` o `F:\SpAMS-Oficial`).
//!
//! No hay embeddings: es un índice de archivos que el modelo lee a demanda.

use std::path::{Path, PathBuf};

/// Tope de texto del "quickref" que se inyecta en el prompt de sistema.
const PRIMER_DOC_MAX: usize = 4_000;
/// Extensiones que se indexan como documentos.
const DOC_EXT: [&str; 4] = ["md", "txt", "json", "toml"];

/// Un documento de la KB.
#[derive(Debug, Clone)]
pub struct Doc {
    pub name: String,
    pub path: PathBuf,
}

/// Índice de la KB local.
#[derive(Debug, Clone, Default)]
pub struct Knowledge {
    roots: Vec<PathBuf>,
    docs: Vec<Doc>,
}

impl Knowledge {
    /// Descubre la KB completa (config + env + autodetección).
    pub fn discover(extra: &[String]) -> Self {
        let mut roots: Vec<PathBuf> = Vec::new();
        for s in extra {
            add_root(&mut roots, PathBuf::from(s));
        }
        if let Some(v) = std::env::var_os("NUCODER_KNOWLEDGE") {
            for p in std::env::split_paths(&v) {
                add_root(&mut roots, p);
            }
        }
        // Bases candidatas del ecosistema SpASM (se agregan sólo si existen).
        let mut bases: Vec<PathBuf> = vec![PathBuf::from("F:/SpAMS-Oficial")];
        if let Some(home) = std::env::var_os("SPASM_HOME") {
            bases.push(PathBuf::from(home));
        }
        if let Ok(cwd) = std::env::current_dir() {
            bases.push(cwd);
        }
        for base in bases {
            add_root(&mut roots, base.join("SPASM_QUICKREF.md"));
            add_root(&mut roots, base.join("skills").join("spasm-architect"));
        }
        Self::from_roots(roots)
    }

    /// Construye el índice a partir de raíces explícitas (útil en tests).
    pub fn from_roots(roots: Vec<PathBuf>) -> Self {
        let mut kb = Knowledge {
            roots,
            docs: Vec::new(),
        };
        kb.index();
        kb
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn docs(&self) -> &[Doc] {
        &self.docs
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    fn index(&mut self) {
        let mut docs: Vec<Doc> = Vec::new();
        for root in &self.roots {
            if root.is_file() {
                docs.push(Doc {
                    name: filename(root),
                    path: root.clone(),
                });
            } else if root.is_dir() {
                walk(root, &mut |p| {
                    if is_doc(p) {
                        let rel = p
                            .strip_prefix(root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .replace('\\', "/");
                        docs.push(Doc {
                            name: rel,
                            path: p.to_path_buf(),
                        });
                    }
                });
            }
        }
        // Desambiguar nombres repetidos con la carpeta padre.
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for d in &mut docs {
            let n = d.name.clone();
            let count = seen.entry(n).or_insert(0);
            if *count > 0 {
                if let Some(parent) = d.path.parent().and_then(|p| p.file_name()) {
                    d.name = format!("{}/{}", parent.to_string_lossy(), d.name);
                }
            }
            *count += 1;
        }
        docs.sort_by(|a, b| a.name.cmp(&b.name));
        self.docs = docs;
    }

    /// Lee un documento por nombre: exacto, luego sufijo y por último "contiene"
    /// (case-insensitive). Devuelve `(nombre, contenido)`.
    pub fn read(&self, name: &str) -> Option<(String, String)> {
        let q = name
            .trim()
            .trim_start_matches("./")
            .replace('\\', "/")
            .to_lowercase();
        if q.is_empty() {
            return None;
        }
        // También probamos con el último segmento (los nombres de doc son
        // relativos a su raíz, así que "skills/x/SKILL.md" puede no existir).
        let base = q.rsplit('/').next().unwrap_or(&q).to_string();
        let name_of = |d: &Doc| d.name.to_lowercase();
        let doc = self
            .docs
            .iter()
            .find(|d| name_of(d) == q)
            .or_else(|| self.docs.iter().find(|d| name_of(d).ends_with(&q)))
            .or_else(|| self.docs.iter().find(|d| name_of(d).contains(&q)))
            .or_else(|| self.docs.iter().find(|d| name_of(d) == base))
            .or_else(|| self.docs.iter().find(|d| name_of(d).ends_with(&base)))
            .or_else(|| self.docs.iter().find(|d| name_of(d).contains(&base)))?;
        let content = std::fs::read_to_string(&doc.path).ok()?;
        Some((doc.name.clone(), content))
    }

    /// Texto compacto para el prompt de sistema: orientación + quickref + índice.
    pub fn primer(&self) -> String {
        if self.docs.is_empty() {
            return String::new();
        }
        let mut s = String::from(
            "\n\nTenés acceso a una base de conocimiento local del dominio (SpASM y \
             proyectos del usuario). Usá la herramienta `read_docs` para leerla: sin \
             argumentos lista los documentos; con `name` devuelve el contenido. No \
             inventes comportamiento del dominio que no esté en la KB: si falta, decilo.\n",
        );
        if let Some(quick) = self
            .docs
            .iter()
            .find(|d| d.name.to_uppercase().contains("QUICKREF"))
        {
            if let Ok(text) = std::fs::read_to_string(&quick.path) {
                let cut = floor_char(&text, PRIMER_DOC_MAX);
                s.push_str("\n--- ");
                s.push_str(&quick.name);
                s.push_str(" ---\n");
                s.push_str(&text[..cut]);
                if cut < text.len() {
                    s.push_str("\n[…recortado; usá read_docs para el resto]");
                }
                s.push('\n');
            }
        }
        s.push_str("\nDocumentos disponibles (usá read_docs):\n");
        for d in self.docs.iter().take(80) {
            s.push_str("  - ");
            s.push_str(&d.name);
            s.push('\n');
        }
        s
    }
}

fn filename(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn add_root(roots: &mut Vec<PathBuf>, p: PathBuf) {
    if p.exists() && !roots.contains(&p) {
        roots.push(p);
    }
}

fn is_doc(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| DOC_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk(base: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(base) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}

fn floor_char(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nucoder-kb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn indexa_y_lee_documentos() {
        let dir = temp_dir("idx");
        std::fs::write(dir.join("SPASM_QUICKREF.md"), "# SpASM\nlenguaje propio").unwrap();
        std::fs::create_dir_all(dir.join("skills/spasm-architect")).unwrap();
        std::fs::write(
            dir.join("skills/spasm-architect/SKILL.md"),
            "detalle del skill",
        )
        .unwrap();
        let kb = Knowledge::from_roots(vec![
            dir.join("SPASM_QUICKREF.md"),
            dir.join("skills/spasm-architect"),
        ]);
        assert_eq!(kb.docs().len(), 2);
        assert_eq!(
            kb.read("SPASM_QUICKREF.md").unwrap().1,
            "# SpASM\nlenguaje propio"
        );
        assert_eq!(
            kb.read("skills/spasm-architect/SKILL.md").unwrap().1,
            "detalle del skill"
        );
        // Por sufijo y por "contiene".
        assert!(kb.read("SKILL.md").is_some());
        let p = kb.primer();
        assert!(p.contains("# SpASM"));
        assert!(p.contains("read_docs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kb_vacia_no_inyecta_primer() {
        let kb = Knowledge::from_roots(Vec::new());
        assert!(kb.is_empty());
        assert!(kb.primer().is_empty());
    }
}
