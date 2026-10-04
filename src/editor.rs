//! Estado de NuIde: el puente entre la ventana Slint y la lógica real.
//!
//! Mantiene el toolchain de SpASM, el árbol de archivos, las pestañas del
//! editor, la terminal real (PTY) y el panel del agente, y cablea los callbacks
//! de la ventana.
//!
//! ### Terminales
//!
//! La terminal local es un **PTY** (ConPTY en Windows) con una shell
//! persistente; los bytes crudos los interpreta el emulador de `vt`. Escribir es
//! mandarle teclas a la shell. Los menús Ejecutar/Compilar escriben la orden en
//! esa misma shell, así la salida aparece en la terminal.
//!
//! El panel del agente sigue siendo una consola *lineal* (`terminal`), porque
//! `opencode run` es un proceso por prompt.
//!
//! Los procesos corren en hilos y avisan por canales. Como los callbacks de
//! Slint viven en el hilo de la UI, un `Timer` de 60 ms drena los canales y
//! vuelca el estado con `sync`. Ese timer se guarda en `Ide` para que no se caiga
//! al salir de `attach`; su closure usa un `Weak` para no crear un ciclo.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::explorer::{Action, Explorer};
use crate::highlight;
use crate::pty::{self, Pty, PtyEvent};
use crate::spasm::Toolchain;
use crate::terminal::{self, Line, Msg, Terminal};
use crate::ui::{
    IdeCodeLine, IdeTab, IdeTermLine, IdeToken, IdeTreeNode, NuIdeWindow, TermLine, TermRun,
};
use crate::vt::Screen;

/// Tope de líneas que se abren de un archivo. Un archivo enorme no puede
/// congelar la UI; lo que sobra no se muestra.
const MAX_OPEN_LINES: usize = 20_000;

/// Tope de pasos de deshacer por archivo.
const MAX_UNDO: usize = 500;

/// Glifo del cursor. Se inserta como un token más para no tener que calcular
/// píxeles: el render es monoespaciado y el caret es un carácter al lado.
const CURSOR_GLYPH: &str = "|";

/// Prompt del panel del agente. Es fijo (el proyecto ya se ve en la ficha).
const AGENT_PROMPT: &str = "opencode> ";

/// Ancho aproximado de un carácter monoespaciado a 12 px y arranque del texto
/// dentro de la fila (ver `IdeCodeRow`). Se usa sólo para ubicar el cursor al
/// hacer clic: un error de una columna en el borde es aceptable.
const CHAR_W: f32 = 7.2;
const CODE_X0: f32 = 58.0;
/// Alto de fila y margen superior del contenido del editor (ver `IdeCodeRow` y
/// el `VerticalLayout` de `IdeEditor`).
const ROW_H: f32 = 20.0;
const CODE_TOP: f32 = 8.0;

/// Tamaño inicial de la terminal real (columnas × filas) hasta que la UI avise
/// el tamaño del panel.
const TERM_COLS: u16 = 100;
const TERM_ROWS: u16 = 24;
/// Métricas de celda de la terminal real. Coinciden con `TerminalScreen`
/// (mono 12 px). El ancho es aproximado: sólo decide cuántas columnas pedirle al
/// PTY, no la posición del cursor (que se dibuja invirtiendo la celda).
const TERM_CELL_W: f32 = 7.2;
const TERM_CELL_H: f32 = 16.0;

/// Posición del cursor dentro de un buffer: línea y columna **en caracteres**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    line: usize,
    col: usize,
}

/// Un archivo abierto en una pestaña del editor.
struct OpenTab {
    path: PathBuf,
    /// Líneas sin el `\n` (el separador se reconstruye al guardar).
    lines: Vec<String>,
    cursor: Cursor,
    /// Ancla de la selección. `None` = sin selección. La selección va de
    /// `anchor` a `cursor`, en cualquier orden.
    anchor: Option<Cursor>,
    dirty: bool,
    /// El archivo original terminaba en salto de línea. Se conserva al guardar.
    trailing_newline: bool,
    /// Binario o ilegible: se muestra pero no se edita ni se guarda.
    read_only: bool,
}

impl OpenTab {
    fn snapshot(&self) -> (Vec<String>, Cursor) {
        (self.lines.clone(), self.cursor)
    }

    fn line_len(&self, line: usize) -> usize {
        self.lines.get(line).map(|l| l.chars().count()).unwrap_or(0)
    }

    /// Mantiene el cursor dentro del buffer.
    fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.line = self.cursor.line.min(self.lines.len() - 1);
        self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.line));
    }

    /// Selección ordenada `(inicio, fin)`, o `None` si no hay.
    fn selection(&self) -> Option<(Cursor, Cursor)> {
        let a = self.anchor?;
        let b = self.cursor;
        if a == b {
            return None;
        }
        Some(if (a.line, a.col) <= (b.line, b.col) {
            (a, b)
        } else {
            (b, a)
        })
    }

    fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// Rango de columnas seleccionado **en una línea**, ya recortado.
    fn sel_on_line(&self, line: usize) -> Option<(usize, usize)> {
        let (a, b) = self.selection()?;
        if line < a.line || line > b.line {
            return None;
        }
        let len = self.line_len(line);
        let start = if line == a.line { a.col } else { 0 };
        let end = if line == b.line { b.col } else { len };
        (start < end).then_some((start, end))
    }

    /// Texto seleccionado, con `\n` entre líneas.
    fn selected_text(&self) -> String {
        let Some((a, b)) = self.selection() else {
            return String::new();
        };
        if a.line == b.line {
            let chars: Vec<char> = self.lines[a.line].chars().collect();
            return chars[a.col..b.col].iter().collect();
        }
        let mut out = String::new();
        let first: Vec<char> = self.lines[a.line].chars().collect();
        out.extend(&first[a.col..]);
        for line in a.line + 1..b.line {
            out.push('\n');
            out.push_str(&self.lines[line]);
        }
        out.push('\n');
        let last: Vec<char> = self.lines[b.line].chars().collect();
        out.extend(&last[..b.col]);
        out
    }

    /// Borra la selección. Devuelve `true` si abarcaba varias líneas (cambió la
    /// cantidad de líneas y hace falta reconstruir el modelo).
    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let crossed = a.line != b.line;
        if !crossed {
            let chars: Vec<char> = self.lines[a.line].chars().collect();
            let head: String = chars[..a.col].iter().collect();
            let tail: String = chars[b.col..].iter().collect();
            self.lines[a.line] = format!("{head}{tail}");
        } else {
            let first: Vec<char> = self.lines[a.line].chars().collect();
            let head: String = first[..a.col].iter().collect();
            let last: Vec<char> = self.lines[b.line].chars().collect();
            let tail: String = last[b.col..].iter().collect();
            self.lines[a.line] = format!("{head}{tail}");
            self.lines.drain(a.line + 1..=b.line);
        }
        self.cursor = a;
        self.anchor = None;
        crossed
    }
}

/// Una sesión de terminal real: una shell en un PTY con su pantalla.
struct LocalSession {
    id: u64,
    name: String,
    pty: Option<Pty>,
    screen: Screen,
    /// `true` si la shell es PowerShell (cambia cómo se cotizan los comandos).
    powershell: bool,
    /// El proceso sigue vivo.
    alive: bool,
}

/// Estado completo de la IDE.
pub struct Ide {
    toolchain: Toolchain,
    /// Terminal real: una shell por sesión.
    local: Vec<LocalSession>,
    local_active: usize,
    next_local_id: u64,
    /// Tamaño actual del panel de terminal (columnas × filas).
    local_cols: u16,
    local_rows: u16,
    local_tx: Sender<PtyEvent>,
    local_rx: Receiver<PtyEvent>,
    /// Árbol de archivos.
    explorer: Explorer,
    /// Índice seleccionado en el explorador (-1 = nada).
    explorer_selected: i32,
    /// Archivos abiertos en el editor.
    tabs: Vec<OpenTab>,
    active_tab: usize,
    /// Modelo de líneas del editor. Se muta por fila para no reconstruir todo en
    /// cada tecla; se rehace entero al abrir/cambiar de pestaña o cambiar el
    /// número de líneas.
    code_model: Rc<VecModel<IdeCodeLine>>,
    /// El modelo ya se entregó a la ventana (a partir de ahí las mutaciones se
    /// ven solas).
    model_attached: bool,
    /// Hay que reconstruir el modelo entero en el próximo `sync`.
    editor_full: bool,
    /// Pila de deshacer del archivo activo.
    undo: Vec<(Vec<String>, Cursor)>,
    /// Pila de rehacer. Se vacía con cada edición nueva.
    redo: Vec<(Vec<String>, Cursor)>,
    /// Consola dedicada al agente (OpenCode CLI). Canal propio para no mezclar
    /// sesiones con la terminal real.
    agent: Terminal,
    agent_input: String,
    agent_tx: Sender<Msg>,
    agent_rx: Receiver<Msg>,
    /// Encabezado del panel de agente.
    agent_version: String,
    agent_model: String,
    /// `.spasm` sobre el que actúan los menús Ejecutar/Compilar.
    active_file: Option<PathBuf>,
    /// Timer del bucle de drenado.
    timer: Option<Timer>,
}

impl Ide {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (agent_tx, agent_rx) = mpsc::channel();
        let mut agent = Terminal::new(cwd.clone());
        agent.sessions[0].name = "opencode".to_string();
        let (local_tx, local_rx) = mpsc::channel();

        let mut ide = Self {
            toolchain: Toolchain::discover(),
            local: Vec::new(),
            local_active: 0,
            next_local_id: 0,
            local_cols: TERM_COLS,
            local_rows: TERM_ROWS,
            local_tx,
            local_rx,
            explorer: Explorer::new(cwd),
            explorer_selected: -1,
            tabs: Vec::new(),
            active_tab: 0,
            code_model: Rc::new(VecModel::default()),
            model_attached: false,
            editor_full: false,
            undo: Vec::new(),
            redo: Vec::new(),
            agent,
            agent_input: String::new(),
            agent_tx,
            agent_rx,
            agent_version: std::env::var("NUIDE_AGENT_VERSION")
                .unwrap_or_else(|_| "OpenCode CLI".to_string()),
            agent_model: std::env::var("NUIDE_AGENT_MODEL")
                .unwrap_or_else(|_| "por defecto".to_string()),
            active_file: None,
            timer: None,
        };
        ide.new_local_session();
        ide
    }

    /// Estado listo para compartir con los callbacks de Slint, con el banner de
    /// arranque ya escrito.
    pub fn shared() -> Rc<RefCell<Self>> {
        let ide = Rc::new(RefCell::new(Self::new()));
        ide.borrow_mut().banner();
        ide
    }
}

// ---------------------------------------------------------------------------
// Arranque
// ---------------------------------------------------------------------------

impl Ide {
    /// Banner del panel del agente. La terminal real no necesita banner: la
    /// shell imprime el suyo.
    fn banner(&mut self) {
        match &self.toolchain.agent {
            Some(p) => {
                self.agent.push_active(Line {
                    text: format!("OpenCode CLI: {}", p.display()),
                    kind: 0,
                });
                self.agent.push_active(Line {
                    text:
                        "escribí un prompt (se corre `opencode run`), o '$ <comando>' para el shell"
                            .into(),
                    kind: 1,
                });
                self.spawn_in_agent(p.clone(), vec!["--version".to_string()]);
            }
            None => self.agent.push_active(Line {
                text: "CLI de OpenCode no encontrado (definí NUIDE_AGENT)".into(),
                kind: 5,
            }),
        }
    }

    /// Abre una shell nueva en un PTY y la deja activa.
    fn new_local_session(&mut self) {
        let id = self.next_local_id;
        self.next_local_id += 1;
        let n = self.local.len() + 1;
        let (shell, args) = pty::default_shell();
        let powershell = pty::is_powershell(&shell);
        let name = if powershell {
            format!("pwsh {n}")
        } else {
            format!("shell {n}")
        };
        let cwd = self.explorer.root().to_path_buf();
        let mut screen = Screen::new(self.local_rows, self.local_cols);
        let pty = match Pty::spawn(
            id,
            &shell,
            &args,
            &cwd,
            self.local_cols,
            self.local_rows,
            self.local_tx.clone(),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                screen.process(format!("no se pudo abrir la shell ({shell}): {e}\r\n").as_bytes());
                None
            }
        };
        self.local.push(LocalSession {
            id,
            name,
            pty,
            screen,
            powershell,
            alive: true,
        });
        self.local_active = self.local.len() - 1;
    }
}

// ---------------------------------------------------------------------------
// Entrada y comandos
// ---------------------------------------------------------------------------

impl Ide {
    /// Teclas de la terminal real: se traducen a bytes y van al PTY activo.
    fn local_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) {
        let Some(bytes) = encode_key(text, control, shift, alt) else {
            return;
        };
        self.write_to_local(&bytes);
    }

    /// Escribe bytes en el PTY de la sesión activa.
    fn write_to_local(&mut self, bytes: &[u8]) {
        if let Some(session) = self.local.get_mut(self.local_active) {
            if let Some(pty) = session.pty.as_mut() {
                let _ = pty.write(bytes);
            }
        }
    }

    /// Escribe una línea de comando en la shell activa (menú Ejecutar/Compilar).
    fn run_in_local(&mut self, program: &Path, args: &[String]) {
        let Some(session) = self.local.get(self.local_active) else {
            return;
        };
        let line = shell_line(session.powershell, program, args);
        let mut bytes = line.into_bytes();
        bytes.push(b'\r');
        self.write_to_local(&bytes);
    }

    /// Tecla cruda del panel del agente.
    fn agent_key(&mut self, text: &str, control: bool) {
        if control {
            match text.to_ascii_lowercase().as_str() {
                "c" | "u" => self.agent_input.clear(),
                "l" => self.agent.clear_active(),
                _ => {}
            }
            return;
        }
        if apply_text_key(&mut self.agent_input, text) {
            self.agent_submit();
        }
    }

    /// Envía la línea de entrada del agente.
    fn agent_submit(&mut self) {
        let raw = std::mem::take(&mut self.agent_input);
        self.agent.push_active(Line {
            text: format!("{AGENT_PROMPT}{raw}"),
            kind: 4,
        });
        let msg = raw.trim().to_string();
        if msg.is_empty() {
            return;
        }

        // `$ <comando>` corre en el shell, en la carpeta del proyecto. El resto
        // se le pasa al agente como `opencode run <mensaje>`.
        if let Some(cmd) = msg.strip_prefix('$') {
            let (program, args) = shell_command(cmd.trim());
            self.spawn_in_agent(program, args);
            return;
        }
        match self.toolchain.agent_command(&msg) {
            Ok((program, args)) => self.spawn_in_agent(program, args),
            Err(e) => self.agent.push_active(Line { text: e, kind: 5 }),
        }
    }

    /// Lanza un proceso en el panel del agente, en la raíz del proyecto.
    fn spawn_in_agent(&mut self, program: PathBuf, args: Vec<String>) {
        let cwd = self.explorer.root().to_path_buf();
        let display = program.display().to_string();
        self.agent.sessions[0].running = true;
        let tx = self.agent_tx.clone();
        if let Err(e) = terminal::spawn_streaming(0, &cwd, &display, &args, tx) {
            self.agent.push(
                0,
                Line {
                    text: format!("no se pudo ejecutar {display}: {e}"),
                    kind: 3,
                },
            );
            self.agent.sessions[0].running = false;
        }
    }

    /// Ejecuta el archivo activo con el intérprete de SpASM: escribe la orden en
    /// la shell activa, así la salida queda en la terminal real.
    fn run(&mut self, _arg: Option<&str>) {
        let Some(file) = self.active_file.clone() else {
            return;
        };
        let Ok(exe) = self.toolchain.interpreter_required() else {
            return;
        };
        self.open_file(file.clone());
        self.run_in_local(&exe, &[file.display().to_string()]);
    }

    /// Transpila el archivo activo a C con `spasmc.py` (y lo construye si hay
    /// compilador de C), escribiendo la orden en la shell activa.
    fn build(&mut self, _arg: &str) {
        let Some(file) = self.active_file.clone() else {
            return;
        };
        self.open_file(file.clone());
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string());
        let out_dir = self.explorer.root().join("build");
        let _ = std::fs::create_dir_all(&out_dir);
        let out_c = out_dir.join(format!("{stem}.c"));
        if let Ok((program, args, _build)) = self.toolchain.compile_command(&file, &out_c) {
            self.run_in_local(&program, &args);
        }
    }

    /// Acciones del menú superior.
    fn menu(&mut self, label: &str) {
        match label {
            "Ejecutar" => self.run(None),
            "Compilar" => self.build(""),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Sesiones
// ---------------------------------------------------------------------------

impl Ide {
    fn select_session(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.local.len() {
            self.local_active = i as usize;
            self.resize_local();
        }
    }

    fn new_session(&mut self) {
        self.new_local_session();
        self.resize_local();
    }

    fn close_session(&mut self) {
        if self.local.len() <= 1 {
            return;
        }
        self.local.remove(self.local_active);
        if self.local_active >= self.local.len() {
            self.local_active = self.local.len() - 1;
        }
        self.resize_local();
    }

    fn split_terminal(&mut self) {
        // Dividir la vista no está implementado; abrir otra sesión es lo útil.
        self.new_local_session();
        self.resize_local();
    }

    /// Avisa a la sesión activa el tamaño actual del panel.
    fn resize_local(&mut self) {
        let (cols, rows) = (self.local_cols, self.local_rows);
        if let Some(session) = self.local.get_mut(self.local_active) {
            session.screen.resize(rows, cols);
            if let Some(pty) = session.pty.as_ref() {
                let _ = pty.resize(cols, rows);
            }
        }
    }

    /// La UI informa el tamaño del área de pantalla de la terminal (en píxeles,
    /// sin el encabezado).
    fn local_resize(&mut self, w: f32, h: f32) {
        let cols = ((w / TERM_CELL_W).floor() as i64).clamp(20, 500) as u16;
        let rows = ((h / TERM_CELL_H).floor() as i64).clamp(2, 200) as u16;
        if cols != self.local_cols || rows != self.local_rows {
            self.local_cols = cols;
            self.local_rows = rows;
            self.resize_local();
        }
    }
}

// ---------------------------------------------------------------------------
// Explorador y editor
// ---------------------------------------------------------------------------

impl Ide {
    /// Activa una fila del explorador: una carpeta abre/cierra, un archivo se
    /// abre en el editor.
    fn node_activated(&mut self, i: i32) {
        if i < 0 {
            return;
        }
        self.explorer_selected = i;
        if let Some(Action::File(path)) = self.explorer.toggle(i as usize) {
            self.open_file(path);
        }
    }

    /// Abre un archivo en una pestaña (o activa la que ya estaba) y lo marca
    /// como el archivo de los menús.
    fn open_file(&mut self, path: PathBuf) {
        if !path.is_file() {
            return;
        }
        self.active_file = Some(path.clone());
        if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
            self.active_tab = i;
            self.editor_full = true;
            self.undo.clear();
            return;
        }
        let (lines, trailing_newline, read_only) = read_lines(&path);
        self.tabs.push(OpenTab {
            path: path.clone(),
            lines,
            cursor: Cursor { line: 0, col: 0 },
            anchor: None,
            dirty: false,
            trailing_newline,
            read_only,
        });
        self.active_tab = self.tabs.len() - 1;
        self.editor_full = true;
        self.undo.clear();
    }

    fn tab_activated(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.tabs.len() {
            self.active_tab = i as usize;
            self.active_file = Some(self.tabs[self.active_tab].path.clone());
            self.editor_full = true;
            self.undo.clear();
        }
    }

    fn tab_closed(&mut self, i: i32) {
        if i < 0 || i as usize >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i as usize);
        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else if (i as usize) < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.active_file = self.tabs.get(self.active_tab).map(|t| t.path.clone());
        self.editor_full = true;
        self.undo.clear();
    }

    /// Migas de pan del archivo, relativas a la raíz del explorador.
    fn breadcrumb_for(&self, path: &Path) -> Vec<slint::SharedString> {
        let root = self.explorer.root();
        let mut out: Vec<slint::SharedString> = Vec::new();
        if let Some(name) = root.file_name() {
            out.push(name.to_string_lossy().into_owned().into());
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        for c in rel.components() {
            out.push(c.as_os_str().to_string_lossy().into_owned().into());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Edición de texto
// ---------------------------------------------------------------------------

impl Ide {
    /// Tecla cruda del editor. Igual que en la terminal, el texto llega como
    /// caracteres: Backspace es `\u{8}`, Enter `\n` y las flechas son caracteres
    /// de uso privado (`\u{F700}`–`\u{F703}`).
    fn editor_key(&mut self, text: &str, control: bool, shift: bool) {
        if self.tabs.is_empty() {
            return;
        }
        if control {
            match text.to_ascii_lowercase().as_str() {
                "s" => self.save_active(),
                "z" if shift => self.redo_active(),
                "z" => self.undo_active(),
                "y" => self.redo_active(),
                "a" => self.select_all(),
                "c" => self.copy_active(),
                "x" => self.cut_active(),
                "v" => self.paste_active(),
                _ => {}
            }
            return;
        }
        if self.tabs[self.active_tab].read_only {
            // En un binario o un archivo ilegible se permite moverse, no editar.
            for ch in text.chars() {
                self.move_for_key(ch, shift);
            }
            return;
        }
        for ch in text.chars() {
            match ch {
                '\u{8}' => self.backspace(),
                '\u{7f}' => self.delete_forward(),
                '\n' | '\r' => self.newline(),
                '\t' => self.insert_str("    "),
                '\u{F700}' | '\u{F701}' | '\u{F702}' | '\u{F703}' | '\u{F729}' | '\u{F72b}'
                | '\u{F72c}' | '\u{F72d}' => self.move_for_key(ch, shift),
                '\u{1b}' => {}
                c if c >= ' ' && c < '\u{E000}' => self.insert_char(c),
                _ => {}
            }
        }
    }

    /// Clic en el área de código: coloca el cursor en la posición más cercana.
    fn editor_cursor(&mut self, y: f32, x: f32, scroll: f32) {
        if self.tabs.is_empty() {
            return;
        }
        let old = self.tabs[self.active_tab].cursor;
        let new = point_to_cursor(&self.tabs[self.active_tab], y, x, scroll);
        self.set_cursor(new, old.line, false);
    }

    /// Arrastre sobre el código: extiende la selección desde donde empezó.
    fn editor_extend(&mut self, y: f32, x: f32, scroll: f32) {
        if self.tabs.is_empty() {
            return;
        }
        self.begin_selection();
        let old = self.tabs[self.active_tab].cursor;
        let new = point_to_cursor(&self.tabs[self.active_tab], y, x, scroll);
        self.set_cursor(new, old.line, true);
    }

    /// Guarda el archivo activo.
    fn save_active(&mut self) {
        let (path, data, read_only) = {
            let t = &self.tabs[self.active_tab];
            let mut data = t.lines.join("\n");
            if t.trailing_newline {
                data.push('\n');
            }
            (t.path.clone(), data, t.read_only)
        };
        if read_only {
            return;
        }
        if std::fs::write(&path, data).is_ok() {
            self.tabs[self.active_tab].dirty = false;
        }
    }

    fn undo_active(&mut self) {
        match self.undo.pop() {
            Some((lines, cursor)) => {
                let current = self.tabs[self.active_tab].snapshot();
                self.redo.push(current);
                self.apply_snapshot(lines, cursor);
            }
            None => {}
        }
    }

    fn redo_active(&mut self) {
        match self.redo.pop() {
            Some((lines, cursor)) => {
                let current = self.tabs[self.active_tab].snapshot();
                self.undo.push(current);
                self.apply_snapshot(lines, cursor);
            }
            None => {}
        }
    }

    fn apply_snapshot(&mut self, lines: Vec<String>, cursor: Cursor) {
        let t = &mut self.tabs[self.active_tab];
        t.lines = lines;
        t.cursor = cursor;
        t.anchor = None;
        t.dirty = true;
        t.clamp_cursor();
        self.editor_full = true;
    }

    /// Guarda el estado actual para poder deshacer. Una edición nueva borra el
    /// rehacer.
    fn push_undo(&mut self) {
        let snap = self.tabs[self.active_tab].snapshot();
        self.undo.push(snap);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Selecciona todo el buffer.
    fn select_all(&mut self) {
        let last = self.tabs[self.active_tab].lines.len().saturating_sub(1);
        let col = self.tabs[self.active_tab].line_len(last);
        self.tabs[self.active_tab].anchor = Some(Cursor { line: 0, col: 0 });
        self.tabs[self.active_tab].cursor = Cursor { line: last, col };
        self.editor_full = true;
    }

    /// Copia la selección; sin selección, copia la línea actual.
    fn copy_active(&mut self) {
        let text = {
            let t = &self.tabs[self.active_tab];
            if t.has_selection() {
                t.selected_text()
            } else {
                format!("{}\n", t.lines[t.cursor.line])
            }
        };
        self.write_clipboard(&text);
    }

    /// Corta la selección; sin selección, corta la línea actual.
    fn cut_active(&mut self) {
        let (text, whole_line) = {
            let t = &self.tabs[self.active_tab];
            if t.has_selection() {
                (t.selected_text(), false)
            } else {
                (format!("{}\n", t.lines[t.cursor.line]), true)
            }
        };
        if !self.write_clipboard(&text) {
            return;
        }
        self.push_undo();
        if whole_line {
            let line = self.tabs[self.active_tab].cursor.line;
            let t = &mut self.tabs[self.active_tab];
            if t.lines.len() == 1 {
                t.lines[0].clear();
            } else {
                t.lines.remove(line);
            }
            t.anchor = None;
            t.clamp_cursor();
            t.dirty = true;
            self.editor_full = true;
        } else {
            let crossed = self.tabs[self.active_tab].delete_selection();
            self.tabs[self.active_tab].dirty = true;
            if crossed {
                self.editor_full = true;
            } else {
                let line = self.tabs[self.active_tab].cursor.line;
                self.update_row(line);
            }
        }
    }

    /// Pega el portapapeles en el cursor, partiendo líneas si hace falta.
    fn paste_active(&mut self) {
        let Some(text) = read_clipboard() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.push_undo();
        self.tabs[self.active_tab].delete_selection();
        let (line, col) = {
            let t = &self.tabs[self.active_tab];
            (t.cursor.line, t.cursor.col)
        };
        let parts: Vec<&str> = text.split('\n').collect();
        {
            let t = &mut self.tabs[self.active_tab];
            let chars: Vec<char> = t.lines[line].chars().collect();
            let head: String = chars[..col].iter().collect();
            let tail: String = chars[col..].iter().collect();
            if parts.len() == 1 {
                t.lines[line] = format!("{head}{}{tail}", parts[0]);
                t.cursor = Cursor {
                    line,
                    col: col + parts[0].chars().count(),
                };
            } else {
                t.lines[line] = format!("{head}{}", parts[0]);
                for (i, mid) in parts[1..parts.len() - 1].iter().enumerate() {
                    t.lines.insert(line + 1 + i, (*mid).to_string());
                }
                let last = parts[parts.len() - 1];
                let last_len = last.chars().count();
                let at = line + parts.len() - 1;
                t.lines.insert(at, format!("{last}{tail}"));
                t.cursor = Cursor {
                    line: at,
                    col: last_len,
                };
            }
            t.anchor = None;
            t.dirty = true;
        }
        self.editor_full = true;
    }

    /// Escribe el portapapeles del sistema.
    fn write_clipboard(&mut self, text: &str) -> bool {
        clipboard_write(text).is_ok()
    }

    /// Inserta un carácter en la posición del cursor.
    fn insert_char(&mut self, ch: char) {
        self.insert_str(&ch.to_string());
    }

    /// Inserta un fragmento (sin saltos) en la posición del cursor. Si hay
    /// selección, la reemplaza.
    fn insert_str(&mut self, s: &str) {
        self.push_undo();
        let crossed = self.tabs[self.active_tab].delete_selection();
        let line = self.tabs[self.active_tab].cursor.line;
        {
            let t = &mut self.tabs[self.active_tab];
            let col = t.cursor.col;
            let mut chars: Vec<char> = t.lines[line].chars().collect();
            for (i, ch) in s.chars().enumerate() {
                chars.insert(col + i, ch);
            }
            t.lines[line] = chars.into_iter().collect();
            t.cursor.col = col + s.chars().count();
            t.anchor = None;
            t.dirty = true;
        }
        if crossed {
            self.editor_full = true;
        } else {
            self.update_row(line);
        }
    }

    fn backspace(&mut self) {
        if self.tabs[self.active_tab].has_selection() {
            self.push_undo();
            let crossed = self.tabs[self.active_tab].delete_selection();
            self.tabs[self.active_tab].dirty = true;
            if crossed {
                self.editor_full = true;
            } else {
                let line = self.tabs[self.active_tab].cursor.line;
                self.update_row(line);
            }
            return;
        }
        let (line, col) = {
            let t = &self.tabs[self.active_tab];
            (t.cursor.line, t.cursor.col)
        };
        if col > 0 {
            self.push_undo();
            let t = &mut self.tabs[self.active_tab];
            let mut chars: Vec<char> = t.lines[line].chars().collect();
            chars.remove(col - 1);
            t.lines[line] = chars.into_iter().collect();
            t.cursor.col = col - 1;
            t.dirty = true;
            self.update_row(line);
        } else if line > 0 {
            self.push_undo();
            let t = &mut self.tabs[self.active_tab];
            let tail = t.lines.remove(line);
            let prev_len = t.lines[line - 1].chars().count();
            t.lines[line - 1].push_str(&tail);
            t.cursor = Cursor {
                line: line - 1,
                col: prev_len,
            };
            t.dirty = true;
            self.editor_full = true;
        }
    }

    fn delete_forward(&mut self) {
        if self.tabs[self.active_tab].has_selection() {
            self.push_undo();
            let crossed = self.tabs[self.active_tab].delete_selection();
            self.tabs[self.active_tab].dirty = true;
            if crossed {
                self.editor_full = true;
            } else {
                let line = self.tabs[self.active_tab].cursor.line;
                self.update_row(line);
            }
            return;
        }
        let (line, col, len) = {
            let t = &self.tabs[self.active_tab];
            (t.cursor.line, t.cursor.col, t.line_len(t.cursor.line))
        };
        if col < len {
            self.push_undo();
            let t = &mut self.tabs[self.active_tab];
            let mut chars: Vec<char> = t.lines[line].chars().collect();
            chars.remove(col);
            t.lines[line] = chars.into_iter().collect();
            t.dirty = true;
            self.update_row(line);
        } else if line + 1 < self.tabs[self.active_tab].lines.len() {
            self.push_undo();
            let t = &mut self.tabs[self.active_tab];
            let next = t.lines.remove(line + 1);
            t.lines[line].push_str(&next);
            t.dirty = true;
            self.editor_full = true;
        }
    }

    fn newline(&mut self) {
        self.push_undo();
        self.tabs[self.active_tab].delete_selection();
        let line = self.tabs[self.active_tab].cursor.line;
        let col = self.tabs[self.active_tab].cursor.col;
        {
            let t = &mut self.tabs[self.active_tab];
            let chars: Vec<char> = t.lines[line].chars().collect();
            let head: String = chars[..col].iter().collect();
            let tail: String = chars[col..].iter().collect();
            // Auto-indentación: se copian los espacios/tabs iniciales.
            let indent: String = head
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            t.lines[line] = head;
            t.lines.insert(line + 1, format!("{indent}{tail}"));
            t.cursor = Cursor {
                line: line + 1,
                col: indent.chars().count(),
            };
            t.anchor = None;
            t.dirty = true;
        }
        self.editor_full = true;
    }

    /// Si no hay ancla, la fija en el cursor: empieza una selección.
    fn begin_selection(&mut self) {
        let t = &mut self.tabs[self.active_tab];
        if t.anchor.is_none() {
            t.anchor = Some(t.cursor);
        }
    }

    fn move_for_key(&mut self, key: char, select: bool) {
        if select {
            self.begin_selection();
        }
        match key {
            '\u{F702}' => self.move_left(select),
            '\u{F703}' => self.move_right(select),
            '\u{F700}' => self.move_up(select),
            '\u{F701}' => self.move_down(select),
            '\u{F729}' => self.move_home(select),
            '\u{F72b}' => self.move_end(select),
            '\u{F72c}' => self.move_lines(-10, select),
            '\u{F72d}' => self.move_lines(10, select),
            _ => {}
        }
    }

    fn move_left(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let new = if old.col > 0 {
            Cursor {
                line: old.line,
                col: old.col - 1,
            }
        } else if old.line > 0 {
            let prev = self.tabs[self.active_tab].line_len(old.line - 1);
            Cursor {
                line: old.line - 1,
                col: prev,
            }
        } else {
            old
        };
        self.set_cursor(new, old.line, select);
    }

    fn move_right(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let len = self.tabs[self.active_tab].line_len(old.line);
        let new = if old.col < len {
            Cursor {
                line: old.line,
                col: old.col + 1,
            }
        } else if old.line + 1 < self.tabs[self.active_tab].lines.len() {
            Cursor {
                line: old.line + 1,
                col: 0,
            }
        } else {
            old
        };
        self.set_cursor(new, old.line, select);
    }

    fn move_up(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let new = if old.line > 0 {
            let col = old
                .col
                .min(self.tabs[self.active_tab].line_len(old.line - 1));
            Cursor {
                line: old.line - 1,
                col,
            }
        } else {
            old
        };
        self.set_cursor(new, old.line, select);
    }

    fn move_down(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let new = if old.line + 1 < self.tabs[self.active_tab].lines.len() {
            let col = old
                .col
                .min(self.tabs[self.active_tab].line_len(old.line + 1));
            Cursor {
                line: old.line + 1,
                col,
            }
        } else {
            old
        };
        self.set_cursor(new, old.line, select);
    }

    fn move_home(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        self.set_cursor(
            Cursor {
                line: old.line,
                col: 0,
            },
            old.line,
            select,
        );
    }

    fn move_end(&mut self, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let col = self.tabs[self.active_tab].line_len(old.line);
        self.set_cursor(
            Cursor {
                line: old.line,
                col,
            },
            old.line,
            select,
        );
    }

    /// Mueve el cursor `delta` líneas (PageUp/PageDown).
    fn move_lines(&mut self, delta: i32, select: bool) {
        let old = self.tabs[self.active_tab].cursor;
        let last = self.tabs[self.active_tab].lines.len().saturating_sub(1);
        let target = (old.line as i32 + delta).clamp(0, last as i32) as usize;
        let col = old.col.min(self.tabs[self.active_tab].line_len(target));
        self.set_cursor(Cursor { line: target, col }, old.line, select);
    }

    /// Mueve el cursor. Si `keep_anchor` es verdadero, conserva el ancla (modo
    /// selección); si había una selección y se cancela, redibuja todo.
    fn set_cursor(&mut self, new: Cursor, old_line: usize, keep_anchor: bool) {
        let had_sel = self.tabs[self.active_tab].anchor.is_some();
        {
            let t = &mut self.tabs[self.active_tab];
            if !keep_anchor {
                t.anchor = None;
            }
            t.cursor = new;
        }
        if keep_anchor || had_sel {
            self.editor_full = true;
        } else {
            self.refresh_cursor(old_line, new.line);
        }
    }

    /// Rehace las filas afectadas por un movimiento de cursor.
    fn refresh_cursor(&self, old_line: usize, new_line: usize) {
        self.update_row(old_line);
        if new_line != old_line {
            self.update_row(new_line);
        }
    }
}

// ---------------------------------------------------------------------------
// Render del editor
// ---------------------------------------------------------------------------

impl Ide {
    /// Reconstruye el modelo entero y lo entrega a la ventana.
    fn render_editor(&mut self, w: &NuIdeWindow) {
        let rows: Vec<IdeCodeLine> = match self.tabs.get(self.active_tab) {
            Some(t) => (0..t.lines.len()).map(|i| row_of(t, i)).collect(),
            None => Vec::new(),
        };
        self.code_model.set_vec(rows);
        w.set_code_lines(ModelRc::from(self.code_model.clone()));
        self.model_attached = true;
    }

    /// Actualiza sólo la fila `line` (una edición o un movimiento de cursor).
    fn update_row(&self, line: usize) {
        if !self.model_attached {
            return;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if line < tab.lines.len() && line < self.code_model.row_count() {
            self.code_model.set_row_data(line, row_of(tab, line));
        }
    }
}

// ---------------------------------------------------------------------------
// Sincronización con la ventana
// ---------------------------------------------------------------------------

impl Ide {
    /// Vuelca el estado a la ventana. Idempotente.
    fn sync(&mut self, w: &NuIdeWindow) {
        // --- terminal real ---
        let sessions: Vec<slint::SharedString> =
            self.local.iter().map(|s| s.name.clone().into()).collect();
        w.set_term_sessions(ModelRc::from(Rc::new(VecModel::from(sessions))));
        w.set_active_session(self.local_active as i32);

        let term_lines: Vec<TermLine> = match self.local.get(self.local_active) {
            Some(session) => {
                let (rows, _cursor) = session.screen.snapshot();
                rows.into_iter()
                    .map(|runs| TermLine {
                        runs: ModelRc::from(Rc::new(VecModel::from(
                            runs.into_iter()
                                .map(|run| TermRun {
                                    text: run.text.into(),
                                    fg: to_color(run.fg),
                                    bg: to_color(run.bg),
                                    bold: run.bold,
                                })
                                .collect::<Vec<_>>(),
                        ))),
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        w.set_term_lines(ModelRc::from(Rc::new(VecModel::from(term_lines))));

        // --- panel del agente ---
        let agent_lines: Vec<IdeTermLine> = self
            .agent
            .active()
            .lines
            .iter()
            .map(|l| IdeTermLine {
                text: l.text.clone().into(),
                kind: l.kind,
            })
            .collect();
        w.set_agent_lines(ModelRc::from(Rc::new(VecModel::from(agent_lines))));
        w.set_agent_input(self.agent_input.clone().into());
        w.set_agent_prompt(AGENT_PROMPT.into());
        w.set_agent_connected(self.toolchain.agent.is_some());
        w.set_agent_version(self.agent_version.clone().into());
        w.set_agent_model(self.agent_model.clone().into());

        // --- proyecto y explorador ---
        let project = self
            .explorer
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.explorer.root().display().to_string());
        w.set_project_name(project.into());

        let nodes: Vec<IdeTreeNode> = self
            .explorer
            .nodes()
            .iter()
            .map(|n| IdeTreeNode {
                depth: n.depth,
                label: n.label.clone().into(),
                kind: n.kind,
            })
            .collect();
        w.set_tree_nodes(ModelRc::from(Rc::new(VecModel::from(nodes))));
        w.set_explorer_selected(self.explorer_selected);

        // --- editor ---
        // Sin archivos abiertos (y sin haber abierto nunca) se deja el contenido
        // demo del `.slint` como pantalla de bienvenida. Con la primera pestaña,
        // el editor pasa a ser real de forma definitiva.
        if self.model_attached || !self.tabs.is_empty() {
            let tabs: Vec<IdeTab> = self
                .tabs
                .iter()
                .map(|t| IdeTab {
                    title: file_name(&t.path).into(),
                    dirty: t.dirty,
                })
                .collect();
            w.set_editor_tabs(ModelRc::from(Rc::new(VecModel::from(tabs))));
            w.set_active_tab(self.active_tab as i32);

            let crumbs = match self.tabs.get(self.active_tab) {
                Some(t) => self.breadcrumb_for(&t.path),
                None => Vec::new(),
            };
            w.set_breadcrumb(ModelRc::from(Rc::new(VecModel::from(crumbs))));

            let (line, col) = self
                .tabs
                .get(self.active_tab)
                .map(|t| (t.cursor.line as i32 + 1, t.cursor.col as i32 + 1))
                .unwrap_or((1, 1));
            w.set_active_line(line);
            w.set_active_column(col);

            // Las mutaciones por fila ya llegaron a la UI; sólo hace falta
            // reconstruir cuando cambió la estructura (abrir, cambiar de
            // pestaña, número de líneas).
            if self.editor_full {
                self.render_editor(w);
                self.editor_full = false;
            }
        }
    }

    /// Drena los mensajes pendientes de los hilos. Se llama desde el timer.
    fn drain(&mut self, w: &NuIdeWindow) {
        let mut got = false;
        while let Ok(event) = self.local_rx.try_recv() {
            self.apply_pty_event(event);
            got = true;
        }
        while let Ok(msg) = self.agent_rx.try_recv() {
            self.agent.apply(msg);
            got = true;
        }
        if got {
            self.sync(w);
        }
    }

    /// Procesa un evento del PTY: alimenta el emulador y contesta el pedido de
    /// posición del cursor (ConPTY lo necesita para poder renderizar).
    fn apply_pty_event(&mut self, event: PtyEvent) {
        match event {
            PtyEvent::Output { session, bytes } => {
                let Some(s) = self.local.iter_mut().find(|s| s.id == session) else {
                    return;
                };
                if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                    let (row, col) = s.screen.cursor().unwrap_or((0, 0));
                    let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                    if let Some(pty) = s.pty.as_mut() {
                        let _ = pty.write(reply.as_bytes());
                    }
                }
                s.screen.process(&bytes);
            }
            PtyEvent::Exit { session, code } => {
                if let Some(s) = self.local.iter_mut().find(|s| s.id == session) {
                    s.alive = false;
                    s.pty = None;
                    s.screen
                        .process(format!("\r\n[proceso terminado: {code}]\r\n").as_bytes());
                }
            }
        }
    }
}

/// Lee un archivo a líneas de texto. Devuelve también si terminaba en salto de
/// línea y si es de sólo lectura (binario o ilegible).
fn read_lines(path: &Path) -> (Vec<String>, bool, bool) {
    match std::fs::read(path) {
        Ok(bytes) if bytes.contains(&0) => (vec!["[archivo binario]".to_string()], false, true),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let trailing_newline = text.ends_with('\n');
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            if lines.is_empty() {
                lines.push(String::new());
            }
            lines.truncate(MAX_OPEN_LINES);
            (lines, trailing_newline, false)
        }
        Err(e) => (vec![format!("no se pudo leer: {e}")], false, true),
    }
}

/// Traduce un punto de la vista (y, x y desplazamiento del scroll) a una
/// posición del buffer. La y crece hacia abajo y el scroll es negativo.
fn point_to_cursor(tab: &OpenTab, y: f32, x: f32, scroll: f32) -> Cursor {
    let line_y = (y - scroll - CODE_TOP).max(0.0);
    let line = ((line_y / ROW_H).floor() as usize).min(tab.lines.len().saturating_sub(1));
    let col = (((x - CODE_X0).max(0.0)) / CHAR_W).round() as usize;
    Cursor {
        line,
        col: col.min(tab.line_len(line)),
    }
}

/// Arma la fila de una línea, con el cursor y la selección que le toquen.
fn row_of(tab: &OpenTab, line: usize) -> IdeCodeLine {
    let cursor = (line == tab.cursor.line).then_some(tab.cursor.col);
    let sel = tab.sel_on_line(line);
    let toks = render_tokens(&tab.lines[line], sel, cursor);
    IdeCodeLine {
        indent: 0.0,
        tokens: ModelRc::from(Rc::new(VecModel::from(
            toks.into_iter()
                .map(|(text, kind)| IdeToken {
                    text: text.into(),
                    kind,
                })
                .collect::<Vec<_>>(),
        ))),
    }
}

/// Tokeniza una línea y le aplica la selección (kind 7) y el cursor (kind 6).
///
/// Trabaja sobre el texto ya troceado por `highlight`, marca la selección por
/// carácter, y recién después agrupa tramos consecutivos del mismo kind. Así el
/// caret es un token más y no hay que calcular píxeles.
fn render_tokens(
    line: &str,
    sel: Option<(usize, usize)>,
    cursor: Option<usize>,
) -> Vec<(String, i32)> {
    // Atributo por carácter según el resaltado.
    let mut chars: Vec<(char, i32)> = Vec::new();
    for (text, kind) in highlight::tokenize_line(line) {
        for c in text.chars() {
            chars.push((c, kind));
        }
    }

    if let Some((a, b)) = sel {
        let n = chars.len();
        for item in chars.iter_mut().take(b.min(n)).skip(a) {
            item.1 = 7;
        }
    }

    let len = chars.len();
    let cursor = cursor.map(|c| c.min(len));

    let mut out: Vec<(String, i32)> = Vec::new();
    let push = |out: &mut Vec<(String, i32)>, text: &str, kind: i32| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.1 == kind {
                last.0.push_str(text);
                return;
            }
        }
        out.push((text.to_string(), kind));
    };

    for (i, (c, k)) in chars.into_iter().enumerate() {
        if cursor == Some(i) {
            push(&mut out, CURSOR_GLYPH, 6);
        }
        push(&mut out, &c.to_string(), k);
    }
    if cursor == Some(len) {
        push(&mut out, CURSOR_GLYPH, 6);
    }
    out
}

/// Escribe el portapapeles del sistema.
fn clipboard_write(text: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text.to_string()).map_err(|e| e.to_string())
}

/// Lee el portapapeles del sistema, si tiene texto.
fn read_clipboard() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut cb| cb.get_text().ok())
}

/// Nombre de archivo para una pestaña.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Aplica una tecla de texto a una línea de entrada. Devuelve `true` cuando la
/// línea debe enviarse (Enter).
fn apply_text_key(input: &mut String, text: &str) -> bool {
    let mut submit = false;
    for ch in text.chars() {
        match ch {
            '\u{8}' | '\u{7f}' => {
                input.pop();
            }
            '\n' | '\r' => submit = true,
            '\u{3}' => input.clear(),
            '\u{1b}' => {}
            c if c >= ' ' && c < '\u{E000}' => input.push(c),
            _ => {}
        }
    }
    submit
}

/// Comando de shell del sistema para una línea.
fn shell_command(cmd: &str) -> (PathBuf, Vec<String>) {
    #[cfg(windows)]
    {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        (
            PathBuf::from(comspec),
            vec!["/D".to_string(), "/C".to_string(), cmd.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (PathBuf::from("sh"), vec!["-c".to_string(), cmd.to_string()])
    }
}

/// Convierte un RGB del emulador a color de Slint.
fn to_color((r, g, b): crate::vt::Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}

/// Traduce una tecla de Slint a los bytes que espera el PTY.
///
/// En Slint los controles llegan como caracteres de control o de uso privado:
/// Backspace es `\u{8}`, Delete `\u{7f}` y las flechas `\u{F700}`–`\u{F703}`.
fn encode_key(text: &str, control: bool, shift: bool, alt: bool) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => out.push(b'\r'),
            '\u{8}' => out.push(0x7f),
            '\u{7f}' => out.extend_from_slice(b"\x1b[3~"),
            '\t' => out.push(b'\t'),
            '\u{1b}' => out.push(0x1b),
            '\u{F700}' => out.extend_from_slice(if shift { b"\x1b[1;2A" } else { b"\x1b[A" }),
            '\u{F701}' => out.extend_from_slice(if shift { b"\x1b[1;2B" } else { b"\x1b[B" }),
            '\u{F703}' => out.extend_from_slice(if shift { b"\x1b[1;2C" } else { b"\x1b[C" }),
            '\u{F702}' => out.extend_from_slice(if shift { b"\x1b[1;2D" } else { b"\x1b[D" }),
            '\u{F729}' => out.extend_from_slice(b"\x1b[H"),
            '\u{F72b}' => out.extend_from_slice(b"\x1b[F"),
            '\u{F72c}' => out.extend_from_slice(b"\x1b[5~"),
            '\u{F72d}' => out.extend_from_slice(b"\x1b[6~"),
            c if control && c.is_ascii_alphabetic() => {
                out.push((c as u8).to_ascii_uppercase() & 0x1f);
            }
            c if c >= ' ' && c < '\u{E000}' => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Arma la línea de comando para la shell, cotizando programa y argumentos.
/// PowerShell necesita el operador `&` para ejecutar un path; cmd no.
fn shell_line(powershell: bool, program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote(program)];
    for a in args {
        parts.push(quote(Path::new(a)));
    }
    if powershell {
        format!("& {}", parts.join(" "))
    } else {
        parts.join(" ")
    }
}

/// Cotiza un argumento para la shell (comillas dobles).
fn quote(p: &Path) -> String {
    format!("\"{}\"", p.display())
}

// ---------------------------------------------------------------------------
// Wiring con Slint
// ---------------------------------------------------------------------------

/// Engancha los callbacks de la ventana y arranca el bucle de drenado.
pub fn attach(window: &NuIdeWindow, ide: Rc<RefCell<Ide>>) {
    ide.borrow_mut().sync(window);

    // Menú superior.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_menu_activated(move |label| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.menu(&label);
                e.sync(&w);
            }
        });
    }

    // Cerrar desde el control de la barra superior.
    window.on_close_requested(|| {
        let _ = slint::quit_event_loop();
    });

    // Teclado de la terminal.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_key(move |text, control, shift, alt| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.local_key(&text, control, shift, alt);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_resize(move |w_px, h_px| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.local_resize(w_px, h_px);
                e.sync(&w);
            }
        });
    }

    // Sesiones.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_session_selected(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_session(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_new_session(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.new_session();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_close_session(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.close_session();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_split_terminal(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.split_terminal();
                e.sync(&w);
            }
        });
    }

    // Explorador y editor.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_node_activated(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.node_activated(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_tab_activated(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.tab_activated(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_tab_closed(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.tab_closed(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_editor_key(move |text, control, shift| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.editor_key(&text, control, shift);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_editor_cursor(move |y, x, scroll| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.editor_cursor(y, x, scroll);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_editor_extend(move |y, x, scroll| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.editor_extend(y, x, scroll);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_key(move |text, control| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.agent_key(&text, control);
                e.sync(&w);
            }
        });
    }

    // Bucle de drenado. El closure usa un `Weak`: si guardara el `Rc` habría un
    // ciclo `Ide -> Timer -> Ide`.
    let timer = Timer::default();
    {
        let weak_ide = Rc::downgrade(&ide);
        let weak_win = window.as_weak();
        timer.start(TimerMode::Repeated, Duration::from_millis(60), move || {
            if let (Some(ide), Some(w)) = (weak_ide.upgrade(), weak_win.upgrade()) {
                ide.borrow_mut().drain(&w);
            }
        });
    }
    ide.borrow_mut().timer = Some(timer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_key_traduce_teclas_a_bytes_del_pty() {
        assert_eq!(encode_key("a", false, false, false), Some(b"a".to_vec()));
        assert_eq!(encode_key("\n", false, false, false), Some(b"\r".to_vec()));
        assert_eq!(encode_key("\u{8}", false, false, false), Some(vec![0x7f]));
        assert_eq!(
            encode_key("\u{F700}", false, false, false),
            Some(b"\x1b[A".to_vec())
        );
        // Ctrl+C es 0x03.
        assert_eq!(encode_key("c", true, false, false), Some(vec![0x03]));
        // Una tecla sin sentido no produce nada.
        assert_eq!(encode_key("\u{F708}", false, false, false), None);
    }

    #[test]
    fn shell_line_cotiza_y_usa_el_call_operator_en_powershell() {
        let args = vec!["--target".to_string(), "c".to_string()];
        let cmd = shell_line(false, Path::new("C:\\x\\spasmc.py"), &args);
        assert_eq!(cmd, "\"C:\\x\\spasmc.py\" \"--target\" \"c\"");

        let ps = shell_line(true, Path::new("C:\\x\\spasmc.py"), &args);
        assert_eq!(ps, "& \"C:\\x\\spasmc.py\" \"--target\" \"c\"");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nuide-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("crear la carpeta de prueba");
        d
    }

    #[test]
    fn read_lines_preserva_el_salto_final_y_detecta_binario() {
        let dir = temp_dir("read");
        let txt = dir.join("a.spasm");
        std::fs::write(&txt, "let x = 1\n; nota\n").unwrap();
        let (lines, trailing, read_only) = read_lines(&txt);
        assert_eq!(lines, vec!["let x = 1", "; nota"]);
        assert!(trailing);
        assert!(!read_only);

        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2]).unwrap();
        let (lines, _, read_only) = read_lines(&bin);
        assert_eq!(lines[0], "[archivo binario]");
        assert!(read_only);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_cursor_se_inserta_como_token() {
        let joined = |col| -> String {
            render_tokens("let x = 1", None, Some(col))
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        assert_eq!(joined(4), "let |x = 1");
        assert_eq!(joined(0), "|let x = 1");
        assert_eq!(joined(9), "let x = 1|");

        let toks = render_tokens("ab", None, Some(2));
        assert!(toks.iter().any(|(t, k)| t == "|" && *k == 6));

        // La selección se marca como kind 7.
        let toks = render_tokens("hola", Some((0, 2)), None);
        assert!(toks.iter().any(|(t, k)| t == "ho" && *k == 7));
    }

    #[test]
    fn editar_inserta_borra_parte_lineas_y_deshace() {
        let dir = temp_dir("edit");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "ab\ncd").unwrap();

        let mut ide = Ide::new();
        ide.open_file(f.clone());

        ide.move_end(false);
        ide.insert_char('c');
        assert_eq!(ide.tabs[0].lines[0], "abc");
        assert!(ide.tabs[0].dirty);

        ide.backspace();
        assert_eq!(ide.tabs[0].lines[0], "ab");

        // La flecha derecha al final de una línea cruza al inicio de la otra.
        ide.move_right(false);
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 1, col: 0 });
        ide.insert_char('Z');
        assert_eq!(ide.tabs[0].lines[1], "Zcd");

        // Backspace al inicio de una línea une con la anterior.
        ide.move_home(false);
        ide.backspace();
        assert_eq!(ide.tabs[0].lines, vec!["abZcd"]);
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 0, col: 2 });

        // Deshacer revierte la unión.
        ide.undo_active();
        assert_eq!(ide.tabs[0].lines, vec!["ab", "Zcd"]);

        // Guardar escribe el buffer y limpia el flag de cambios.
        ide.move_up(false);
        ide.move_end(false);
        ide.insert_char('!');
        assert!(ide.tabs[0].dirty);
        ide.save_active();
        assert!(!ide.tabs[0].dirty);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "ab!\nZcd");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seleccionar_y_reemplazar_con_deshacer_y_rehacer() {
        let dir = temp_dir("sel");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "hola mundo").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());

        ide.tabs[0].anchor = Some(Cursor { line: 0, col: 0 });
        ide.tabs[0].cursor = Cursor { line: 0, col: 4 };
        assert_eq!(ide.tabs[0].selected_text(), "hola");
        ide.insert_str("chau");
        assert_eq!(ide.tabs[0].lines[0], "chau mundo");

        ide.undo_active();
        assert_eq!(ide.tabs[0].lines[0], "hola mundo");
        ide.redo_active();
        assert_eq!(ide.tabs[0].lines[0], "chau mundo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_seleccion_multilinea_se_borra_entera() {
        let dir = temp_dir("sel-multi");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "uno\ndos\ntres").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());

        ide.tabs[0].anchor = Some(Cursor { line: 0, col: 1 });
        ide.tabs[0].cursor = Cursor { line: 2, col: 2 };
        assert_eq!(ide.tabs[0].selected_text(), "no\ndos\ntr");
        ide.backspace();
        assert_eq!(ide.tabs[0].lines, vec!["ues"]);
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 0, col: 1 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enter_auto_indenta_copiando_el_espaciado_inicial() {
        let dir = temp_dir("indent");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "    let x = 1").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());

        ide.move_end(false);
        ide.newline();
        assert_eq!(ide.tabs[0].lines[1], "    ");
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 1, col: 4 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_punto_de_la_vista_se_traduce_a_linea_y_columna() {
        let dir = temp_dir("point");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "uno\ndos\ntres").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());
        let t = &ide.tabs[0];

        // y = 28 => (28 - 8) / 20 = línea 1; x = 58 => columna 0.
        assert_eq!(
            point_to_cursor(t, 28.0, 58.0, 0.0),
            Cursor { line: 1, col: 0 }
        );
        // x = 58 + 2*7.2 => columna 2 (dentro de "dos").
        assert_eq!(point_to_cursor(t, 28.0, 58.0 + 14.4, 0.0).col, 2);
        // Con scroll -20 el mismo punto cae en la línea siguiente.
        assert_eq!(
            point_to_cursor(t, 28.0, 58.0, -20.0),
            Cursor { line: 2, col: 0 }
        );
        // Más allá del final se acota a la última línea.
        assert_eq!(
            point_to_cursor(t, 500.0, 0.0, 0.0),
            Cursor { line: 2, col: 0 }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abrir_un_archivo_crea_pestana_y_cerrarla_la_quita() {
        let dir = temp_dir("tabs");
        let a = dir.join("a.spasm");
        let b = dir.join("b.spasm");
        std::fs::write(&a, "end;").unwrap();
        std::fs::write(&b, "end;").unwrap();

        let mut ide = Ide::new();
        ide.open_file(a.clone());
        assert_eq!(ide.tabs.len(), 1);
        assert_eq!(ide.active_file.as_deref(), Some(a.as_path()));

        ide.open_file(b.clone());
        assert_eq!(ide.tabs.len(), 2);

        // Reabrir no duplica: activa la pestaña existente.
        ide.open_file(a.clone());
        assert_eq!(ide.tabs.len(), 2);
        assert_eq!(ide.tabs[ide.active_tab].path, a);

        ide.tab_closed(ide.active_tab as i32);
        assert_eq!(ide.tabs.len(), 1);
        assert_eq!(ide.tabs[0].path, b);

        ide.tab_closed(0);
        assert!(ide.tabs.is_empty());
        assert!(ide.active_file.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
