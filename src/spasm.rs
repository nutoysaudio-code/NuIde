//! Descubrimiento del toolchain de SpASM.
//!
//! NuIde no asume rutas fijas: busca el intérprete (`spasm.exe`) y el compilador
//! AOT (`spasmc.py`) en variables de entorno, en las ubicaciones conocidas del
//! ecosistema (`F:\SpASM`, `F:\SpAMS-Oficial`) y, por último, en el `PATH`.
//!
//! El intérprete corre un `.spasm` directamente. El compilador AOT transpila a C
//! portable (`--target c`); con `--build` llega hasta el binario nativo.
//!
//! Ninguna herramienta es obligatoria: si falta, la IDE lo dice en la terminal y
//! sigue funcionando como consola de shell.

use std::path::{Path, PathBuf};

/// Rutas de las herramientas de SpASM disponibles en esta máquina.
#[derive(Debug, Clone, Default)]
pub struct Toolchain {
    /// Intérprete `spasm.exe` (ejecuta `.spasm` sin compilar).
    pub interpreter: Option<PathBuf>,
    /// Compilador AOT `spasmc.py` (dispatcher de targets).
    pub compiler: Option<PathBuf>,
    /// Intérprete de Python con el que correr `spasmc.py`.
    pub python: Option<PathBuf>,
    /// Compilador de C (`CC` o uno del `PATH`). Sin él, `spasmc --build` no
    /// puede pasar del `.c` al binario.
    pub cc: Option<String>,
    /// CLI de OpenCode (`opencode.cmd` en Windows, `opencode` en el resto). Con
    /// él funciona el panel de agente.
    pub agent: Option<PathBuf>,
}

impl Toolchain {
    /// Busca las herramientas. Se puede forzar cada una con
    /// `NUIDE_SPASM`, `NUIDE_SPASMC` y `NUIDE_PYTHON`.
    pub fn discover() -> Self {
        Self {
            interpreter: from_env("NUIDE_SPASM")
                .or_else(|| from_env("SPASM_EXE"))
                .or_else(|| first_existing(&[r"F:\SpASM\spasm.exe"]))
                .or_else(|| on_path(&["spasm.exe", "spasm"])),
            compiler: from_env("NUIDE_SPASMC")
                .or_else(|| from_env("SPASMC"))
                .or_else(|| first_existing(&[r"F:\SpAMS-Oficial\src\shared\spasmc.py"]))
                .or_else(|| on_path(&["spasmc.py"])),
            python: from_env("NUIDE_PYTHON")
                .or_else(|| from_env("PYTHON"))
                .or_else(|| on_path(&["python.exe", "python3.exe", "py.exe"]))
                .or_else(|| on_path(&["python", "python3", "py"])),
            cc: std::env::var("CC")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    on_path(&[
                        "gcc.exe",
                        "gcc",
                        "clang.exe",
                        "clang",
                        "cc.exe",
                        "cc",
                        "tcc.exe",
                        "tcc",
                    ])
                    .map(|p| p.display().to_string())
                }),
            agent: from_env("NUIDE_AGENT")
                .or_else(|| on_path(&["opencode.cmd", "opencode.exe", "opencode"])),
        }
    }

    /// Comando para pedirle algo al agente de OpenCode.
    ///
    /// Usa el modo no interactivo `opencode run <mensaje>`: cada envío es un
    /// proceso nuevo, que es lo que encaja con la consola lineal.
    pub fn agent_command(&self, message: &str) -> Result<(PathBuf, Vec<String>), String> {
        let bin = self
            .agent
            .clone()
            .ok_or_else(|| "no se encontró el CLI de OpenCode (definí NUIDE_AGENT)".to_string())?;
        Ok((bin, vec!["run".to_string(), message.to_string()]))
    }

    /// Intérprete listo para usar, o `None` con un motivo para mostrar.
    pub fn interpreter_required(&self) -> Result<PathBuf, String> {
        self.interpreter
            .clone()
            .ok_or_else(|| "no se encontró el intérprete spasm.exe (definí NUIDE_SPASM)".into())
    }

    /// Par `(python, argumentos de spasmc, ¿compiló a binario?)`.
    ///
    /// El tercer valor es `false` cuando no hay compilador de C: en ese caso
    /// `spasmc` transpila a `.c` y para, y la UI lo avisa.
    pub fn compile_command(
        &self,
        source: &Path,
        out_c: &Path,
    ) -> Result<(PathBuf, Vec<String>, bool), String> {
        let python = self
            .python
            .clone()
            .ok_or_else(|| "no se encontró Python para correr spasmc.py".to_string())?;
        let compiler = self.compiler.clone().ok_or_else(|| {
            "no se encontró el compilador spasmc.py (definí NUIDE_SPASMC)".to_string()
        })?;
        let build = self.cc.is_some();
        let mut args = vec![compiler.display().to_string()];
        args.extend(compile_args(source, out_c, build));
        Ok((python, args, build))
    }
}

/// Argumentos de `spasmc.py` para transpilar `source` a C en `out_c`.
///
/// Con `build` se agrega `--build`: `spasmc` encadena `spasm-c-gen.py --build`,
/// que invoca el compilador de C (`CC` o `cc`). Al target `c` se le pasa `--out`
/// con el archivo `.c`; `--out-dir` es exclusivo del target `linux-kmod`.
pub fn compile_args(source: &Path, out_c: &Path, build: bool) -> Vec<String> {
    let mut args = vec![
        source.display().to_string(),
        "--target".to_string(),
        "c".to_string(),
        "--out".to_string(),
        out_c.display().to_string(),
    ];
    if build {
        args.push("--build".to_string());
    }
    args
}

/// Valor de una variable de entorno que apunte a un archivo existente.
fn from_env(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Primera ruta de la lista que exista.
fn first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// Busca uno de los nombres en las carpetas del `PATH`.
///
/// En Windows un `python` del `PATH` vive como `python.exe`, y el kernel lo
/// resuelve solo al lanzarlo, pero para *verificar* existencia hay que probar el
/// nombre con extensión. Por eso el llamador pasa las dos variantes.
fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in names {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_args_usa_out_no_out_dir() {
        let args = compile_args(Path::new("prog.spasm"), Path::new("build/prog.c"), true);
        assert_eq!(args[0], "prog.spasm");
        assert!(args.windows(2).any(|w| w == ["--target", "c"]));
        assert!(args.windows(2).any(|w| w == ["--out", "build/prog.c"]));
        assert!(args.iter().any(|a| a == "--build"));
        // `--out-dir` es del target kmod; con `c` rompe.
        assert!(!args.iter().any(|a| a == "--out-dir"));
    }

    #[test]
    fn sin_compilador_de_c_no_se_pide_build() {
        let args = compile_args(Path::new("prog.spasm"), Path::new("build/prog.c"), false);
        assert!(!args.iter().any(|a| a == "--build"));
    }

    #[test]
    fn agent_command_arma_run_con_el_mensaje() {
        let tc = Toolchain {
            agent: Some(PathBuf::from("opencode.cmd")),
            ..Default::default()
        };
        let (prog, args) = tc.agent_command("hola").expect("arma el comando");
        assert_eq!(prog, PathBuf::from("opencode.cmd"));
        assert_eq!(args, vec!["run", "hola"]);

        let vacio = Toolchain::default();
        assert!(vacio.agent_command("x").is_err());
    }

    /// Prueba real de que el CLI de OpenCode se puede lanzar y su salida llega
    /// por el canal (en Windows es un `.cmd`). Se saltea si no está instalado.
    #[test]
    fn el_agente_responde_a_version() {
        let Some(agent) = Toolchain::discover().agent else {
            eprintln!("test salteado: no se encontró opencode");
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        crate::terminal::spawn_streaming(
            0,
            &std::env::temp_dir(),
            &agent.display().to_string(),
            &["--version".to_string()],
            tx,
        )
        .expect("lanzar opencode");

        let mut lines = Vec::new();
        let mut exited = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !exited && std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(crate::terminal::Msg::Line { text, .. }) => lines.push(text),
                Ok(crate::terminal::Msg::Exit { .. }) => exited = true,
                Err(_) => {}
            }
        }
        assert!(
            lines.iter().any(|l| l.to_lowercase().contains("opencode")),
            "el agente no devolvió su versión: {lines:?}"
        );
    }

    /// Prueba de punta a punta del menú «Ejecutar»: el intérprete detectado corre
    /// un programa del dialecto de intérprete y su salida llega por el canal.
    /// Se saltea sola si en la máquina no hay `spasm.exe`.
    #[test]
    fn el_interprete_corre_un_programa_de_interprete() {
        let Some(exe) = Toolchain::discover().interpreter else {
            eprintln!("test salteado: no se encontró spasm.exe");
            return;
        };

        let dir = std::env::temp_dir().join(format!("nuide-asm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("hola.spasm");
        std::fs::write(&file, "print \"hola-test\";\nend;\n").expect("escribir el .spasm");

        let (tx, rx) = std::sync::mpsc::channel();
        crate::terminal::spawn_streaming(
            0,
            &dir,
            &exe.display().to_string(),
            &[file.display().to_string()],
            tx,
        )
        .expect("lanzar el intérprete");

        let mut lines = Vec::new();
        let mut exited = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !exited && std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(crate::terminal::Msg::Line { text, .. }) => lines.push(text),
                Ok(crate::terminal::Msg::Exit { .. }) => exited = true,
                Err(_) => {}
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            lines.iter().any(|l| l.contains("hola-test")),
            "el intérprete no devolvió la salida esperada: {lines:?}"
        );
    }
}
