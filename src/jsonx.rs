//! Utilidades JSON mínimas con conocimiento de *spans* (posiciones de bytes)
//! para editar un JSON sin tocar el resto del archivo.
//!
//! No es una biblioteca completa: alcanza para localizar claves, reemplazar
//! valores, validar sintaxis y hornear JSON. Tolera comentarios (JSONC).

/// Valor JSON ya parseado (se usa para validar antes de guardar).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Object(Vec<(String, Value)>),
    Array(Vec<Value>),
    Str(String),
    Num(String),
    Bool(bool),
    Null,
}

/// Ubicación de `"clave": <valor>` dentro de un texto.
#[derive(Debug, Clone, Copy)]
pub struct Loc {
    pub key_start: usize,
    pub val_start: usize,
    pub val_end: usize,
}

/// Saltea espacios y comentarios (`//` y `/* */`).
fn skip_ws(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\n' || b[i] == b'\r') {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            break;
        }
    }
    i
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Lee un string JSON que empieza en `i` (debe estar en la comilla).
/// Devuelve el contenido decodificado y el índice que sigue a la comilla.
fn read_string(b: &[u8], i: usize) -> Option<(String, usize)> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'"' => return Some((String::from_utf8(out).ok()?, j + 1)),
            b'\\' => {
                let e = *b.get(j + 1)?;
                match e {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let hex = std::str::from_utf8(b.get(j + 2..j + 6)?).ok()?;
                        let cp = u32::from_str_radix(hex, 16).ok()?;
                        if (0xD800..0xDC00).contains(&cp) {
                            // Par sustituto: esperamos otro `\uXXXX` pegado.
                            let rest = b.get(j + 6..)?;
                            if rest.first() == Some(&b'\\') && rest.get(1) == Some(&b'u') {
                                let hex2 = std::str::from_utf8(rest.get(2..6)?).ok()?;
                                let low = u32::from_str_radix(hex2, 16).ok()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                                    push_char(&mut out, char::from_u32(c)?);
                                    j += 12;
                                    continue;
                                }
                            }
                            return None;
                        } else if (0xDC00..0xE000).contains(&cp) {
                            return None;
                        }
                        push_char(&mut out, char::from_u32(cp)?);
                        j += 6;
                        continue;
                    }
                    _ => return None,
                }
                j += 2;
            }
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    None
}

/// Salta un par de llaves/corchetes balanceado que empieza en `open`.
fn nested_end(b: &[u8], open: usize, op: u8, cl: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            if let Some((_, e)) = read_string(b, i) {
                i = e;
                continue;
            }
        } else if c == op {
            depth += 1;
        } else if c == cl {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

/// Índice (exclusivo) donde termina el valor que empieza en `i`.
pub fn value_end(text: &str, i: usize) -> Option<usize> {
    let b = text.as_bytes();
    match b.get(i).copied()? {
        b'{' => nested_end(b, i, b'{', b'}'),
        b'[' => nested_end(b, i, b'[', b']'),
        b'"' => read_string(b, i).map(|(_, e)| e),
        _ => {
            let mut j = i;
            while j < b.len() && !b[j].is_ascii_whitespace() && !matches!(b[j], b',' | b'}' | b']')
            {
                j += 1;
            }
            Some(j)
        }
    }
}

/// Busca `"clave":` desde `from` y devuelve dónde están la clave y su valor.
pub fn find_key_value(text: &str, key: &str, from: usize) -> Option<Loc> {
    let b = text.as_bytes();
    let mut i = skip_ws(b, from);
    while i < b.len() {
        if b[i] == b'"' {
            if let Some((content, after)) = read_string(b, i) {
                let j = skip_ws(b, after);
                if j < b.len() && b[j] == b':' {
                    let val_start = skip_ws(b, j + 1);
                    let val_end = value_end(text, val_start)?;
                    if content == key {
                        return Some(Loc {
                            key_start: i,
                            val_start,
                            val_end,
                        });
                    }
                    i = skip_ws(b, val_end);
                    continue;
                }
                i = skip_ws(b, after);
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Itera los miembros de un objeto: `f(clave, Loc)`.
pub fn for_each_member(obj: &str, mut f: impl FnMut(&str, &Loc)) {
    let b = obj.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return;
    }
    i = skip_ws(b, i + 1);
    while i < b.len() {
        if b[i] == b'"' {
            if let Some((key, after)) = read_string(b, i) {
                let j = skip_ws(b, after);
                if b.get(j) == Some(&b':') {
                    let val_start = skip_ws(b, j + 1);
                    if let Some(val_end) = value_end(obj, val_start) {
                        let loc = Loc {
                            key_start: i,
                            val_start,
                            val_end,
                        };
                        f(&key, &loc);
                        i = skip_ws(b, val_end);
                        continue;
                    }
                }
            }
        }
        // Salta el separador (coma o lo que haya) y sigue.
        i = skip_ws(b, i + 1);
    }
}

/// Reemplaza `[start..end)` del texto por `new`.
pub fn replace_range(text: &str, start: usize, end: usize, new: &str) -> String {
    let mut out = String::with_capacity(text.len() - (end - start) + new.len());
    out.push_str(&text[..start]);
    out.push_str(new);
    out.push_str(&text[end..]);
    out
}

/// Reemplaza el valor (no la clave) de un `Loc`.
pub fn replace_value(text: &str, loc: &Loc, new: &str) -> String {
    replace_range(text, loc.val_start, loc.val_end, new)
}

/// Valor string de un `Loc` (sin las comillas, con escapes sin decodificar).
pub fn string_value<'a>(text: &'a str, loc: &Loc) -> Option<&'a str> {
    let b = text.as_bytes();
    if b.get(loc.val_start) == Some(&b'"') && b.get(loc.val_end.saturating_sub(1)) == Some(&b'"') {
        Some(&text[loc.val_start + 1..loc.val_end - 1])
    } else {
        None
    }
}

/// Serializa un string como literal JSON (con comillas y escapes).
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Decodifica los escapes de un string JSON crudo (el contenido entre comillas).
pub fn unescape(raw: &str) -> Result<String, String> {
    let b = raw.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            let e = *b.get(i + 1).ok_or("escape truncado")?;
            match e {
                b'"' => {
                    out.push(b'"');
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'/' => {
                    out.push(b'/');
                    i += 2;
                }
                b'b' => {
                    out.push(8);
                    i += 2;
                }
                b'f' => {
                    out.push(12);
                    i += 2;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'u' => {
                    let hex = std::str::from_utf8(b.get(i + 2..i + 6).ok_or("\\u truncado")?)
                        .map_err(|_| "\\u inválido")?;
                    let cp = u32::from_str_radix(hex, 16).map_err(|_| "\\u inválido")?;
                    if (0xD800..0xDC00).contains(&cp) {
                        let rest = b.get(i + 6..).ok_or("surrogate truncado")?;
                        if rest.first() == Some(&b'\\') && rest.get(1) == Some(&b'u') {
                            let hex2 =
                                std::str::from_utf8(rest.get(2..6).ok_or("surrogate truncado")?)
                                    .map_err(|_| "\\u inválido")?;
                            let low = u32::from_str_radix(hex2, 16).map_err(|_| "\\u inválido")?;
                            if (0xDC00..0xE000).contains(&low) {
                                let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                                push_char(&mut out, char::from_u32(c).ok_or("\\u inválido")?);
                                i += 12;
                                continue;
                            }
                        }
                        return Err("surrogate alto sin par".into());
                    }
                    push_char(&mut out, char::from_u32(cp).ok_or("\\u inválido")?);
                    i += 6;
                }
                _ => return Err("escape desconocido".into()),
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "UTF-8 inválido".into())
}

/// Inserta un miembro al comienzo de un objeto (después de la `{`).
pub fn insert_first_member(obj: &str, member: &str) -> String {
    let b = obj.as_bytes();
    let i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return obj.to_string();
    }
    let rest = skip_ws(b, i + 1);
    if b.get(rest) == Some(&b'}') {
        // Objeto vacío: sin coma.
        replace_range(obj, rest, rest, member)
    } else {
        replace_range(obj, rest, rest, &format!("{member}, "))
    }
}

/// Reemplaza el valor string de `key`, o inserta el miembro si no existe.
pub fn upsert_string(obj: &str, key: &str, value: &str) -> String {
    match find_key_value(obj, key, 0) {
        Some(loc) => replace_range(obj, loc.val_start, loc.val_end, &json_string(value)),
        None => insert_first_member(
            obj,
            &format!("{}: {}", json_string(key), json_string(value)),
        ),
    }
}

/// Elimina un miembro (clave + valor) del texto, arreglando la coma.
pub fn remove_member(text: &str, loc: &Loc) -> String {
    let b = text.as_bytes();
    // Coma previa (si la hay).
    let mut left = loc.key_start;
    while left > 0 && b[left - 1].is_ascii_whitespace() {
        left -= 1;
    }
    let prev_comma = left > 0 && b[left - 1] == b',';
    // Coma siguiente (si la hay).
    let mut right = loc.val_end;
    while right < b.len() && b[right].is_ascii_whitespace() {
        right += 1;
    }
    let next_comma = right < b.len() && b[right] == b',';
    let (cut_from, cut_to) = if prev_comma {
        // Se saca ", clave: valor" (la coma previa); si hay coma siguiente,
        // queda para separar al miembro que viene.
        (left - 1, right)
    } else if next_comma {
        // Primer miembro: "clave: valor," (se saca también el espacio previo).
        (left, right + 1)
    } else {
        (left, right)
    };
    format!("{}{}", &text[..cut_from], &text[cut_to..])
}

fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    let mut digits = 0;
    let mut leading_zero = false;
    while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
        if digits == 0 && b[i] == b'0' {
            leading_zero = true;
        }
        i += 1;
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    // JSON no permite ceros a la izquierda (01, 007…), salvo el "0" solo.
    if leading_zero && digits > 1 {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let mut frac = 0;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
            frac += 1;
        }
        if frac == 0 {
            return false;
        }
    }
    if b.get(i).is_some_and(|c| *c == b'e' || *c == b'E') {
        i += 1;
        if b.get(i).is_some_and(|c| *c == b'+' || *c == b'-') {
            i += 1;
        }
        let mut exp = 0;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
            exp += 1;
        }
        if exp == 0 {
            return false;
        }
    }
    i == b.len()
}

fn expect(text: &str, i: &mut usize, lit: &str) -> Result<(), String> {
    let b = text.as_bytes();
    let end = skip_ws(b, *i);
    if end + lit.len() <= b.len() && &b[end..end + lit.len()] == lit.as_bytes() {
        *i = end + lit.len();
        Ok(())
    } else {
        Err(format!("se esperaba «{lit}»"))
    }
}

fn parse_value(text: &str, i: &mut usize) -> Result<Value, String> {
    let b = text.as_bytes();
    *i = skip_ws(b, *i);
    let c = *b.get(*i).ok_or("fin de entrada inesperado")?;
    match c {
        b'{' => parse_object(text, i),
        b'[' => parse_array(text, i),
        b'"' => {
            let (s, end) = read_string(b, *i).ok_or("string mal formado")?;
            *i = end;
            Ok(Value::Str(s))
        }
        b't' => {
            expect(text, i, "true")?;
            Ok(Value::Bool(true))
        }
        b'f' => {
            expect(text, i, "false")?;
            Ok(Value::Bool(false))
        }
        b'n' => {
            expect(text, i, "null")?;
            Ok(Value::Null)
        }
        _ => {
            let start = *i;
            let mut j = start;
            while j < b.len() && !b[j].is_ascii_whitespace() && !matches!(b[j], b',' | b'}' | b']')
            {
                j += 1;
            }
            if j == start {
                return Err(format!("token inesperado en la posición {}", *i));
            }
            let raw = &text[start..j];
            if !is_number(raw) {
                return Err(format!("número inválido: {raw}"));
            }
            *i = j;
            Ok(Value::Num(raw.to_string()))
        }
    }
}

fn parse_object(text: &str, i: &mut usize) -> Result<Value, String> {
    let b = text.as_bytes();
    *i = skip_ws(b, *i + 1);
    let mut members: Vec<(String, Value)> = Vec::new();
    loop {
        *i = skip_ws(b, *i);
        let c = *b.get(*i).ok_or("objeto sin cerrar")?;
        if c == b'}' {
            *i += 1;
            return Ok(Value::Object(members));
        }
        if c != b'"' {
            return Err(format!("se esperaba una clave string (posición {})", *i));
        }
        let (key, after) = read_string(b, *i).ok_or("clave mal formada")?;
        *i = skip_ws(b, after);
        if b.get(*i) != Some(&b':') {
            return Err("se esperaba «:» después de la clave".into());
        }
        *i = skip_ws(b, *i + 1);
        let v = parse_value(text, i)?;
        members.push((key, v));
        *i = skip_ws(b, *i);
        match b.get(*i) {
            Some(b',') => {
                *i += 1;
                // Coma final (`,}`): inválida en JSON estricto.
                if matches!(b.get(skip_ws(b, *i)), Some(b'}')) {
                    return Err("coma final en objeto".into());
                }
            }
            Some(b'}') => {
                *i += 1;
                return Ok(Value::Object(members));
            }
            _ => return Err("se esperaba «,» o «}»".into()),
        }
    }
}

fn parse_array(text: &str, i: &mut usize) -> Result<Value, String> {
    let b = text.as_bytes();
    *i = skip_ws(b, *i + 1);
    let mut items: Vec<Value> = Vec::new();
    loop {
        *i = skip_ws(b, *i);
        match b.get(*i) {
            Some(b']') => {
                *i += 1;
                return Ok(Value::Array(items));
            }
            None => return Err("array sin cerrar".into()),
            _ => {
                let v = parse_value(text, i)?;
                items.push(v);
                *i = skip_ws(b, *i);
                match b.get(*i) {
                    Some(b',') => {
                        *i += 1;
                        if matches!(b.get(skip_ws(b, *i)), Some(b']')) {
                            return Err("coma final en array".into());
                        }
                    }
                    Some(b']') => {
                        *i += 1;
                        return Ok(Value::Array(items));
                    }
                    _ => return Err("se esperaba «,» o «]»".into()),
                }
            }
        }
    }
}

/// Parsea el texto completo (tolera comentarios JSONC).
pub fn parse(text: &str) -> Result<Value, String> {
    let b = text.as_bytes();
    let mut i = 0;
    let v = parse_value(text, &mut i)?;
    let end = skip_ws(b, i);
    if end != b.len() {
        return Err("sobra texto después del valor".into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localiza_una_clave_y_su_valor() {
        let t = r#"{ "a": 1, "providers": { "x": { "name": "X" } } }"#;
        let loc = find_key_value(t, "providers", 0).expect("debe encontrar providers");
        assert_eq!(
            &t[loc.val_start..loc.val_end],
            r#"{ "x": { "name": "X" } }"#
        );
    }

    #[test]
    fn localiza_con_comentarios_jsonc() {
        let t = "{\n  // provider armado a mano\n  \"providers\": {}\n}";
        assert!(find_key_value(t, "providers", 0).is_some());
        let jsonc = "{\n  \"a\": 1, /* comentario */ \"providers\": {}\n}";
        assert!(find_key_value(jsonc, "providers", 0).is_some());
    }

    #[test]
    fn saltea_strings_con_llaves() {
        // La llave adentro de un string no debe contar para `value_end`.
        let t = r#"{ "msg": "hola } mundo", "n": 2 }"#;
        let loc = find_key_value(t, "msg", 0).expect("msg");
        assert_eq!(&t[loc.val_start..loc.val_end], r#""hola } mundo""#);
    }

    #[test]
    fn upsert_reemplaza_o_inserta() {
        let t = r#"{ "name": "A", "package": "p" }"#;
        let out = upsert_string(t, "name", "B");
        assert_eq!(out, r#"{ "name": "B", "package": "p" }"#);
        let out2 = upsert_string(t, "apiKey", "sk-x");
        assert!(out2.contains("\"apiKey\": \"sk-x\""));
        assert!(find_key_value(&out2, "apiKey", 0).is_some());
    }

    #[test]
    fn remove_arregla_la_coma() {
        let t = r#"{ "a": 1, "b": 2, "c": 3 }"#;
        let loc = find_key_value(t, "b", 0).unwrap();
        let out = remove_member(t, &loc);
        assert_eq!(out, r#"{ "a": 1, "c": 3 }"#);
        assert!(jsonx_parse_ok(&out));

        let t2 = r#"{ "b": 2, "c": 3 }"#;
        let loc2 = find_key_value(t2, "b", 0).unwrap();
        assert_eq!(remove_member(t2, &loc2), r#"{ "c": 3 }"#);

        let t3 = r#"{ "a": 1, "b": 2 }"#;
        let loc3 = find_key_value(t3, "b", 0).unwrap();
        let out3 = remove_member(t3, &loc3);
        // El espacio antes de `}` puede variar; lo que importa es que quede
        // JSON válido con el miembro sacado.
        assert!(!out3.contains("b"));
        assert!(jsonx_parse_ok(&out3));
        assert!(out3.contains("\"a\""));
    }

    fn jsonx_parse_ok(t: &str) -> bool {
        parse(t).is_ok()
    }

    #[test]
    fn valida_bien_y_mal() {
        assert!(parse(r#"{"a": [1, 2, {"b": true}], "c": null}"#).is_ok());
        assert!(parse(r#"{ "a": 1, }"#).is_err());
        assert!(parse(r#"{"a": 01}"#).is_err());
        assert!(parse("{\n  // nota\n  \"a\": 1\n}").is_ok());
    }

    #[test]
    fn strings_con_escapes_redondan() {
        let s = "hola \"mundo\"\nlinea2";
        let lit = json_string(s);
        let doc = format!("{{\"k\": {lit}}}");
        let loc = find_key_value(&doc, "k", 0).unwrap();
        let raw = string_value(&doc, &loc).unwrap();
        assert_eq!(unescape(raw).unwrap(), s);
    }
}
