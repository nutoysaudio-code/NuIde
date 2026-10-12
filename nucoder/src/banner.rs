//! Presentación de NuCoder: arte ASCII moderno (bloques sólidos, legible) que se
//! muestra al iniciar la TUI y en `--help`, con la versión del software.

/// Ancho del arte en columnas. Si la terminal es más angosta, se usa `compact`.
pub const WIDTH: usize = 41;

/// "NUCODER" en un bloque de 5 filas (glifos sólidos).
pub const ART: [&str; 5] = [
    "█   █ █   █  ███   ███  ████  █████ ████ ",
    "██  █ █   █ █   █ █   █ █   █ █     █   █",
    "█ █ █ █   █ █     █   █ █   █ ████  ████ ",
    "█  ██ █   █ █   █ █   █ █   █ █     █  █ ",
    "█   █  ███   ███   ███  ████  █████ █   █",
];

/// El arte como texto (filas separadas por salto de línea).
pub fn text() -> String {
    ART.join("\n")
}

/// Línea de título con la versión (TUI).
pub fn title() -> String {
    format!(
        "NuCoder {}  ·  agente de código  ·  router de modelos (gratis primero)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Título compacto para terminales angostas.
pub fn compact() -> String {
    format!(
        "NuCoder {} — router de modelos (gratis primero) + coder",
        env!("CARGO_PKG_VERSION")
    )
}

/// Encabezado para `--help`.
pub fn header() -> String {
    format!(
        "NuCoder {} — Router de modelos AI (gratis primero) + coder",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arte_alineado_y_legible() {
        for (i, l) in ART.iter().enumerate() {
            assert_eq!(l.chars().count(), WIDTH, "fila {i} desalineada");
        }
        assert!(text().contains('█'));
    }

    #[test]
    fn titulo_lleva_la_version() {
        assert!(title().contains(env!("CARGO_PKG_VERSION")));
    }
}
