//! Internacionalización (i18n).
//!
//! Dos mecanismos, **un solo catálogo** (`lang/<locale>/LC_MESSAGES/nuide.po`):
//!
//! - los `.slint` usan `@tr(...)` y Slint los resuelve con las traducciones
//!   **empaquetadas** (ver `build.rs`);
//! - el código Rust usa [`t`], que lee el mismo `.po` (embebido con
//!   `include_str!`) para las cadenas que arma Rust (menús, paleta,
//!   configuración, diálogos).
//!
//! El idioma se fija al arrancar (ver `main.rs`); cambiarlo se aplica al
//! reiniciar.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Idiomas disponibles: `(locale, nombre nativo)`.
pub const LANGS: [(&str, &str); 3] = [
    ("es", "Español"),
    ("en", "English"),
    ("pt-BR", "Português (BR)"),
];

/// Locale por defecto.
pub const DEFAULT: &str = "es";

static CURRENT: OnceLock<String> = OnceLock::new();
static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Fija el idioma activo (llamar una vez, al arrancar).
pub fn set_locale(locale: &str) {
    let _ = CURRENT.set(locale.to_string());
}

/// Locale activo.
pub fn locale() -> &'static str {
    CURRENT.get().map(String::as_str).unwrap_or(DEFAULT)
}

/// Nombre nativo del locale (para el selector).
pub fn display_name(locale: &str) -> &'static str {
    LANGS
        .iter()
        .find(|(l, _)| *l == locale)
        .map(|(_, n)| *n)
        .unwrap_or(LANGS[0].1)
}

/// Traduce `msgid`; si no hay traducción, devuelve el propio `msgid`.
pub fn t(msgid: &'static str) -> &'static str {
    match catalog().get(msgid) {
        Some(tr) => tr.as_str(),
        None => msgid,
    }
}

fn catalog() -> &'static HashMap<String, String> {
    CATALOG.get_or_init(|| parse_po(catalog_text(locale())))
}

fn catalog_text(locale: &str) -> &'static str {
    match locale {
        "en" => include_str!("../lang/en/LC_MESSAGES/nuide.po"),
        "pt-BR" => include_str!("../lang/pt-BR/LC_MESSAGES/nuide.po"),
        _ => include_str!("../lang/es/LC_MESSAGES/nuide.po"),
    }
}

/// Parser mínimo de `.po`: pares `msgid`/`msgstr` de una línea. Ignora el
/// encabezado (`msgid ""`) y las entradas sin traducción.
fn parse_po(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut msgid: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            msgid = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            if let Some(id) = msgid.take() {
                let tr = unquote(rest);
                if !id.is_empty() && !tr.is_empty() && tr != id {
                    map.insert(id, tr);
                }
            }
        }
    }
    map
}

/// Quita las comillas y desescapa `\n`, `\"`, `\\` y `\t`.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_pares_msgid_msgstr() {
        let po = "msgid \"\"\nmsgstr \"\"\n\"Header: x\\n\"\n\nmsgid \"Hola\"\nmsgstr \"Hello\"\n";
        let m = parse_po(po);
        assert_eq!(m.get("Hola").map(String::as_str), Some("Hello"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn sin_traduccion_devuelve_msgid() {
        // Clave inexistente en cualquier catálogo: cae al propio msgid.
        assert_eq!(t("__clave_inexistente__"), "__clave_inexistente__");
    }

    #[test]
    fn idiomas_tienen_nombre() {
        assert_eq!(display_name("pt-BR"), "Português (BR)");
        assert_eq!(display_name("xx"), "Español");
    }

    #[test]
    fn catalogo_en_tiene_los_tooltips() {
        let m = parse_po(catalog_text("en"));
        assert_eq!(m.get("Minimizar").map(String::as_str), Some("Minimize"));
        assert_eq!(
            m.get("Cerrar la búsqueda (Esc)").map(String::as_str),
            Some("Close the search (Esc)")
        );
        assert!(m.contains_key("Idioma"));
    }

    /// El `.pot` tiene que estar al día con `@tr`/`t()` del código. Se saltea si
    /// no hay Python (igual que el test del C generado de SpASM).
    #[test]
    fn el_pot_esta_al_dia() {
        use std::process::Command;
        let Some(py) = ["python", "python3", "py"].into_iter().find(|c| {
            Command::new(c)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }) else {
            eprintln!("test salteado: falta Python");
            return;
        };
        let out = Command::new(py)
            .args(["tools/i18n_extract.py", "--check"])
            .output()
            .expect("correr tools/i18n_extract.py");
        assert!(
            out.status.success(),
            "i18n desactualizado:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
