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
    Python,
    JavaScript,
    TypeScript,
    C,
    Cpp,
    CSharp,
    Java,
    Go,
    Kotlin,
    Swift,
    Lua,
    Ruby,
    Php,
    Html,
    Xml,
    Css,
    Sql,
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
            "json" | "jsonc" => Language::Json,
            "toml" | "ini" | "cfg" | "lock" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" | "ps1" | "psm1" => Language::Shell,
            "rs" => Language::Rust,
            "py" | "pyw" | "pyi" => Language::Python,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "c++" => Language::Cpp,
            "cs" => Language::CSharp,
            "java" => Language::Java,
            "go" => Language::Go,
            "kt" | "kts" => Language::Kotlin,
            "swift" => Language::Swift,
            "lua" => Language::Lua,
            "rb" | "rake" | "gemspec" => Language::Ruby,
            "php" | "phtml" => Language::Php,
            "html" | "htm" => Language::Html,
            "xml" | "xsl" | "xslt" | "svg" | "plist" => Language::Xml,
            "css" | "scss" | "less" => Language::Css,
            "sql" => Language::Sql,
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
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::CSharp => "C#",
            Language::Java => "Java",
            Language::Go => "Go",
            Language::Kotlin => "Kotlin",
            Language::Swift => "Swift",
            Language::Lua => "Lua",
            Language::Ruby => "Ruby",
            Language::Php => "PHP",
            Language::Html => "HTML",
            Language::Xml => "XML",
            Language::Css => "CSS",
            Language::Sql => "SQL",
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
        Language::Python => generic(line, &PYTHON),
        Language::JavaScript => generic(line, &JAVASCRIPT),
        Language::TypeScript => generic(line, &TYPESCRIPT),
        Language::C => generic(line, &C),
        Language::Cpp => generic(line, &CPP),
        Language::CSharp => generic(line, &CSHARP),
        Language::Java => generic(line, &JAVA),
        Language::Go => generic(line, &GO),
        Language::Kotlin => generic(line, &KOTLIN),
        Language::Swift => generic(line, &SWIFT),
        Language::Lua => generic(line, &LUA),
        Language::Ruby => generic(line, &RUBY),
        Language::Php => generic(line, &PHP),
        Language::Html | Language::Xml => markup(line),
        Language::Css => generic(line, &CSS),
        Language::Sql => generic(line, &SQL),
        Language::Plain => plain(line),
    }
}

/// `true` si la palabra es clave del dialecto de SpASM.
pub fn is_keyword(word: &str) -> bool {
    SPASM_KEYWORDS.contains(&word)
}

/// Palabras clave y builtins de SpASM (para la ayuda de sintaxis).
pub fn spasm_words() -> &'static [&'static str] {
    SPASM_KEYWORDS
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

const PYTHON: Spec = Spec {
    comments: &["#"],
    quotes: &['"', '\''],
    keywords: &[
        "and",
        "as",
        "assert",
        "async",
        "await",
        "break",
        "class",
        "continue",
        "def",
        "del",
        "elif",
        "else",
        "except",
        "finally",
        "for",
        "from",
        "global",
        "if",
        "import",
        "in",
        "is",
        "lambda",
        "None",
        "nonlocal",
        "not",
        "or",
        "pass",
        "raise",
        "return",
        "self",
        "True",
        "False",
        "try",
        "while",
        "with",
        "yield",
        "print",
        "len",
        "range",
        "int",
        "float",
        "str",
        "list",
        "dict",
        "set",
        "tuple",
        "bool",
        "open",
        "enumerate",
        "zip",
        "map",
        "filter",
        "sum",
        "min",
        "max",
        "abs",
        "sorted",
        "isinstance",
        "super",
        "type",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const JAVASCRIPT: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\'', '`'],
    keywords: &[
        "var",
        "let",
        "const",
        "function",
        "return",
        "if",
        "else",
        "for",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "new",
        "delete",
        "typeof",
        "instanceof",
        "in",
        "of",
        "class",
        "extends",
        "super",
        "this",
        "null",
        "undefined",
        "true",
        "false",
        "import",
        "export",
        "from",
        "async",
        "await",
        "yield",
        "try",
        "catch",
        "finally",
        "throw",
        "void",
        "static",
        "get",
        "set",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const TYPESCRIPT: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\'', '`'],
    keywords: &[
        "var",
        "let",
        "const",
        "function",
        "return",
        "if",
        "else",
        "for",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "new",
        "delete",
        "typeof",
        "instanceof",
        "in",
        "of",
        "class",
        "extends",
        "super",
        "this",
        "null",
        "undefined",
        "true",
        "false",
        "import",
        "export",
        "from",
        "async",
        "await",
        "yield",
        "try",
        "catch",
        "finally",
        "throw",
        "void",
        "static",
        "get",
        "set",
        "interface",
        "type",
        "enum",
        "namespace",
        "declare",
        "readonly",
        "public",
        "private",
        "protected",
        "abstract",
        "implements",
        "any",
        "string",
        "number",
        "boolean",
        "unknown",
        "never",
        "keyof",
        "as",
        "is",
        "infer",
        "satisfies",
        "override",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const C: Spec = Spec {
    comments: &["//"],
    quotes: &['"'],
    keywords: &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
        "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
        "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
        "unsigned", "void", "volatile", "while", "NULL", "true", "false", "size_t", "uint8_t",
        "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const CPP: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\''],
    keywords: &[
        "auto",
        "break",
        "case",
        "catch",
        "char",
        "class",
        "const",
        "constexpr",
        "continue",
        "default",
        "delete",
        "do",
        "double",
        "else",
        "enum",
        "explicit",
        "extern",
        "false",
        "float",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "int",
        "long",
        "namespace",
        "new",
        "nullptr",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "register",
        "return",
        "short",
        "signed",
        "sizeof",
        "static",
        "static_cast",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "true",
        "try",
        "typedef",
        "typename",
        "union",
        "unsigned",
        "using",
        "virtual",
        "void",
        "volatile",
        "while",
        "std",
        "string",
        "vector",
        "cout",
        "cin",
        "endl",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const CSHARP: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\''],
    keywords: &[
        "namespace",
        "using",
        "class",
        "struct",
        "interface",
        "enum",
        "public",
        "private",
        "protected",
        "internal",
        "static",
        "readonly",
        "const",
        "void",
        "int",
        "string",
        "bool",
        "var",
        "new",
        "return",
        "if",
        "else",
        "for",
        "foreach",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "try",
        "catch",
        "finally",
        "throw",
        "this",
        "base",
        "null",
        "true",
        "false",
        "async",
        "await",
        "get",
        "set",
        "override",
        "virtual",
        "abstract",
        "sealed",
        "partial",
        "out",
        "ref",
        "in",
        "params",
        "is",
        "as",
        "typeof",
        "nameof",
        "delegate",
        "event",
        "where",
        "yield",
        "float",
        "double",
        "long",
        "decimal",
        "char",
        "object",
        "byte",
        "short",
        "uint",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const JAVA: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\''],
    keywords: &[
        "public",
        "private",
        "protected",
        "class",
        "interface",
        "enum",
        "extends",
        "implements",
        "static",
        "final",
        "void",
        "int",
        "long",
        "double",
        "float",
        "boolean",
        "char",
        "String",
        "byte",
        "short",
        "new",
        "return",
        "if",
        "else",
        "for",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "try",
        "catch",
        "finally",
        "throw",
        "throws",
        "this",
        "super",
        "null",
        "true",
        "false",
        "package",
        "import",
        "abstract",
        "synchronized",
        "volatile",
        "transient",
        "native",
        "instanceof",
        "assert",
        "var",
        "record",
        "sealed",
        "yield",
        "record",
        "var",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const GO: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '`'],
    keywords: &[
        "package",
        "import",
        "func",
        "var",
        "const",
        "type",
        "struct",
        "interface",
        "map",
        "chan",
        "go",
        "defer",
        "return",
        "if",
        "else",
        "for",
        "range",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "fallthrough",
        "goto",
        "select",
        "nil",
        "true",
        "false",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "float32",
        "float64",
        "string",
        "bool",
        "byte",
        "rune",
        "error",
        "make",
        "new",
        "len",
        "cap",
        "append",
        "copy",
        "delete",
        "panic",
        "recover",
        "print",
        "println",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const KOTLIN: Spec = Spec {
    comments: &["//"],
    quotes: &['"', '\''],
    keywords: &[
        "fun",
        "val",
        "var",
        "class",
        "object",
        "interface",
        "enum",
        "data",
        "sealed",
        "abstract",
        "open",
        "override",
        "private",
        "public",
        "protected",
        "internal",
        "package",
        "import",
        "return",
        "if",
        "else",
        "when",
        "for",
        "while",
        "do",
        "break",
        "continue",
        "try",
        "catch",
        "finally",
        "throw",
        "this",
        "super",
        "null",
        "true",
        "false",
        "is",
        "as",
        "in",
        "out",
        "by",
        "lateinit",
        "companion",
        "init",
        "constructor",
        "suspend",
        "inline",
        "where",
        "typealias",
        "Unit",
        "Int",
        "Long",
        "Double",
        "Float",
        "Boolean",
        "Char",
        "String",
        "Array",
        "List",
        "Map",
        "Set",
        "Any",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const SWIFT: Spec = Spec {
    comments: &["//"],
    quotes: &['"'],
    keywords: &[
        "func",
        "let",
        "var",
        "class",
        "struct",
        "enum",
        "protocol",
        "extension",
        "import",
        "return",
        "if",
        "else",
        "guard",
        "for",
        "while",
        "repeat",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "try",
        "catch",
        "throw",
        "throws",
        "self",
        "super",
        "nil",
        "true",
        "false",
        "init",
        "deinit",
        "subscript",
        "static",
        "final",
        "private",
        "public",
        "internal",
        "fileprivate",
        "open",
        "mutating",
        "override",
        "convenience",
        "required",
        "lazy",
        "weak",
        "unowned",
        "some",
        "any",
        "where",
        "as",
        "is",
        "in",
        "typealias",
        "associatedtype",
        "inout",
        "async",
        "await",
        "actor",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const LUA: Spec = Spec {
    comments: &["--"],
    quotes: &['"', '\''],
    keywords: &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        "print", "pairs", "ipairs", "type", "tostring", "tonumber", "require", "error", "assert",
        "self",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const RUBY: Spec = Spec {
    comments: &["#"],
    quotes: &['"', '\''],
    keywords: &[
        "def",
        "end",
        "if",
        "elsif",
        "else",
        "unless",
        "while",
        "until",
        "for",
        "in",
        "do",
        "begin",
        "rescue",
        "ensure",
        "class",
        "module",
        "return",
        "yield",
        "self",
        "nil",
        "true",
        "false",
        "and",
        "or",
        "not",
        "then",
        "case",
        "when",
        "break",
        "next",
        "redo",
        "retry",
        "super",
        "lambda",
        "proc",
        "require",
        "require_relative",
        "attr_accessor",
        "attr_reader",
        "attr_writer",
        "puts",
        "print",
        "new",
        "include",
        "extend",
        "raise",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: false,
    section: false,
    keys: &[],
};

const PHP: Spec = Spec {
    comments: &["//", "#"],
    quotes: &['"', '\''],
    keywords: &[
        "function",
        "return",
        "if",
        "else",
        "elseif",
        "endif",
        "for",
        "foreach",
        "while",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "class",
        "interface",
        "trait",
        "extends",
        "implements",
        "public",
        "private",
        "protected",
        "static",
        "abstract",
        "final",
        "const",
        "new",
        "echo",
        "print",
        "true",
        "false",
        "null",
        "array",
        "use",
        "namespace",
        "require",
        "include",
        "require_once",
        "include_once",
        "try",
        "catch",
        "finally",
        "throw",
        "global",
        "isset",
        "unset",
        "empty",
        "list",
        "as",
        "match",
        "fn",
    ],
    at_keyword: false,
    dollar: true,
    hash_color: false,
    section: false,
    keys: &[],
};

const CSS: Spec = Spec {
    comments: &["/*", "//"],
    quotes: &['"', '\''],
    keywords: &[
        "important",
        "inherit",
        "initial",
        "unset",
        "none",
        "auto",
        "block",
        "inline",
        "flex",
        "grid",
        "absolute",
        "relative",
        "fixed",
        "sticky",
        "hidden",
        "visible",
        "bold",
        "center",
        "left",
        "right",
        "top",
        "bottom",
        "solid",
        "dashed",
        "transparent",
    ],
    at_keyword: false,
    dollar: false,
    hash_color: true,
    section: false,
    keys: &[':'],
};

const SQL: Spec = Spec {
    comments: &["--"],
    quotes: &['\''],
    keywords: &[
        "select",
        "from",
        "where",
        "insert",
        "into",
        "values",
        "update",
        "set",
        "delete",
        "create",
        "table",
        "alter",
        "drop",
        "index",
        "view",
        "join",
        "inner",
        "left",
        "right",
        "full",
        "outer",
        "on",
        "group",
        "by",
        "order",
        "having",
        "limit",
        "offset",
        "distinct",
        "as",
        "and",
        "or",
        "not",
        "null",
        "is",
        "in",
        "like",
        "between",
        "primary",
        "key",
        "foreign",
        "references",
        "unique",
        "default",
        "cascade",
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "case",
        "when",
        "then",
        "else",
        "end",
        "union",
        "all",
        "exists",
        "asc",
        "desc",
        "int",
        "integer",
        "varchar",
        "text",
        "date",
        "timestamp",
        "boolean",
        "float",
        "double",
        "decimal",
        "begin",
        "commit",
        "rollback",
        "transaction",
        "if",
        "replace",
        "constraint",
        "add",
        "column",
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

/// HTML/XML: colorea tags, atributos y strings.
fn markup(line: &str) -> Vec<(String, i32)> {
    let chars: Vec<char> = line.replace('\t', "    ").chars().collect();
    let mut out: Vec<(String, i32)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Comentario `<!-- ... -->`.
        if c == '<' && chars.get(i + 1) == Some(&'!') {
            flush(&mut plain, &mut out);
            let start = i;
            while i < chars.len() && chars[i] != '>' {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_COMMENT));
            continue;
        }
        // `<name` o `</name`.
        if c == '<' {
            flush(&mut plain, &mut out);
            let start = i;
            i += 1;
            if chars.get(i) == Some(&'/') {
                i += 1;
            }
            while i < chars.len()
                && (chars[i].is_alphanumeric()
                    || chars[i] == '-'
                    || chars[i] == ':'
                    || chars[i] == '_')
            {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_KEYWORD));
            continue;
        }
        // String.
        if c == '"' || c == '\'' {
            flush(&mut plain, &mut out);
            let quote = c;
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push((chars[start..i].iter().collect(), KIND_STRING));
            continue;
        }
        // Atributo: identificador seguido de `=`.
        if c.is_alphabetic() || c == '_' || c == '-' {
            flush(&mut plain, &mut out);
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if next_non_space(&chars, i) == Some('=') {
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
            (Language::Python, "def f(x): return x + 1  # nota"),
            (Language::JavaScript, "const x = `hola ${name}`;"),
            (Language::TypeScript, "interface P { name: string }"),
            (Language::C, "int main(void) { return 0; }"),
            (Language::Cpp, "std::vector<int> v; // nota"),
            (Language::CSharp, "public void F() { }"),
            (Language::Java, "private static final int N = 3;"),
            (Language::Go, "func main() { println(1) }"),
            (Language::Kotlin, "fun main() { val x = 1 }"),
            (Language::Swift, "let x: Int = 0"),
            (Language::Lua, "local x = 1 -- nota"),
            (Language::Ruby, "def f; puts 'hola'; end"),
            (Language::Php, "echo $x; // nota"),
            (Language::Html, "<a href=\"x\">link</a>"),
            (Language::Xml, "<tag attr='1'/>"),
            (Language::Css, "color: #fff; /* nota */"),
            (Language::Sql, "SELECT id FROM t WHERE x = 'a' -- nota"),
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
        assert_eq!(Language::from_path(Path::new("app.py")), Python);
        assert_eq!(Language::from_path(Path::new("main.js")), JavaScript);
        assert_eq!(Language::from_path(Path::new("x.tsx")), TypeScript);
        assert_eq!(Language::from_path(Path::new("a.c")), C);
        assert_eq!(Language::from_path(Path::new("a.cpp")), Cpp);
        assert_eq!(Language::from_path(Path::new("A.cs")), CSharp);
        assert_eq!(Language::from_path(Path::new("Main.java")), Java);
        assert_eq!(Language::from_path(Path::new("main.go")), Go);
        assert_eq!(Language::from_path(Path::new("index.html")), Html);
        assert_eq!(Language::from_path(Path::new("a.xml")), Xml);
        assert_eq!(Language::from_path(Path::new("s.css")), Css);
        assert_eq!(Language::from_path(Path::new("q.sql")), Sql);
        assert_eq!(Language::from_path(Path::new("a.php")), Php);
        assert_eq!(Language::from_path(Path::new("a.rb")), Ruby);
        assert_eq!(Language::from_path(Path::new("datos.bin")), Plain);
        assert_eq!(Language::from_path(Path::new("sin_extension")), Plain);
    }

    #[test]
    fn resalta_los_lenguajes_conocidos() {
        // Python: `def` keyword y comentario.
        let t = tokenize_line("def f(): pass  # nota", Language::Python);
        assert!(t.iter().any(|(s, k)| s == "def" && *k == KIND_KEYWORD));
        assert!(t.iter().any(|(s, k)| s == "# nota" && *k == KIND_COMMENT));

        // JavaScript: template literal y keyword.
        let t = tokenize_line("const x = `hola ${n}`;", Language::JavaScript);
        assert!(t.iter().any(|(s, k)| s == "const" && *k == KIND_KEYWORD));
        assert!(t
            .iter()
            .any(|(s, k)| s == "`hola ${n}`" && *k == KIND_STRING));

        // C: tipo y número.
        let t = tokenize_line("int n = 0x1F;", Language::C);
        assert!(t.iter().any(|(s, k)| s == "int" && *k == KIND_KEYWORD));
        assert!(t.iter().any(|(s, k)| s == "0x1F" && *k == KIND_NUMBER));

        // HTML: tag keyword, atributo label y string.
        let t = tokenize_line("<a href=\"x\">y</a>", Language::Html);
        assert!(t.iter().any(|(s, k)| s == "<a" && *k == KIND_KEYWORD));
        assert!(t.iter().any(|(s, k)| s == "href" && *k == KIND_LABEL));
        assert!(t.iter().any(|(s, k)| s == "\"x\"" && *k == KIND_STRING));

        // SQL (sin distinguir mayúsculas): SELECT keyword.
        let t = tokenize_line("SELECT id FROM t", Language::Sql);
        assert!(t.iter().any(|(s, k)| s == "SELECT" && *k == KIND_KEYWORD));
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
