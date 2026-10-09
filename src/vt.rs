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
    /// Huella de cada fila visible en el último `changed_rows`. Con ella el
    /// refresco deja de ser "toda la pantalla": sólo se reconstruyen las filas
    /// cuyo texto o estilo cambió (una TUI que mueve un spinner paga una fila,
    /// no las 24-200 que se ven).
    sigs: Vec<u64>,
    /// Flag de "sucio": se activa cuando llega salida nueva del PTY.
    /// `changed_rows` solo recalcula si está activo, evitando trabajo cuando
    /// la terminal está quieta.
    dirty: bool,
    /// Última selección usada en `changed_rows`. Si cambia, se fuerza el
    /// recálculo aunque no haya salida nueva (la selección pinta fondo).
    last_sel: Option<Selection>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), SCROLLBACK),
            rows: rows.max(1),
            cols: cols.max(1),
            sigs: Vec::new(),
            dirty: true, // Primera vez: todo está "sucio" para dibujar la pantalla inicial.
            last_sel: None,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.dirty = true; // Llegó salida nueva: marcar para recalcular.
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        // El reflujo del texto invalida cualquier huella guardada.
        self.sigs.clear();
        self.parser.screen_mut().set_size(self.rows, self.cols);
        self.dirty = true; // Resize: todo cambió.
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

    /// Modo de *mouse reporting* que pidió la aplicación (`None` = deshabilitado).
    pub fn mouse_protocol_mode(&self) -> vt100::MouseProtocolMode {
        self.parser.screen().mouse_protocol_mode()
    }

    /// Codificación del *mouse reporting* activo (SGR, UTF-8 o por defecto).
    pub fn mouse_protocol_encoding(&self) -> vt100::MouseProtocolEncoding {
        self.parser.screen().mouse_protocol_encoding()
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
    /// `sel` pinta el fondo de las celdas seleccionadas. Reconstruye **todo**:
    /// para el refresco periódico está `changed_rows`.
    pub fn snapshot(&self, sel: Option<Selection>) -> (Vec<Vec<Run>>, Option<(u16, u16)>) {
        let cursor = self.cursor();
        let lines = (0..self.rows)
            .map(|row| self.build_row(row, sel, cursor))
            .collect();
        (lines, cursor)
    }

    /// Filas que cambiaron desde la llamada anterior, como `(índice, tramos)`.
    /// La primera vez (o tras un `resize`) devuelve todas. Si no hay salida
    /// nueva desde el último llamado y no cambió la selección, devuelve vacío
    /// sin hacer trabajo.
    pub fn changed_rows(&mut self, sel: Option<Selection>) -> Vec<(usize, Vec<Run>)> {
        // Si no llegó salida nueva y no cambió la selección, no hay nada que hacer.
        let sel_changed = sel != self.last_sel;
        self.last_sel = sel;
        if !self.dirty && !sel_changed {
            return Vec::new();
        }
        self.dirty = false;

        let cursor = self.cursor();
        let previous = std::mem::take(&mut self.sigs);
        let mut sigs: Vec<u64> = Vec::with_capacity(self.rows as usize);
        let mut changed: Vec<(usize, Vec<Run>)> = Vec::new();
        for row in 0..self.rows {
            let hash = self.row_hash(row, sel, cursor);
            if previous.get(row as usize).copied().unwrap_or(0) != hash {
                changed.push((row as usize, self.build_row(row, sel, cursor)));
            }
            sigs.push(hash);
        }
        self.sigs = sigs;
        changed
    }

    /// Colores y negrita de una celda, ya con inversión, cursor y selección.
    fn cell_paint(cell: &vt100::Cell, is_cursor: bool, selected: bool) -> (Rgb, Rgb, bool) {
        let mut fg = color_to_rgb(cell.fgcolor(), DEFAULT_FG);
        let mut bg = color_to_rgb(cell.bgcolor(), DEFAULT_BG);
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        if is_cursor {
            std::mem::swap(&mut fg, &mut bg);
        }
        // La selección gana sobre el cursor: el fondo elegido indica
        // qué se va a copiar.
        if selected {
            bg = SELECTION_BG;
        }
        (fg, bg, cell.bold())
    }

    /// Huella de la fila: recorre las celdas leyendo texto y estilo **sin
    /// asignar memoria** (el `contents()` de `vt100` es un `&str` prestado por
    /// la pantalla). Es el filtro barato para saber si hay que reconstruirla.
    /// Usa un hash rápido (FNV-1a simplificado) en vez del `DefaultHasher` de
    /// Rust, que es más caro. Para detectar cambios de una fila a la siguiente
    /// es más que suficiente.
    fn row_hash(&self, row: u16, sel: Option<Selection>, cursor: Option<(u16, u16)>) -> u64 {
        let screen = self.parser.screen();
        // FNV-1a: rápido y buena distribución para strings cortos.
        let mut hash: u64 = 0xcbf29ce484222325;
        for col in 0..self.cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            // Hash del contenido de la celda (bytes).
            for b in cell.contents().bytes() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            let (fg, bg, bold) = Self::cell_paint(
                cell,
                cursor == Some((row, col)),
                sel.is_some_and(|s| in_selection(s, row, col)),
            );
            // Hash de los colores y negrita (6 bytes).
            hash ^= fg.0 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= fg.1 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= fg.2 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= bg.0 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= bg.1 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= bg.2 as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash ^= bold as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Tramos de una fila: agrupa celdas consecutivas del mismo estilo.
    fn build_row(&self, row: u16, sel: Option<Selection>, cursor: Option<(u16, u16)>) -> Vec<Run> {
        let screen = self.parser.screen();
        let mut runs: Vec<Run> = Vec::new();
        let mut current: Option<Run> = None;

        for col in 0..self.cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let (fg, bg, bold) = Self::cell_paint(
                cell,
                cursor == Some((row, col)),
                sel.is_some_and(|s| in_selection(s, row, col)),
            );
            match &mut current {
                Some(run) if run.fg == fg && run.bg == bg && run.bold == bold => {
                    // La celda vacía se manda como espacio para que el `Text`
                    // de Slint ocupe su columna.
                    if cell.contents().is_empty() {
                        run.text.push(' ');
                    } else {
                        run.text.push_str(cell.contents());
                    }
                }
                _ => {
                    if let Some(run) = current.take() {
                        runs.push(run);
                    }
                    let mut text = String::from(cell.contents());
                    if text.is_empty() {
                        text.push(' ');
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
        runs
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
    fn detecta_el_mouse_reporting() {
        let mut screen = Screen::new(24, 80);
        assert_eq!(screen.mouse_protocol_mode(), vt100::MouseProtocolMode::None);
        // `?1000h` habilita "press/release"; `?1002h` "button motion";
        // `?1006h` SGR.
        screen.process(b"\x1b[?1000h");
        assert_eq!(
            screen.mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease
        );
        screen.process(b"\x1b[?1002h");
        assert_eq!(
            screen.mouse_protocol_mode(),
            vt100::MouseProtocolMode::ButtonMotion
        );
        screen.process(b"\x1b[?1006h");
        assert_eq!(
            screen.mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr
        );
        screen.process(b"\x1b[?1002l\x1b[?1000l");
        assert_eq!(screen.mouse_protocol_mode(), vt100::MouseProtocolMode::None);
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

    #[test]
    fn el_refresco_incremental_solo_trae_las_filas_cambiadas() {
        let mut screen = Screen::new(6, 20);
        // La primera vez no hay huellas: trae todas las filas.
        assert_eq!(screen.changed_rows(None).len(), 6);
        // Sin salida nueva y sin cambio de estilo: no se redrawa nada.
        assert!(screen.changed_rows(None).is_empty());

        // Salida en una fila: cambian ésa y la que queda bajo el cursor.
        screen.process(b"holamundo\r\n");
        let changed = screen.changed_rows(None);
        let indices: Vec<usize> = changed.iter().map(|(i, _)| *i).collect();
        assert!(indices.contains(&0), "la fila escrita cambió: {indices:?}");
        assert!(
            changed.len() < 6,
            "no se reconstruyó toda la pantalla: {}",
            changed.len()
        );

        // El tramo que devuelve es exactamente el que daría el snapshot completo:
        // reusar filas no cambia lo que se ve.
        assert!(!changed.is_empty());
        let (all, _) = screen.snapshot(None);
        for (i, runs) in &changed {
            assert_eq!(
                runs, &all[*i],
                "fila {i} distinta entre refresco e incremental"
            );
        }

        // La selección entra en la huella: seleccionar redrawa las filas tocadas.
        assert!(screen
            .changed_rows(Some((0, 0, 0, 4)))
            .iter()
            .any(|(i, _)| *i == 0));
        // Y el resize vuelve a traer todo.
        screen.resize(4, 20);
        assert_eq!(screen.changed_rows(None).len(), 4);
    }
}
