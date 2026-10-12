//! Diagnóstico y esquema de SpASM usando el parser real del ecosistema.
//!
//! Invoca el frontend `spasm-asm-gen.py` (tokenizer + parser + chequeo de
//! tipos) con el intérprete de Python, en un hilo aparte, y devuelve:
//!
//! - los **errores** con su número de línea (diagnóstico), y
//! - el **esquema de símbolos** (funciones y variables) del AST, para el outline
//!   y "ir a definición".

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

/// Un símbolo del archivo (función o variable) para el esquema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// 0 función, 1 variable local.
    pub kind: i32,
    pub name: String,
    /// Línea (1-based) donde se declara.
    pub line: usize,
    /// Detalle (tipo de retorno de una función).
    pub detail: String,
}

/// Resultado de una corrida del analizador.
pub struct LintResult {
    /// Época de la corrida (para descartar resultados viejos).
    pub gen: u64,
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<Symbol>,
}

/// Script de Python que carga el frontend por ruta y reporta diagnósticos y
/// símbolos con un formato de líneas separadas por tabuladores (sin JSON, para
/// no sumar dependencias).
const LINT_PY: &str = r#"
import sys, importlib.util
frontend, source = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("spasm_frontend", frontend)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
with open(source, encoding="utf-8") as fh:
    text = fh.read()

def walk(node, out):
    if isinstance(node, tuple):
        if node and node[0] in ("let", "recurso", "recurso_file"):
            name = node[1]
            line = node[-1] if isinstance(node[-1], int) else 0
            out.append((1, name, line, ""))
        for x in node:
            walk(x, out)
    elif isinstance(node, list):
        for x in node:
            walk(x, out)

diags = []
symbols = []
try:
    prog = mod.Parser(mod.tokenize(text)).parse()
    mod.analyze(prog)
    for f in prog.funcs:
        name, body, line = f[1], f[3], f[4]
        detail = f[5] if len(f) > 5 else ""
        symbols.append((0, name, line, detail))
        walk(body, symbols)
    walk(prog.main_body, symbols)
except mod.SpasmAsmError as exc:
    diags.append(str(exc))
except Exception as exc:
    diags.append(str(exc))

for d in diags:
    line, msg = 0, d
    if d.startswith("linea "):
        num, _, tail = d[6:].partition(":")
        if num.strip().isdigit():
            line, msg = int(num.strip()), tail.strip()
    print(f"D\t{line}\t{msg}")
for kind, name, line, detail in symbols:
    print(f"S\t{kind}\t{name}\t{line}\t{detail}")
"#;

/// Corre el analizador de SpASM sobre `source` en un hilo y manda el resultado
/// por `tx`. Si el proceso no arranca, devuelve un diagnóstico con el motivo.
pub fn spawn(
    python: PathBuf,
    frontend: PathBuf,
    source: PathBuf,
    gen: u64,
    tx: Sender<LintResult>,
) {
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

        let (diagnostics, symbols) = match cmd.output() {
            Ok(output) => parse(&String::from_utf8_lossy(&output.stdout)),
            Err(e) => (
                vec![Diagnostic {
                    line: 0,
                    message: format!("no se pudo ejecutar el analizador de SpASM: {e}"),
                }],
                Vec::new(),
            ),
        };
        let _ = tx.send(LintResult {
            gen,
            path: source,
            diagnostics,
            symbols,
        });
    });
}

/// Parsea la salida `D\t…` / `S\t…` del script.
fn parse(stdout: &str) -> (Vec<Diagnostic>, Vec<Symbol>) {
    let mut diagnostics = Vec::new();
    let mut symbols = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split('\t');
        match parts.next() {
            Some("D") => {
                let line_no = parts.next().and_then(|v| v.trim().parse().ok());
                let message = parts.next().unwrap_or("").trim();
                if let (Some(line), false) = (line_no, message.is_empty()) {
                    diagnostics.push(Diagnostic {
                        line,
                        message: message.to_string(),
                    });
                }
            }
            Some("S") => {
                let kind = parts.next().and_then(|v| v.trim().parse().ok());
                let name = parts.next().unwrap_or("").to_string();
                let line = parts.next().and_then(|v| v.trim().parse().ok());
                let detail = parts.next().unwrap_or("").to_string();
                if let (Some(kind), Some(line), false) = (kind, line, name.is_empty()) {
                    symbols.push(Symbol {
                        kind,
                        name,
                        line,
                        detail,
                    });
                }
            }
            _ => {}
        }
    }
    (diagnostics, symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_diagnosticos_y_simbolos() {
        let out = "D\t3\tvariable no declarada: x\nS\t0\tfoo\t12\tint\nS\t1\tn\t15\t\n";
        let (diagnostics, symbols) = parse(out);
        assert_eq!(
            diagnostics,
            vec![Diagnostic {
                line: 3,
                message: "variable no declarada: x".into()
            }]
        );
        assert_eq!(
            symbols,
            vec![
                Symbol {
                    kind: 0,
                    name: "foo".into(),
                    line: 12,
                    detail: "int".into()
                },
                Symbol {
                    kind: 1,
                    name: "n".into(),
                    line: 15,
                    detail: String::new()
                },
            ]
        );
    }

    #[test]
    fn ignora_lineas_vacias_o_raras() {
        assert_eq!(parse("").0.len(), 0);
        let (d, s) = parse("basura sin tab\n\n");
        assert!(d.is_empty() && s.is_empty());
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
        let file = dir.join("demo.spasm");
        std::fs::write(
            &file,
            "programa \"x\"\n\nfunc suma(a : int) -> int {\n    let b = a\n    return b\n}\n\nmain {\n    let c = suma(1)\n}\n",
        )
        .unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        spawn(python, frontend, file.clone(), 0, tx);
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(40))
            .expect("el linter devolvió un resultado");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(
            result
                .symbols
                .iter()
                .any(|s| s.name == "suma" && s.kind == 0 && s.line == 3),
            "esquema inesperado: {:?}",
            result.symbols
        );
        assert!(
            result.symbols.iter().any(|s| s.name == "c" && s.kind == 1),
            "esquema inesperado: {:?}",
            result.symbols
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
