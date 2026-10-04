//! Emulador de pantalla para la terminal real.
//!
//! Envuelve el parser VT de `vt100` y lo convierte a algo que Slint pueda
//! dibujar: cada fila es una lista de **tramos** (`Run`) de texto con el mismo
//! frente, fondo y negrita. Agrupar celdas iguales evita generar un `Text` por
//! carácter.
//!
//! El cursor se dibuja invirtiendo los colores de su celda: así no hay que
//! calcular píxeles. El **scrollback** lo maneja `vt100`: se pide con
//! `set_scrollback` y el propio parser mantiene la vista anclada al contenido
//! cuando llega salida nueva mientras se está mirando hacia atrás.
//!
//! La **selección** se pasa como una región de celdas (fila/columna de inicio y
//! fin, fin exclusivo) y se pinta cambiando el fondo; el texto para copiar sale
//! de `contents_between`, que respeta las líneas que envuelven.

/// Color RGB de 8 bits por canal.
pub type Rgb = (u8, u8, u8);

/// Región de selección en coordenadas de la vista: `(fila, columna)` de inicio
/// y fin. El fin es **exclusivo** (la celda final no entra).
pub type Selection = (u16, u16, u16, u16);

/// Frente y fondo por defecto (coinciden con el panel del IDE).
pub const DEFAULT_FG: Rgb = (0xe8, 0xe2, 0xd0);
pub const DEFAULT_BG: Rgb = (0x15, 0x1d, 0x1f);

/// Fondo de las celdas seleccionadas (mismo tono que el editor).
pub const SELECTION_BG: Rgb = (0x29, 0x44, 0x43);

/// Líneas de historial que guarda el emulador. Es el tope del scrollback: más
/// allá se descarta lo más viejo.
const SCROLLBACK: usize = 2_000;

/// Un tramo de texto con un mismo estilo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    /// Columna (celda) donde arranca el tramo dentro de la fila.
    pub col: u16,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
}

/// Pantalla de terminal viva.
pub struct Screen {
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), SCROLLBACK),
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        self.parser.screen_mut().set_size(self.rows, self.cols);
    }

    /// Líneas que hay ahora mismo por encima de la pantalla visible (0 = la
    /// vista sigue el final).
    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// Total de líneas guardadas en el historial, sin importar dónde esté la
    /// vista. `vt100` no lo expone, así que se sondea pidiendo el máximo (se
    /// acota al historial real) y restaurando la posición.
    pub fn scrollback_total(&mut self) -> usize {
        let saved = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let total = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(saved);
        total
    }

    /// Mueve la vista `delta` líneas (positivo = hacia atrás, más viejo).
    pub fn scroll_by(&mut self, delta: i32) {
        let current = self.scrollback() as i32;
        let total = self.scrollback_total() as i32;
        let target = (current + delta).clamp(0, total) as usize;
        self.parser.screen_mut().set_scrollback(target);
    }

    /// Vuelve la vista al final.
    pub fn scroll_to_end(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Texto entre dos celdas de la vista, para copiar. `vt100` maneja las
    /// líneas que envuelven (no mete un `\n` donde la línea se continuó).
    pub fn text_between(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        self.parser
            .screen()
            .contents_between(start_row, start_col, end_row, end_col)
    }

    /// Posición del cursor, si se ve.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            None
        } else {
            Some(screen.cursor_position())
        }
    }

    /// Devuelve las filas (como tramos) y la posición del cursor, si se ve.
    /// `sel` pinta el fondo de las celdas seleccionadas.
    pub fn snapshot(&self, sel: Option<Selection>) -> (Vec<Vec<Run>>, Option<(u16, u16)>) {
        let screen = self.parser.screen();
        let cursor = self.cursor();

        let mut lines: Vec<Vec<Run>> = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            let mut runs: Vec<Run> = Vec::new();
            let mut current: Option<Run> = None;

            for col in 0..self.cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let mut text = cell.contents().to_string();
                if text.is_empty() {
                    text.push(' ');
                }
                let mut fg = color_to_rgb(cell.fgcolor(), DEFAULT_FG);
                let mut bg = color_to_rgb(cell.bgcolor(), DEFAULT_BG);
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let is_cursor = cursor == Some((row, col));
                if is_cursor {
                    std::mem::swap(&mut fg, &mut bg);
                }
                // La selección gana sobre el cursor: el fondo elegido indica
                // qué se va a copiar.
                if sel.is_some_and(|s| in_selection(s, row, col)) {
                    bg = SELECTION_BG;
                }
                let bold = cell.bold();

                match &mut current {
                    Some(run) if run.fg == fg && run.bg == bg && run.bold == bold => {
                        run.text.push_str(&text);
                    }
                    _ => {
                        if let Some(run) = current.take() {
                            runs.push(run);
                        }
                        current = Some(Run {
                            text,
                            col,
                            fg,
                            bg,
                            bold,
                        });
                    }
                }
            }
            if let Some(run) = current {
                runs.push(run);
            }
            lines.push(runs);
        }

        (lines, cursor)
    }
}

/// `true` si la celda `(row, col)` cae dentro de la región (fin exclusivo).
fn in_selection((sr, sc, er, ec): Selection, row: u16, col: u16) -> bool {
    (row, col) >= (sr, sc) && (row, col) < (er, ec)
}

/// Traduce un color de `vt100` a RGB.
fn color_to_rgb(color: vt100::Color, default: Rgb) -> Rgb {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => idx_to_rgb(i),
        vt100::Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// Paleta xterm de 256 colores (16 base + cubo 6×6×6 + 24 grises).
fn idx_to_rgb(i: u8) -> Rgb {
    const BASE: [Rgb; 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            (r * 51, g * 51, b * 51)
        }
        _ => {
            let v = 8u16 + (i as u16 - 232) * 10;
            let v = v.min(255) as u8;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_paleta_cubre_los_extremos() {
        assert_eq!(idx_to_rgb(0), (0, 0, 0));
        assert_eq!(idx_to_rgb(15), (255, 255, 255));
        assert_eq!(idx_to_rgb(16), (0, 0, 0));
        assert_eq!(idx_to_rgb(231), (255, 255, 255));
        assert_eq!(idx_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn agrupa_celdas_con_el_mismo_estilo() {
        let mut screen = Screen::new(2, 10);
        screen.process(b"hola");
        let (lines, cursor) = screen.snapshot(None);
        // La primera fila arranca con "hola" en un tramo (mismo estilo).
        let joined: String = lines[0].iter().map(|r| r.text.as_str()).collect();
        assert!(joined.starts_with("hola"));
        // El cursor quedó después de "hola".
        assert_eq!(cursor, Some((0, 4)));
    }

    #[test]
    fn el_cursor_invierte_los_colores() {
        let mut screen = Screen::new(1, 4);
        screen.process(b"ab");
        let (lines, _) = screen.snapshot(None);
        // El tramo del cursor tiene el fondo cambiado.
        let has_inverted = lines[0].iter().any(|r| r.bg == DEFAULT_FG);
        assert!(has_inverted, "no se invirtió el cursor: {:?}", lines[0]);
    }

    #[test]
    fn respeta_el_color_de_frente() {
        let mut screen = Screen::new(1, 10);
        // "A" en rojo (SGR 31) y reset.
        screen.process(b"\x1b[31mA\x1b[0mB");
        let (lines, _) = screen.snapshot(None);
        assert!(lines[0]
            .iter()
            .any(|r| r.text.contains('A') && r.fg == idx_to_rgb(1)));
    }

    #[test]
    fn el_scrollback_guarda_lineas_y_se_recorre() {
        let mut screen = Screen::new(2, 8);
        // Tres líneas en una pantalla de dos filas: "uno" pasa al historial.
        screen.process(b"uno\r\ndos\r\ntres");
        assert_eq!(screen.scrollback_total(), 1);
        assert_eq!(screen.scrollback(), 0);

        // Subir una línea deja "uno" a la vista.
        screen.scroll_by(1);
        assert_eq!(screen.scrollback(), 1);
        let (lines, _) = screen.snapshot(None);
        let top: String = lines[0].iter().map(|r| r.text.as_str()).collect();
        assert!(top.starts_with("uno"), "primer tramo: {top:?}");

        // Pedir de más se acota al total y volver al final.
        screen.scroll_by(99);
        assert_eq!(screen.scrollback(), 1);
        screen.scroll_to_end();
        assert_eq!(screen.scrollback(), 0);
    }

    #[test]
    fn la_seleccion_marca_el_texto_y_lo_devuelve() {
        let mut screen = Screen::new(2, 10);
        screen.process(b"hola mundo\r\nsegunda");
        // "hola" = fila 0, columnas 0..4 (fin exclusivo).
        let (lines, _) = screen.snapshot(Some((0, 0, 0, 4)));
        let selected_bg = lines[0]
            .iter()
            .filter(|r| r.bg == SELECTION_BG)
            .map(|r| r.text.as_str())
            .collect::<String>();
        assert_eq!(selected_bg, "hola");

        let text = screen.text_between(0, 0, 0, 4);
        assert_eq!(text, "hola");

        // Multilínea: de "hola mundo" (fila 0) hasta "seg" (fila 1).
        let multi = screen.text_between(0, 5, 1, 3);
        assert!(multi.starts_with("mundo"), "multilínea: {multi:?}");
        assert!(multi.trim_end().ends_with("seg"), "multilínea: {multi:?}");
    }
}
