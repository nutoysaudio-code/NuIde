//! Árbol de archivos del explorador.
//!
//! El modelo es una **lista plana con profundidad**: exactamente lo que dibuja
//! `IdeExplorer` en la UI (`for node[i] in nodes`). No hay un árbol vivo en la
//! UI; el aplanado lo hace este módulo, que además guarda el `path` real de cada
//! fila para poder actuar al hacer clic.
//!
//! La raíz es la carpeta del proyecto: la última elegida (recordada en la
//! config) o la carpeta de lanzamiento. Se cambia con `set_root`. Hay topes de
//! entradas por carpeta y de nodos totales para que una carpeta enorme
//! (`target/`, `node_modules/`) no cuelgue la UI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Tope de entradas listadas por carpeta. Lo que sobra se resume en una fila.
const MAX_PER_DIR: usize = 500;
/// Tope de nodos en el árbol completo.
const MAX_NODES: usize = 4000;

/// Carpetas generadas que se pueden ocultar del árbol.
const GENERATED: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "bin",
    "vendor",
];

/// ¿Es una entrada "generada" (se oculta cuando el filtro está activo)?
fn is_generated(name: &str) -> bool {
    name.starts_with('.') || GENERATED.contains(&name)
}

/// Un nodo ya aplanado, en el orden en que se dibuja.
#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub label: String,
    pub depth: i32,
    /// 0 archivo, 1 carpeta cerrada, 2 carpeta abierta, 3 configuración.
    pub kind: i32,
    pub is_dir: bool,
}

/// Qué pasó al activar una fila.
pub enum Action {
    /// Se abrió o cerró una carpeta: el árbol cambió.
    Toggled,
    /// Se clickeó un archivo.
    File(PathBuf),
}

pub struct Explorer {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    nodes: Vec<Node>,
    /// Mostrar carpetas/archivos generados (`.`-ocultos, `target`, …).
    show_generated: bool,
}

impl Explorer {
    /// Arranca con la raíz abierta.
    pub fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let mut e = Self {
            root,
            expanded,
            nodes: Vec::new(),
            show_generated: true,
        };
        e.rebuild();
        e
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// ¿Se muestran los archivos/carpetas generadas?
    pub fn shows_generated(&self) -> bool {
        self.show_generated
    }

    /// Cambia el filtro de generados y reconstruye.
    pub fn set_show_generated(&mut self, show: bool) {
        if self.show_generated == show {
            return;
        }
        self.show_generated = show;
        self.rebuild();
    }

    /// Cambia la raíz. Si es la misma, no toca nada.
    pub fn set_root(&mut self, root: PathBuf) {
        if root == self.root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        self.expanded.insert(self.root.clone());
        self.rebuild();
    }

    /// Abre o cierra una carpeta; en un archivo, avisa que hay que abrirlo.
    pub fn toggle(&mut self, index: usize) -> Option<Action> {
        let node = self.nodes.get(index)?.clone();
        if !node.is_dir {
            return Some(Action::File(node.path));
        }
        if !self.expanded.insert(node.path.clone()) {
            self.expanded.remove(&node.path);
        }
        self.rebuild();
        Some(Action::Toggled)
    }

    /// Reconstruye la lista plana desde disco.
    pub fn rebuild(&mut self) {
        self.nodes.clear();
        let root = self.root.clone();
        self.nodes.push(Node {
            label: dir_label(&root),
            path: root.clone(),
            depth: 0,
            kind: 2,
            is_dir: true,
        });
        self.walk(&root, 1);
    }

    fn walk(&mut self, dir: &Path, depth: i32) {
        if self.nodes.len() >= MAX_NODES {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };

        let show_generated = self.show_generated;
        let mut entries: Vec<(PathBuf, String, bool)> = rd
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if !show_generated && is_generated(&name) {
                    return None;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                Some((path, name, is_dir))
            })
            .collect();

        // Carpetas primero, después alfabético sin distinguir mayúsculas.
        entries.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });

        for (i, (path, label, is_dir)) in entries.into_iter().enumerate() {
            if self.nodes.len() >= MAX_NODES {
                break;
            }
            if i >= MAX_PER_DIR {
                self.nodes.push(Node {
                    path: dir.to_path_buf(),
                    label: format!("… ({} entradas más)", entries_len(dir)),
                    depth,
                    kind: 0,
                    is_dir: false,
                });
                break;
            }

            let open = is_dir && self.expanded.contains(&path);
            let kind = if is_dir {
                if open {
                    2
                } else {
                    1
                }
            } else {
                file_kind(&path)
            };
            self.nodes.push(Node {
                path: path.clone(),
                label,
                depth,
                kind,
                is_dir,
            });
            if open {
                self.walk(&path, depth + 1);
            }
        }
    }
}

/// Nombre visible de una carpeta, o la ruta entera si no tiene nombre.
fn dir_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Extensión de archivo de configuración → icono de engranaje (kind 3).
fn file_kind(path: &Path) -> i32 {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("toml" | "json" | "ini" | "cfg" | "yaml" | "yml" | "lock") => 3,
        _ => 0,
    }
}

/// Cuenta las entradas de una carpeta para la fila resumen, sin listarlas.
fn entries_len(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nuide-explorer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("crear la carpeta de prueba");
        d
    }

    #[test]
    fn la_raiz_arranca_abierta_y_lista_sus_hijos() {
        let dir = temp_dir("raiz");
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.spasm"), "end;").unwrap();

        let e = Explorer::new(dir.clone());
        let labels: Vec<&str> = e.nodes().iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels[0], dir.file_name().unwrap().to_str().unwrap());
        // Carpeta primero, después los archivos ordenados.
        assert_eq!(&labels[1..], ["sub", "a.spasm", "b.txt"]);
        assert_eq!(e.nodes()[1].kind, 1); // carpeta cerrada

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abrir_y_cerrar_una_carpeta_agrega_y_quita_hijos() {
        let dir = temp_dir("toggle");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("dentro.spasm"), "end;").unwrap();

        let mut e = Explorer::new(dir.clone());
        assert_eq!(e.nodes().len(), 2); // raíz + sub

        // `sub` está en el índice 1.
        assert!(matches!(e.toggle(1), Some(Action::Toggled)));
        assert_eq!(e.nodes().len(), 3);
        assert_eq!(e.nodes()[2].label, "dentro.spasm");

        assert!(matches!(e.toggle(1), Some(Action::Toggled)));
        assert_eq!(e.nodes().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clickear_un_archivo_devuelve_su_ruta() {
        let dir = temp_dir("archivo");
        std::fs::write(dir.join("x.spasm"), "end;").unwrap();
        let mut e = Explorer::new(dir.clone());

        let idx = e
            .nodes()
            .iter()
            .position(|n| n.label == "x.spasm")
            .expect("el archivo está listado");
        match e.toggle(idx) {
            Some(Action::File(p)) => assert_eq!(p, dir.join("x.spasm")),
            _ => panic!("esperaba Action::File"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cambiar_de_raiz_resetea_lo_abierto() {
        let a = temp_dir("root-a");
        let b = temp_dir("root-b");
        std::fs::create_dir(b.join("otro")).unwrap();

        let mut e = Explorer::new(a.clone());
        e.set_root(b.clone());
        assert_eq!(e.root(), b.as_path());
        assert!(e.nodes().iter().any(|n| n.label == "otro"));

        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn las_extensiones_conocidas_van_con_engranaje() {
        assert_eq!(file_kind(Path::new("a.toml")), 3);
        assert_eq!(file_kind(Path::new("a.spasm")), 0);
    }

    #[test]
    fn oculta_generados_cuando_el_filtro_esta_activo() {
        let dir = temp_dir("generated");
        std::fs::create_dir(dir.join("target")).unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        std::fs::create_dir(dir.join("src")).unwrap();

        let mut e = Explorer::new(dir.clone());
        let labels =
            |e: &Explorer| -> Vec<String> { e.nodes().iter().map(|n| n.label.clone()).collect() };
        assert!(labels(&e).iter().any(|l| l == "target"));
        assert!(labels(&e).iter().any(|l| l == "src"));
        assert!(e.shows_generated());

        e.set_show_generated(false);
        let names = labels(&e);
        assert!(!names.iter().any(|l| l == "target"));
        assert!(!names.iter().any(|l| l == ".git"));
        assert!(names.iter().any(|l| l == "src"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
