//! Resaltado de sintaxis por lenguaje.
//!
//! Un tokenizador por línea, suficiente para un editor: no valida nada, sólo
//! colorea. Los `kind` son los mismos que entiende `IdeCodeRow` en la UI:
//!
//! | kind | qué |
//! |---|---|
//! | 0 | texto normal (incluye espacios, para conservar el alineado) |
//! | 1 | comentario |
//! | 2 | palabra clave |
//! | 3 | string |
//! | 4 | número |
//! | 5 | etiqueta / clave / heading |
//!
//! El contrato es que los tokens, concatenados, reproducen la línea entera (con
//! los tabs expandidos a cuatro espacios).

use std::path::Path;

pub const KIND_TEXT: i32 = 0;
pub const KIND_COMMENT: i32 = 1;
pub const KIND_KEYWORD: i32 = 2;
pub const KIND_STRING: i32 = 3;
pub const KIND_NUMBER: i32 = 4;
pub const KIND_LABEL: i32 = 5;

/// Lenguaje del documento, detectado por la extensión.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Spasm,
    Slint,
    Json,
    Toml,
    Yaml,
    Markdown,
    Shell,
    Rust,
    Plain,
}

impl Language {
    /// Detecta el lenguaje por la extensión del archivo.
    pub fn from_path(path: &Path) -> Language {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "spasm" | "asm" => Language::Spasm,
            "slint" => Language::Slint,
            "json" => Language::Json,
            "toml" | "ini" | "cfg" | "lock" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" | "ps1" | "psm1" => Language::Shell,
            "rs" => Language::Rust,
            _ => Language::Plain,
        }
    }

    /// Nombre para la barra de estado.
    pub fn name(self) -> &'static str {
        match self {
            Language::Spasm => "SpASM",
            Language::Slint => "Slint",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Markdown => "Markdown",
            Language::Shell => "Shell",
            Language::Rust => "Rust",
            Language::Plain => "Texto",
        }
    }
}

/// Tokeniza una línea del lenguaje indicado.
pub fn tokenize_line(line: &str, lang: Language) -> Vec<(String, i32)> {
    match lang {
        Language::Spasm => spasm(line),
        Language::Slint => generic(line, &SLINT),
        Language::Json => generic(line, &JSON),
        Language::Toml => generic(line, &TOML),
        Language::Yaml => generic(line, &YAML),
        Language::Markdown => markdown(line),
        Language::Shell => generic(line, &SHELL),
        Language::Rust => generic(line, &RUST),
        Language::Plain => plain(line),
    }
}

/// `true` si la palabra es clave del dialecto de SpASM.
pub fn is_keyword(word: &str) -> bool {
    SPASM_KEYWORDS.contains(&word)
}

/// Palabras clave y builtins de SpASM (ambos dialectos).
const SPASM_KEYWORDS: &[&str] = &[
    // estructura / control
    "programa",
    "main",
    "func",
    "end",
    "return",
    "let",
    "var",
    "assign",
    "if",
    "else",
    "while",
    "for",
    "break",
    "continue",
    "select",
    "case",
    "default",
    // tipos y literales
    "int",
    "float",
    "double",
    "string",
    "bool",
    "true",
    "false",
    // E/S y buffers
    "print",
    "print_int",
    "print_float",
    "input",
    "push",
    "append",
    "clear",
    "copy",
    // strings
    "strlen",
    "strcmp",
    "str_equals",
    "str_slice",
    "str_trim",
    "str_index_of",
    "str_byte_at",
    "buffer_len",
    "buffer_str",
    "buffer_cap",
    "to_str",
    // matemática
    "sin",
    "cos",
    "tan",
    "exp",
    "log",
    "pow",
    "sqrt",
    "floor",
    "ceil",
    "round",
    "fabs",
    "atan2",
    "asin",
    "acos",
    "atan",
    "sinh",
    "cosh",
    "tanh",
    "log10",
];

/// Una línea sin resaltado (pero con los tabs expandidos).
fn plain(line: &str) -> Vec<(String, i32)> {
    vec![(line.replace('\t', "    "), KIND_TEXT)]
}

/// Tokeniza una línea de SpASM. Ver el módulo para los `kind`.
pub fn spasm(line: &str) -> Vec<(String, i32)> {
    // Tabs a cuatro espacios: el render es monoespaciado y un tab real
    // desalinearía respecto de los espacios.
    let chars: Vec<char> = line.replace('\t', "    ").chars().collect();
    let mut out: Vec<(String, i32)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Comentario: ';' o '//' hasta el final.
        if c == ';' || (c == '/' && chars.get(i + 1) == Some(&'/')) {
            flush(&mut plain, &mut out);
            out.push((chars[i..].iter().collect(), KIND_COMMENT));
            break;
        }

        // String con escapes.
        if c == '"' {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i = (i + 2).min(chars.len());
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_STRING));
            continue;
        }

        // Número: entero, float o hex 0x...
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            flush(&mut plain, &mut out);
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X')) {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
            }
            out.push((chars[start..i].iter().collect(), KIND_NUMBER));
            continue;
        }

        // Identificador, palabra clave o etiqueta. Se permite el prefijo `.`
        // (etiquetas locales del dialecto compilado: `.next_char:`).
        if c.is_alphabetic()
            || c == '_'
            || (c == '.'
                && chars
                    .get(i + 1)
                    .is_some_and(|d| d.is_alphabetic() || *d == '_'))
        {
            flush(&mut plain, &mut out);
            let start = i;
            if c == '.' {
                i += 1;
            }
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let is_label = chars.get(i) == Some(&':');
            if is_label {
                i += 1; // la etiqueta incluye los dos puntos
            }
            let word: String = chars[start..i].iter().collect();
            if is_label {
                out.push((word, KIND_LABEL));
            } else if is_keyword(&word) {
                out.push((word, KIND_KEYWORD));
            } else {
                // Texto normal: se acumula para no generar un token por letra.
                plain.push_str(&word);
            }
            continue;
        }

        plain.push(c);
        i += 1;
    }

    flush(&mut plain, &mut out);
    out
}

/// Configuración de un tokenizador genérico (lenguajes tipo C/hash).
struct Spec {
    /// Prefijos de comentario de línea.
    comments: &'static [&'static str],
    /// Caracteres que abren un string.
    quotes: &'static [char],
    /// Palabras clave.
    keywords: &'static [&'static str],
    /// Trata `@ident` como palabra clave (Slint).
    at_keyword: bool,
    /// Trata `$var`/`${...}` como palabra clave (shell).
    dollar: bool,
    /// Trata `#rrggbb` como color (Slint).
    hash_color: bool,
    /// Trata `[texto]` como sección (TOML).
    section: bool,
    /// Si un identificador/string va seguido de uno de estos, es una clave.
    keys: &'static [char],
}

const SLINT: Spec = Spec {
    comments: &["//"],
    quotes: &['"'],
    keywords: &[
        "component",
        "export",
        "import",
        "from",
        "global",
        "struct",
        "enum",
        "property",
        "callback",
        "function",
        "pure",
        "in",
        "out",
        "private",
        "public",
        "if",
        "else",
        "for",
        "return",
        "true",
        "false",
        "self",
        "parent",
        "root",
        "states",
        "transitions",
        "animate",
        "changed",
        "init",
        "forward-focus",
        "clicked",
        "moved",
        "pressed",
        "released",
        "accepted",
    ],
    at_keyword: true,
    dollar: false,
    hash_color: true,
    section: false,
    keys: &[':'],
};

const JSON: Spec = Spec {
    comments: &[],
    quotes: &['"'],
    keywords: &["true", "false", "null"],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[':'],
};

const TOML: Spec = Spec {
    comments: &["#"],
    quotes: &['"', '\''],
    keywords: &["true", "false"],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: true,
    keys: &['=', ':'],
};

const YAML: Spec = Spec {
    comments: &["#"],
    quotes: &['"', '\''],
    keywords: &["true", "false", "null", "yes", "no"],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[':'],
};

const SHELL: Spec = Spec {
    comments: &["#"],
    quotes: &['"', '\''],
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "function", "return", "exit", "in", "local", "export", "source", "set", "unset", "echo",
        "printf", "cd", "read", "test", "true", "false",
    ],
    at_keyword: false,
    dollar: true,
    hash_color: false,
    section: false,
    keys: &[],
};

const RUST: Spec = Spec {
    comments: &["//"],
    quotes: &['"'],
    keywords: &[
        "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "for", "while",
        "loop", "if", "else", "match", "return", "self", "Self", "crate", "super", "as", "const",
        "static", "ref", "move", "async", "await", "dyn", "where", "type", "in", "true", "false",
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64",
        "bool", "str", "String", "Vec", "Option", "Result", "Some", "None", "Ok", "Err",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

/// Tokenizador genérico por config.
fn generic(line: &str, spec: &Spec) -> Vec<(String, i32)> {
    let chars: Vec<char> = line.replace('\t', "    ").chars().collect();
    let mut out: Vec<(String, i32)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Comentario de línea.
        if let Some(len) = spec.comments.iter().find_map(|p| {
            let p: Vec<char> = p.chars().collect();
            chars[i..].starts_with(&p).then_some(p.len())
        }) {
            flush(&mut plain, &mut out);
            let _ = len;
            out.push((chars[i..].iter().collect(), KIND_COMMENT));
            break;
        }

        // String.
        if spec.quotes.contains(&c) {
            flush(&mut plain, &mut out);
            let quote = c;
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && quote == '"' {
                    i = (i + 2).min(chars.len());
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            let kind = if spec
                .keys
                .iter()
                .any(|k| next_non_space(&chars, i) == Some(*k))
            {
                KIND_LABEL
            } else {
                KIND_STRING
            };
            out.push((text, kind));
            continue;
        }

        // Sección `[...]` (TOML).
        if spec.section && c == '[' {
            flush(&mut plain, &mut out);
            let start = i;
            while i < chars.len() && chars[i] != ']' {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_KEYWORD));
            continue;
        }

        // `@keyword` (Slint).
        if spec.at_keyword && c == '@' {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_KEYWORD));
            continue;
        }

        // `$var` / `${...}` (shell).
        if spec.dollar && c == '$' {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            if chars.get(i) == Some(&'{') {
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
            } else {
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            }
            out.push((chars[start..i].iter().collect(), KIND_KEYWORD));
            continue;
        }

        // `#rrggbb` (Slint).
        if spec.hash_color && c == '#' && chars.get(i + 1).is_some_and(|d| d.is_ascii_hexdigit()) {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            while i < chars.len() && chars[i].is_ascii_hexdigit() {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_NUMBER));
            continue;
        }

        // Número.
        if c.is_ascii_digit() {
            flush(&mut plain, &mut out);
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X')) {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
            }
            out.push((chars[start..i].iter().collect(), KIND_NUMBER));
            continue;
        }

        // Identificador / palabra clave / clave.
        if c.is_alphabetic() || c == '_' || c == '-' {
            flush(&mut plain, &mut out);
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let lower = word.to_ascii_lowercase();
            let is_kw = spec
                .keywords
                .iter()
                .any(|k| *k == word || k.to_ascii_lowercase() == lower);
            let is_key = spec
                .keys
                .iter()
                .any(|k| next_non_space(&chars, i) == Some(*k));
            if is_kw {
                out.push((word, KIND_KEYWORD));
            } else if is_key {
                out.push((word, KIND_LABEL));
            } else {
                plain.push_str(&word);
            }
            continue;
        }

        plain.push(c);
        i += 1;
    }

    flush(&mut plain, &mut out);
    out
}

/// Markdown: headings, cercos de código, código en línea y links.
fn markdown(line: &str) -> Vec<(String, i32)> {
    let expanded = line.replace('\t', "    ");
    let trimmed = expanded.trim_start();

    if trimmed.starts_with('#') {
        return vec![(expanded, KIND_KEYWORD)];
    }
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return vec![(expanded, KIND_COMMENT)];
    }

    let chars: Vec<char> = expanded.chars().collect();
    let mut out: Vec<(String, i32)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Código en línea `...`.
        if c == '`' {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_STRING));
            continue;
        }
        // Link [texto](url).
        if c == '[' {
            if let Some(close) = chars[i..].iter().position(|&x| x == ']') {
                let close = i + close;
                if chars.get(close + 1) == Some(&'(') {
                    let end = chars[close + 1..]
                        .iter()
                        .position(|&x| x == ')')
                        .map(|p| close + 1 + p + 1)
                        .unwrap_or(chars.len());
                    flush(&mut plain, &mut out);
                    out.push((chars[i..end].iter().collect(), KIND_STRING));
                    i = end;
                    continue;
                }
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

/// Vuelca lo acumulado como un token de texto normal.
fn flush(plain: &mut String, out: &mut Vec<(String, i32)>) {
    if !plain.is_empty() {
        out.push((std::mem::take(plain), KIND_TEXT));
    }
}

/// Siguiente carácter que no sea espacio a partir de `i`.
fn next_non_space(chars: &[char], i: usize) -> Option<char> {
    chars[i..].iter().copied().find(|c| !c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariante del render: la suma de los textos es la línea (con los tabs ya
    /// expandidos a cuatro espacios, que es lo que hace el tokenizador).
    fn rejoin(line: &str, lang: Language) -> String {
        tokenize_line(line, lang)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn los_tokens_reconstruyen_la_linea() {
        let cases = [
            (Language::Spasm, "    print \"hola\\n\";"),
            (Language::Slint, "    color: #83d9cb; // nota"),
            (Language::Json, "  \"clave\": \"valor\", 3.14"),
            (Language::Toml, "[seccion]"),
            (Language::Yaml, "clave: valor # nota"),
            (Language::Markdown, "- item `code` [x](y)"),
            (Language::Shell, "echo $HOME # hola"),
            (Language::Rust, "let x: i32 = 0x1F;"),
            (Language::Plain, "texto\tcon tab"),
        ];
        for (lang, line) in cases {
            assert_eq!(
                rejoin(line, lang),
                line.replace('\t', "    "),
                "no se reconstruyó {line:?} en {lang:?}"
            );
        }
    }

    #[test]
    fn spasm_reconoce_comentarios_strings_numeros_y_claves() {
        let t = spasm("let s = \"x\" ; nota");
        assert!(t.iter().any(|(txt, k)| txt == "let" && *k == KIND_KEYWORD));
        assert!(t.iter().any(|(txt, k)| txt == "\"x\"" && *k == KIND_STRING));
        assert!(t
            .iter()
            .any(|(txt, k)| txt == "; nota" && *k == KIND_COMMENT));

        let t = spasm("x = 0x1F + 2.5");
        assert!(t.iter().any(|(txt, k)| txt == "0x1F" && *k == KIND_NUMBER));

        let t = spasm("loop:");
        assert_eq!(t, vec![("loop:".to_string(), KIND_LABEL)]);
    }

    #[test]
    fn detecta_el_lenguaje_por_extension() {
        use Language::*;
        assert_eq!(Language::from_path(Path::new("a.spasm")), Spasm);
        assert_eq!(Language::from_path(Path::new("a.SLINT")), Slint);
        assert_eq!(Language::from_path(Path::new("Cargo.toml")), Toml);
        assert_eq!(Language::from_path(Path::new("x.yaml")), Yaml);
        assert_eq!(Language::from_path(Path::new("README.md")), Markdown);
        assert_eq!(Language::from_path(Path::new("run.PS1")), Shell);
        assert_eq!(Language::from_path(Path::new("lib.rs")), Rust);
        assert_eq!(Language::from_path(Path::new("datos.bin")), Plain);
        assert_eq!(Language::from_path(Path::new("sin_extension")), Plain);
    }

    #[test]
    fn resalta_por_lenguaje() {
        // Slint: color hex y palabra clave.
        let t = tokenize_line("    color: #e8e2d0;", Language::Slint);
        assert!(t.iter().any(|(s, k)| s == "#e8e2d0" && *k == KIND_NUMBER));
        let t = tokenize_line("export component Foo {", Language::Slint);
        assert!(t.iter().any(|(s, k)| s == "export" && *k == KIND_KEYWORD));

        // JSON: clave como etiqueta, booleano como clave.
        let t = tokenize_line("\"a\": true", Language::Json);
        assert!(t.iter().any(|(s, k)| s == "\"a\"" && *k == KIND_LABEL));
        assert!(t.iter().any(|(s, k)| s == "true" && *k == KIND_KEYWORD));

        // Markdown: heading.
        let t = tokenize_line("## Título", Language::Markdown);
        assert_eq!(t, vec![("## Título".to_string(), KIND_KEYWORD)]);

        // Shell: variable.
        let t = tokenize_line("echo $HOME", Language::Shell);
        assert!(t.iter().any(|(s, k)| s == "$HOME" && *k == KIND_KEYWORD));
    }
}
