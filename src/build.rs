//! Ejecución del toolchain de SpASM desde la IDE.
//!
//! A diferencia de escribir la orden en la shell, acá se corre el proceso, se
//! captura su salida (stdout + stderr) y se **parsean los diagnósticos** (con su
//! número de línea) para marcarlos en el editor. La salida cruda se muestra en
//! una sesión de terminal dedicada ("build").

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

use crate::lint::Diagnostic;

/// Resultado de una corrida del toolchain.
pub struct BuildResult {
    /// Época de la corrida (para descartar resultados viejos).
    pub gen: u64,
    /// Diagnósticos parseados (línea + mensaje).
    pub diagnostics: Vec<Diagnostic>,
    /// Salida cruda (stdout + stderr) para mostrar en la terminal.
    pub output: String,
    /// Código de salida del último paso que corrió.
    pub code: i32,
}

/// Un paso de la cadena de build: un programa con sus argumentos.
pub struct Step {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Corre los `steps` en orden en `cwd` (en un hilo), captura la salida y manda el
/// resultado por `tx`. Se detiene en el primer paso que falle.
pub fn spawn(
    steps: Vec<Step>,
    cwd: PathBuf,
    gen: u64,
    cancel: Arc<AtomicBool>,
    tx: Sender<BuildResult>,
) {
    thread::spawn(move || {
        let mut text = String::new();
        let mut code = 0;
        for step in &steps {
            // Cancelación cooperativa: se comprueba entre pasos.
            if cancel.load(Ordering::Relaxed) {
                text.push_str("\n[build cancelado]\n");
                code = -1;
                break;
            }
            let mut cmd = Command::new(&step.program);
            cmd.args(&step.args).current_dir(&cwd);

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }

            match cmd.output() {
                Ok(out) => {
                    text.push_str(&String::from_utf8_lossy(&out.stdout));
                    let err = String::from_utf8_lossy(&out.stderr);
                    if !err.is_empty() {
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        text.push_str(&err);
                    }
                    code = out.status.code().unwrap_or(-1);
                    if code != 0 {
                        break;
                    }
                }
                Err(e) => {
                    text.push_str(&format!(
                        "no se pudo ejecutar {}: {e}\n",
                        step.program.display()
                    ));
                    code = -1;
                    break;
                }
            }
        }
        let _ = tx.send(BuildResult {
            gen,
            diagnostics: parse_diagnostics(&text),
            output: text,
            code,
        });
    });
}

/// Extrae diagnósticos (línea + mensaje) de la salida del compilador. Soporta el
/// formato de SpASM (`linea N: …`) y el de gcc/clang (`archivo:N:col: error: …`).
///
/// El análisis de cada línea vive en **SpASM** (`spasm/diagnostics.spasm`);
/// este bucle sólo itera y junta. Ver `crate::spasm_logic`.
pub fn parse_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(crate::spasm_logic::diagnostic_of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_el_formato_de_spasm() {
        let text = "Compilando...\nerror: linea 5: funcion no declarada: foo\n";
        let d = parse_diagnostics(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, 5);
        assert!(d[0].message.contains("no declarada"));
    }

    #[test]
    fn parsea_el_formato_de_gcc() {
        let text = "build/x.c:12:5: error: expected ';' before '}'\n";
        let d = parse_diagnostics(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, 12);
        assert!(d[0].message.contains("expected"));
    }

    #[test]
    fn ignora_la_salida_normal() {
        assert!(parse_diagnostics("ok: build/x.c\nsiguiente: gcc ...\n").is_empty());
    }

    /// Punta a punta con el compilador real (se saltea si falta el toolchain).
    #[test]
    fn corre_el_compilador_y_parsea_el_error() {
        let toolchain = crate::spasm::Toolchain::discover();
        let Some(python) = toolchain.python.clone() else {
            eprintln!("test salteado: falta Python");
            return;
        };
        // Se usa el compilador del repo del ecosistema (el instalado puede estar
        // incompleto). Se saltea si no está.
        let compiler = std::path::PathBuf::from(r"F:\SpAMS-Oficial\src\shared\spasmc.py");
        if !compiler.is_file() {
            eprintln!("test salteado: no está el compilador del ecosistema");
            return;
        }
        let dir = std::env::temp_dir().join(format!("nuide-build-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bad = dir.join("mal.spasm");
        std::fs::write(
            &bad,
            "programa \"x\"\n\nmain {\n    let a = no_existe()\n}\n",
        )
        .unwrap();
        let out_c = dir.join("mal.c");
        let mut args = vec![compiler.display().to_string()];
        args.extend(crate::spasm::compile_args(&bad, &out_c, false));

        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            vec![Step {
                program: python,
                args,
            }],
            dir.clone(),
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
        );
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(40))
            .expect("el build devolvió un resultado");
        assert_ne!(result.code, 0, "el build no debería haber tenido éxito");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.line == 4 && d.message.contains("no declarada")),
            "diagnósticos inesperados: {:?}\nsalida: {}",
            result.diagnostics,
            result.output
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Punta a punta del paso nativo: `spasmc` → `.c` → compilador de C → `.exe`.
    /// Se saltea si falta Python, el compilador o un compilador de C.
    #[test]
    fn compila_a_binario_nativo() {
        let toolchain = crate::spasm::Toolchain::discover();
        let Some(python) = toolchain.python.clone() else {
            eprintln!("test salteado: falta Python");
            return;
        };
        let compiler = std::path::PathBuf::from(r"F:\SpAMS-Oficial\src\shared\spasmc.py");
        if !compiler.is_file() {
            eprintln!("test salteado: no está el compilador del ecosistema");
            return;
        }
        let dir = std::env::temp_dir().join(format!("nuide-native-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("prog.spasm");
        std::fs::write(
            &src,
            "programa \"x\"\n\nmain {\n    print \"hola nativo\"\n}\n",
        )
        .unwrap();
        let out_c = dir.join("prog.c");
        let out_exe = dir.join("prog.exe");
        let Some((cc, cc_args)) = toolchain.native_command(&out_c, &out_exe) else {
            eprintln!("test salteado: no hay compilador de C (gcc/clang/MSVC)");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        let mut args = vec![compiler.display().to_string()];
        args.extend(crate::spasm::compile_args(&src, &out_c, false));
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            vec![
                Step {
                    program: python,
                    args,
                },
                Step {
                    program: cc,
                    args: cc_args,
                },
            ],
            dir.clone(),
            0,
            Arc::new(AtomicBool::new(false)),
            tx,
        );
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(180))
            .expect("el build devolvió un resultado");
        assert!(
            out_exe.is_file(),
            "no se generó el binario (código {})\nsalida: {}",
            result.code,
            result.output
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
