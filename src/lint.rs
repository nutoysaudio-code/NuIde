//! Diagnóstico de SpASM usando el parser real del ecosistema.
//!
//! Invoca el frontend `spasm-asm-gen.py` (tokenizer + parser + chequeo de
//! tipos) con el intérprete de Python, en un hilo aparte, y devuelve los
//! errores con su número de línea. No es un language server: alcanza con
//! parsear el archivo y reportar el primer error del dialecto compilado.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::thread;

/// Un error de SpASM. `line` es 1-based; 0 significa "sin línea" (por ejemplo,
/// falta el bloque `main`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

/// Resultado de una corrida del linter.
pub struct LintResult {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

/// Script de Python que carga el frontend por ruta y reporta los diagnósticos
/// como líneas `linea<TAB>mensaje` (sin JSON, para no sumar dependencias).
const LINT_PY: &str = r#"
import sys, importlib.util
frontend, source = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("spasm_frontend", frontend)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
with open(source, encoding="utf-8") as fh:
    text = fh.read()

def emit(msg):
    line = 0
    if msg.startswith("linea "):
        num, _, tail = msg[6:].partition(":")
        if num.strip().isdigit():
            line = int(num.strip())
            msg = tail.strip()
    print(f"{line}\t{msg}")

try:
    prog = mod.Parser(mod.tokenize(text)).parse()
    mod.analyze(prog)
except mod.SpasmAsmError as exc:
    emit(str(exc))
except Exception as exc:
    emit(str(exc))
"#;

/// Corre el analizador de SpASM sobre `source` en un hilo y manda el resultado
/// por `tx`. Si el proceso no arranca, devuelve un diagnóstico con el motivo.
pub fn spawn(python: PathBuf, frontend: PathBuf, source: PathBuf, tx: Sender<LintResult>) {
    thread::spawn(move || {
        let mut cmd = Command::new(&python);
        cmd.arg("-c").arg(LINT_PY).arg(&frontend).arg(&source);

        // En Windows no queremos que parpadee una consola.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let diagnostics = match cmd.output() {
            Ok(output) => parse(&String::from_utf8_lossy(&output.stdout)),
            Err(e) => vec![Diagnostic {
                line: 0,
                message: format!("no se pudo ejecutar el analizador de SpASM: {e}"),
            }],
        };
        let _ = tx.send(LintResult {
            path: source,
            diagnostics,
        });
    });
}

/// Parsea la salida `linea<TAB>mensaje` del script.
fn parse(stdout: &str) -> Vec<Diagnostic> {
    stdout
        .lines()
        .filter_map(|line| {
            let (line_no, message) = line.split_once('\t')?;
            let line = line_no.trim().parse::<usize>().ok()?;
            (!message.trim().is_empty()).then(|| Diagnostic {
                line,
                message: message.trim().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_las_lineas_del_linter() {
        let out = "3\tvariable no declarada: x\n0\tfalta el bloque 'main { ... }'\n";
        let diags = parse(out);
        assert_eq!(
            diags,
            vec![
                Diagnostic {
                    line: 3,
                    message: "variable no declarada: x".into()
                },
                Diagnostic {
                    line: 0,
                    message: "falta el bloque 'main { ... }'".into()
                },
            ]
        );
    }

    #[test]
    fn ignora_lineas_vacias_o_raras() {
        assert!(parse("").is_empty());
        assert!(parse("basura sin tab\n\n").is_empty());
    }

    /// Prueba de punta a punta con el parser real (se saltea si falta Python o
    /// el frontend de SpASM).
    #[test]
    fn corre_el_analizador_si_esta_disponible() {
        let toolchain = crate::spasm::Toolchain::discover();
        let (Some(python), Some(frontend)) = (toolchain.python, toolchain.frontend) else {
            eprintln!("test salteado: falta Python o el frontend de SpASM");
            return;
        };

        let dir = std::env::temp_dir().join(format!("nuide-lint-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bad = dir.join("mal.spasm");
        std::fs::write(
            &bad,
            "programa \"x\"\n\nmain {\n    let a = no_existe()\n}\n",
        )
        .unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        spawn(python, frontend, bad.clone(), tx);
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(40))
            .expect("el linter devolvió un resultado");
        assert_eq!(result.path, bad);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.line == 4 && d.message.contains("no declarada")),
            "diagnósticos inesperados: {:?}",
            result.diagnostics
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
