//! Descubrimiento del toolchain de SpASM.
//!
//! NuIde no asume rutas fijas: busca el intérprete (`spasm.exe`) y el compilador
//! AOT (`spasmc.py`) en variables de entorno, en las ubicaciones conocidas del
//! ecosistema (`F:\SpASM`, `F:\SpAMS-Oficial`) y, por último, en el `PATH`.
//!
//! La IDE **siempre compila** el código SpASM a binario nativo (AOT): transpila
//! a C portable con `spasmc.py --target c` y compila ese `.c` con el compilador
//! nativo detectado (gcc/clang/tcc o MSVC). El intérprete `spasm.exe` se
//! descubre pero **no se usa** en el flujo de la IDE.
//!
//! Ninguna herramienta es obligatoria: si falta, la IDE lo dice en la terminal y
//! sigue funcionando como consola de shell.

use std::path::{Path, PathBuf};

/// Compilador de C nativo disponible, para pasar del `.c` al binario.
#[derive(Debug, Clone)]
pub enum CCompiler {
    /// `gcc`/`clang`/`cc`/`tcc` directo.
    Direct(PathBuf),
    /// MSVC: hay que cargar el entorno con `vcvars64.bat` antes de `cl`.
    Msvc(PathBuf),
}

/// Rutas de las herramientas de SpASM disponibles en esta máquina.
#[derive(Debug, Clone, Default)]
pub struct Toolchain {
    /// Intérprete `spasm.exe` (sólo informativo: la IDE compila, no interpreta).
    pub interpreter: Option<PathBuf>,
    /// Compilador AOT `spasmc.py` (dispatcher de targets).
    pub compiler: Option<PathBuf>,
    /// Intérprete de Python con el que correr `spasmc.py`.
    pub python: Option<PathBuf>,
    /// Compilador de C (`CC` o uno del `PATH`). Sin él, `spasmc --build` no
    /// puede pasar del `.c` al binario.
    pub cc: Option<String>,
    /// Compilador de C nativo detectado (PATH o MSVC), para el paso final.
    pub cc_native: Option<CCompiler>,
    /// CLI de OpenCode (`opencode.cmd` en Windows, `opencode` en el resto). Con
    /// él funciona el panel de agente.
    pub agent: Option<PathBuf>,
    /// Frontend del compilador (`spasm-asm-gen.py`): tiene el tokenizer, el
    /// parser y el chequeo de tipos que usa el diagnóstico.
    pub frontend: Option<PathBuf>,
}

impl Toolchain {
    /// Busca las herramientas. Se puede forzar cada una con
    /// `NUIDE_SPASM`, `NUIDE_SPASMC` y `NUIDE_PYTHON`.
    pub fn discover() -> Self {
        let compiler = from_env("NUIDE_SPASMC")
            .or_else(|| from_env("SPASMC"))
            .or_else(|| first_existing(&[r"F:\SpAMS-Oficial\src\shared\spasmc.py"]))
            .or_else(|| on_path(&["spasmc.py"]));
        let frontend = from_env("NUIDE_SPASM_FRONTEND")
            .or_else(|| from_env("SPASM_FRONTEND"))
            .or_else(|| compiler.as_deref().and_then(frontend_beside))
            .or_else(|| first_existing(&[r"F:\SpAMS-Oficial\src\common\spasm-asm-gen.py"]))
            .or_else(|| on_path(&["spasm-asm-gen.py"]));
        Self {
            interpreter: from_env("NUIDE_SPASM")
                .or_else(|| from_env("SPASM_EXE"))
                .or_else(|| first_existing(&[r"F:\SpASM\spasm.exe"]))
                .or_else(|| on_path(&["spasm.exe", "spasm"])),
            compiler,
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
            cc_native: detect_cc_native(),
            agent: from_env("NUIDE_AGENT")
                .or_else(|| on_path(&["opencode.cmd", "opencode.exe", "opencode"])),
            frontend,
        }
    }

    /// `(python, argumentos)` para **transpilar** `source` a C en `out_c`
    /// (`spasmc.py --target c`). No usa `--build`: el paso nativo lo hace la IDE.
    pub fn transpile_command(
        &self,
        source: &Path,
        out_c: &Path,
    ) -> Result<(PathBuf, Vec<String>), String> {
        let python = self
            .python
            .clone()
            .ok_or_else(|| "no se encontró Python para correr spasmc.py".to_string())?;
        let compiler = self.compiler.clone().ok_or_else(|| {
            "no se encontró el compilador spasmc.py (definí NUIDE_SPASMC)".to_string()
        })?;
        let mut args = vec![compiler.display().to_string()];
        args.extend(compile_args(source, out_c, false));
        Ok((python, args))
    }

    /// `(programa, argumentos)` para compilar el `.c` a `out_exe` con el
    /// compilador nativo detectado (o `None` si no hay).
    pub fn native_command(&self, out_c: &Path, out_exe: &Path) -> Option<(PathBuf, Vec<String>)> {
        match self.cc_native.as_ref()? {
            CCompiler::Direct(cc) => {
                let mut args = vec![
                    out_c.display().to_string(),
                    "-o".to_string(),
                    out_exe.display().to_string(),
                ];
                if cfg!(not(windows)) {
                    args.push("-lm".to_string());
                }
                Some((cc.clone(), args))
            }
            CCompiler::Msvc(vcvars) => {
                // `cmd /C "call vcvars && cl ..."` tiene problemas de
                // entrecomillado; se escribe un `.bat` y se lo corre. Se hace
                // `cd` a la carpeta de salida para que el `.obj` no ensucie el
                // proyecto.
                let out_dir = out_exe
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                let bat = out_exe.with_extension("build.bat");
                let script = format!(
                    "@echo off\r\ncall \"{}\" >NUL\r\nif errorlevel 1 exit /b 1\r\ncd /d \"{}\"\r\ncl /nologo /O2 \"{}\" /Fe:\"{}\"\r\nexit /b %errorlevel%\r\n",
                    vcvars.display(),
                    out_dir.display(),
                    out_c.display(),
                    out_exe.display()
                );
                if std::fs::write(&bat, script).is_err() {
                    return None;
                }
                Some((
                    PathBuf::from("cmd"),
                    vec!["/C".to_string(), bat.display().to_string()],
                ))
            }
        }
    }

    /// Compila `source` a binario nativo dentro de `out_dir`, corriendo
    /// `spasmc.py` (transpila a C) y el compilador de C detectado de forma
    /// **bloqueante** con la salida capturada. Devuelve la ruta del binario o
    /// `Err` con la salida del paso que falló. Lo usa el módulo de despliegue
    /// para correr el script sin intérprete.
    pub fn compile_binary(&self, source: &Path, out_dir: &Path) -> Result<PathBuf, String> {
        use std::process::Command;

        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string());
        let _ = std::fs::create_dir_all(out_dir);
        let out_c = out_dir.join(format!("{stem}.c"));
        let out_exe = out_dir.join(format!("{stem}.exe"));

        // 1) spasmc.py --target c
        let (python, args) = self.transpile_command(source, &out_c)?;
        let out = Command::new(&python)
            .args(&args)
            .current_dir(out_dir)
            .output()
            .map_err(|e| format!("no se pudo lanzar spasmc.py: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "spasmc.py falló ({}): {}{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }

        // 2) compilador de C nativo (.c -> binario)
        let Some((cc, cc_args)) = self.native_command(&out_c, &out_exe) else {
            return Err(
                "no hay compilador de C (gcc/clang o MSVC) para pasar del .c al binario; definí NUIDE_CC"
                    .to_string(),
            );
        };
        let out = Command::new(&cc)
            .args(&cc_args)
            .current_dir(out_dir)
            .output()
            .map_err(|e| format!("no se pudo lanzar el compilador de C: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "el compilador de C falló ({}): {}{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(out_exe)
    }
}

/// Detecta un compilador de C nativo: `NUIDE_CC`/`CC`, uno del `PATH`, o MSVC
/// (`vcvars64.bat` de Visual Studio).
fn detect_cc_native() -> Option<CCompiler> {
    if let Some(cc) = from_env("NUIDE_CC").or_else(|| from_env("CC")) {
        return Some(CCompiler::Direct(cc));
    }
    if let Some(cc) = on_path(&[
        "gcc.exe",
        "gcc",
        "clang.exe",
        "clang",
        "cc.exe",
        "cc",
        "tcc.exe",
        "tcc",
    ]) {
        return Some(CCompiler::Direct(cc));
    }
    find_vcvars().map(CCompiler::Msvc)
}

/// Busca `vcvars64.bat` en las instalaciones de Visual Studio.
fn find_vcvars() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // `vswhere` es lo canónico.
        let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
        if std::path::Path::new(vswhere).is_file() {
            let out = std::process::Command::new(vswhere)
                .args(["-latest", "-products", "*", "-property", "installationPath"])
                .output();
            if let Ok(o) = out {
                let base = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !base.is_empty() {
                    let cand = PathBuf::from(base)
                        .join("VC")
                        .join("Auxiliary")
                        .join("Build")
                        .join("vcvars64.bat");
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
        // Reserva: recorre las dos raíces conocidas (dos niveles).
        for root in [
            r"C:\Program Files\Microsoft Visual Studio",
            r"C:\Program Files (x86)\Microsoft Visual Studio",
        ] {
            if let Ok(years) = std::fs::read_dir(root) {
                for year in years.flatten() {
                    if let Ok(editions) = std::fs::read_dir(year.path()) {
                        for edition in editions.flatten() {
                            let cand = edition
                                .path()
                                .join("VC")
                                .join("Auxiliary")
                                .join("Build")
                                .join("vcvars64.bat");
                            if cand.is_file() {
                                return Some(cand);
                            }
                        }
                    }
                }
            }
        }
    }
    None
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

/// El frontend vive junto a `spasmc.py`: `src/shared/spasmc.py` →
/// `src/common/spasm-asm-gen.py`.
fn frontend_beside(compiler: &Path) -> Option<PathBuf> {
    let src = compiler.parent()?.parent()?;
    let candidate = src.join("common").join("spasm-asm-gen.py");
    candidate.is_file().then_some(candidate)
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
