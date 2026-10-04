//! Resaltado de sintaxis de SpASM.
//!
//! Un tokenizador de una línea, suficiente para el editor de lectura: no valida
//! el programa, sólo colorea. Los `kind` son los mismos que entiende
//! `IdeCodeRow` en la UI:
//!
//! | kind | qué |
//! |---|---|
//! | 0 | texto normal (incluye espacios, para conservar el alineado) |
//! | 1 | comentario |
//! | 2 | palabra clave |
//! | 3 | string |
//! | 4 | número |
//! | 5 | etiqueta (`nombre:`) |
//!
//! El contrato es que los tokens, concatenados, reproducen la línea entera. La
//! UI los dibuja uno al lado del otro con tipografía monoespaciada, así que la
//! suma de los textos tiene que ser exactamente la línea.

/// Palabras clave y builtins del lenguaje (ambos dialectos).
const KEYWORDS: &[&str] = &[
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

pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

/// Tokeniza una línea. Devuelve `(texto, kind)` en orden.
pub fn tokenize_line(line: &str) -> Vec<(String, i32)> {
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
            out.push((chars[i..].iter().collect(), 1));
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
            out.push((chars[start..i].iter().collect(), 3));
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
            out.push((chars[start..i].iter().collect(), 4));
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
                out.push((word, 5));
            } else if is_keyword(&word) {
                out.push((word, 2));
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

/// Vuelca lo acumulado como un token de texto normal.
fn flush(plain: &mut String, out: &mut Vec<(String, i32)>) {
    if !plain.is_empty() {
        out.push((std::mem::take(plain), 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariante del render: la suma de los textos es la línea (con los tabs ya
    /// expandidos a cuatro espacios, que es lo que hace el tokenizador).
    fn rejoin(line: &str) -> String {
        tokenize_line(line).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn los_tokens_reconstruyen_la_linea() {
        for line in [
            "",
            "let x = 42",
            "    print \"hola\\n\";",
            "start:",
            "\tif (a == b) {",
            "0xFF + 3.14",
            "sin(pi/2) ; comentario",
        ] {
            assert_eq!(
                rejoin(line),
                line.replace('\t', "    "),
                "no se reconstruyó: {line:?}"
            );
        }
    }

    fn kinds(line: &str) -> Vec<(String, i32)> {
        tokenize_line(line)
    }

    #[test]
    fn reconoce_comentarios_strings_numeros_y_claves() {
        let t = kinds("let s = \"x\" ; nota");
        assert!(t.iter().any(|(txt, k)| txt == "let" && *k == 2));
        assert!(t.iter().any(|(txt, k)| txt == "\"x\"" && *k == 3));
        assert!(t.iter().any(|(txt, k)| txt == "; nota" && *k == 1));

        let t = kinds("x = 0x1F + 2.5");
        assert!(t.iter().any(|(txt, k)| txt == "0x1F" && *k == 4));
        assert!(t.iter().any(|(txt, k)| txt == "2.5" && *k == 4));
    }

    #[test]
    fn las_etiquetas_incluyen_los_dos_puntos() {
        let t = kinds("loop:");
        assert_eq!(t, vec![("loop:".to_string(), 5)]);

        let t = kinds(".next_char:");
        assert_eq!(t, vec![(".next_char:".to_string(), 5)]);
    }

    #[test]
    fn las_palabras_que_solo_empiezan_igual_no_son_clave() {
        // "letter" empieza con "let" pero no es clave.
        let t = kinds("letter = 1");
        assert!(t.iter().all(|(_, k)| *k != 2));
    }
}
