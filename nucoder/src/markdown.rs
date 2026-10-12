//! Parseo de Markdown del chat, **UI-agnóstico**.
//!
//! Las respuestas del modelo son Markdown. En vez de renderizar a una librería
//! de terminal, acá se **segmenta** en dos cosas que el frontend sabe dibujar:
//!
//! - `Segment::Prose`: Markdown "de prosa" (títulos, listas, citas, reglas,
//!   tablas y estilos en línea) que el frontend pasa tal cual a su renderizador
//!   (NuIde usa `slint::StyledText::from_markdown`).
//! - `Segment::Code`: un bloque de código **ya tokenizado** por lenguaje, para
//!   que el frontend lo pinte con el resaltado de sintaxis de cada token.
//!
//! El resaltado reconoce Rust, C/C++/Java/C#, Python, JS/TS, JSON, TOML/YAML/INI,
//! shell, PowerShell, SQL y SpASM (palabras clave, cadenas, comentarios, números,
//! tipos y funciones); el resto cae a un genérico. Sin dependencias.

/// Categoría semántica de un token (el frontend elige el color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Texto normal.
    Plain,
    /// Palabra clave.
    Keyword,
    /// Cadena.
    Str,
    /// Número.
    Number,
    /// Tipo (identificador capitalizado).
    Type,
    /// Llamada a función (identificador seguido de `(`).
    Func,
    /// Comentario.
    Comment,
}

/// Un trozo de una línea de código.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
}

/// Un bloque de código: lenguaje de la cerca y sus líneas tokenizadas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    pub lang: String,
    pub lines: Vec<Vec<Token>>,
}

/// Parte de una respuesta del asistente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Markdown de prosa (sin cercos de código), para el renderizador del frontend.
    Prose(String),
    /// Bloque de código resaltado.
    Code(CodeBlock),
}

/// Segmenta una respuesta Markdown en prosa y bloques de código.
///
/// Un cerco abierto sin cerrar (habitual en streaming) se trata como bloque de
/// código hasta el final de lo recibido.
pub fn segments(md: &str) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut prose = String::new();
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();

    for raw in md.lines() {
        let trimmed = raw.trim_start();
        if in_code {
            if trimmed.starts_with("```") {
                out.push(Segment::Code(code_block(&code_lang, &code_lines)));
                code_lines.clear();
                code_lang.clear();
                in_code = false;
            } else {
                code_lines.push(raw.to_string());
            }
            continue;
        }
        if let Some(lang) = trimmed.strip_prefix("```") {
            if !prose.trim().is_empty() {
                out.push(Segment::Prose(std::mem::take(&mut prose)));
            }
            in_code = true;
            code_lang = lang.trim().to_string();
            continue;
        }
        prose.push_str(raw);
        prose.push('\n');
    }

    if in_code {
        out.push(Segment::Code(code_block(&code_lang, &code_lines)));
    }
    if !prose.trim().is_empty() {
        out.push(Segment::Prose(prose));
    }
    out
}

/// Tokeniza un bloque de código completo (mantiene el estado entre líneas).
fn code_block(lang: &str, lines: &[String]) -> CodeBlock {
    let syn = syntax_for(lang);
    let mut state = LexState::default();
    let tokens = lines
        .iter()
        .map(|line| highlight(line, &syn, &mut state))
        .collect();
    CodeBlock {
        lang: lang.to_string(),
        lines: tokens,
    }
}

/// Configuración de resaltado por lenguaje.
struct Syntax {
    keywords: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comment: bool,
    backtick_string: bool,
    triple_string: bool,
}

/// Estado que persiste entre líneas de un mismo bloque.
#[derive(Default)]
struct LexState {
    block_comment: bool,
    triple: Option<char>,
}

/// Tokeniza una línea y le asigna categorías.
fn highlight(line: &str, syn: &Syntax, st: &mut LexState) -> Vec<Token> {
    let c: Vec<char> = line.chars().collect();
    let n = c.len();

    let mut tokens: Vec<Token> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                tokens.push(Token {
                    text: std::mem::take(&mut plain),
                    kind: TokenKind::Plain,
                });
            }
        };
    }

    while i < n {
        // Continuación de un comentario de bloque.
        if st.block_comment {
            let end = find_chars(&c, i, &['*', '/']);
            let (s, next) = match end {
                Some(k) => (chars_range(&c, i, k + 2), k + 2),
                None => (chars_range(&c, i, n), n),
            };
            flush!();
            tokens.push(Token {
                text: s,
                kind: TokenKind::Comment,
            });
            st.block_comment = end.is_none();
            i = next;
            continue;
        }

        // Continuación de una cadena triple (Python).
        if let Some(q) = st.triple {
            let seq = [q, q, q];
            let end = find_chars(&c, i, &seq);
            let (s, next) = match end {
                Some(k) => (chars_range(&c, i, k + 3), k + 3),
                None => (chars_range(&c, i, n), n),
            };
            flush!();
            tokens.push(Token {
                text: s,
                kind: TokenKind::Str,
            });
            st.triple = end.is_none().then_some(q);
            i = next;
            continue;
        }

        // Comentario de línea.
        if syn.line_comments.iter().any(|lc| has_at(&c, i, lc)) {
            flush!();
            tokens.push(Token {
                text: chars_range(&c, i, n),
                kind: TokenKind::Comment,
            });
            break;
        }

        // Comentario de bloque `/* … */`.
        if syn.block_comment && has_at(&c, i, "/*") {
            if let Some(k) = find_str(&c, i + 2, "*/") {
                flush!();
                tokens.push(Token {
                    text: chars_range(&c, i, k + 2),
                    kind: TokenKind::Comment,
                });
                i = k + 2;
            } else {
                flush!();
                tokens.push(Token {
                    text: chars_range(&c, i, n),
                    kind: TokenKind::Comment,
                });
                st.block_comment = true;
                i = n;
            }
            continue;
        }

        // Cadena triple de apertura.
        if syn.triple_string && (has_at(&c, i, "\"\"\"") || has_at(&c, i, "'''")) {
            let q = c[i];
            let seq = [q, q, q];
            if let Some(k) = find_chars(&c, i + 3, &seq) {
                flush!();
                tokens.push(Token {
                    text: chars_range(&c, i, k + 3),
                    kind: TokenKind::Str,
                });
                i = k + 3;
            } else {
                flush!();
                tokens.push(Token {
                    text: chars_range(&c, i, n),
                    kind: TokenKind::Str,
                });
                st.triple = Some(q);
                i = n;
            }
            continue;
        }

        let ch = c[i];

        // Cadenas.
        if ch == '"' || ch == '\'' || (syn.backtick_string && ch == '`') {
            let q = ch;
            let mut j = i + 1;
            let mut s = String::from(q);
            while j < n {
                s.push(c[j]);
                if c[j] == '\\' && j + 1 < n {
                    s.push(c[j + 1]);
                    j += 2;
                    continue;
                }
                if c[j] == q {
                    j += 1;
                    break;
                }
                j += 1;
            }
            flush!();
            tokens.push(Token {
                text: s,
                kind: TokenKind::Str,
            });
            i = j;
            continue;
        }

        // Números.
        if ch.is_ascii_digit() {
            let mut j = i;
            while j < n && (c[j].is_ascii_alphanumeric() || c[j] == '.' || c[j] == '_') {
                j += 1;
            }
            flush!();
            tokens.push(Token {
                text: chars_range(&c, i, j),
                kind: TokenKind::Number,
            });
            i = j;
            continue;
        }

        // Identificadores / palabras clave / tipos / funciones.
        if ch == '_' || ch.is_ascii_alphabetic() {
            let mut j = i;
            while j < n && (c[j] == '_' || c[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            let word = chars_range(&c, i, j);
            let kind = if syn.keywords.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if j < n && c[j] == '(' {
                TokenKind::Func
            } else if word.chars().next().is_some_and(|x| x.is_ascii_uppercase()) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            flush!();
            tokens.push(Token { text: word, kind });
            i = j;
            continue;
        }

        plain.push(ch);
        i += 1;
    }

    flush!();
    if tokens.is_empty() {
        tokens.push(Token {
            text: String::new(),
            kind: TokenKind::Plain,
        });
    }
    tokens
}

fn chars_range(c: &[char], a: usize, b: usize) -> String {
    c[a.min(c.len())..b.min(c.len())].iter().collect()
}

/// ¿`c[i..]` empieza con `needle`?
fn has_at(c: &[char], i: usize, needle: &str) -> bool {
    let nc: Vec<char> = needle.chars().collect();
    !nc.is_empty() && i + nc.len() <= c.len() && c[i..i + nc.len()] == nc[..]
}

/// Índice de `needle` en `c` a partir de `from`.
fn find_str(c: &[char], from: usize, needle: &str) -> Option<usize> {
    let nc: Vec<char> = needle.chars().collect();
    find_chars(c, from, &nc)
}

/// Índice de la secuencia `needle` (chars) en `c` a partir de `from`.
fn find_chars(c: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || c.len() < needle.len() {
        return None;
    }
    (from..=c.len() - needle.len()).find(|&k| c[k..k + needle.len()] == *needle)
}

/// Configuración de resaltado para un lenguaje (por su etiqueta de cerca).
fn syntax_for(lang: &str) -> Syntax {
    let l = lang.trim().to_lowercase();
    let l = l
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '#')
        .next()
        .unwrap_or("")
        .to_string();
    match l.as_str() {
        "rust" | "rs" => Syntax {
            keywords: KW_RUST,
            line_comments: &["//"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
        "c" | "cpp" | "c++" | "cc" | "h" | "hpp" | "cs" | "java" => Syntax {
            keywords: KW_C,
            line_comments: &["//"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
        "python" | "py" => Syntax {
            keywords: KW_PY,
            line_comments: &["#"],
            block_comment: false,
            backtick_string: false,
            triple_string: true,
        },
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => Syntax {
            keywords: KW_JS,
            line_comments: &["//"],
            block_comment: true,
            backtick_string: true,
            triple_string: false,
        },
        "json" | "jsonc" => Syntax {
            keywords: KW_JSON,
            line_comments: &[],
            block_comment: false,
            backtick_string: false,
            triple_string: false,
        },
        "toml" | "yaml" | "yml" | "ini" | "conf" => Syntax {
            keywords: KW_CONF,
            line_comments: &["#"],
            block_comment: false,
            backtick_string: false,
            triple_string: false,
        },
        "sh" | "bash" | "shell" | "zsh" | "console" => Syntax {
            keywords: KW_SH,
            line_comments: &["#"],
            block_comment: false,
            backtick_string: true,
            triple_string: false,
        },
        "powershell" | "ps1" | "pwsh" => Syntax {
            keywords: KW_PS,
            line_comments: &["#"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
        "spasm" => Syntax {
            keywords: KW_SPASM,
            line_comments: &["//", "#"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
        "sql" => Syntax {
            keywords: KW_SQL,
            line_comments: &["--"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
        _ => Syntax {
            keywords: &[],
            line_comments: &["//", "#"],
            block_comment: true,
            backtick_string: false,
            triple_string: false,
        },
    }
}

const KW_RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "Some", "None", "Ok", "Err",
];
const KW_C: &[&str] = &[
    "auto", "break", "case", "char", "class", "const", "continue", "default", "delete", "do",
    "double", "else", "enum", "extern", "false", "float", "for", "goto", "if", "inline", "int",
    "long", "namespace", "new", "nullptr", "private", "protected", "public", "register", "return",
    "short", "signed", "sizeof", "static", "struct", "switch", "template", "this", "true",
    "typedef", "union", "unsigned", "using", "virtual", "void", "volatile", "while", "NULL",
    "String", "public",
];
const KW_PY: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield", "self",
];
const KW_JS: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete",
    "do", "else", "enum", "export", "extends", "false", "finally", "for", "from", "function", "if",
    "implements", "import", "in", "instanceof", "interface", "let", "new", "null", "of", "private",
    "public", "readonly", "return", "super", "switch", "this", "throw", "true", "try", "type",
    "typeof", "undefined", "var", "void", "while", "yield",
];
const KW_JSON: &[&str] = &["true", "false", "null"];
const KW_CONF: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];
const KW_SH: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in",
    "local", "readonly", "return", "set", "source", "then", "unset", "while",
];
const KW_PS: &[&str] = &[
    "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end", "enum",
    "finally", "filter", "for", "foreach", "function", "if", "in", "param", "process", "return",
    "switch", "throw", "try", "until", "while",
];
const KW_SPASM: &[&str] = &[
    "let", "var", "const", "func", "function", "end", "if", "else", "elif", "while", "for",
    "return", "break", "continue", "program", "main", "import", "from", "as", "true", "false",
    "null", "and", "or", "not", "recurso", "crear", "asignar", "escribir", "liberar", "vivo",
    "inutil", "zombie",
];
const KW_SQL: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "drop", "alter", "join", "inner", "left", "right", "outer", "on", "group", "by",
    "order", "having", "limit", "offset", "union", "as", "and", "or", "not", "null", "primary",
    "key", "foreign", "references", "index", "distinct", "count", "sum", "avg", "min", "max",
    "case", "when", "then", "else", "end", "SELECT", "FROM", "WHERE",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn prose_of(segs: &[Segment]) -> String {
        segs.iter()
            .filter_map(|s| match s {
                Segment::Prose(p) => Some(p.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn separa_prosa_y_codigo() {
        let segs = segments("hola\n```rust\nlet x = 1;\n```\nchau");
        assert_eq!(segs.len(), 3);
        assert!(matches!(segs[0], Segment::Prose(_)));
        match &segs[1] {
            Segment::Code(cb) => {
                assert_eq!(cb.lang, "rust");
                assert_eq!(cb.lines.len(), 1);
            }
            _ => panic!("esperaba código"),
        }
        assert!(matches!(segs[2], Segment::Prose(_)));
    }

    #[test]
    fn cerco_sin_cerrar_es_codigo() {
        let segs = segments("```python\nprint(1)");
        assert_eq!(segs.len(), 1);
        assert!(matches!(segs[0], Segment::Code(_)));
    }

    #[test]
    fn resalta_rust_keyword_numero_comentario() {
        let segs = segments("```rust\nlet x = 42; // nota\n```");
        let Segment::Code(cb) = &segs[0] else {
            panic!("esperaba código")
        };
        let toks = &cb.lines[0];
        assert!(toks
            .iter()
            .any(|t| t.text == "let" && t.kind == TokenKind::Keyword));
        assert!(toks
            .iter()
            .any(|t| t.text == "42" && t.kind == TokenKind::Number));
        assert!(toks.iter().any(|t| t.text.contains("// nota")
            && t.kind == TokenKind::Comment));
    }

    #[test]
    fn resalta_cadenas_y_tipos() {
        let segs = segments("```python\nclass Foo:\n    s = \"hola\"\n```");
        let Segment::Code(cb) = &segs[0] else {
            panic!("esperaba código")
        };
        let flat: Vec<&Token> = cb.lines.iter().flatten().collect();
        assert!(flat
            .iter()
            .any(|t| t.text == "class" && t.kind == TokenKind::Keyword));
        assert!(flat
            .iter()
            .any(|t| t.text == "Foo" && t.kind == TokenKind::Type));
        assert!(flat
            .iter()
            .any(|t| t.text.contains("hola") && t.kind == TokenKind::Str));
    }

    #[test]
    fn prosa_conserva_markdown() {
        let segs = segments("# Título\n- uno\n- dos");
        assert_eq!(segs.len(), 1);
        assert!(prose_of(&segs).contains("# Título"));
    }
}
