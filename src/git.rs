//! Integración con Git vía la línea de comandos.
//!
//! No agrega dependencias: ejecuta `git` en la raíz del proyecto y parsea
//! `status --porcelain`. Las operaciones que pueden tardar (commit, push) corren
//! en un hilo y avisan por un canal, igual que el linter y el build.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::thread;

/// Un archivo con cambios. `index` es el estado en el índice (`X`) y `worktree`
/// el del directorio de trabajo (`Y`), como los reporta `git status --porcelain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Ruta relativa a la raíz del repositorio.
    pub path: String,
    pub index: char,
    pub worktree: char,
}

impl Change {
    /// El cambio está preparado (en el índice).
    pub fn staged(&self) -> bool {
        self.index != ' ' && self.index != '?'
    }

    /// Texto de dos columnas para la UI (`M `, ` M`, `??`, `A `, …).
    pub fn status(&self) -> String {
        if self.index == '?' {
            return "??".to_string();
        }
        format!("{}{}", self.index, self.worktree)
    }
}

/// Estado del repositorio: rama y archivos con cambios.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// La carpeta es un repositorio Git.
    pub is_repo: bool,
    pub branch: String,
    pub changes: Vec<Change>,
}

/// Operación pedida desde la UI.
pub enum Op {
    /// Sólo releer el estado.
    Refresh,
    /// `git init` (cuando la carpeta no es un repositorio).
    Init,
    /// `git add -A`.
    StageAll,
    /// Preparar (`stage`) o quitar del índice (`!stage`) un archivo.
    Toggle { path: String, stage: bool },
    /// `git commit -m <mensaje>`.
    Commit(String),
    /// `git push`.
    Push,
}

/// Resultado de una operación, con el estado ya refrescado.
pub struct GitResult {
    /// Mensaje para mostrar al usuario.
    pub note: String,
    /// El mensaje es un error.
    pub error: bool,
    /// Se creó un commit (para limpiar el campo de mensaje).
    pub committed: bool,
    pub status: Status,
}

/// Lee el estado del repositorio en `root`.
pub fn status(root: &Path) -> Status {
    let out = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain=v1", "-b"])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_status(&String::from_utf8_lossy(&o.stdout)),
        _ => Status::default(),
    }
}

/// Parsea la salida de `git status --porcelain=v1 -b`.
fn parse_status(text: &str) -> Status {
    let mut st = Status {
        is_repo: true,
        ..Default::default()
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            st.branch = branch_of(rest);
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let b = line.as_bytes();
        let index = b[0] as char;
        let worktree = b[1] as char;
        // `line[3..]` salta "XY ". En renombres, `git` reporta "viejo -> nuevo".
        let mut path = line[3..].to_string();
        if let Some((_, new)) = path.split_once(" -> ") {
            path = new.to_string();
        }
        // `git` entrecomilla rutas con caracteres especiales.
        if path.starts_with('"') && path.ends_with('"') && path.len() >= 2 {
            path = path[1..path.len() - 1].to_string();
        }
        st.changes.push(Change {
            path,
            index,
            worktree,
        });
    }
    st
}

/// Extrae el nombre de la rama de la primera línea de `status -b`
/// (`main...origin/main`, `No commits yet on main`, `HEAD (no branch)`).
fn branch_of(rest: &str) -> String {
    let rest = rest.trim();
    if let Some(name) = rest.strip_prefix("No commits yet on ") {
        return name.trim().to_string();
    }
    if let Some(head) = rest.strip_prefix("HEAD (no branch)") {
        return head.trim().to_string();
    }
    rest.split(['.', ' ', '('])
        .next()
        .unwrap_or(rest)
        .trim()
        .to_string()
}

/// Corre `git` en `root` y devuelve la salida (stdout, o stderr si stdout está
/// vacío). Un fallo devuelve el stderr como error.
fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        Err(if stderr.is_empty() {
            format!("git terminó con {}", out.status)
        } else {
            stderr
        })
    }
}

/// Ejecuta la operación sin hilo (la usan los tests y el hilo de trabajo).
pub fn perform(root: &Path, op: &Op) -> Result<String, String> {
    match op {
        Op::Refresh => Ok(String::new()),
        Op::Init => run(root, &["init"]).map(|_| "Repositorio inicializado.".to_string()),
        Op::StageAll => run(root, &["add", "-A"]).map(|_| "Cambios preparados.".to_string()),
        Op::Toggle { path, stage } => if *stage {
            run(root, &["add", "--", path])
        } else {
            run(root, &["reset", "-q", "HEAD", "--", path])
        }
        .map(|_| {
            if *stage {
                format!("Preparado: {path}")
            } else {
                format!("Quitado del índice: {path}")
            }
        }),
        Op::Commit(msg) => run(root, &["commit", "-m", msg]).map(|_| "Commit creado.".to_string()),
        Op::Push => run(root, &["push"]).map(|_| "Push realizado.".to_string()),
    }
}

/// Corre la operación en un hilo y manda el resultado (con el estado nuevo).
pub fn spawn(root: PathBuf, op: Op, tx: Sender<GitResult>) {
    thread::spawn(move || {
        let committed = matches!(op, Op::Commit(_));
        let (note, error) = match perform(&root, &op) {
            Ok(msg) => (msg, false),
            Err(e) => (e, true),
        };
        let status = status(&root);
        let _ = tx.send(GitResult {
            note,
            error,
            committed: committed && !error,
            status,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_rama_y_cambios() {
        let text = "## main...origin/main [ahead 1]\n M src/a.spasm\nM  src/b.spasm\n?? nuevo.txt\nA  preparado.txt\n";
        let st = parse_status(text);
        assert!(st.is_repo);
        assert_eq!(st.branch, "main");
        assert_eq!(st.changes.len(), 4);
        assert_eq!(st.changes[0].path, "src/a.spasm");
        assert_eq!(st.changes[0].status(), " M");
        assert!(!st.changes[0].staged());
        assert!(st.changes[1].staged());
        assert_eq!(st.changes[2].status(), "??");
        assert!(!st.changes[2].staged());
    }

    #[test]
    fn parsea_rama_sin_commits_y_renombres() {
        let st = parse_status("## No commits yet on main\nR  viejo.txt -> nuevo.txt\n");
        assert_eq!(st.branch, "main");
        assert_eq!(st.changes[0].path, "nuevo.txt");
        assert_eq!(st.changes[0].index, 'R');

        let st = parse_status("## HEAD (no branch)\n");
        assert_eq!(st.branch, "");
    }

    #[test]
    fn parsea_rutas_entrecomilladas() {
        let st = parse_status("?? \"con espacio.txt\"\n");
        assert_eq!(st.changes[0].path, "con espacio.txt");
    }

    #[test]
    fn una_carpeta_normal_no_es_repo() {
        let dir = std::env::temp_dir().join(format!("nuide-git-norepo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // En Windows, TEMP suele no ser un repo; si lo fuera, el test no aplica.
        let st = status(&dir);
        if !st.is_repo {
            assert!(st.branch.is_empty());
            assert!(st.changes.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flujo_real_de_un_repositorio_temporal() {
        // Sin `git` instalado, el test se saltea.
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nuide-git-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        run(&dir, &["init"]).unwrap();
        run(&dir, &["config", "user.email", "test@nuide.local"]).unwrap();
        run(&dir, &["config", "user.name", "NuIde Test"]).unwrap();
        run(&dir, &["config", "commit.gpgsign", "false"]).unwrap();

        // Un archivo nuevo aparece como no preparado.
        std::fs::write(dir.join("a.txt"), "hola\n").unwrap();
        let st = status(&dir);
        assert!(st.is_repo, "debería ser un repo");
        assert!(st.changes.iter().any(|c| c.path == "a.txt" && !c.staged()));

        // Preparar todo y commitear limpia el estado.
        perform(&dir, &Op::StageAll).unwrap();
        assert!(status(&dir).changes.iter().any(|c| c.staged()));
        perform(&dir, &Op::Commit("inicial".into())).unwrap();
        assert!(status(&dir).changes.is_empty());

        // Modificar, preparar y volver a quitar del índice.
        std::fs::write(dir.join("a.txt"), "chau\n").unwrap();
        assert_eq!(status(&dir).changes.len(), 1);
        perform(
            &dir,
            &Op::Toggle {
                path: "a.txt".into(),
                stage: true,
            },
        )
        .unwrap();
        assert!(status(&dir).changes[0].staged());
        perform(
            &dir,
            &Op::Toggle {
                path: "a.txt".into(),
                stage: false,
            },
        )
        .unwrap();
        assert!(!status(&dir).changes[0].staged());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
