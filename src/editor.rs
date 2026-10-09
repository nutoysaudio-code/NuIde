//! Estado de NuIde: el puente entre la ventana Slint y la lógica real.
//!
//! Mantiene el toolchain de SpASM, el árbol de archivos, las pestañas del
//! editor, la terminal real (PTY) y el panel del agente, y cablea los callbacks
//! de la ventana.
//!
//! ### Terminales
//!
//! Las dos terminales —la local inferior y el panel del agente— son un **PTY**
//! (ConPTY en Windows) con un proceso persistente; los bytes crudos los
//! interpreta el emulador de `vt`. Escribir es mandarle teclas al proceso. En la
//! terminal local corre una shell; en el panel del agente corre una shell que
//! lanza `opencode` al arrancar (si sale de opencode, queda en la shell). Los
//! menús Ejecutar/Compilar escriben la orden en la shell local, así la salida
//! aparece ahí.
//!
//! Los procesos corren en hilos y avisan por canales. Como los callbacks de
//! Slint viven en el hilo de la UI, un `Timer` de 16 ms drena los canales y
//! vuelca el estado con `sync`. Ese timer se guarda en `Ide` para que no se caiga
//! al salir de `attach`; su closure usa un `Weak` para no crear un ciclo.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelNotify, ModelRc, Timer, TimerMode, VecModel};

use crate::build;
use crate::config::Config;
use crate::deploy::{self, DeployMsg};
use crate::elevate;
use crate::explorer::{Action, Explorer};
use crate::git;
use crate::highlight;
use crate::lint;
use crate::pty::{self, Pty, PtyEvent};
use crate::search;
use crate::site::{Site, Transport};
use crate::spasm::Toolchain;
use crate::sqlite::{self, Tool as SqliteTool};
use crate::sysinfo;
use crate::templates;
use crate::ui::{
    DockDrag, IdeCodeLine, IdeDiffLine, IdeGitFile, IdeMenuEntry, IdeOutlineItem, IdePaletteItem,
    IdePrefRow, IdeRecent, IdeSearchHit, IdeSetting, IdeTab, IdeToken, IdeTreeNode, NuIdeWindow,
    TermLine, TermRun,
};
use crate::vt::{Run, Screen, Selection};
use crate::webserver::{Server as WebServer, WebConfig, WebEvent};

/// Tope de líneas que se abren de un archivo. Un archivo enorme no puede
/// congelar la UI; lo que sobra no se muestra.
const MAX_OPEN_LINES: usize = 20_000;

/// Tope de pasos de deshacer por archivo.
const MAX_UNDO: usize = 500;

/// Glifo del cursor. Se inserta como un token más para no tener que calcular
/// píxeles: el render es monoespaciado y el caret es un carácter al lado.
const CURSOR_GLYPH: &str = "|";

/// Ancho aproximado de un carácter monoespaciado a 12 px y arranque del texto
/// dentro de la fila (ver `IdeCodeRow`). Se usa sólo para ubicar el cursor al
/// hacer clic: un error de una columna en el borde es aceptable.
const CHAR_W: f32 = 7.2;
const CODE_X0: f32 = 58.0;
/// Alto de fila y margen superior del contenido del editor (ver `IdeCodeRow` y
/// el `VerticalLayout` de `IdeEditor`).
const ROW_H: f32 = 20.0;
const CODE_TOP: f32 = 8.0;

/// Tamaño inicial de cada terminal (columnas × filas) hasta que la UI avise el
/// tamaño real del panel.
const TERM_COLS: u16 = 100;
const TERM_ROWS: u16 = 24;
/// El panel del agente es más angosto que la terminal inferior.
const AGENT_COLS: u16 = 60;
const AGENT_ROWS: u16 = 30;
/// Métricas de celda de la terminal real. Coinciden con `TerminalScreen`
/// (mono 12 px). El ancho es aproximado: sólo decide cuántas columnas pedirle al
/// PTY, no la posición del cursor (que se dibuja invirtiendo la celda).
const TERM_CELL_W: f32 = 7.2;
const TERM_CELL_H: f32 = 16.0;

/// Familias preferidas (nombre, archivo de fuente). Slint no soporta listas de
/// fuentes, así que se elige una sola según lo instalado.
///
/// La **interfaz** usa JetBrains Mono por defecto (es la fuente del proyecto y
/// mantiene el mismo aire que el código y las terminales); si no está instalada
/// se cae a las otras. Se puede ciclar desde Preferencias.
const SANS_FONTS: &[(&str, &str)] = &[
    ("JetBrains Mono", "JetBrainsMono-Regular.ttf"),
    ("Montserrat", "Montserrat-Regular.ttf"),
    ("Century Gothic", "GOTHIC.TTF"),
    ("Segoe UI", "segoeui.ttf"),
];
const MONO_FONTS: &[(&str, &str)] = &[
    ("JetBrains Mono", "JetBrainsMono-Regular.ttf"),
    ("Cascadia Mono", "CascadiaMono.ttf"),
    ("Consolas", "consola.ttf"),
];

/// Acentos disponibles en Preferencias (nombre, RGB).
const ACCENTS: &[(&str, (u8, u8, u8))] = &[
    ("menta", (0x83, 0xd9, 0xcb)),
    ("ámbar", (0xe7, 0xba, 0x70)),
    ("celeste", (0x8f, 0xc7, 0xe8)),
    ("violeta", (0xc9, 0xa5, 0xe0)),
    ("verde", (0x9f, 0xd9, 0x9f)),
];

/// Pasos de opacidad de los paneles (Preferencias → Opacidad). Se cicla entre
/// ellos; 1.0 es el aspecto sólido de siempre.
const PANEL_ALPHAS: &[f32] = &[1.0, 0.92, 0.85, 0.75, 0.65];

/// Acciones globales con atajo configurable: (id, etiqueta, por defecto).
const KEY_ACTIONS: &[(&str, &str, &str)] = &[
    ("palette", "Paleta de comandos", "Ctrl+Shift+P"),
    ("files", "Buscar archivos", "Ctrl+P"),
    ("search", "Buscar en el proyecto", "Ctrl+Shift+F"),
    ("help", "Ayuda SpASM", "F1"),
];

/// Posición del cursor dentro de un buffer: línea y columna **en caracteres**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    line: usize,
    col: usize,
}

/// Qué tipo de documento tiene una pestaña.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocKind {
    /// Texto editable con resaltado.
    Text,
    /// Imagen (se muestra, no se edita).
    Image,
}

/// Operación de edición atómica para undo/redo incremental.
#[derive(Debug, Clone)]
enum EditOp {
    /// Insertar un carácter en (line, col).
    InsertChar { line: usize, col: usize, ch: char },
    /// Borrar un carácter en (line, col) hacia atrás.
    DeleteChar { line: usize, col: usize, ch: char },
    /// Insertar una línea nueva después de `line`.
    InsertLine { after: usize, content: String },
    /// Borrar la línea `line`.
    DeleteLine { line: usize },
    /// Unir la línea `line` con la siguiente (borrar el salto).
    JoinLine { line: usize },
    /// Dividir la línea `line` en la columna `at`.
    SplitLine { line: usize, at: usize },
    /// Snapshot completo del documento (sistema legacy).
    Snapshot { lines: Vec<String>, cursor: Cursor },
}

/// Un archivo abierto en una pestaña del editor.
struct OpenTab {
    path: PathBuf,
    /// Texto o imagen.
    kind: DocKind,
    /// Imagen cargada (sólo para `DocKind::Image`).
    image: Option<slint::Image>,
    /// Zoom de la imagen (1.0 = ajustada al panel).
    image_zoom: f32,
    /// Lenguaje detectado por la extensión (para el resaltado y la barra).
    language: highlight::Language,
    /// Markdown: mostrar la vista previa en vez del código.
    preview: bool,
    /// Texto con estilo ya construido para la vista previa.
    preview_doc: Option<slint::StyledText>,
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
    /// El proceso sigue vivo.
    alive: bool,
    /// Selección de texto en coordenadas de la vista, si la hay.
    sel: Option<TermSel>,
    /// Último botón de mouse apretado (0/1/2) o 3 = ninguno (para el modo
    /// "motion" del mouse reporting).
    mouse_button: u8,
    /// Filas para la UI. Es **siempre el mismo `Rc`**: el refresco reemplaza
    /// sólo las filas que cambiaron y avisa con `row_changed` (ver `TermRows`),
    /// así Slint no destruye ni vuelve a crear los elementos del panel en cada
    /// chunk de salida de una TUI.
    term_model: Rc<TermRows>,
}

/// Modelo de filas de una terminal, **mutable en su sitio**.
///
/// `VecModel` no tiene un "cambiar fila": lo que hay es `remove` + `insert`, que
/// mandan `row_removed`/`row_added` y hacen que Slint **destruya el elemento del
/// `for` y cree uno nuevo** (con sus `Text` y el shaping de cada tramo). Medido
/// en reposo con el panel de OpenCode abierto, eso era el ~70 % del CPU.
///
/// `TermRows` guarda el mismo `Vec` y avisa con `row_changed`: Slint re-usa el
/// elemento existente y sólo re-evalúa sus bindings.
#[derive(Default)]
struct TermRows {
    rows: RefCell<Vec<TermLine>>,
    notify: ModelNotify,
}

impl TermRows {
    fn len(&self) -> usize {
        self.rows.borrow().len()
    }

    /// Reemplaza una fila y avisa el cambio. Se suelta el borrow antes de
    /// notificar: el cambio re-ingresa a `row_data`.
    fn set_row(&self, index: usize, line: TermLine) {
        {
            let mut rows = self.rows.borrow_mut();
            if index >= rows.len() {
                return;
            }
            rows[index] = line;
        }
        self.notify.row_changed(index);
    }

    /// Cambia el contenido y el conteo (arranque, o cambió el alto del panel).
    fn set_all(&self, lines: Vec<TermLine>) {
        *self.rows.borrow_mut() = lines;
        self.notify.reset();
    }

    /// Copia de las filas; la usan sólo las pruebas.
    #[cfg(test)]
    fn snapshot(&self) -> Vec<TermLine> {
        self.rows.borrow().clone()
    }
}

impl Model for TermRows {
    type Data = TermLine;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<TermLine> {
        self.rows.borrow().get(row).cloned()
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}

/// Ancla y extremo de una selección de terminal: dos celdas `(fila, columna)`
/// en coordenadas de la vista (las mismas que devuelve `Screen::snapshot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TermSel {
    anchor: (u16, u16),
    head: (u16, u16),
}

impl TermSel {
    /// Región ordenada con el fin **exclusivo**, lista para pintar y copiar.
    /// `None` si no hay nada seleccionado (un clic suelto).
    fn region(&self, cols: u16) -> Option<Selection> {
        let (start, end) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        if start == end {
            return None;
        }
        let end_col = (end.1 + 1).min(cols);
        Some((start.0, start.1, end.0, end_col))
    }
}

/// Cuál de las dos terminales de la ventana. Las dos comparten el mismo
/// emulador y los mismos gestos (teclado, rueda, selección); sólo cambian el
/// proceso que corre y el panel donde se dibujan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Terminal local inferior (una shell).
    Local,
    /// Panel derecho del agente (la TUI de `opencode`).
    Agent,
}

/// Evento de mouse reenviado al proceso cuando la terminal lo pide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MouseKind {
    /// Botón apretado.
    Press(u8),
    /// Botón soltado.
    Release(u8),
    /// Movimiento (con o sin botón).
    Move,
    /// Rueda: `true` = arriba, `false` = abajo.
    Wheel(bool),
}

/// Traduce el código que manda la UI (`kind`, `button`) al evento de mouse.
/// `kind`: 0 apretar, 1 soltar, 2 mover, 3 rueda arriba, 4 rueda abajo.
fn mouse_kind(kind: i32, button: i32) -> MouseKind {
    match kind {
        0 => MouseKind::Press(button.clamp(0, 2) as u8),
        1 => MouseKind::Release(button.clamp(0, 2) as u8),
        2 => MouseKind::Move,
        3 => MouseKind::Wheel(true),
        _ => MouseKind::Wheel(false),
    }
}

/// Dónde vive un panel. Los que comparten región se apilan (hasta 3 por lado).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dock {
    Left,
    Right,
    Bottom,
    Hidden,
}

impl Dock {
    fn from_code(code: i32) -> Dock {
        match code {
            0 => Dock::Left,
            1 => Dock::Right,
            2 => Dock::Bottom,
            _ => Dock::Hidden,
        }
    }

    fn code(self) -> i32 {
        match self {
            Dock::Left => 0,
            Dock::Right => 1,
            Dock::Bottom => 2,
            Dock::Hidden => 3,
        }
    }

    /// Siguiente dock al hacer clic en el menú Ver.
    fn next(self) -> Dock {
        match self {
            Dock::Left => Dock::Right,
            Dock::Right => Dock::Bottom,
            Dock::Bottom => Dock::Hidden,
            Dock::Hidden => Dock::Left,
        }
    }
}

/// Estado de la búsqueda del editor.
#[derive(Default)]
struct Find {
    /// La barra está abierta.
    open: bool,
    /// Se ve el segundo campo (reemplazo).
    replace_open: bool,
    query: String,
    replace: String,
    case_sensitive: bool,
    /// Sólo coincidencias que sean palabras completas.
    whole_word: bool,
    /// La consulta es una expresión regular.
    regex: bool,
    /// Sólo buscar dentro de la selección (`selection` guarda el rango).
    in_selection: bool,
    /// Rango de la selección al activar `in_selection`:
    /// `(línea_ini, col_ini, línea_fin, col_fin)`.
    selection: Option<(usize, usize, usize, usize)>,
    /// Consultas anteriores (para las flechas arriba/abajo).
    history: Vec<String>,
    /// Posición dentro del historial (`None` = editando la consulta actual).
    history_pos: Option<usize>,
    /// El foco está en el campo de reemplazo (si no, en el de búsqueda).
    focus_replace: bool,
    /// Posición del cursor dentro de cada campo (en caracteres).
    query_cursor: usize,
    replace_cursor: usize,
    /// Coincidencias de la pestaña activa: `(línea, col_inicio, col_fin)`, con el
    /// fin **exclusivo** y columnas en caracteres.
    matches: Vec<(usize, usize, usize)>,
    /// Las mismas coincidencias agrupadas por línea, para el render.
    by_line: HashMap<usize, Vec<(usize, usize)>>,
    /// Índice de la coincidencia actual dentro de `matches`.
    current: Option<usize>,
}

impl Find {
    /// Rango `(línea, inicio, fin)` de la coincidencia actual.
    fn current_range(&self) -> Option<(usize, usize, usize)> {
        self.current.and_then(|i| self.matches.get(i).copied())
    }
}

/// Estado de "buscar en el proyecto".
#[derive(Default)]
struct Search {
    open: bool,
    query: String,
    replace: String,
    replace_open: bool,
    case_sensitive: bool,
    regex: bool,
    focus_replace: bool,
    query_cursor: usize,
    replace_cursor: usize,
    hits: Vec<crate::search::SearchHit>,
    selected: usize,
    truncated: bool,
    searching: bool,
}

/// Decoraciones de resaltado que dependen del cursor y del documento, además de
/// la selección y la búsqueda. Se recalculan una vez por pasada de render.
#[derive(Default)]
struct Decorations {
    /// Ocurrencias de la palabra bajo el cursor, por línea.
    words: HashMap<usize, Vec<(usize, usize)>>,
    /// Las dos llaves correspondientes: `(línea, columna)`.
    brackets: Vec<(usize, usize)>,
}

/// Entrada de la lista de "Recientes" de la bienvenida.
#[derive(Clone)]
enum RecentEntry {
    Project(PathBuf),
    File(PathBuf),
}

/// Comandos que ofrece la paleta (`Ctrl+Shift+P`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Build,
    BuildAndRun,
    Lint,
    NextError,
    Find,
    Replace,
    GotoDef,
    GoToLine,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Preview,
    Save,
    SaveAs,
    CloseTab,
    CloseAll,
    OpenProject,
    OpenFile,
    NewFile,
    FindFiles,
    NewTerminal,
    NewAdminTerminal,
    SplitTerminal,
    ToggleExplorer,
    ToggleAgent,
    ToggleTerminal,
    ToggleGit,
    GitRefresh,
    GitStageAll,
    GitPush,
    CycleTabSize,
    ToggleInsertSpaces,
    RestartSession,
    RenameSession,
    ProjectSearch,
    GitPull,
    GitUnstageAll,
    GitAmend,
    GitDiff,
    RunPickEntry,
    RunUseActive,
    RunEditArgs,
    RunToggleBuildOnSave,
    SpasmHelp,
    NewProject,
    TemplateCompiled,
    TemplateInterpreter,
    TemplateEmpty,
    Preferences,
    RelaunchElevated,
}

impl Command {
    /// Todos, en el orden en que se muestran.
    const ALL: [Command; 54] = [
        Command::Run,
        Command::Build,
        Command::BuildAndRun,
        Command::Lint,
        Command::NextError,
        Command::Find,
        Command::Replace,
        Command::GotoDef,
        Command::GoToLine,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::Preview,
        Command::Save,
        Command::SaveAs,
        Command::CloseTab,
        Command::CloseAll,
        Command::FindFiles,
        Command::OpenProject,
        Command::OpenFile,
        Command::NewFile,
        Command::NewTerminal,
        Command::NewAdminTerminal,
        Command::SplitTerminal,
        Command::ToggleExplorer,
        Command::ToggleAgent,
        Command::ToggleTerminal,
        Command::ToggleGit,
        Command::GitRefresh,
        Command::GitStageAll,
        Command::GitPush,
        Command::CycleTabSize,
        Command::ToggleInsertSpaces,
        Command::RestartSession,
        Command::RenameSession,
        Command::ProjectSearch,
        Command::GitPull,
        Command::GitUnstageAll,
        Command::GitAmend,
        Command::GitDiff,
        Command::RunPickEntry,
        Command::RunUseActive,
        Command::RunEditArgs,
        Command::RunToggleBuildOnSave,
        Command::SpasmHelp,
        Command::NewProject,
        Command::TemplateCompiled,
        Command::TemplateInterpreter,
        Command::TemplateEmpty,
        Command::Preferences,
        Command::RelaunchElevated,
    ];

    fn label(self) -> &'static str {
        match self {
            Command::Run => "Ejecutar el programa",
            Command::Build => "Compilar (spasmc)",
            Command::BuildAndRun => "Compilar y ejecutar",
            Command::Lint => "Analizar / diagnóstico",
            Command::NextError => "Siguiente error",
            Command::Find => "Buscar",
            Command::Replace => "Buscar y reemplazar",
            Command::GotoDef => "Ir a definición",
            Command::GoToLine => "Ir a línea…",
            Command::Undo => "Deshacer",
            Command::Redo => "Rehacer",
            Command::Cut => "Cortar",
            Command::Copy => "Copiar",
            Command::Paste => "Pegar",
            Command::SelectAll => "Seleccionar todo",
            Command::Preview => "Vista previa de Markdown",
            Command::Save => "Guardar",
            Command::SaveAs => "Guardar como…",
            Command::CloseTab => "Cerrar pestaña",
            Command::CloseAll => "Cerrar todas las pestañas",
            Command::FindFiles => "Buscar archivos",
            Command::OpenProject => "Abrir carpeta del proyecto…",
            Command::OpenFile => "Abrir archivo…",
            Command::NewFile => "Nuevo archivo…",
            Command::NewTerminal => "Nueva terminal",
            Command::NewAdminTerminal => "Terminal: nueva como administrador",
            Command::SplitTerminal => "Dividir la terminal",
            Command::ToggleExplorer => "Ver: Explorador",
            Command::ToggleAgent => "Ver: OpenCode CLI",
            Command::ToggleTerminal => "Ver: Terminal",
            Command::ToggleGit => "Ver: Control de código",
            Command::GitRefresh => "Git: refrescar",
            Command::GitStageAll => "Git: preparar todo",
            Command::GitPush => "Git: push",
            Command::CycleTabSize => "Editor: tamaño de tab (2/4/8)",
            Command::ToggleInsertSpaces => "Editor: insertar espacios",
            Command::RestartSession => "Terminal: reiniciar sesión",
            Command::RenameSession => "Terminal: renombrar sesión…",
            Command::ProjectSearch => "Buscar en el proyecto",
            Command::GitPull => "Git: bajar (pull)",
            Command::GitUnstageAll => "Git: quitar todo del índice",
            Command::GitAmend => "Git: enmendar el último commit",
            Command::GitDiff => "Git: ver diff del archivo",
            Command::RunPickEntry => "Ejecutar: elegir archivo de entrada…",
            Command::RunUseActive => "Ejecutar: usar el archivo activo",
            Command::RunEditArgs => "Ejecutar: argumentos…",
            Command::RunToggleBuildOnSave => "Ejecutar: compilar al guardar",
            Command::SpasmHelp => "SpASM: ayuda de sintaxis",
            Command::NewProject => "SpASM: nuevo proyecto…",
            Command::TemplateCompiled => "Plantilla: release (compilado)…",
            Command::TemplateInterpreter => "Plantilla: desarrollo (intérprete)…",
            Command::TemplateEmpty => "Plantilla: archivo vacío…",
            Command::Preferences => "Preferencias",
            Command::RelaunchElevated => "NuIde: reiniciar como administrador",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Command::Run => "F5",
            Command::Build => "Ctrl+B",
            Command::BuildAndRun => "Ctrl+Shift+B",
            Command::Lint => "F6",
            Command::NextError => "F8",
            Command::Find => "Ctrl+F",
            Command::Replace => "Ctrl+H",
            Command::GotoDef => "F12",
            Command::GoToLine => "Ctrl+G",
            Command::Undo => "Ctrl+Z",
            Command::Redo => "Ctrl+Y",
            Command::Cut => "Ctrl+X",
            Command::Copy => "Ctrl+C",
            Command::Paste => "Ctrl+V",
            Command::SelectAll => "Ctrl+A",
            Command::Preview => "Ctrl+Shift+M",
            Command::Save => "Ctrl+S",
            Command::SaveAs => "Ctrl+Shift+S",
            Command::CloseTab => "Ctrl+W",
            Command::FindFiles => "Ctrl+P",
            Command::NewTerminal | Command::SplitTerminal => "terminal",
            Command::NewAdminTerminal => "admin",
            Command::ToggleExplorer
            | Command::ToggleAgent
            | Command::ToggleTerminal
            | Command::ToggleGit => "panel",
            Command::GitRefresh | Command::GitStageAll | Command::GitPush => "git",
            Command::CycleTabSize => "4",
            Command::ToggleInsertSpaces => "espacios",
            Command::RestartSession | Command::RenameSession => "terminal",
            Command::ProjectSearch => "Ctrl+Shift+F",
            Command::GitPull | Command::GitUnstageAll | Command::GitAmend | Command::GitDiff => {
                "git"
            }
            Command::RunPickEntry
            | Command::RunUseActive
            | Command::RunEditArgs
            | Command::RunToggleBuildOnSave => "F5",
            Command::SpasmHelp => "F1",
            Command::NewProject => "spasm",
            Command::TemplateCompiled | Command::TemplateInterpreter | Command::TemplateEmpty => {
                "plantilla"
            }
            Command::Preferences => "Ctrl+,",
            Command::RelaunchElevated => "admin",
            _ => "",
        }
    }
}

/// Comandos que matchean `query` (etiqueta o atajo). Vacío = todos.
fn filter_commands(query: &str) -> Vec<Command> {
    let q = query.to_lowercase();
    Command::ALL
        .iter()
        .copied()
        .filter(|c| {
            q.is_empty()
                || c.label().to_lowercase().contains(&q)
                || c.hint().to_lowercase().contains(&q)
        })
        .collect()
}

/// Fila con acción para un menú desplegable.
fn menu_entry(id: &str, label: &str, detail: &str, enabled: bool) -> IdeMenuEntry {
    IdeMenuEntry {
        id: id.into(),
        label: label.into(),
        detail: detail.into(),
        enabled,
    }
}

/// Separador de menú.
fn menu_sep() -> IdeMenuEntry {
    IdeMenuEntry {
        id: "".into(),
        label: "".into(),
        detail: "".into(),
        enabled: false,
    }
}

/// Separa una línea de argumentos respetando comillas simples y dobles.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c.is_whitespace() {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Convierte un diff unificado en líneas tipadas para la UI.
/// kind: 0 contexto, 1 agregada, 2 borrada, 3 hunk (`@@`), 4 metadatos.
fn parse_diff_lines(text: &str) -> Vec<IdeDiffLine> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let kind = if line.starts_with("@@") {
                3
            } else if line.starts_with("+++")
                || line.starts_with("---")
                || line.starts_with("diff ")
                || line.starts_with("index ")
                || line.starts_with("new file")
                || line.starts_with("deleted file")
                || line.starts_with("similarity ")
                || line.starts_with("rename ")
                || line.starts_with("archivo nuevo")
            {
                4
            } else if line.starts_with('+') {
                1
            } else if line.starts_with('-') {
                2
            } else {
                0
            };
            IdeDiffLine {
                text: line.to_string().into(),
                kind,
            }
        })
        .collect()
}

/// Carpeta donde crear algo a partir de un nodo: la suya si es carpeta, si no
/// la de su padre.
fn target_dir_of(node: &crate::explorer::Node) -> PathBuf {
    if node.is_dir {
        node.path.clone()
    } else {
        node.path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| node.path.clone())
    }
}

/// Nombre de archivo/carpeta válido (sin separadores ni caracteres reservados).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
}

/// Abre el explorador del sistema con el archivo seleccionado.
fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Abre una URL en el navegador del sistema.
fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Tipo de paleta abierta.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum PaletteKind {
    #[default]
    Commands,
    Files,
    /// "Ir a línea…": el texto es un número.
    GoToLine,
    /// Ayuda de sintaxis de SpASM: inserta la palabra elegida.
    Help,
}

/// Acción de una fila de la paleta.
#[derive(Clone)]
enum PaletteAction {
    Command(Command),
    File(PathBuf),
    GoToLine(usize),
    /// Inserta la palabra en el cursor.
    InsertWord(String),
}

/// Una fila ya resuelta para dibujar (texto + acción).
#[derive(Clone)]
struct PaletteRow {
    label: String,
    detail: String,
    action: PaletteAction,
}

/// Estado de la paleta de comandos / buscador de archivos.
#[derive(Default)]
struct Palette {
    open: bool,
    kind: PaletteKind,
    query: String,
    rows: Vec<PaletteRow>,
    selected: usize,
    /// Archivos del proyecto, juntados al abrir el buscador.
    files: Vec<PathBuf>,
}

/// Acción pendiente de confirmar cuando hay cambios sin guardar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Confirm {
    /// Cerrar la pestaña activa.
    CloseTab,
    /// Cerrar todas las pestañas.
    CloseAll,
    /// Salir de la aplicación.
    Quit,
    /// Reiniciar la aplicación elevada (UAC).
    RelaunchElevated,
}

/// Confirmación de dos botones pendiente.
enum Ask {
    /// Eliminar una ruta.
    Delete(PathBuf),
    /// Reemplazar en todo el proyecto.
    ReplaceInProject,
    /// Descartar los cambios de un archivo de Git.
    Discard { path: String, untracked: bool },
    /// Eliminar un proveedor de OpenCode de la config global.
    DeleteProvider(String),
}

/// Borrador del proveedor de OpenCode que se edita en el formulario.
struct ProviderDraft {
    /// Id original (vacío si es un proveedor nuevo).
    original_id: String,
    id: String,
    name: String,
    package: String,
    base_url: String,
    api_key: String,
    /// Texto JSON del objeto `models` (se valida como JSON al guardar).
    models: String,
}

impl Default for ProviderDraft {
    fn default() -> Self {
        ProviderDraft {
            original_id: String::new(),
            id: String::new(),
            name: String::new(),
            package: "@opencode/ai/providers/openai-compatible".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            models: "{}".to_string(),
        }
    }
}

/// Visor de diff abierto.
struct DiffState {
    /// Ruta relativa a la raíz.
    path: String,
    staged: bool,
    untracked: bool,
    lines: Vec<IdeDiffLine>,
    note: String,
    error: bool,
}

/// Construye una fila de la pantalla de configuración.
///
/// `kind`: 0 texto · 1 booleano · 2 opción (ciclo) · 3 deslizador · 4 acción ·
/// 5 contraseña · 6 informativo. `scope`: 0 global · 1 del proyecto abierto.
fn setting(
    id: &str,
    section: i32,
    kind: i32,
    label: &str,
    value: &str,
    hint: &str,
    scope: i32,
) -> IdeSetting {
    IdeSetting {
        id: id.into(),
        section,
        kind,
        label: label.into(),
        value: value.into(),
        hint: hint.into(),
        scope,
        num: 0.0,
        min: 0.0,
        max: 1.0,
        step: 0.0,
    }
}

/// Qué pide el diálogo de entrada de texto.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    NewFile,
    NewFolder,
    Rename,
    /// Renombrar la sesión de terminal activa.
    SessionName,
    /// Crear una rama nueva en Git.
    NewBranch,
    /// Argumentos extra para ejecutar (F5).
    RunArgs,
    /// Editar un campo del sitio de despliegue ("host", "user", ...).
    SiteField(&'static str),
    /// Editar un campo del módulo de servidor web ("port", "root", "cors").
    WebField(&'static str),
}

/// Diálogo de entrada de texto pendiente.
struct Prompt {
    kind: PromptKind,
    /// Carpeta destino (nuevo) o ruta a renombrar.
    target: PathBuf,
    text: String,
}

/// Estado completo de la IDE.
pub struct Ide {
    toolchain: Toolchain,
    /// Terminal real: una shell por sesión.
    local: Vec<LocalSession>,
    local_active: usize,
    /// Si hay vista dividida, el índice de la sesión del segundo panel.
    split: Option<usize>,
    next_local_id: u64,
    /// Tamaño actual del panel de terminal (columnas × filas).
    local_cols: u16,
    local_rows: u16,
    local_tx: Sender<PtyEvent>,
    local_rx: Receiver<PtyEvent>,
    /// Ancho real de una celda monoespaciada (lo mide la UI). Se usa para
    /// calcular las columnas del PTY y para ubicar las celdas en la grilla.
    cell_w: f32,
    /// Último tamaño en píxeles de cada terminal (para recalcular al cambiar la
    /// celda).
    local_px: (f32, f32),
    agent_px: (f32, f32),
    /// Árbol de archivos.
    explorer: Explorer,
    /// Índice seleccionado en el explorador (-1 = nada).
    explorer_selected: i32,
    /// Menú contextual del explorador abierto y sus filas.
    explorer_context_open: bool,
    explorer_context_items: Vec<IdeMenuEntry>,
    /// Menú contextual de las terminales (clic derecho): abierto, filas y a qué
    /// terminal apunta (copiar/pegar).
    term_context_open: bool,
    term_context_items: Vec<IdeMenuEntry>,
    term_context_target: Target,
    /// Diálogo de entrada de texto (nuevo archivo/carpeta, renombrar).
    prompt: Option<Prompt>,
    /// Confirmación de borrado o reemplazo pendiente.
    ask: Option<Ask>,
    /// Archivos abiertos en el editor.
    tabs: Vec<OpenTab>,
    active_tab: usize,
    /// Últimos proyectos y archivos usados (sección "Recientes" de la
    /// bienvenida).
    recent_projects: Vec<PathBuf>,
    recent_files: Vec<PathBuf>,
    /// Visibilidad de las regiones (barra izquierda, barra derecha, panel
    /// inferior). Las regiones las controlan los botones de la barra.
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    /// Dónde vive cada panel (Explorador, OpenCode, Terminal, Git). Los que
    /// comparten región se apilan.
    docks: [Dock; 4],
    /// El menú desplegable abierto ("Archivo", "Editar", "Ver") o vacío.
    menu_open: String,
    /// Diálogo de "cambios sin guardar" pendiente.
    confirm: Option<Confirm>,
    /// Pestaña objetivo de la confirmación (para "Cerrar pestaña").
    confirm_tab: Option<usize>,
    /// Paleta de comandos / buscador de archivos.
    palette: Palette,
    /// Configuración persistida (proyecto, paneles, ventana, pestañas).
    config: Config,
    /// Modelo de líneas del editor. Se muta por fila para no reconstruir todo en
    /// cada tecla; se rehace entero al abrir/cambiar de pestaña o cambiar el
    /// número de líneas.
    code_model: Rc<VecModel<IdeCodeLine>>,
    /// El modelo ya se entregó a la ventana (a partir de ahí las mutaciones se
    /// ven solas).
    model_attached: bool,
    /// Hay que reconstruir el modelo entero en el próximo `sync`.
    editor_full: bool,
    /// Pila de deshacer del archivo activo: cada entrada es una lista de
    /// operaciones atómicas que forman una "edición" completa.
    undo: Vec<Vec<EditOp>>,
    /// Pila de rehacer. Se vacía con cada edición nueva.
    redo: Vec<Vec<EditOp>>,
    /// Ajustes del editor: ancho de tab y si inserta espacios.
    tab_size: usize,
    insert_spaces: bool,
    /// Configuración de ejecución (F5): archivo de entrada, argumentos y si
    /// compila al guardar.
    run_entry: Option<PathBuf>,
    run_args: String,
    run_build_on_save: bool,
    /// Preferencias: fuentes, tamaño del código y acento.
    sans: String,
    mono: String,
    editor_font_size: f32,
    accent: usize,
    /// Opacidad de las superficies de los paneles (ver `PANEL_ALPHAS`).
    panel_alpha: f32,
    prefs_open: bool,
    /// Atajos globales: acción → combinación.
    keymap: HashMap<String, String>,
    /// Acción esperando una combinación nueva.
    capturing_key: Option<String>,
    /// Diálogo "Acerca de".
    about_open: bool,
    /// Versión de la app (para Acerca de).
    version: String,
    /// Uso de CPU/RAM del sistema (medidor discreto del pie).
    monitor: sysinfo::Monitor,
    metrics: sysinfo::Metrics,
    /// Contador para muestrear el sistema cada ~1.5 s.
    drain_tick: u32,
    /// Estado de la búsqueda del editor (barra, coincidencias).
    find: Find,
    /// Buscar en el proyecto: estado y canal del hilo.
    search: Search,
    search_tx: Sender<search::SearchResult>,
    search_rx: Receiver<search::SearchResult>,
    search_gen: u64,
    /// Diagnósticos del archivo activo (errores del parser de SpASM).
    diagnostics: Vec<lint::Diagnostic>,
    /// Esquema del archivo activo (funciones y variables).
    outline: Vec<lint::Symbol>,
    /// Archivo al que corresponden los diagnósticos actuales.
    diagnostics_path: Option<PathBuf>,
    /// Canal del linter (corre en un hilo).
    lint_tx: Sender<lint::LintResult>,
    lint_rx: Receiver<lint::LintResult>,
    /// Build/Run desde la IDE: salida capturada en un hilo.
    build_tx: Sender<build::BuildResult>,
    build_rx: Receiver<build::BuildResult>,
    /// Id de la sesión de terminal dedicada a la salida de build.
    build_session: Option<u64>,
    /// Sitio de despliegue del proyecto abierto (FTP/FTPS/SFTP, ver `site.rs`).
    site: Site,
    /// Despliegue en curso: canal del hilo de transferencias.
    deploy_tx: Sender<DeployMsg>,
    deploy_rx: Receiver<DeployMsg>,
    /// `true` mientras hay un despliegue o una prueba corriendo.
    deploy_busy: bool,
    /// --- módulo: servidor web local con CORS ---
    web: WebConfig,
    /// El servidor en marcha (`None` = apagado).
    web_server: Option<WebServer>,
    web_tx: Sender<WebEvent>,
    web_rx: Receiver<WebEvent>,
    /// --- módulo: visor de SQLite ---
    sqlite_enabled: bool,
    sqlite_open: bool,
    sqlite_db: Option<PathBuf>,
    sqlite_tool: Option<SqliteTool>,
    sqlite_tables: Vec<String>,
    sqlite_selected: usize,
    /// Columnas y filas ya como texto, listas para el panel.
    sqlite_header: String,
    sqlite_lines: Vec<String>,
    sqlite_note: String,
    sqlite_busy: bool,
    sqlite_tx: Sender<sqlite::Loaded>,
    sqlite_rx: Receiver<sqlite::Loaded>,
    /// --- pantalla de configuración ---
    settings_open: bool,
    settings_section: usize,
    /// CRUD de proveedores de OpenCode (config global).
    provider_open: bool,
    provider_draft: ProviderDraft,
    provider_note: String,
    /// Reinicio elevado pendiente: los callbacks guardan el estado y relanzan.
    relaunch: bool,
    providers: Vec<crate::providers::Provider>,
    /// Nota para agregar a la salida del próximo build (p. ej. falta de cc).
    build_note: Option<String>,
    /// Git del proyecto: estado, nota, hilo de operaciones.
    git_status: git::Status,
    git_note: String,
    git_error: bool,
    git_busy: bool,
    git_tx: Sender<git::GitResult>,
    git_rx: Receiver<git::GitResult>,
    /// Visor de diff abierto.
    diff: Option<DiffState>,
    /// Menú de ramas abierto y sus filas.
    branch_menu: bool,
    branch_items: Vec<IdeMenuEntry>,
    /// Panel del agente: `opencode` en un PTY con su propia pantalla. Canal
    /// aparte para no mezclar la salida con la terminal local.
    agent: Option<LocalSession>,
    agent_tx: Sender<PtyEvent>,
    agent_rx: Receiver<PtyEvent>,
    /// Tamaño actual del panel del agente (columnas × filas).
    agent_cols: u16,
    agent_rows: u16,
    /// `.spasm` sobre el que actúan los menús Ejecutar/Compilar.
    active_file: Option<PathBuf>,
    /// Timer del bucle de drenado.
    timer: Option<Timer>,
}

impl Default for Ide {
    fn default() -> Self {
        Self::new()
    }
}

impl Ide {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let config = Config::load();
        // Si hay un proyecto recordado y sigue existiendo, se arranca ahí.
        let root = config.project().unwrap_or(cwd);
        let recent_projects = load_recent(&config, "recent.projects");
        let recent_files = load_recent(&config, "recent.files");
        let show_left = config.bool("show.left", true);
        let show_right = config.bool("show.right", true);
        let show_bottom = config.bool("show.bottom", true);
        let show_generated = config.bool("explorer.generated", true);
        let tab_size = config.i32("editor.tab-size", 4).clamp(1, 16) as usize;
        let insert_spaces = config.bool("editor.insert-spaces", true);
        let run_entry = config
            .get("run.entry")
            .map(PathBuf::from)
            .filter(|p| p.is_file());
        let run_args = config.get("run.args").unwrap_or("").to_string();
        let run_build_on_save = config.bool("run.build-on-save", false);
        let sans = config
            .get("font.sans")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| pick_font(SANS_FONTS, "Segoe UI"));
        let mono = config
            .get("font.mono")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| pick_font(MONO_FONTS, "Consolas"));
        let editor_font_size = config.f32("font.editor-size", 12.0).clamp(9.0, 24.0);
        let accent = config
            .usize("theme.accent", 0)
            .min(ACCENTS.len().saturating_sub(1));
        // Opacidad de los paneles: el valor guardado se acerca al paso más
        // parecido, para que siempre sea uno de `PANEL_ALPHAS`.
        let saved_alpha = config.f32("theme.panel-alpha", 1.0);
        let panel_alpha = *PANEL_ALPHAS
            .iter()
            .min_by(|a, b| {
                (**a - saved_alpha)
                    .abs()
                    .total_cmp(&(**b - saved_alpha).abs())
            })
            .unwrap_or(&1.0);
        let mut keymap: HashMap<String, String> = KEY_ACTIONS
            .iter()
            .map(|(id, _, d)| (id.to_string(), d.to_string()))
            .collect();
        for (id, _, _) in KEY_ACTIONS {
            if let Some(v) = config.get(&format!("keys.{id}")) {
                if !v.trim().is_empty() {
                    keymap.insert(id.to_string(), v.trim().to_string());
                }
            }
        }
        let docks = [
            Dock::from_code(config.i32("panel.0.dock", 0)),
            Dock::from_code(config.i32("panel.1.dock", 1)),
            Dock::from_code(config.i32("panel.2.dock", 2)),
            Dock::from_code(config.i32("panel.3.dock", 0)),
        ];
        let (local_tx, local_rx) = mpsc::channel();
        let (agent_tx, agent_rx) = mpsc::channel();
        let (lint_tx, lint_rx) = mpsc::channel();
        let (build_tx, build_rx) = mpsc::channel();
        let (git_tx, git_rx) = mpsc::channel();
        let (search_tx, search_rx) = mpsc::channel();
        // Despliegue: el hilo de transferencias (curl/scp) habla por este canal.
        let (deploy_tx, deploy_rx) = mpsc::channel();
        // Módulos: servidor web y visor de SQLite.
        let (web_tx, web_rx) = mpsc::channel();
        let (sqlite_tx, sqlite_rx) = mpsc::channel();
        // En las pruebas no se consulta Git (mantiene los tests rápidos y ajenos
        // al estado del repo; el parseo se cubre en `git::tests`).
        let git_status = if cfg!(test) {
            git::Status::default()
        } else {
            git::status(&root)
        };

        // El sitio de despliegue del proyecto que estaba abierto (el config se
        // mueve más abajo en el literal, así que se lee acá).
        let site = Site::load(&root, &config);

        // El config y la raíz se mueven dentro del literal: los valores de los
        // módulos se leen antes.
        let web = WebConfig::load(&root, &config);
        let sqlite_enabled = config.bool("module.sqlite", false);

        let mut ide = Self {
            toolchain: Toolchain::discover(),
            local: Vec::new(),
            local_active: 0,
            split: None,
            next_local_id: 0,
            local_cols: TERM_COLS,
            local_rows: TERM_ROWS,
            local_tx,
            local_rx,
            cell_w: TERM_CELL_W,
            local_px: (0.0, 0.0),
            agent_px: (0.0, 0.0),
            explorer: Explorer::new(root),
            explorer_selected: -1,
            explorer_context_open: false,
            explorer_context_items: Vec::new(),
            term_context_open: false,
            term_context_items: Vec::new(),
            term_context_target: Target::Local,
            prompt: None,
            ask: None,
            tabs: Vec::new(),
            active_tab: 0,
            recent_projects,
            recent_files,
            show_left,
            show_right,
            show_bottom,
            docks,
            menu_open: String::new(),
            confirm: None,
            confirm_tab: None,
            palette: Palette::default(),
            config,
            code_model: Rc::new(VecModel::default()),
            model_attached: false,
            editor_full: false,
            undo: Vec::new(),
            redo: Vec::new(),
            tab_size,
            insert_spaces,
            run_entry,
            run_args,
            run_build_on_save,
            sans,
            mono,
            editor_font_size,
            accent,
            panel_alpha,
            prefs_open: false,
            keymap,
            capturing_key: None,
            about_open: false,
            version: env!("CARGO_PKG_VERSION").to_string(),
            monitor: sysinfo::Monitor::default(),
            metrics: sysinfo::Metrics::default(),
            drain_tick: 0,
            find: Find::default(),
            search: Search::default(),
            search_tx,
            search_rx,
            search_gen: 0,
            diagnostics: Vec::new(),
            outline: Vec::new(),
            diagnostics_path: None,
            lint_tx,
            lint_rx,
            build_tx,
            build_rx,
            build_session: None,
            site,
            deploy_tx,
            deploy_rx,
            deploy_busy: false,
            web,
            web_server: None,
            web_tx,
            web_rx,
            sqlite_enabled,
            sqlite_open: false,
            sqlite_db: None,
            sqlite_tool: None,
            sqlite_tables: Vec::new(),
            sqlite_selected: 0,
            sqlite_header: String::new(),
            sqlite_lines: Vec::new(),
            sqlite_note: String::new(),
            sqlite_busy: false,
            sqlite_tx,
            sqlite_rx,
            settings_open: false,
            settings_section: 0,
            provider_open: false,
            provider_draft: ProviderDraft::default(),
            provider_note: String::new(),
            relaunch: false,
            providers: crate::providers::load_from(&crate::providers::config_dir()),
            build_note: None,
            git_status,
            git_note: String::new(),
            git_error: false,
            git_busy: false,
            git_tx,
            git_rx,
            diff: None,
            branch_menu: false,
            branch_items: Vec::new(),
            agent: None,
            agent_tx,
            agent_rx,
            agent_cols: AGENT_COLS,
            agent_rows: AGENT_ROWS,
            active_file: None,
            timer: None,
        };
        ide.new_local_session();
        ide.explorer.set_show_generated(show_generated);
        // En las pruebas no se lanza la TUI real (abriría procesos de más); el
        // arranque de `opencode` en un PTY se cubre con `pty::tests`.
        if !cfg!(test) {
            ide.start_agent();
            ide.restore_tabs();
        }
        ide
    }

    /// Reabre las pestañas recordadas (si el archivo sigue existiendo).
    fn restore_tabs(&mut self) {
        let count = self.config.usize("tabs", 0);
        let paths: Vec<PathBuf> = (0..count)
            .filter_map(|i| self.config.get(&format!("tab.{i}")).map(PathBuf::from))
            .collect();
        for path in paths {
            self.open_file(path);
        }
        // Restaura el cursor recordado de cada pestaña.
        for i in 0..self.tabs.len() {
            if let Some(v) = self.config.get(&format!("tab.{i}.cursor")) {
                if let Some((line, col)) = v.split_once(',') {
                    if let (Ok(l), Ok(c)) = (line.parse::<usize>(), col.parse::<usize>()) {
                        let t = &mut self.tabs[i];
                        let last = t.lines.len().saturating_sub(1);
                        t.cursor = Cursor {
                            line: l.min(last),
                            col: c,
                        };
                    }
                }
            }
        }
        if let Some(active) = self
            .config
            .get("active_tab")
            .and_then(|v| v.parse::<usize>().ok())
        {
            if active < self.tabs.len() {
                self.tab_activated(active as i32);
            }
        }
    }

    /// Vuelca a la config el estado que sobrevive entre ejecuciones: geometría
    /// de la ventana, paneles visibles y pestañas abiertas.
    pub fn save_state(&mut self, window: &NuIdeWindow) {
        let size = window.window().size();
        self.config.set("win.w", size.width);
        self.config.set("win.h", size.height);
        let position = window.window().position();
        self.config.set("win.x", position.x);
        self.config.set("win.y", position.y);
        self.config.set("show.left", u8::from(self.show_left));
        self.config.set("show.right", u8::from(self.show_right));
        self.config.set("show.bottom", u8::from(self.show_bottom));
        for (i, dock) in self.docks.iter().enumerate() {
            self.config.set(&format!("panel.{i}.dock"), dock.code());
        }
        self.config.set("tabs", self.tabs.len());
        for (i, tab) in self.tabs.iter().enumerate() {
            self.config.set(&format!("tab.{i}"), tab.path.display());
            self.config.set(
                &format!("tab.{i}.cursor"),
                format!("{},{}", tab.cursor.line, tab.cursor.col),
            );
        }
        self.config.set("active_tab", self.active_tab);
        self.config
            .set("recent.projects", save_recent(&self.recent_projects));
        self.config
            .set("recent.files", save_recent(&self.recent_files));
        self.config.set(
            "explorer.generated",
            u8::from(self.explorer.shows_generated()),
        );
        self.config.set("editor.tab-size", self.tab_size);
        self.config
            .set("editor.insert-spaces", u8::from(self.insert_spaces));
        self.config.set(
            "run.entry",
            self.run_entry
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        self.config.set("run.args", &self.run_args);
        self.config
            .set("run.build-on-save", u8::from(self.run_build_on_save));
        self.config.set("font.sans", &self.sans);
        self.config.set("font.mono", &self.mono);
        self.config.set("font.editor-size", self.editor_font_size);
        self.config.set("theme.accent", self.accent);
        self.config.save();
    }

    /// Aplica la geometría recordada (sin salirse de la pantalla por una
    /// posición vieja).
    pub fn apply_window(&self, window: &NuIdeWindow) {
        let width = self.config.f32("win.w", 0.0);
        let height = self.config.f32("win.h", 0.0);
        if width >= 400.0 && height >= 300.0 {
            window
                .window()
                .set_size(slint::PhysicalSize::new(width as u32, height as u32));
        }
        let x = self.config.f32("win.x", f32::MIN);
        let y = self.config.f32("win.y", f32::MIN);
        if x > -5000.0 && y > -5000.0 {
            window
                .window()
                .set_position(slint::PhysicalPosition::new(x as i32, y as i32));
        }
    }

    /// Estado listo para compartir con los callbacks de Slint.
    pub fn shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new()))
    }
}

// ---------------------------------------------------------------------------
// Arranque
// ---------------------------------------------------------------------------

impl Ide {
    /// Lanza una **terminal pura** en el panel derecho: una shell real en la
    /// raíz del proyecto, como la inferior. `opencode` no se abre solo: se
    /// inyecta cuando el usuario elige *Lanzar opencode* (paleta). Si opencode
    /// se cierra, la terminal queda en la shell.
    fn start_agent(&mut self) {
        let (shell, args) = pty::default_shell();
        let powershell = pty::is_powershell(&shell);
        let name = if powershell {
            "pwsh agente".to_string()
        } else {
            "shell agente".to_string()
        };
        let cwd = self.explorer.root().to_path_buf();
        let mut screen = Screen::new(self.agent_rows, self.agent_cols);
        let pty = match Pty::spawn(
            0,
            &shell,
            &args,
            &cwd,
            self.agent_cols,
            self.agent_rows,
            self.agent_tx.clone(),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                screen.process(format!("no se pudo abrir la shell ({shell}): {e}\r\n").as_bytes());
                None
            }
        };
        let alive = pty.is_some();
        self.agent = Some(LocalSession {
            id: 0,
            name,
            pty,
            screen,
            alive,
            sel: None,
            mouse_button: 3,
            term_model: Rc::new(TermRows::default()),
        });
    }

    /// Inyecta el comando que arranca `opencode` en la shell del panel derecho,
    /// sólo si el usuario lo pidió. Si la shell está muerta, la relanza antes.
    fn launch_agent(&mut self) {
        let Some(bin) = self.toolchain.agent.clone() else {
            return;
        };
        if !self.agent.as_ref().is_some_and(|a| a.alive) {
            self.agent = None;
            self.start_agent();
        }
        // Con comillas, para que funcione aunque la ruta tenga espacios. En
        // PowerShell hace falta el operador de llamada `&`.
        let (shell, _) = pty::default_shell();
        let command = if pty::is_powershell(&shell) {
            format!("& '{}'\r", bin.display())
        } else {
            format!("\"{}\"\r", bin.display())
        };
        self.write_to(Target::Agent, command.as_bytes());
    }

    /// Abre el diálogo nativo para elegir la carpeta del proyecto y la fija como
    /// raíz del explorador.
    fn pick_project(&mut self) {
        let start = self.explorer.root().to_path_buf();
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Elegir carpeta del proyecto")
            .set_directory(&start)
            .pick_folder()
        {
            self.set_project(dir);
        }
    }

    /// Abre el diálogo nativo para elegir un archivo y lo abre en una pestaña.
    fn pick_file(&mut self) {
        let start = self.explorer.root().to_path_buf();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Abrir archivo")
            .set_directory(&start)
            .pick_file()
        {
            self.open_file(path);
        }
    }

    /// Crea un archivo SpASM nuevo (diálogo de guardar) y lo abre. Si la ruta ya
    /// existe, no la pisa: sólo la abre.
    fn new_file(&mut self) {
        let body = templates::by_id("desarrollo").map(|t| t.body).unwrap_or("");
        self.new_file_with(body);
    }

    /// Proyectos y archivos recientes, en el orden de la lista de la bienvenida
    /// (primero proyectos, luego archivos). El índice es el que manda la UI.
    fn recent_entries(&self) -> Vec<RecentEntry> {
        self.recent_projects
            .iter()
            .cloned()
            .map(RecentEntry::Project)
            .chain(self.recent_files.iter().cloned().map(RecentEntry::File))
            .collect()
    }

    /// Abre una entrada de "Recientes" de la bienvenida.
    fn open_recent(&mut self, i: i32) {
        if i < 0 {
            return;
        }
        match self.recent_entries().into_iter().nth(i as usize) {
            Some(RecentEntry::Project(dir)) => self.set_project(dir),
            Some(RecentEntry::File(path)) => self.open_file(path),
            None => {}
        }
    }

    // --- Git -----------------------------------------------------------------

    /// Lanza una operación de Git en segundo plano (una por vez).
    fn git_spawn(&mut self, op: git::Op) {
        if self.git_busy {
            return;
        }
        self.git_busy = true;
        self.git_error = false;
        self.git_note = "…".to_string();
        git::spawn(self.explorer.root().to_path_buf(), op, self.git_tx.clone());
    }

    /// Relee el estado del repositorio.
    fn git_refresh(&mut self) {
        self.git_spawn(git::Op::Refresh);
    }

    /// Crea el repositorio (`git init`) en la raíz del proyecto.
    fn git_init(&mut self) {
        self.git_spawn(git::Op::Init);
    }

    /// Prepara todos los cambios (`git add -A`).
    fn git_stage_all(&mut self) {
        self.git_spawn(git::Op::StageAll);
    }

    /// Prepara (`stage`) o quita del índice un archivo, según su estado actual.
    fn git_toggle(&mut self, path: String, staged: bool) {
        self.git_spawn(git::Op::Toggle {
            path,
            stage: !staged,
        });
    }

    /// Crea un commit con el mensaje dado (si no está vacío).
    fn git_commit(&mut self, message: String) {
        let message = message.trim().to_string();
        if message.is_empty() {
            return;
        }
        self.git_spawn(git::Op::Commit(message));
    }

    /// Sube los commits al remoto (`git push`).
    fn git_push(&mut self) {
        self.git_spawn(git::Op::Push);
    }

    /// Abre en el editor un archivo del panel de Git (ruta relativa a la raíz).
    fn git_open(&mut self, path: &str) {
        self.open_file(self.explorer.root().join(path));
    }

    /// Sube/quita del índice todos los cambios.
    fn git_unstage_all(&mut self) {
        self.git_spawn(git::Op::UnstageAll);
    }

    /// `git pull`.
    fn git_pull(&mut self) {
        self.git_spawn(git::Op::Pull);
    }

    /// `git commit --amend` con el mensaje actual (o `--no-edit` si está vacío).
    fn git_amend(&mut self, message: String) {
        self.git_spawn(git::Op::Amend(message.trim().to_string()));
    }

    /// Abre el visor de diff de un archivo del panel de Git.
    fn git_diff(&mut self, path: &str, staged: bool, untracked: bool) {
        let lines = parse_diff_lines(&git::diff(self.explorer.root(), path));
        let note = if lines.is_empty() {
            "Sin cambios respecto de HEAD.".to_string()
        } else {
            String::new()
        };
        self.diff = Some(DiffState {
            path: path.to_string(),
            staged,
            untracked,
            lines,
            note,
            error: false,
        });
    }

    /// Cierra el visor de diff.
    fn git_close_diff(&mut self) {
        self.diff = None;
    }

    /// `git diff` del archivo activo (si está dentro del repo). Comando de paleta.
    fn git_diff_active(&mut self) {
        let Some(active) = self.active_file.clone() else {
            return;
        };
        let Ok(rel) = active.strip_prefix(self.explorer.root()) else {
            return;
        };
        let path = rel.to_string_lossy().replace('\\', "/");
        let change = self
            .git_status
            .changes
            .iter()
            .find(|c| c.path == path)
            .cloned();
        let (staged, untracked) = match change {
            Some(c) => (c.staged(), c.index == '?'),
            None => (false, false),
        };
        self.git_diff(&path, staged, untracked);
    }

    /// Preparar o quitar del índice el archivo del diff abierto.
    fn git_diff_toggle_stage(&mut self) {
        let Some(d) = self.diff.as_ref() else {
            return;
        };
        let (path, staged, untracked) = (d.path.clone(), d.staged, d.untracked);
        if untracked {
            // Un archivo nuevo no tiene nada que quitar del índice.
            if staged {
                self.git_spawn(git::Op::Toggle {
                    path: path.clone(),
                    stage: false,
                });
            } else {
                self.git_spawn(git::Op::Toggle {
                    path: path.clone(),
                    stage: true,
                });
            }
        } else {
            self.git_spawn(git::Op::Toggle {
                path: path.clone(),
                stage: !staged,
            });
        }
    }

    /// Pide confirmación para descartar los cambios del archivo del diff.
    fn git_diff_discard(&mut self) {
        let Some(d) = self.diff.as_ref() else {
            return;
        };
        self.ask = Some(Ask::Discard {
            path: d.path.clone(),
            untracked: d.untracked,
        });
    }

    /// Abre el menú de ramas (locales + "Nueva rama…").
    fn git_branch_menu(&mut self) {
        let branches = git::branches(self.explorer.root());
        let mut items: Vec<IdeMenuEntry> = branches
            .iter()
            .map(|(name, current)| {
                let label = if *current {
                    format!("● {name}")
                } else {
                    format!("  {name}")
                };
                menu_entry(&format!("branch.switch:{name}"), &label, "", true)
            })
            .collect();
        items.push(menu_sep());
        items.push(menu_entry("branch.new", "Nueva rama…", "", true));
        self.branch_items = items;
        self.branch_menu = true;
    }

    /// Ejecuta una acción del menú de ramas.
    fn git_branch_command(&mut self, id: &str) {
        self.branch_menu = false;
        if id == "branch.new" {
            self.prompt = Some(Prompt {
                kind: PromptKind::NewBranch,
                target: PathBuf::new(),
                text: String::new(),
            });
        } else if let Some(name) = id.strip_prefix("branch.switch:") {
            let current = self.git_status.branch.clone();
            if name != current {
                self.git_spawn(git::Op::SwitchBranch(name.to_string()));
            }
        }
    }

    /// Fija `dir` como raíz: reconstruye el explorador, reinicia el agente en la
    /// carpeta nueva y recuerda la elección.
    fn set_project(&mut self, dir: PathBuf) {
        if !dir.is_dir() || dir == self.explorer.root() {
            return;
        }
        push_recent(&mut self.recent_projects, dir.clone());
        self.explorer.set_root(dir.clone());
        // El estado de Git es por proyecto: se relee al cambiar de carpeta.
        self.git_status = git::status(&dir);
        // El sitio de despliegue también es por proyecto.
        self.site = Site::load(&dir, &self.config);
        self.git_note.clear();
        self.git_error = false;
        self.config.set("project", dir.display());
        self.config.save();
        // El agente corre en la raíz del proyecto: se le cambia la carpeta sin
        // reiniciarlo (opencode tiene el comando `/cd`).
        self.retarget_agent(&dir);
        // Las migas de pan dependen de la raíz; se rehace el editor.
        self.editor_full = true;
    }

    /// Reinicia la terminal del panel derecho en la carpeta nueva: la shell
    /// arranca en el cwd del proyecto. opencode no se relanza (se lanza sólo
    /// bajo demanda con *Lanzar opencode*).
    fn retarget_agent(&mut self, _dir: &Path) {
        self.agent = None;
        self.start_agent();
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
            alive: true,
            sel: None,
            mouse_button: 3,
            term_model: Rc::new(TermRows::default()),
        });
        self.local_active = self.local.len() - 1;
    }

    /// Abre una terminal **elevada** en su propia ventana (UAC), en la carpeta
    /// del proyecto. No toca la IDE: sirve para un comando suelto que pide admin.
    fn new_admin_terminal(&mut self) {
        if elevate::is_elevated() {
            self.admin_note(
                "NuIde ya corre como administrador: las terminales del panel ya son elevadas.",
            );
            return;
        }
        let (shell, args) = pty::default_shell();
        let dir = self.explorer.root().to_path_buf();
        if let Err(e) = elevate::run_as_admin(&shell, &args, Some(&dir)) {
            if !elevate::cancelled(&e) {
                self.admin_note(&format!("no se pudo abrir la terminal elevada: {e}"));
            }
        }
    }

    /// Pide reiniciar toda la IDE elevada: así las terminales **embebidas** del
    /// panel corren como administrador. Con cambios sin guardar, pasa por el
    /// diálogo de confirmación.
    fn request_relaunch_elevated(&mut self) {
        if elevate::is_elevated() {
            self.admin_note("NuIde ya corre como administrador.");
            return;
        }
        if self.has_dirty() {
            self.confirm = Some(Confirm::RelaunchElevated);
            self.confirm_tab = None;
        } else {
            self.relaunch = true;
        }
    }

    /// Si quedó pendiente un reinicio elevado: guarda el estado, lanza la copia
    /// elevada y cierra esta instancia. Devuelve `true` si hay que salir.
    fn take_relaunch(&mut self, w: &NuIdeWindow) -> bool {
        if !self.relaunch {
            return false;
        }
        self.relaunch = false;
        self.save_state(w);
        match elevate::relaunch_as_admin() {
            Ok(()) => {
                let _ = slint::quit_event_loop();
                true
            }
            Err(e) => {
                if !elevate::cancelled(&e) {
                    self.admin_note(&format!("no se pudo reiniciar como administrador: {e}"));
                }
                false
            }
        }
    }

    /// Deja un mensaje avisando algo en la terminal local activa (si no hay, en la
    /// del agente), con el mismo formato que los avisos de arranque de shell.
    fn admin_note(&mut self, text: &str) {
        let msg = format!("\r\n\x1b[33m[nuide] {text}\x1b[0m\r\n");
        if let Some(s) = self.local.get_mut(self.local_active) {
            s.screen.process(msg.as_bytes());
        } else if let Some(a) = self.agent.as_mut() {
            a.screen.process(msg.as_bytes());
        }
    }
}

// ---------------------------------------------------------------------------
// Entrada y comandos
// ---------------------------------------------------------------------------

impl Ide {
    /// Teclas de la terminal local. `true` si hay que volcar toda la ventana.
    fn local_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) -> bool {
        self.pty_key(Target::Local, text, control, shift, alt)
    }

    /// Teclas del panel del agente. `true` si hay que volcar toda la ventana.
    fn agent_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) -> bool {
        self.pty_key(Target::Agent, text, control, shift, alt)
    }

    /// Teclado compartido por las dos terminales. Devuelve `true` cuando la tecla
    /// tocó algo fuera de la terminal (diálogo, paleta, búsqueda, atajo global),
    /// en cuyo caso hace falta el vuelcado completo; si no, alcanza con
    /// `sync_terms`.
    ///
    /// Antes de mandar las teclas al PTY se atienden los atajos que no son del
    /// proceso: `Ctrl+Shift+C/V` copian/pegan la selección y `Shift+PageUp/Down`
    /// recorren el scrollback. Cualquier otra tecla vuelve la vista al final.
    fn pty_key(
        &mut self,
        target: Target,
        text: &str,
        control: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        // Diálogo de entrada (modal), paleta/búsqueda abiertas, captura de atajo
        // y atajos globales configurables; después, las teclas del proceso.
        if self.provider_intercept(text, control) {
            return true;
        }
        if self.prompt_intercept(text, control) {
            return true;
        }
        if self.palette_intercept(text, control, shift) {
            return true;
        }
        if self.search_intercept(text, control, shift) {
            return true;
        }
        if self.capture_key(text, control, shift, alt) {
            return true;
        }
        if let Some(action) = self.keymap_action(text, control, shift, alt) {
            self.run_key_action(&action);
            return true;
        }
        if control && shift {
            match text.to_ascii_lowercase().as_str() {
                "c" => {
                    self.copy_selection(target);
                    return false;
                }
                "v" => {
                    self.paste_into(target);
                    return false;
                }
                _ => {}
            }
        }
        if shift && matches!(text, "\u{F72c}" | "\u{F72d}") {
            let half = (self.dimensions(target).1 / 2).max(1) as i32;
            let delta = if text == "\u{F72c}" { half } else { -half };
            self.scroll_session(target, delta);
            return false;
        }
        // Cualquier tecla normal abandona el historial y la selección.
        if let Some(session) = self.session_mut(target) {
            session.sel = None;
            session.screen.scroll_to_end();
        }
        if let Some(bytes) = encode_key(text, control, shift, alt) {
            self.write_to(target, &bytes);
        }
        false
    }

    /// Escribe bytes en el PTY de la terminal indicada.
    fn write_to(&mut self, target: Target, bytes: &[u8]) {
        if let Some(session) = self.session_mut(target) {
            if let Some(pty) = session.pty.as_mut() {
                let _ = pty.write(bytes);
            }
        }
    }

    /// Tamaño actual (columnas × filas) de la terminal indicada.
    fn dimensions(&self, target: Target) -> (u16, u16) {
        match target {
            Target::Local => (self.local_cols, self.local_rows),
            Target::Agent => (self.agent_cols, self.agent_rows),
        }
    }

    /// Sesión de la terminal indicada (la local es la activa).
    fn session(&self, target: Target) -> Option<&LocalSession> {
        match target {
            Target::Local => self.local.get(self.local_active),
            Target::Agent => self.agent.as_ref(),
        }
    }

    fn session_mut(&mut self, target: Target) -> Option<&mut LocalSession> {
        match target {
            Target::Local => self.local.get_mut(self.local_active),
            Target::Agent => self.agent.as_mut(),
        }
    }

    /// Busca la sesión por su id dentro de la terminal indicada: los eventos del
    /// PTY llegan con el id de la sesión.
    fn session_by_id_mut(&mut self, target: Target, id: u64) -> Option<&mut LocalSession> {
        match target {
            Target::Local => self.local.iter_mut().find(|s| s.id == id),
            Target::Agent => self.agent.as_mut().filter(|s| s.id == id),
        }
    }

    /// Traduce una `(y, x)` en píxeles (relativas a la pantalla de la terminal)
    /// a la celda más cercana. Usa las mismas métricas de celda que el PTY.
    fn term_cell(&self, target: Target, y: f32, x: f32) -> (u16, u16) {
        let (cols, rows) = self.dimensions(target);
        let rows = rows.max(1) as i64;
        let cols = cols.max(1) as i64;
        let row = ((y.max(0.0) / TERM_CELL_H).floor() as i64).clamp(0, rows - 1) as u16;
        let col = ((x.max(0.0) / self.cell_w.max(1.0)).floor() as i64).clamp(0, cols - 1) as u16;
        (row, col)
    }

    /// ¿La sesión `index` (de la terminal local) pide *mouse reporting*?
    fn session_mouse(&self, index: usize) -> bool {
        self.local
            .get(index)
            .map(|s| s.screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None)
            .unwrap_or(false)
    }

    /// Reenvía un evento de mouse al proceso si la app lo pidió (mouse
    /// reporting). Si no, no hace nada (la UI ya usa el mouse para seleccionar).
    fn forward_mouse(&mut self, target: Target, kind: MouseKind, y: f32, x: f32) {
        let (mode, encoding, held) = {
            let Some(s) = self.session(target) else {
                return;
            };
            (
                s.screen.mouse_protocol_mode(),
                s.screen.mouse_protocol_encoding(),
                s.mouse_button,
            )
        };
        if mode == vt100::MouseProtocolMode::None {
            return;
        }
        // ¿Corresponde reportar este evento según el modo activo?
        let report = match (mode, kind) {
            (vt100::MouseProtocolMode::None, _) => false,
            (vt100::MouseProtocolMode::Press, MouseKind::Press(_) | MouseKind::Wheel(_)) => true,
            (vt100::MouseProtocolMode::Press, _) => false,
            (
                vt100::MouseProtocolMode::PressRelease,
                MouseKind::Press(_) | MouseKind::Release(_) | MouseKind::Wheel(_),
            ) => true,
            (vt100::MouseProtocolMode::PressRelease, MouseKind::Move) => false,
            (vt100::MouseProtocolMode::ButtonMotion, MouseKind::Move) => held != 3,
            (vt100::MouseProtocolMode::ButtonMotion, _) => true,
            (vt100::MouseProtocolMode::AnyMotion, _) => true,
        };
        if !report {
            return;
        }
        let (row, col) = self.term_cell(target, y, x);
        let (code, release) = match kind {
            MouseKind::Press(b) => (b as u32, false),
            MouseKind::Release(b) => ((if b == 3 { held } else { b }) as u32, true),
            MouseKind::Move => (32 + held as u32, false),
            MouseKind::Wheel(up) => (if up { 64 } else { 65 }, false),
        };
        if let Some(s) = self.session_mut(target) {
            match kind {
                MouseKind::Press(b) => s.mouse_button = b,
                MouseKind::Release(_) => s.mouse_button = 3,
                _ => {}
            }
        }
        let bytes = encode_mouse(encoding, code, col as u32, row as u32, release);
        self.write_to(target, &bytes);
    }

    /// Empieza (o reinicia) una selección con el mouse.
    fn select_start(&mut self, target: Target, y: f32, x: f32) {
        let cell = self.term_cell(target, y, x);
        if let Some(session) = self.session_mut(target) {
            session.sel = Some(TermSel {
                anchor: cell,
                head: cell,
            });
        }
    }

    /// Extiende la selección hasta la celda bajo el mouse.
    fn select_update(&mut self, target: Target, y: f32, x: f32) {
        let cell = self.term_cell(target, y, x);
        if let Some(session) = self.session_mut(target) {
            if let Some(sel) = session.sel.as_mut() {
                sel.head = cell;
            }
        }
    }

    /// Cierra la selección y, si quedó texto, lo copia al portapapeles.
    fn select_end(&mut self, target: Target) {
        let text = self.selected_text(target);
        if !text.is_empty() {
            let _ = clipboard_write(&text);
        }
    }

    /// Texto de la selección activa, sin espacios sobrantes al final de cada
    /// línea. `String::new()` si no hay selección.
    fn selected_text(&self, target: Target) -> String {
        let Some(session) = self.session(target) else {
            return String::new();
        };
        let Some((sr, sc, er, ec)) = session.sel.and_then(|s| s.region(session.screen.size().1))
        else {
            return String::new();
        };
        session
            .screen
            .text_between(sr, sc, er, ec)
            .split('\n')
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }

    /// Copia la selección actual, si la hay.
    fn copy_selection(&mut self, target: Target) {
        let text = self.selected_text(target);
        if !text.is_empty() {
            let _ = clipboard_write(&text);
        }
    }

    /// Manda el portapapeles al PTY indicado (como si se tipeara).
    fn paste_into(&mut self, target: Target) {
        if let Some(text) = read_clipboard() {
            if !text.is_empty() {
                let text = text.replace("\r\n", "\r").replace('\n', "\r");
                self.write_to(target, text.as_bytes());
            }
        }
    }

    /// Recorre el scrollback de la terminal indicada (`delta` positivo = más
    /// atrás).
    fn scroll_session(&mut self, target: Target, delta: i32) {
        if let Some(session) = self.session_mut(target) {
            session.sel = None;
            session.screen.scroll_by(delta);
        }
    }

    /// Rueda del mouse: `dy` es el delta en píxeles (positivo = hacia abajo).
    fn wheel(&mut self, target: Target, dy: f32) {
        let lines = (-dy / TERM_CELL_H).round() as i32;
        let lines = if lines == 0 {
            if dy > 0.0 {
                -1
            } else {
                1
            }
        } else {
            lines
        };
        self.scroll_session(target, lines);
    }

    /// Corre el toolchain en segundo plano (una cadena de pasos) y muestra la
    /// salida en la sesión "build" de la terminal.
    fn spawn_build(&mut self, steps: Vec<build::Step>) {
        let cwd = self.explorer.root().to_path_buf();
        let index = self.ensure_build_session();
        // La salida queda a la vista.
        self.local_active = index;
        self.resize_local();
        build::spawn(steps, cwd, self.build_tx.clone());
    }

    /// Crea (o reutiliza) la sesión "build" de la terminal. Devuelve su índice.
    fn ensure_build_session(&mut self) -> usize {
        if let Some(id) = self.build_session {
            if let Some(index) = self.local.iter().position(|s| s.id == id) {
                return index;
            }
        }
        let id = self.next_local_id;
        self.next_local_id += 1;
        let screen = Screen::new(self.local_rows, self.local_cols);
        self.local.push(LocalSession {
            id,
            name: "build".to_string(),
            pty: None,
            screen,
            alive: true,
            sel: None,
            mouse_button: 3,
            term_model: Rc::new(TermRows::default()),
        });
        self.build_session = Some(id);
        self.local.len() - 1
    }

    /// Escribe en la pantalla de la sesión "build" (limpiándola antes).
    fn write_build_output(&mut self, text: &str) {
        let Some(id) = self.build_session else {
            return;
        };
        let Some(session) = self.local.iter_mut().find(|s| s.id == id) else {
            return;
        };
        session.screen.process(b"\x1b[2J\x1b[H");
        // El proceso usa '\n'; la terminal espera '\r\n'.
        let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
        session.screen.process(normalized.as_bytes());
    }

    /// Ejecuta el archivo de entrada (o el activo) **compilado**: lo transpila a
    /// C (`spasmc.py`), lo compila a binario nativo y corre el binario con los
    /// argumentos configurados. El intérprete de SpASM ya no participa; la
    /// salida se captura en la sesión "build".
    fn run(&mut self, _arg: Option<&str>) {
        let Some(file) = self.run_entry.clone().or_else(|| self.active_file.clone()) else {
            return;
        };
        self.open_file(file.clone());
        let (mut steps, exe, note) = self.compile_binary_steps(&file);
        if steps.is_empty() {
            // Sin `spasmc.py`/Python no hay nada que correr.
            self.write_build_output(&format!("{}\n", note.unwrap_or_default()));
            self.local_active = self.ensure_build_session();
            self.resize_local();
            return;
        }
        let Some(exe) = exe else {
            // Sin compilador de C el `.c` se generó pero no hay binario.
            self.write_build_output(&format!("{}\n", note.unwrap_or_default()));
            self.local_active = self.ensure_build_session();
            self.resize_local();
            return;
        };
        steps.push(build::Step {
            program: exe,
            args: shell_split(&self.run_args),
        });
        self.spawn_build(steps);
    }

    /// Elige el archivo de entrada de F5 con el diálogo nativo.
    fn run_pick_entry(&mut self) {
        let start = self.explorer.root().to_path_buf();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Archivo de entrada para Ejecutar")
            .set_directory(&start)
            .add_filter("SpASM", &["spasm", "asm"])
            .pick_file()
        {
            self.config.set("run.entry", path.display());
            self.config.save();
            self.run_entry = Some(path);
        }
    }

    /// Vuelve a "usar el archivo activo" como entrada.
    fn run_use_active(&mut self) {
        self.run_entry = None;
        self.config.set("run.entry", "");
        self.config.save();
    }

    /// Edita los argumentos extra de la ejecución.
    fn run_edit_args(&mut self) {
        self.prompt = Some(Prompt {
            kind: PromptKind::RunArgs,
            target: PathBuf::new(),
            text: self.run_args.clone(),
        });
    }

    /// Activa o desactiva compilar al guardar.
    fn run_toggle_build_on_save(&mut self) {
        self.run_build_on_save = !self.run_build_on_save;
        self.config
            .set("run.build-on-save", u8::from(self.run_build_on_save));
        self.config.save();
    }

    /// Crea un archivo nuevo con el cuerpo de una plantilla.
    fn new_file_with(&mut self, body: &str) {
        let start = self.explorer.root().to_path_buf();
        let path = match rfd::FileDialog::new()
            .set_title("Nuevo archivo")
            .set_directory(&start)
            .set_file_name("nuevo.spasm")
            .save_file()
        {
            Some(p) => p,
            None => return,
        };
        if !path.exists() {
            if std::fs::write(&path, body).is_err() {
                return;
            }
            self.explorer.rebuild();
        }
        self.open_file(path);
    }

    /// Crea un proyecto mínimo en una carpeta elegida y lo abre.
    fn new_project(&mut self) {
        let start = self.explorer.root().to_path_buf();
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Carpeta del nuevo proyecto")
            .set_directory(&start)
            .pick_folder()
        else {
            return;
        };
        let src = dir.join("src");
        let _ = std::fs::create_dir_all(&src);
        let main = src.join("main.spasm");
        if !main.exists() {
            let body = templates::by_id("desarrollo").map(|t| t.body).unwrap_or("");
            if std::fs::write(&main, body).is_err() {
                return;
            }
        }
        self.set_project(dir);
        self.open_file(main);
    }

    /// Abre la ayuda de sintaxis de SpASM en la paleta.
    fn spasm_help(&mut self) {
        self.palette_open(PaletteKind::Help);
    }

    /// Transpila el archivo activo a C con `spasmc.py` y lo compila a binario con
    /// el compilador de C nativo (si hay). Con `run_after`, corre el binario.
    fn build(&mut self, run_after: bool) {
        let Some(file) = self.active_file.clone() else {
            return;
        };
        self.open_file(file.clone());
        let (mut steps, exe, note) = self.compile_binary_steps(&file);
        if steps.is_empty() {
            // Sin `spasmc.py`/Python no hay nada que compilar.
            self.build_note = note;
            return;
        }
        let Some(exe) = exe else {
            self.build_note = if run_after {
                Some(
                    "No hay compilador de C: no se pudo generar ni correr el binario. \
                     Instalá gcc/clang o MSVC, o definí NUIDE_CC."
                        .to_string(),
                )
            } else {
                note
            };
            self.spawn_build(steps);
            return;
        };
        if run_after {
            steps.push(build::Step {
                program: exe,
                args: Vec::new(),
            });
        }
        self.build_note = None;
        self.spawn_build(steps);
    }

    /// Transpila `file` a C (`spasmc.py --target c`) y lo compila a binario
    /// nativo con el compilador de C detectado (gcc/clang o MSVC). Devuelve los
    /// pasos listos para [`Self::spawn_build`], la ruta del binario (si se pudo
    /// generar) y una nota con el problema si faltó eslabón de la cadena.
    fn compile_binary_steps(
        &self,
        file: &Path,
    ) -> (Vec<build::Step>, Option<PathBuf>, Option<String>) {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string());
        let out_dir = self.explorer.root().join("build");
        let _ = std::fs::create_dir_all(&out_dir);
        let out_c = out_dir.join(format!("{stem}.c"));
        let out_exe = out_dir.join(format!("{stem}.exe"));

        let Ok((python, args)) = self.toolchain.transpile_command(file, &out_c) else {
            return (
                Vec::new(),
                None,
                Some("no se encontró el compilador spasmc.py (definí NUIDE_SPASMC)".to_string()),
            );
        };
        let steps = vec![build::Step {
            program: python,
            args,
        }];
        match self.toolchain.native_command(&out_c, &out_exe) {
            Some((cc, cc_args)) => {
                let mut steps = steps;
                steps.push(build::Step {
                    program: cc,
                    args: cc_args,
                });
                (steps, Some(out_exe), None)
            }
            None => (
                steps,
                None,
                Some(
                    "No hay compilador de C: se generó el .c pero no el binario. \
                     Instalá gcc/clang o MSVC, o definí NUIDE_CC."
                        .to_string(),
                ),
            ),
        }
    }

    /// Mueve el cursor a una línea (1-based) del archivo activo.
    fn goto_line(&mut self, line: i32) {
        if self.tabs.is_empty() || line < 1 {
            return;
        }
        let last = self.tabs[self.active_tab].lines.len().saturating_sub(1);
        let new = (line as usize - 1).min(last);
        let old = self.tabs[self.active_tab].cursor.line;
        self.set_cursor(Cursor { line: new, col: 0 }, old, false);
    }

    /// Ir a la definición del identificador bajo el cursor (F12): busca el
    /// símbolo con ese nombre en el esquema y salta a su línea.
    fn go_to_definition(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(word) = word_under_cursor(tab) else {
            return;
        };
        if let Some(symbol) = self.outline.iter().find(|s| s.name == word) {
            let line = symbol.line as i32;
            self.goto_line(line);
        }
    }

    /// Salta al diagnóstico siguiente o anterior (F8 / Shift+F8), circular.
    fn diagnostic_step(&mut self, forward: bool) {
        let lines: Vec<usize> = self
            .diagnostics
            .iter()
            .map(|d| d.line)
            .filter(|l| *l > 0)
            .collect();
        if lines.is_empty() {
            return;
        }
        let cursor = self.tabs[self.active_tab].cursor.line + 1; // 1-based
        let target = if forward {
            lines
                .iter()
                .find(|l| **l > cursor)
                .copied()
                .unwrap_or(lines[0])
        } else {
            lines
                .iter()
                .rev()
                .find(|l| **l < cursor)
                .copied()
                .unwrap_or_else(|| *lines.last().unwrap())
        };
        let old = self.tabs[self.active_tab].cursor.line;
        let new = (target - 1).min(self.tabs[self.active_tab].lines.len().saturating_sub(1));
        self.set_cursor(Cursor { line: new, col: 0 }, old, false);
    }

    /// Acciones del menú superior.
    fn menu(&mut self, label: &str) {
        match label {
            "Ejecutar" => self.run(None),
            "Compilar" => self.build(false),
            // Archivo/Editar/Ver/Ayuda abren su desplegable (y lo cierran si ya está).
            "Archivo" | "Editar" | "Ver" | "Ayuda" => {
                self.menu_open = if self.menu_open == label {
                    String::new()
                } else {
                    label.to_string()
                };
            }
            _ => {}
        }
    }

    /// Muestra u oculta una región. `kind`: 0 izquierda, 1 derecha, 2 inferior.
    fn toggle_region(&mut self, kind: i32) {
        match kind {
            0 => self.show_left = !self.show_left,
            1 => self.show_right = !self.show_right,
            2 => self.show_bottom = !self.show_bottom,
            _ => {}
        }
    }

    /// Cambia un panel de dock (izquierda → derecha → abajo → oculto). `kind`:
    /// 0 Explorador, 1 OpenCode, 2 Terminal, 3 Control de código.
    fn cycle_dock(&mut self, kind: i32) {
        if let Some(dock) = self.docks.get_mut(kind as usize) {
            *dock = dock.next();
        }
    }

    /// Soltó un panel arrastrado de su cabecera: lo mueve a la zona elegida.
    /// `zona`: 0 izquierda, 1 derecha, 2 abajo, 3 ocultar. Si la región estaba
    /// apagada se enciende, porque si no el panel quedaría invisible y el gesto
    /// parecería perdido. Se persiste con `save_state` (`panel.<i>.dock`).
    fn dock_drop(&mut self, panel: i32, zone: i32) {
        let Some(dock) = self.docks.get_mut(panel as usize) else {
            return;
        };
        *dock = match zone {
            0 => {
                self.show_left = true;
                Dock::Left
            }
            1 => {
                self.show_right = true;
                Dock::Right
            }
            2 => {
                self.show_bottom = true;
                Dock::Bottom
            }
            _ => Dock::Hidden,
        };
    }

    /// Cierra el menú desplegable abierto.
    fn close_menu(&mut self) {
        self.menu_open.clear();
    }
}

// ---------------------------------------------------------------------------
// Paleta de comandos / buscador de archivos
// ---------------------------------------------------------------------------

impl Ide {
    /// Abre la paleta (comandos o archivos) y recalcula sus filas.
    fn palette_open(&mut self, kind: PaletteKind) {
        self.palette.open = true;
        self.palette.kind = kind;
        self.palette.query.clear();
        self.palette.selected = 0;
        if kind == PaletteKind::Files {
            self.palette.files = collect_files(self.explorer.root());
        }
        if kind == PaletteKind::Help {
            // Arranca con la palabra bajo el cursor (ayuda contextual).
            if let Some(word) = self.tabs.get(self.active_tab).and_then(word_under_cursor) {
                self.palette.query = word;
            }
        }
        self.palette_recompute();
    }

    /// Cierra la paleta.
    fn palette_close(&mut self) {
        self.palette.open = false;
    }

    /// Recalcula las filas según el tipo y la consulta.
    fn palette_recompute(&mut self) {
        let root = self.explorer.root().to_path_buf();
        let q = self.palette.query.to_lowercase();
        self.palette.rows = match self.palette.kind {
            PaletteKind::Commands => filter_commands(&self.palette.query)
                .into_iter()
                .map(|c| PaletteRow {
                    label: c.label().to_string(),
                    detail: c.hint().to_string(),
                    action: PaletteAction::Command(c),
                })
                .collect(),
            PaletteKind::Files => {
                let mut matches: Vec<(usize, usize, PathBuf)> = self
                    .palette
                    .files
                    .iter()
                    .filter_map(|p| {
                        let rel = p.strip_prefix(&root).unwrap_or(p);
                        let rel_low = rel.to_string_lossy().to_lowercase();
                        if q.is_empty() {
                            Some((usize::MAX, rel_low.len(), p.clone()))
                        } else {
                            rel_low.find(&q).map(|pos| (pos, rel_low.len(), p.clone()))
                        }
                    })
                    .collect();
                // Primero por dónde aparece el match, después por ruta más corta.
                matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                matches
                    .into_iter()
                    .take(50)
                    .map(|(_, _, p)| {
                        let rel = p.strip_prefix(&root).unwrap_or(&p);
                        let detail = rel
                            .parent()
                            .map(|d| d.to_string_lossy().into_owned())
                            .filter(|d| !d.is_empty())
                            .unwrap_or_else(|| ".".to_string());
                        PaletteRow {
                            label: file_name(&p),
                            detail,
                            action: PaletteAction::File(p),
                        }
                    })
                    .collect()
            }
            PaletteKind::GoToLine => match self.palette.query.trim().parse::<usize>() {
                Ok(line) if line >= 1 => vec![PaletteRow {
                    label: format!("Ir a la línea {line}"),
                    detail: String::new(),
                    action: PaletteAction::GoToLine(line),
                }],
                _ => Vec::new(),
            },
            PaletteKind::Help => {
                let q = self.palette.query.to_lowercase();
                highlight::spasm_words()
                    .iter()
                    .filter(|w| q.is_empty() || w.to_lowercase().contains(&q))
                    .map(|w| PaletteRow {
                        label: (*w).to_string(),
                        detail: "SpASM".to_string(),
                        action: PaletteAction::InsertWord((*w).to_string()),
                    })
                    .collect()
            }
        };
        let len = self.palette.rows.len();
        self.palette.selected = if len == 0 {
            0
        } else {
            self.palette.selected.min(len - 1)
        };
    }

    /// Mueve la selección `delta` filas (con tope en los extremos).
    fn palette_move(&mut self, delta: i32) {
        let len = self.palette.rows.len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.palette.selected as i32 + delta).clamp(0, len - 1);
        self.palette.selected = next as usize;
    }

    /// Tecla con la paleta abierta. Devuelve si consumió la tecla.
    fn palette_key(&mut self, text: &str, control: bool) -> bool {
        match text {
            "\u{1b}" => self.palette_close(),
            "\n" | "\r" => self.palette_accept(),
            "\u{F700}" => self.palette_move(-1),
            "\u{F701}" => self.palette_move(1),
            "\u{8}" => {
                let _ = self.palette.query.pop();
                self.palette_recompute();
            }
            _ => {
                if !control && !text.is_empty() && !text.starts_with('\u{F7}') {
                    // "Ir a línea" sólo acepta dígitos.
                    if self.palette.kind == PaletteKind::GoToLine
                        && !text.chars().all(|c| c.is_ascii_digit())
                    {
                        // Se ignora lo que no sea número.
                    } else {
                        self.palette.query.push_str(text);
                        self.palette_recompute();
                    }
                }
            }
        }
        true
    }

    /// Atajo que abre la paleta, o desvío de teclas mientras está abierta.
    /// Devuelve `true` si la tecla ya se atendió.
    fn palette_intercept(&mut self, text: &str, control: bool, shift: bool) -> bool {
        if self.palette.open {
            return self.palette_key(text, control);
        }
        let _ = shift;
        false
    }

    /// Ejecuta la fila seleccionada y cierra la paleta.
    fn palette_accept(&mut self) {
        let action = self.palette.rows.get(self.palette.selected).cloned();
        self.palette.open = false;
        if let Some(row) = action {
            match row.action {
                PaletteAction::Command(c) => self.run_command(c),
                PaletteAction::File(p) => self.open_file(p),
                PaletteAction::GoToLine(n) => self.goto_line_number(n),
                PaletteAction::InsertWord(w) => {
                    if self
                        .tabs
                        .get(self.active_tab)
                        .is_some_and(|t| t.kind == DocKind::Text)
                    {
                        self.insert_str(&w);
                    }
                }
            }
        }
    }

    /// Ejecuta la fila `i` (clic).
    fn palette_accept_at(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.palette.rows.len() {
            self.palette.selected = i as usize;
            self.palette_accept();
        }
    }

    /// Ejecuta un comando de la paleta.
    fn run_command(&mut self, c: Command) {
        match c {
            Command::Run => self.run(None),
            Command::Build => self.build(false),
            Command::BuildAndRun => self.build(true),
            Command::Lint => self.lint_active(),
            Command::NextError => self.diagnostic_step(true),
            Command::Find => self.find_open(false),
            Command::Replace => self.find_open(true),
            Command::GotoDef => self.go_to_definition(),
            Command::GoToLine => self.palette_open(PaletteKind::GoToLine),
            Command::Undo => self.undo_active(),
            Command::Redo => self.redo_active(),
            Command::Cut => self.cut_active(),
            Command::Copy => self.copy_active(),
            Command::Paste => self.paste_active(),
            Command::SelectAll => self.select_all(),
            Command::Preview => self.toggle_preview(),
            Command::Save => self.save_active(),
            Command::SaveAs => self.save_as(),
            Command::CloseTab => self.request_close_tab(self.active_tab as i32),
            Command::CloseAll => self.request_close_all(),
            Command::FindFiles => self.palette_open(PaletteKind::Files),
            Command::OpenProject => self.pick_project(),
            Command::OpenFile => self.pick_file(),
            Command::NewFile => self.new_file(),
            Command::NewTerminal => {
                self.new_local_session();
                self.resize_local();
            }
            Command::NewAdminTerminal => self.new_admin_terminal(),
            Command::SplitTerminal => self.split_terminal(),
            Command::ToggleExplorer => self.toggle_panel(0),
            Command::ToggleAgent => self.toggle_panel(1),
            Command::ToggleTerminal => self.toggle_panel(2),
            Command::ToggleGit => self.toggle_panel(3),
            Command::GitRefresh => self.git_refresh(),
            Command::GitStageAll => self.git_stage_all(),
            Command::GitPush => self.git_push(),
            Command::CycleTabSize => self.cycle_tab_size(),
            Command::ToggleInsertSpaces => self.toggle_insert_spaces(),
            Command::RestartSession => self.restart_session(),
            Command::RenameSession => self.prompt_rename_session(),
            Command::ProjectSearch => self.search_open(),
            Command::GitPull => self.git_pull(),
            Command::GitUnstageAll => self.git_unstage_all(),
            Command::GitAmend => self.git_amend(String::new()),
            Command::GitDiff => self.git_diff_active(),
            Command::RunPickEntry => self.run_pick_entry(),
            Command::RunUseActive => self.run_use_active(),
            Command::RunEditArgs => self.run_edit_args(),
            Command::RunToggleBuildOnSave => self.run_toggle_build_on_save(),
            Command::SpasmHelp => self.spasm_help(),
            Command::NewProject => self.new_project(),
            Command::TemplateCompiled => {
                let body = templates::by_id("release").map(|t| t.body).unwrap_or("");
                self.new_file_with(body);
            }
            Command::TemplateInterpreter => {
                let body = templates::by_id("desarrollo").map(|t| t.body).unwrap_or("");
                self.new_file_with(body);
            }
            Command::TemplateEmpty => {
                let body = templates::by_id("vacio").map(|t| t.body).unwrap_or("");
                self.new_file_with(body);
            }
            Command::Preferences => self.prefs_toggle(),
            Command::RelaunchElevated => self.request_relaunch_elevated(),
        }
    }

    /// Muestra u oculta un panel (0 Explorador, 1 OpenCode, 2 Terminal, 3 Git).
    /// Si estaba oculto vuelve a un dock por defecto y enciende la región.
    fn toggle_panel(&mut self, i: usize) {
        const DEFAULT: [Dock; 4] = [Dock::Left, Dock::Right, Dock::Bottom, Dock::Left];
        let Some(dock) = self.docks.get_mut(i) else {
            return;
        };
        if *dock == Dock::Hidden {
            *dock = DEFAULT[i];
            match *dock {
                Dock::Left => self.show_left = true,
                Dock::Right => self.show_right = true,
                Dock::Bottom => self.show_bottom = true,
                Dock::Hidden => {}
            }
        } else {
            *dock = Dock::Hidden;
        }
    }
}

// ---------------------------------------------------------------------------
// Menús, archivos y confirmación
// ---------------------------------------------------------------------------

impl Ide {
    /// Salta a la línea `line` (1-based) del archivo activo (paleta "Ir a línea").
    fn goto_line_number(&mut self, line: usize) {
        if self.tabs.is_empty() {
            return;
        }
        let last = self.tabs[self.active_tab].lines.len().saturating_sub(1);
        let new = line.saturating_sub(1).min(last);
        let old = self.tabs[self.active_tab].cursor.line;
        self.set_cursor(Cursor { line: new, col: 0 }, old, false);
    }

    /// Diálogo "Guardar como": escribe en otra ruta y reapunta la pestaña.
    fn save_as(&mut self) {
        let (start, name, data) = {
            let Some(t) = self.tabs.get(self.active_tab) else {
                return;
            };
            if t.kind == DocKind::Image {
                return;
            }
            let mut data = t.lines.join("\n");
            if t.trailing_newline {
                data.push('\n');
            }
            let start = t
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.explorer.root().to_path_buf());
            (start, file_name(&t.path), data)
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Guardar como")
            .set_directory(&start)
            .set_file_name(&name)
            .save_file()
        else {
            return;
        };
        if std::fs::write(&path, data).is_err() {
            return;
        }
        push_recent(&mut self.recent_files, path.clone());
        {
            let tab = &mut self.tabs[self.active_tab];
            tab.path = path.clone();
            tab.language = highlight::Language::from_path(&path);
            tab.dirty = false;
            tab.read_only = false;
        }
        self.active_file = Some(path);
        self.editor_full = true;
        self.explorer.rebuild();
        self.lint_active();
        if !cfg!(test) {
            self.git_refresh();
        }
    }

    /// Guarda la pestaña `i` (sin diálogos). Devuelve si se escribió.
    fn save_tab(&mut self, i: usize) -> bool {
        let Some(t) = self.tabs.get(i) else {
            return false;
        };
        if t.read_only {
            return false;
        }
        let mut data = t.lines.join("\n");
        if t.trailing_newline {
            data.push('\n');
        }
        let path = t.path.clone();
        if std::fs::write(&path, data).is_ok() {
            self.tabs[i].dirty = false;
            true
        } else {
            false
        }
    }

    /// Hay al menos un archivo con cambios sin guardar.
    fn has_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    /// Cierra la pestaña `i`, pidiendo confirmación si tiene cambios.
    fn request_close_tab(&mut self, i: i32) {
        if i < 0 || i as usize >= self.tabs.len() {
            return;
        }
        if self.tabs[i as usize].dirty {
            self.confirm = Some(Confirm::CloseTab);
            self.confirm_tab = Some(i as usize);
        } else {
            self.tab_closed(i);
        }
    }

    /// Cierra todas las pestañas, pidiendo confirmación si hay cambios.
    fn request_close_all(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        if self.has_dirty() {
            self.confirm = Some(Confirm::CloseAll);
            self.confirm_tab = None;
        } else {
            self.close_all_tabs();
        }
    }

    /// Decide si salir ya o pedir confirmación. `true` = salir ya.
    fn request_quit(&mut self) -> bool {
        if self.has_dirty() {
            self.confirm = Some(Confirm::Quit);
            self.confirm_tab = None;
            false
        } else {
            true
        }
    }

    /// Cierra todas las pestañas sin preguntar.
    fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.active_tab = 0;
        self.active_file = None;
        self.editor_full = true;
        self.undo.clear();
        self.redo.clear();
    }

    /// El usuario eligió "Guardar" en el diálogo. `true` = salir.
    fn confirm_save(&mut self) -> bool {
        match self.confirm {
            Some(Confirm::CloseTab) => {
                if let Some(i) = self.confirm_tab {
                    let _ = self.save_tab(i);
                }
            }
            Some(Confirm::CloseAll) | Some(Confirm::Quit) | Some(Confirm::RelaunchElevated) => {
                let dirty: Vec<usize> = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.dirty)
                    .map(|(i, _)| i)
                    .collect();
                for i in dirty {
                    let _ = self.save_tab(i);
                }
                self.lint_active();
                if !cfg!(test) {
                    self.git_refresh();
                }
            }
            None => {}
        }
        self.finish_confirm()
    }

    /// El usuario eligió "Descartar". `true` = salir.
    fn confirm_discard(&mut self) -> bool {
        self.finish_confirm()
    }

    /// El usuario canceló: se cierra el diálogo y no pasa nada.
    fn confirm_cancel(&mut self) {
        self.confirm = None;
        self.confirm_tab = None;
    }

    /// Aplica la acción confirmada. `true` = salir.
    fn finish_confirm(&mut self) -> bool {
        let what = self.confirm.take();
        let tab = self.confirm_tab.take();
        match what {
            Some(Confirm::CloseTab) => {
                if let Some(i) = tab {
                    self.tab_closed(i as i32);
                }
                false
            }
            Some(Confirm::CloseAll) => {
                self.close_all_tabs();
                false
            }
            Some(Confirm::Quit) => true,
            Some(Confirm::RelaunchElevated) => {
                // El relanzamiento lo hace el callback, que tiene la ventana para
                // guardar el estado: acá sólo queda la intención.
                self.relaunch = true;
                false
            }
            None => false,
        }
    }

    /// Ejecuta una acción de los menús Archivo/Editar. `true` = salir.
    fn menu_command(&mut self, id: &str) -> bool {
        self.close_menu();
        match id {
            "file.new" => self.new_file(),
            "file.open" => self.pick_file(),
            "file.open-folder" => self.pick_project(),
            "file.save" => self.save_active(),
            "file.save-as" => self.save_as(),
            "file.close" => self.request_close_tab(self.active_tab as i32),
            "file.close-all" => self.request_close_all(),
            "file.exit" => return self.request_quit(),
            "edit.undo" => self.undo_active(),
            "edit.redo" => self.redo_active(),
            "edit.cut" => self.cut_active(),
            "edit.copy" => self.copy_active(),
            "edit.paste" => self.paste_active(),
            "edit.select-all" => self.select_all(),
            "edit.find" => self.find_open(false),
            "edit.replace" => self.find_open(true),
            "edit.goto-line" => self.palette_open(PaletteKind::GoToLine),
            "help.about" => self.about_show(),
            "help.syntax" => self.spasm_help(),
            "help.keys" => self.prefs_toggle(),
            "help.config" => reveal_in_explorer(&crate::config::config_dir()),
            "help.repo" => {
                // El remoto `origin` del proyecto abierto; si no hay, el de NuIde.
                let url = crate::git::remote_url(self.explorer.root())
                    .unwrap_or_else(|| "https://github.com/nutoysaudio-code/NuIde".to_string());
                open_url(&url);
            }
            _ => {}
        }
        false
    }

    /// Filas del menú Archivo.
    fn file_menu_items(&self) -> Vec<IdeMenuEntry> {
        let has = !self.tabs.is_empty();
        vec![
            menu_entry("file.new", "Nuevo archivo…", "Ctrl+N", true),
            menu_entry("file.open", "Abrir archivo…", "Ctrl+O", true),
            menu_entry("file.open-folder", "Abrir carpeta…", "", true),
            menu_sep(),
            menu_entry("file.save", "Guardar", "Ctrl+S", has),
            menu_entry("file.save-as", "Guardar como…", "Ctrl+Shift+S", has),
            menu_sep(),
            menu_entry("file.close", "Cerrar pestaña", "Ctrl+W", has),
            menu_entry("file.close-all", "Cerrar todas", "", has),
            menu_sep(),
            menu_entry("file.exit", "Salir", "", true),
        ]
    }

    /// Filas del menú Editar.
    fn edit_menu_items(&self) -> Vec<IdeMenuEntry> {
        let has = !self.tabs.is_empty();
        vec![
            menu_entry("edit.undo", "Deshacer", "Ctrl+Z", has),
            menu_entry("edit.redo", "Rehacer", "Ctrl+Y", has),
            menu_sep(),
            menu_entry("edit.cut", "Cortar", "Ctrl+X", has),
            menu_entry("edit.copy", "Copiar", "Ctrl+C", has),
            menu_entry("edit.paste", "Pegar", "Ctrl+V", has),
            menu_entry("edit.select-all", "Seleccionar todo", "Ctrl+A", has),
            menu_sep(),
            menu_entry("edit.find", "Buscar", "Ctrl+F", has),
            menu_entry("edit.replace", "Reemplazar", "Ctrl+H", has),
            menu_entry("edit.goto-line", "Ir a línea…", "Ctrl+G", has),
        ]
    }

    /// Filas del menú Ayuda.
    fn help_menu_items(&self) -> Vec<IdeMenuEntry> {
        vec![
            menu_entry("help.about", "Acerca de...", "", true),
            menu_entry("help.syntax", "Ayuda de sintaxis SpASM", "F1", true),
            menu_entry("help.keys", "Preferencias y atajos", "Ctrl+,", true),
            menu_sep(),
            menu_entry("help.config", "Abrir carpeta de configuración", "", true),
            menu_entry("help.repo", "Ver repositorio", "", true),
        ]
    }
}

// ---------------------------------------------------------------------------
// Explorador: menú contextual y operaciones de archivos
// ---------------------------------------------------------------------------

impl Ide {
    /// Abre el menú contextual sobre la fila `index`.
    fn explorer_context(&mut self, index: i32) {
        if index < 0 || index as usize >= self.explorer.nodes().len() {
            return;
        }
        let idx = index as usize;
        let (is_dir, is_root) = {
            let n = &self.explorer.nodes()[idx];
            (n.is_dir, idx == 0)
        };
        self.explorer_selected = index;
        self.explorer_context_items = vec![
            menu_entry("ctx.new-file", "Nuevo archivo…", "", true),
            menu_entry("ctx.new-folder", "Nueva carpeta…", "", true),
            menu_sep(),
            menu_entry("ctx.rename", "Renombrar…", "", !is_root),
            menu_entry("ctx.delete", "Eliminar", "", !is_root),
            menu_sep(),
            menu_entry("ctx.copy-path", "Copiar ruta", "", true),
            menu_entry("ctx.reveal", "Mostrar en el explorador", "", true),
            menu_entry("ctx.refresh", "Refrescar", "", true),
        ];
        // "Nuevo" va a la carpeta: la del nodo si es carpeta, si no la padre.
        let _ = is_dir;
        self.explorer_context_open = true;
    }

    /// Ejecuta una acción del menú contextual.
    fn explorer_context_command(&mut self, id: &str) {
        self.explorer_context_open = false;
        let Some(node) = self
            .explorer
            .nodes()
            .get(self.explorer_selected.max(0) as usize)
            .cloned()
        else {
            return;
        };
        match id {
            "ctx.new-file" => {
                let dir = target_dir_of(&node);
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewFile,
                    target: dir,
                    text: String::new(),
                });
            }
            "ctx.new-folder" => {
                let dir = target_dir_of(&node);
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewFolder,
                    target: dir,
                    text: String::new(),
                });
            }
            "ctx.rename" => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Rename,
                    target: node.path.clone(),
                    text: node.label.clone(),
                });
            }
            "ctx.delete" => self.ask = Some(Ask::Delete(node.path.clone())),
            "ctx.copy-path" => {
                let _ = clipboard_write(&node.path.display().to_string());
            }
            "ctx.reveal" => reveal_in_explorer(&node.path),
            "ctx.refresh" => self.explorer.rebuild(),
            _ => {}
        }
    }

    /// Abre el menú contextual de una terminal (clic derecho): copiar, pegar y
    /// elevar a administrador.
    fn terminal_context(&mut self, target: Target) {
        self.term_context_target = target;
        let has_sel = !self.selected_text(target).is_empty();
        let has_clip = read_clipboard().is_some_and(|t| !t.is_empty());
        self.term_context_items = vec![
            menu_entry("term.copy", "Copiar", "Ctrl+Shift+C", has_sel),
            menu_entry("term.paste", "Pegar", "Ctrl+Shift+V", has_clip),
            menu_sep(),
            menu_entry("term.elevate", "Elevar a administrador", "UAC", true),
        ];
        self.term_context_open = true;
    }

    /// Ejecuta una acción del menú contextual de las terminales.
    fn terminal_context_command(&mut self, id: &str) {
        self.term_context_open = false;
        match id {
            "term.copy" => self.copy_selection(self.term_context_target),
            "term.paste" => self.paste_into(self.term_context_target),
            "term.elevate" => self.new_admin_terminal(),
            _ => {}
        }
    }

    /// Reconstruye el árbol (botón ⟳ o `F5` en el explorador).
    fn explorer_refresh(&mut self) {
        self.explorer.rebuild();
    }

    /// Muestra u oculta los archivos generados.
    fn explorer_toggle_generated(&mut self) {
        let show = !self.explorer.shows_generated();
        self.explorer.set_show_generated(show);
        self.config.set("explorer.generated", u8::from(show));
        self.config.save();
    }

    /// Tecla con el diálogo de entrada abierto. Devuelve si la consumió.
    fn prompt_key(&mut self, text: &str, control: bool) {
        match text {
            "\u{1b}" => self.prompt = None,
            "\n" | "\r" => self.prompt_accept(),
            "\u{8}" => {
                if let Some(p) = self.prompt.as_mut() {
                    let _ = p.text.pop();
                }
            }
            _ => {
                if !control && !text.is_empty() && !text.starts_with('\u{F7}') {
                    if let Some(p) = self.prompt.as_mut() {
                        p.text.push_str(text);
                    }
                }
            }
        }
    }

    /// Atajo de teclado del diálogo de entrada (modal).
    fn prompt_intercept(&mut self, text: &str, control: bool) -> bool {
        if self.prompt.is_some() {
            self.prompt_key(text, control);
            return true;
        }
        false
    }

    /// Acepta el diálogo de entrada: crea, renombra o no hace nada.
    fn prompt_accept(&mut self) {
        let Some(p) = self.prompt.take() else {
            return;
        };
        let name = p.text.trim().to_string();
        // Un campo del sitio puede quedar vacío (limpiar la contraseña, el script)
        // y admite caracteres que `valid_name` rechaza por ser nombres de archivo.
        let site_field = matches!(p.kind, PromptKind::SiteField(_) | PromptKind::WebField(_));
        if name.is_empty() && !site_field {
            return;
        }
        if !site_field && p.kind != PromptKind::SessionName && !valid_name(&name) {
            return;
        }
        match p.kind {
            PromptKind::NewFile => {
                let path = p.target.join(&name);
                if !path.exists() && std::fs::write(&path, "").is_ok() {
                    self.explorer.rebuild();
                    self.open_file(path);
                }
            }
            PromptKind::NewFolder => {
                let path = p.target.join(&name);
                if std::fs::create_dir(&path).is_ok() {
                    self.explorer.rebuild();
                }
            }
            PromptKind::Rename => {
                let dest = p
                    .target
                    .parent()
                    .map(|d| d.join(&name))
                    .unwrap_or_else(|| p.target.clone());
                if dest != p.target && std::fs::rename(&p.target, &dest).is_ok() {
                    self.retarget_open_tabs(&p.target, &dest);
                    self.explorer.rebuild();
                    if !cfg!(test) {
                        self.git_refresh();
                    }
                }
            }
            PromptKind::SessionName => {
                if let Some(s) = self.local.get_mut(self.local_active) {
                    s.name = name;
                }
            }
            PromptKind::NewBranch => {
                self.git_spawn(git::Op::CreateBranch(name));
            }
            PromptKind::RunArgs => {
                self.run_args = p.text.trim().to_string();
                self.config.set("run.args", &self.run_args);
                self.config.save();
            }
            PromptKind::SiteField(field) => self.site_field_set(field, p.text.clone()),
            PromptKind::WebField(field) => self.web_field_set(field, p.text.clone()),
        }
    }

    /// Confirma la acción pendiente (borrar o reemplazar en el proyecto).
    fn ask_accept(&mut self) {
        let Some(ask) = self.ask.take() else {
            return;
        };
        match ask {
            Ask::Delete(path) => self.delete_path(&path),
            Ask::ReplaceInProject => self.replace_in_project(),
            Ask::Discard { path, untracked } => {
                self.diff = None;
                self.git_spawn(git::Op::Discard { path, untracked });
            }
            Ask::DeleteProvider(id) => self.provider_do_delete(&id),
        }
    }

    /// Borra una ruta y cierra las pestañas que quedaron dentro.
    fn delete_path(&mut self, path: &Path) {
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        };
        if ok {
            self.tabs.retain(|t| !t.path.starts_with(path));
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len().saturating_sub(1);
            }
            self.active_file = self.tabs.get(self.active_tab).map(|t| t.path.clone());
            self.editor_full = true;
            self.explorer.rebuild();
            if !cfg!(test) {
                self.git_refresh();
            }
        }
    }

    /// Reemplaza la consulta en todos los archivos con coincidencias y recarga
    /// las pestañas abiertas que hayan cambiado.
    fn replace_in_project(&mut self) {
        let query = self.search.query.clone();
        if query.is_empty() {
            return;
        }
        let replacement = self.search.replace.clone();
        let case = self.search.case_sensitive;
        let re = if self.search.regex {
            match build_regex(&query, case) {
                Some(re) => Some(re),
                None => return,
            }
        } else {
            None
        };
        let mut files: Vec<PathBuf> = self.search.hits.iter().map(|h| h.path.clone()).collect();
        files.sort();
        files.dedup();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let trailing = text.ends_with('\n');
            let mut new = String::new();
            for (i, line) in text.lines().enumerate() {
                if i > 0 {
                    new.push('\n');
                }
                match &re {
                    Some(re) => new.push_str(&re.replace_all(line, replacement.as_str())),
                    None => {
                        let (updated, _) = replace_in_line(line, &query, &replacement, case, false);
                        new.push_str(&updated);
                    }
                }
            }
            if trailing {
                new.push('\n');
            }
            if std::fs::write(&path, new).is_ok() {
                self.reload_open_tab(&path);
            }
        }
        self.search_run();
        if !cfg!(test) {
            self.git_refresh();
        }
    }

    /// Recarga desde disco una pestaña abierta (tras reemplazar en el proyecto).
    fn reload_open_tab(&mut self, path: &Path) {
        let Some(i) = self.tabs.iter().position(|t| t.path == path) else {
            return;
        };
        if self.tabs[i].read_only {
            return;
        }
        let (lines, trailing, _) = read_lines(path);
        let last = lines.len().saturating_sub(1);
        self.tabs[i].lines = lines;
        self.tabs[i].trailing_newline = trailing;
        self.tabs[i].dirty = false;
        let clamped_line = self.tabs[i].cursor.line.min(last);
        let max_col = self.tabs[i].line_len(clamped_line);
        let col = self.tabs[i].cursor.col.min(max_col);
        self.tabs[i].cursor = Cursor {
            line: clamped_line,
            col,
        };
        self.editor_full = true;
    }

    /// Reapunta las pestañas abiertas al mover/renombrar una ruta.
    fn retarget_open_tabs(&mut self, from: &Path, to: &Path) {
        for tab in &mut self.tabs {
            if let Ok(rel) = tab.path.strip_prefix(from) {
                tab.path = to.join(rel);
                tab.language = highlight::Language::from_path(&tab.path);
            }
        }
        if let Some(active) = self.active_file.as_ref() {
            if let Ok(rel) = active.strip_prefix(from) {
                self.active_file = Some(to.join(rel));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Buscar en el proyecto
// ---------------------------------------------------------------------------

impl Ide {
    /// Abre el panel de búsqueda en el proyecto (hereda la consulta del editor).
    fn search_open(&mut self) {
        self.search.open = true;
        self.search.selected = 0;
        if self.search.query.is_empty() && !self.find.query.is_empty() {
            self.search.query = self.find.query.clone();
            self.search.query_cursor = self.search.query.chars().count();
        }
        self.search_run();
    }

    /// Cierra el panel de búsqueda.
    fn search_close(&mut self) {
        self.search.open = false;
    }

    /// Lanza la búsqueda en un hilo (descarta respuestas viejas por generación).
    fn search_run(&mut self) {
        self.search_gen += 1;
        self.search.searching = true;
        search::spawn(
            self.explorer.root().to_path_buf(),
            self.search.query.clone(),
            self.search.case_sensitive,
            self.search.regex,
            self.search_gen,
            self.search_tx.clone(),
        );
    }

    /// Alterna distinguir mayúsculas en la búsqueda del proyecto.
    fn search_toggle_case(&mut self) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.search_run();
    }

    /// Alterna expresiones regulares en la búsqueda del proyecto.
    fn search_toggle_regex(&mut self) {
        self.search.regex = !self.search.regex;
        self.search_run();
    }

    /// Muestra u oculta el campo de reemplazo.
    fn search_toggle_replace(&mut self) {
        self.search.replace_open = !self.search.replace_open;
        if !self.search.replace_open {
            self.search.focus_replace = false;
        }
    }

    /// Mueve la selección de resultados.
    fn search_move(&mut self, delta: i32) {
        let len = self.search.hits.len() as i32;
        if len == 0 {
            return;
        }
        self.search.selected = (self.search.selected as i32 + delta).clamp(0, len - 1) as usize;
    }

    /// Abre el resultado seleccionado en la línea correspondiente.
    fn search_accept(&mut self) {
        let Some(hit) = self.search.hits.get(self.search.selected) else {
            return;
        };
        let (path, line, col) = (hit.path.clone(), hit.line, hit.col);
        self.open_file(path);
        if let Some(t) = self.tabs.get_mut(self.active_tab) {
            let last = t.lines.len().saturating_sub(1);
            t.cursor = Cursor {
                line: line.min(last),
                col,
            };
            t.anchor = None;
        }
        self.editor_full = true;
        self.search.open = false;
    }

    /// Ejecuta el resultado `i` (clic).
    fn search_accept_at(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.search.hits.len() {
            self.search.selected = i as usize;
            self.search_accept();
        }
    }

    /// Inserta un carácter en el campo con foco y vuelve a buscar.
    fn search_insert(&mut self, ch: char) {
        if self.search.focus_replace {
            let at = self
                .search
                .replace_cursor
                .min(self.search.replace.chars().count());
            let mut chars: Vec<char> = self.search.replace.chars().collect();
            chars.insert(at, ch);
            self.search.replace = chars.into_iter().collect();
            self.search.replace_cursor = at + 1;
        } else {
            let at = self
                .search
                .query_cursor
                .min(self.search.query.chars().count());
            let mut chars: Vec<char> = self.search.query.chars().collect();
            chars.insert(at, ch);
            self.search.query = chars.into_iter().collect();
            self.search.query_cursor = at + 1;
            self.search_run();
        }
    }

    /// Borra el carácter anterior al cursor del campo con foco.
    fn search_backspace(&mut self) {
        if self.search.focus_replace {
            let at = self
                .search
                .replace_cursor
                .min(self.search.replace.chars().count());
            if at == 0 {
                return;
            }
            let mut chars: Vec<char> = self.search.replace.chars().collect();
            chars.remove(at - 1);
            self.search.replace = chars.into_iter().collect();
            self.search.replace_cursor = at - 1;
        } else {
            let at = self
                .search
                .query_cursor
                .min(self.search.query.chars().count());
            if at == 0 {
                return;
            }
            let mut chars: Vec<char> = self.search.query.chars().collect();
            chars.remove(at - 1);
            self.search.query = chars.into_iter().collect();
            self.search.query_cursor = at - 1;
            self.search_run();
        }
    }

    /// Tecla del panel de búsqueda del proyecto. Devuelve si la consumió.
    fn search_key(&mut self, text: &str, control: bool, shift: bool) {
        match text {
            "\u{1b}" => self.search_close(),
            "\n" | "\r" => self.search_accept(),
            "\t" => {
                if self.search.replace_open {
                    self.search.focus_replace = !self.search.focus_replace;
                }
            }
            "\u{F700}" => self.search_move(-1),
            "\u{F701}" => self.search_move(1),
            "\u{8}" => self.search_backspace(),
            _ => {
                if !control && !text.is_empty() && !text.starts_with('\u{F7}') {
                    for ch in text.chars() {
                        self.search_insert(ch);
                    }
                }
            }
        }
        let _ = shift;
    }

    /// Atajo (`Ctrl+Shift+F`) o desvío de teclas con el panel abierto.
    fn search_intercept(&mut self, text: &str, control: bool, shift: bool) -> bool {
        if self.search.open {
            self.search_key(text, control, shift);
            return true;
        }
        false
    }

    /// Pide confirmación para reemplazar en todo el proyecto.
    fn search_request_replace_all(&mut self) {
        if self.search.query.is_empty() || self.search.hits.is_empty() {
            return;
        }
        self.ask = Some(Ask::ReplaceInProject);
    }
}

// ---------------------------------------------------------------------------
// Preferencias y atajos
// ---------------------------------------------------------------------------

impl Ide {
    /// Abre o cierra el panel de Preferencias.
    fn prefs_toggle(&mut self) {
        // La tarjeta vieja de Preferencias quedó reemplazada por la pantalla de
        // configuración (maneja por datos, con los agregados adentro).
        self.settings_toggle();
    }

    /// Cierra Preferencias.
    fn prefs_close(&mut self) {
        self.prefs_open = false;
        self.capturing_key = None;
    }

    /// Abre el diálogo "Acerca de NuIde".
    fn about_show(&mut self) {
        self.about_open = true;
    }

    /// Cierra "Acerca de".
    fn about_close(&mut self) {
        self.about_open = false;
    }

    // --- despliegue (sitio del proyecto) ---

    /// Guarda el sitio en el config, indexado por proyecto.
    fn save_site(&mut self) {
        let project = self.explorer.root().to_path_buf();
        let site = self.site.clone();
        site.save(&project, &mut self.config);
        self.config.save();
    }

    /// Raíz del proyecto abierto (las claves por proyecto del config la usan).
    fn site_root(&self) -> PathBuf {
        self.explorer.root().to_path_buf()
    }

    /// Cicla el transporte del sitio (FTP → FTPS → SFTP).
    fn prefs_cycle_transport(&mut self) {
        self.site.transport = match self.site.transport {
            Transport::Ftp => Transport::Ftps,
            Transport::Ftps => Transport::Sftp,
            Transport::Sftp => Transport::Ftp,
        };
        self.save_site();
    }

    /// Abre el diálogo de entrada para editar un campo del sitio.
    fn prefs_edit_site(&mut self, field: &str) {
        let current = self.site.field(field).to_string();
        self.prompt = Some(Prompt {
            kind: PromptKind::SiteField(match field {
                "user" => "user",
                "pass" => "pass",
                "remote" => "remote",
                "local-sub" => "local-sub",
                "include" => "include",
                "script" => "script",
                _ => "host",
            }),
            target: PathBuf::new(),
            text: current,
        });
    }

    /// Aplica el valor editado a un campo del sitio.
    fn site_field_set(&mut self, field: &str, value: String) {
        let value = value.trim().to_string();
        match field {
            "user" => self.site.user = value,
            "pass" => self.site.pass = value,
            "remote" => self.site.remote_root = value,
            "local-sub" => self.site.local_sub = value,
            "include" => self.site.include = value,
            "script" => self.site.script = value,
            "port" => self.site.port = value.parse().unwrap_or(0),
            _ => self.site.host = value,
        }
        self.save_site();
    }

    /// Prueba el sitio sin subir nada (curl `--list-only`).
    fn deploy_test(&mut self) {
        if self.deploy_busy {
            self.write_build_output("ya hay un despliegue en curso.\n");
            return;
        }
        self.deploy_busy = true;
        self.local_active = self.ensure_build_session();
        self.resize_local();
        let tx = self.deploy_tx.clone();
        deploy::spawn_test(self.site.clone(), tx);
    }

    /// Sube los cambios: corre el script del proyecto (o el escaneo por
    /// extensión) y ejecuta las transferencias.
    fn deploy_upload(&mut self) {
        if self.deploy_busy {
            self.write_build_output("ya hay un despliegue en curso.\n");
            return;
        }
        self.deploy_busy = true;
        self.local_active = self.ensure_build_session();
        self.resize_local();
        let tx = self.deploy_tx.clone();
        deploy::spawn_deploy(
            self.explorer.root().to_path_buf(),
            self.site.clone(),
            self.toolchain.clone(),
            tx,
        );
    }

    /// Abre una sesión de terminal contra el servidor (`ssh user@host`).
    fn deploy_ssh_terminal(&mut self) {
        if self.site.host.trim().is_empty() {
            self.write_build_output(
                "el sitio no tiene host: completá Preferencias → Despliegue.\n",
            );
            self.local_active = self.ensure_build_session();
            self.resize_local();
            return;
        }
        let id = self.next_local_id;
        self.next_local_id += 1;
        let (program, args) = match pty::on_path(&["ssh.exe", "ssh"]) {
            Some(p) => (p, self.site.ssh_args()),
            None => ("ssh.exe".to_string(), self.site.ssh_args()),
        };
        let name = format!("ssh {}", self.site.host);
        let mut screen = Screen::new(self.local_rows, self.local_cols);
        let cwd = self.explorer.root().to_path_buf();
        let pty = match Pty::spawn(
            id,
            &program,
            &args,
            &cwd,
            self.local_cols,
            self.local_rows,
            self.local_tx.clone(),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                screen.process(format!("no se pudo abrir ssh ({program}): {e}\r\n").as_bytes());
                None
            }
        };
        self.local.push(LocalSession {
            id,
            name,
            pty,
            screen,
            alive: true,
            sel: None,
            mouse_button: 3,
            term_model: Rc::new(TermRows::default()),
        });
        self.local_active = self.local.len() - 1;
        self.resize_local();
    }

    // --- módulos: servidor web y visor de SQLite ---

    /// Prende o apaga el servidor web local.
    fn toggle_web(&mut self) {
        if self.web.enabled {
            self.stop_web();
            return;
        }
        let root = self.site_root();
        let cfg = self.web.clone();
        let tx = self.web_tx.clone();
        match WebServer::start(cfg.clone(), self.web.root(&root), tx) {
            Ok(server) => {
                self.web.enabled = true;
                self.web_server = Some(server);
                self.write_build_output(&format!(
                    "servidor web activo en {} (raíz: {}, CORS: {})\n",
                    cfg.url(),
                    cfg.root(&root).display(),
                    cfg.cors_origin
                ));
                self.local_active = self.ensure_build_session();
                self.resize_local();
            }
            Err(e) => {
                self.write_build_output(&format!("no se pudo prender el servidor: {e}\n"));
                self.local_active = self.ensure_build_session();
                self.resize_local();
                return;
            }
        }
        self.save_web();
    }

    /// Apaga el servidor (destruye el listener y espera al hilo).
    fn stop_web(&mut self) {
        self.web.enabled = false;
        if let Some(server) = self.web_server.take() {
            server.stop();
        }
        self.save_web();
    }

    fn save_web(&mut self) {
        let project = self.site_root();
        let web = self.web.clone();
        web.save(&project, &mut self.config);
        self.config.save();
    }

    /// Abre el diálogo para editar puerto / carpeta / CORS.
    fn edit_web(&mut self, field: &str) {
        let current = match field {
            "port" => self.web.port.to_string(),
            "root" => self.web.root_sub.clone(),
            _ => self.web.cors_origin.clone(),
        };
        self.prompt = Some(Prompt {
            kind: PromptKind::WebField(match field {
                "port" => "port",
                "root" => "root",
                _ => "cors",
            }),
            target: PathBuf::new(),
            text: current,
        });
    }

    /// Aplica un campo del servidor; si estaba prendido, lo reinicia.
    fn web_field_set(&mut self, field: &str, value: String) {
        let value = value.trim().to_string();
        match field {
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    if p >= 1024 {
                        self.web.port = p;
                    }
                }
            }
            "root" => self.web.root_sub = value,
            _ => self.web.cors_origin = if value.is_empty() { "*".into() } else { value },
        }
        if self.web.enabled {
            self.stop_web();
            self.web.enabled = true;
            self.toggle_web();
        } else {
            self.save_web();
        }
    }

    /// Prende/apaga el visor de SQLite; al prenderlo pide la base si no hay.
    fn toggle_sqlite_module(&mut self) {
        self.sqlite_enabled = !self.sqlite_enabled;
        self.config
            .set("module.sqlite", if self.sqlite_enabled { "1" } else { "0" });
        self.config.save();
        if !self.sqlite_enabled {
            self.sqlite_open = false;
            return;
        }
        if self.sqlite_db.is_none() {
            self.sqlite_pick_db();
        } else {
            self.sqlite_reload();
        }
    }

    /// Elegidor nativo de archivo de base.
    fn sqlite_pick_db(&mut self) {
        let start = self.site_root();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Base de datos SQLite")
            .set_directory(&start)
            .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
            .add_filter("Todos", &["*"])
            .pick_file()
        else {
            return;
        };
        if !sqlite::probe(&path) {
            self.sqlite_note = "el archivo no parece una base SQLite".to_string();
            self.sqlite_db = Some(path);
            self.sqlite_open = true;
            return;
        }
        self.sqlite_db = Some(path);
        self.sqlite_open = true;
        self.sqlite_reload();
    }

    /// Vuelve a leer tablas y filas en un hilo (nunca en el de la UI).
    fn sqlite_reload(&mut self) {
        let Some(path) = self.sqlite_db.clone() else {
            self.sqlite_note = "elegí una base primero".to_string();
            self.sqlite_open = true;
            return;
        };
        let Some(tool) = sqlite::Tool::discover().or_else(|| self.sqlite_tool.clone()) else {
            self.sqlite_note = "no se encontró sqlite3.exe ni python para leer la base".to_string();
            self.sqlite_open = true;
            self.sqlite_busy = false;
            return;
        };
        self.sqlite_tool = Some(tool.clone());
        self.sqlite_busy = true;
        self.sqlite_note = String::new();
        let table = self
            .sqlite_tables
            .get(self.sqlite_selected)
            .cloned()
            .or_else(|| self.sqlite_last_table());
        let tx = self.sqlite_tx.clone();
        sqlite::spawn_load(tool, path, table, tx);
    }

    /// Tabla elegida antes de cerrar el panel (para reabrirla en la misma).
    fn sqlite_last_table(&self) -> Option<String> {
        None
    }

    /// Cambia la tabla mostrada.
    fn sqlite_select(&mut self, index: usize) {
        self.sqlite_selected = index;
        self.sqlite_reload();
    }

    /// Cierra el panel del visor.
    fn sqlite_close(&mut self) {
        self.sqlite_open = false;
    }

    /// Recibe el resultado de una lectura del visor.
    fn sqlite_loaded(&mut self, msg: sqlite::Loaded) {
        self.sqlite_busy = false;
        let index = msg.tables.iter().position(|t| *t == msg.table).unwrap_or(0);
        self.sqlite_selected = index;
        self.sqlite_tables = msg.tables;
        self.sqlite_header = msg.header;
        self.sqlite_lines = msg.lines;
        self.sqlite_note = msg.note;
    }

    // --- pantalla de configuración (maneja por datos) ---

    /// Secciones de la pantalla, en el orden de la navegación.
    pub const SETTINGS_SECTIONS: &[&str] = &[
        "Interfaz",
        "Editor",
        "Terminales",
        "Agente OpenCode",
        "Proveedores",
        "Despliegue",
        "Módulos",
        "Atajos",
    ];

    /// Arma todas las filas de la pantalla, con su valor actual.
    /// Arma todas las filas de la pantalla, con su valor actual. Cada control
    /// se resuelve por `kind` en `ide_settings.slint`; para agregar un ajuste
    /// alcanza con pushar una fila acá.
    pub fn settings_rows(&self) -> Vec<IdeSetting> {
        let mut v: Vec<IdeSetting> = Vec::new();
        let s =
            |id: &str,
             section: i32,
             kind: i32,
             label: &str,
             value: String,
             hint: &str,
             scope: i32| { setting(id, section, kind, label, &value, hint, scope) };
        let b = |id: &str, section: i32, label: &str, on: bool, hint: &str, scope: i32| {
            setting(
                id,
                section,
                1,
                label,
                if on { "1" } else { "0" },
                hint,
                scope,
            )
        };

        // --- Interfaz (0) ---
        v.push(s(
            "ui.font.mono",
            0,
            2,
            "Fuente del código",
            self.mono.clone(),
            "Familia monoespaciada del editor y las terminales; clic para ciclar las instaladas",
            0,
        ));
        v.push(s(
            "ui.font.sans",
            0,
            2,
            "Fuente de la interfaz",
            self.sans.clone(),
            "Familia de los textos de la IDE (menús, paneles)",
            0,
        ));
        let mut size = s(
            "ui.font.size",
            0,
            3,
            "Tamaño del código",
            format!("{} px", self.editor_font_size as i32),
            "Puntos de la fuente monoespaciada",
            0,
        );
        size.num = self.editor_font_size;
        size.min = 9.0;
        size.max = 24.0;
        size.step = 1.0;
        v.push(size);
        v.push(s(
            "ui.accent",
            0,
            2,
            "Acento",
            self.accent_name(),
            "Color de resaltado (pestañas activas, foco, marcadores)",
            0,
        ));
        let mut alpha = s(
            "ui.alpha",
            0,
            3,
            "Opacidad de los paneles",
            format!("{:.0} %", self.panel_alpha * 100.0),
            "Compone las superficies contra el fondo de la ventana",
            0,
        );
        alpha.num = self.panel_alpha;
        alpha.min = 0.6;
        alpha.max = 1.0;
        alpha.step = 0.01;
        v.push(alpha);
        v.push(b(
            "ui.show.left",
            0,
            "Región izquierda (explorador y Git)",
            self.show_left,
            "Se puede mover cada panel arrastrando su cabecera",
            0,
        ));
        v.push(b(
            "ui.show.right",
            0,
            "Región derecha (OpenCode CLI)",
            self.show_right,
            "Prende o apaga la columna del agente",
            0,
        ));
        v.push(b(
            "ui.show.bottom",
            0,
            "Región inferior (terminal)",
            self.show_bottom,
            "Prende o apaga la terminal local",
            0,
        ));

        // --- Editor (1) ---
        v.push(s(
            "ed.tab-size",
            1,
            2,
            "Tamaño de tab",
            self.tab_size.to_string(),
            "Espacios por tabulador al expandir",
            0,
        ));
        v.push(b(
            "ed.insert-spaces",
            1,
            "Insertar espacios en lugar de tab",
            self.insert_spaces,
            "Al tipear Tab se insertan espacios",
            0,
        ));
        v.push(b(
            "ed.generated",
            1,
            "Mostrar archivos generados",
            self.explorer.shows_generated(),
            "Objetos, binarios y salidas del toolchain en el explorador",
            1,
        ));
        v.push(b(
            "ed.build-on-save",
            1,
            "Compilar al guardar",
            self.run_build_on_save,
            "Cada Ctrl+S corre la cadena de compilación",
            1,
        ));
        v.push(s(
            "ed.run-args",
            1,
            0,
            "Argumentos de ejecución",
            self.run_args.clone(),
            "Se pasan al programa cuando apretás F5",
            1,
        ));
        v.push(s(
            "ed.run-entry",
            1,
            0,
            "Archivo de entrada (F5)",
            self.config.get("run.entry").unwrap_or("").to_string(),
            "Ruta del .spasm que se ejecuta",
            1,
        ));

        // --- Terminales (2) ---
        let (shell, _) = pty::default_shell();
        v.push(s(
            "term.shell",
            2,
            6,
            "Shell por defecto",
            shell,
            "Se cambia con la variable de entorno NUIDE_SHELL",
            0,
        ));
        v.push(s(
            "term.grid",
            2,
            6,
            "Grilla de la terminal",
            format!("{} × {} celdas", self.local_cols, self.local_rows),
            "Se recalcula sola al estirar el panel",
            0,
        ));
        v.push(s(
            "term.sessions",
            2,
            6,
            "Sesiones abiertas",
            self.local.len().to_string(),
            "Cada sesión es un PTY con su shell",
            0,
        ));
        v.push(s(
            "term.new",
            2,
            4,
            "Nueva sesión de terminal",
            "crear".into(),
            "Abre otra shell en la raíz del proyecto",
            0,
        ));
        v.push(s(
            "term.restart",
            2,
            4,
            "Reiniciar la sesión activa",
            "reiniciar".into(),
            "Reemplaza la sesión actual por una shell nueva",
            0,
        ));
        v.push(s(
            "term.split",
            2,
            4,
            "Dividir la terminal",
            "dividir".into(),
            "Dos vistas lado a lado",
            0,
        ));

        // --- Agente OpenCode (3) ---
        let agent = self
            .toolchain
            .agent
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no encontrado (defini NUIDE_AGENT)".to_string());
        v.push(s(
            "agent.bin",
            3,
            6,
            "CLI de OpenCode",
            agent,
            "Ruta de la CLI que se lanza bajo demanda con «Lanzar opencode»",
            0,
        ));
        v.push(s(
            "agent.version",
            3,
            0,
            "Versión mostrada",
            String::new(),
            "Etiqueta que se ve en la ficha del panel",
            0,
        ));
        v.push(s(
            "agent.launch",
            3,
            4,
            "Lanzar opencode",
            "abrir".into(),
            "Inyecta el comando de opencode en la terminal del panel derecho",
            0,
        ));
        v.push(s(
            "agent.restart",
            3,
            4,
            "Reiniciar la terminal",
            "reiniciar".into(),
            "Relanza la shell del panel derecho en la raíz del proyecto",
            0,
        ));
        v.push(b(
            "agent.show",
            3,
            "Mostrar el panel del agente",
            self.show_right && self.docks[1] == Dock::Right,
            "Igual que el botón de layout derecho",
            0,
        ));

        // --- Proveedores (4) ---
        for p in &self.providers {
            let label = if p.name.is_empty() {
                p.id.clone()
            } else {
                format!("{} ({})", p.name, p.id)
            };
            let hint = if p.base_url.is_empty() {
                "Config global de OpenCode (~/.config/opencode)".to_string()
            } else {
                format!("{} · config global", p.base_url)
            };
            v.push(s(
                &format!("ocp.edit:{}", p.id),
                4,
                4,
                &label,
                "editar".into(),
                &hint,
                0,
            ));
        }
        v.push(s(
            "ocp.add",
            4,
            4,
            "Agregar proveedor",
            "nuevo".into(),
            "Da de alta un proveedor nuevo en la config global de OpenCode",
            0,
        ));

        // --- Despliegue (5) ---
        v.push(s(
            "site.transport",
            5,
            2,
            "Transporte",
            self.site.transport.label().to_string(),
            "FTP (21), FTPS (FTP + TLS) o SFTP (SSH, 22)",
            1,
        ));
        v.push(s(
            "site.host",
            5,
            0,
            "Host",
            self.site.host.clone(),
            "Servidor del sitio, ej. ftp.midominio.com",
            1,
        ));
        v.push(s(
            "site.user",
            5,
            0,
            "Usuario",
            self.site.user.clone(),
            "Usuario de FTP/SFTP del hosting",
            1,
        ));
        v.push(s(
            "site.pass",
            5,
            5,
            "Contraseña",
            self.site.pass.clone(),
            "Se guarda en %APPDATA%\\NuIde, indexada por proyecto: nunca en el repositorio",
            1,
        ));
        v.push(s(
            "site.remote",
            5,
            0,
            "Carpeta remota",
            self.site.remote_root.clone(),
            "Ruta en el servidor, ej. public_html",
            1,
        ));
        v.push(s(
            "site.local-sub",
            5,
            0,
            "Subcarpeta local a subir",
            self.site.local_sub.clone(),
            "Qué carpeta del proyecto se despliega (vacío = todo)",
            1,
        ));
        v.push(s(
            "site.include",
            5,
            0,
            "Extensiones",
            self.site.include.clone(),
            "Lista separada por coma de extensiones que se suben",
            1,
        ));
        v.push(s(
            "site.script",
            5,
            0,
            "Script de despliegue",
            self.site.script.clone(),
            "Script SpASM que decide qué subir; imprime líneas UPLOAD/SKIP/LOG",
            1,
        ));
        v.push(s(
            "site.test",
            5,
            4,
            "Probar conexión",
            "probar".into(),
            "Lista el directorio remoto con curl sin subir nada",
            0,
        ));
        v.push(s(
            "site.upload",
            5,
            4,
            "Subir cambios",
            "subir".into(),
            "Corre el script (o el escaneo) y transfiere con curl/scp",
            0,
        ));
        v.push(s(
            "site.ssh",
            5,
            4,
            "Terminal SSH",
            "abrir".into(),
            "Sesión ssh user@host en una pestaña de terminal",
            0,
        ));

        // --- Módulos (6) ---
        v.push(b(
            "web.enabled",
            6,
            "Servidor web local",
            self.web.enabled,
            "Sirve la carpeta del proyecto en http://localhost:<puerto>/ con CORS",
            1,
        ));
        v.push(s(
            "web.port",
            6,
            0,
            "Puerto del servidor",
            self.web.port.to_string(),
            "Puerto local (1024 o más)",
            1,
        ));
        v.push(s(
            "web.root",
            6,
            0,
            "Carpeta servida",
            self.web.root_sub.clone(),
            "Subcarpeta del proyecto; vacío = la raíz",
            1,
        ));
        v.push(s(
            "web.cors",
            6,
            0,
            "CORS: origen permitido",
            self.web.cors_origin.clone(),
            "Access-Control-Allow-Origin; * para cualquiera",
            1,
        ));
        v.push(b(
            "sqlite.enabled",
            6,
            "Visor de SQLite",
            self.sqlite_enabled,
            "Panel para mirar las tablas de una base .db del proyecto",
            0,
        ));
        v.push(s(
            "sqlite.db",
            6,
            0,
            "Base abierta",
            self.sqlite_db
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "Ruta del archivo .db / .sqlite",
            1,
        ));
        v.push(s(
            "sqlite.pick",
            6,
            4,
            "Elegir base",
            "buscar".into(),
            "Abre el diálogo nativo para elegir el archivo",
            0,
        ));
        v.push(s(
            "sqlite.reload",
            6,
            4,
            "Refrescar el visor",
            "recargar".into(),
            "Vuelve a leer tablas y filas",
            0,
        ));

        // --- Atajos (7) ---
        for (id, label, _) in KEY_ACTIONS {
            let chord = self.keymap.get(*id).cloned().unwrap_or_default();
            v.push(s(
                id,
                7,
                4,
                label,
                chord,
                "Clic y presioná la combinación nueva",
                0,
            ));
        }
        v
    }

    /// Aplica el cambio de una fila de la pantalla de configuración.
    fn setting_set(&mut self, id: &str, value: &str) {
        match id {
            "ui.font.mono" => self.prefs_cycle_mono(),
            "ui.font.sans" => self.prefs_cycle_sans(),
            "ui.font.size" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.editor_font_size = v.clamp(9.0, 24.0);
                    self.save_prefs();
                }
            }
            "ui.accent" => self.prefs_cycle_accent(),
            "ui.alpha" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.panel_alpha = v.clamp(0.6, 1.0);
                    self.save_prefs();
                }
            }
            "ui.show.left" => {
                if self.show_left != (value == "1") {
                    self.toggle_region(0);
                }
            }
            "ui.show.right" => {
                if self.show_right != (value == "1") {
                    self.toggle_region(1);
                }
            }
            "ui.show.bottom" => {
                if self.show_bottom != (value == "1") {
                    self.toggle_region(2);
                }
            }
            "ed.tab-size" => self.cycle_tab_size(),
            "ed.insert-spaces" => self.toggle_insert_spaces(),
            "ed.generated" => self.explorer_toggle_generated(),
            "ed.build-on-save" => self.run_toggle_build_on_save(),
            "ed.run-args" => {
                self.run_args = value.trim().to_string();
                self.config.set("run.args", &self.run_args);
                self.config.save();
            }
            "ed.run-entry" => {
                self.config.set("run.entry", value.trim());
                self.config.save();
            }
            "agent.version" => {}
            "agent.model" => {}
            "site.host" => self.site_field_set("host", value.to_string()),
            "site.user" => self.site_field_set("user", value.to_string()),
            "site.pass" => self.site_field_set("pass", value.to_string()),
            "site.remote" => self.site_field_set("remote", value.to_string()),
            "site.local-sub" => self.site_field_set("local-sub", value.to_string()),
            "site.include" => self.site_field_set("include", value.to_string()),
            "site.script" => self.site_field_set("script", value.to_string()),
            "site.transport" => self.prefs_cycle_transport(),
            "web.enabled" => self.toggle_web(),
            "web.port" => self.web_field_set("port", value.to_string()),
            "web.root" => self.web_field_set("root", value.to_string()),
            "web.cors" => self.web_field_set("cors", value.to_string()),
            "sqlite.enabled" => self.toggle_sqlite_module(),
            "sqlite.db" => {
                let p = PathBuf::from(value.trim());
                if p.is_file() {
                    self.sqlite_db = Some(p);
                    self.sqlite_reload();
                }
            }
            _ => {}
        }
    }

    /// Ejecuta la acción de una fila (botones y choices que no tienen valor).
    fn setting_activate(&mut self, id: &str) {
        match id {
            "term.new" => self.new_session(),
            "term.restart" => self.restart_session(),
            "term.split" => self.split_terminal(),
            "agent.launch" => self.launch_agent(),
            "agent.restart" => {
                self.agent = None;
                self.start_agent();
            }
            "agent.show" => self.toggle_region(1),
            "site.test" => self.deploy_test(),
            "site.upload" => self.deploy_upload(),
            "site.ssh" => self.deploy_ssh_terminal(),
            "sqlite.pick" => self.sqlite_pick_db(),
            "sqlite.reload" => self.sqlite_reload(),
            "ocp.add" => self.provider_open_new(),
            id if id.starts_with("ocp.edit:") => {
                self.provider_open_edit(&id["ocp.edit:".len()..]);
            }
            other => {
                // Un atajo: empieza a capturar la combinación nueva.
                if KEY_ACTIONS.iter().any(|(k, _, _)| *k == other) {
                    self.prefs_capture(other);
                }
            }
        }
    }

    /// Abre o cierra la pantalla de configuración.
    fn settings_toggle(&mut self) {
        self.settings_open = !self.settings_open;
        if !self.settings_open {
            self.capturing_key = None;
        }
    }

    /// Vuelca el estado de la pantalla de configuración a la ventana.
    fn sync_settings(&self, w: &NuIdeWindow) {
        w.set_settings_open(self.settings_open);
        w.set_settings_section(self.settings_section as i32);
        w.set_settings_sections(
            Self::SETTINGS_SECTIONS
                .iter()
                .map(|s| slint::SharedString::from(*s))
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
        );
        let rows: Vec<IdeSetting> = self
            .settings_rows()
            .into_iter()
            .filter(|r| r.section == self.settings_section as i32)
            .collect();
        w.set_settings_rows(rows.as_slice().into());
    }

    // --- CRUD de proveedores de OpenCode (config global) ---

    /// Abre el formulario vacío (dar de alta).
    fn provider_open_new(&mut self) {
        self.providers = crate::providers::load_from(&crate::providers::config_dir());
        self.provider_draft = ProviderDraft::default();
        self.provider_note.clear();
        self.provider_open = true;
    }

    /// Abre el formulario con un proveedor existente.
    fn provider_open_edit(&mut self, id: &str) {
        self.providers = crate::providers::load_from(&crate::providers::config_dir());
        let Some(p) = self.providers.iter().find(|p| p.id == id) else {
            return;
        };
        self.provider_draft = ProviderDraft {
            original_id: p.id.clone(),
            id: p.id.clone(),
            name: p.name.clone(),
            package: p.package.clone(),
            base_url: p.base_url.clone(),
            api_key: p.api_key.clone(),
            models: p.models.clone(),
        };
        self.provider_note.clear();
        self.provider_open = true;
    }

    /// Actualiza un campo del borrador desde el formulario.
    fn provider_set(&mut self, field: &str, value: &str) {
        match field {
            "id" => self.provider_draft.id = value.into(),
            "name" => self.provider_draft.name = value.into(),
            "package" => self.provider_draft.package = value.into(),
            "base-url" => self.provider_draft.base_url = value.into(),
            "api-key" => self.provider_draft.api_key = value.into(),
            "models" => self.provider_draft.models = value.into(),
            _ => {}
        }
        self.provider_note.clear();
    }

    /// Valida el borrador y lo escribe en la config global (union → json).
    fn provider_save(&mut self) {
        // El id es la clave del objeto; sin espacios ni llaves.
        let id = self.provider_draft.id.trim().to_string();
        if id.is_empty()
            || id
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '"' | '{' | '}' | '[' | ']' | ':' | ','))
        {
            self.provider_note =
                "El ID no puede estar vacío ni tener espacios, llaves o comas.".into();
            return;
        }
        if self
            .providers
            .iter()
            .any(|p| p.id == id && p.id != self.provider_draft.original_id)
        {
            self.provider_note = format!("Ya existe un proveedor llamado «{id}».");
            return;
        }
        // Los modelos se validan como objeto JSON (dijo el usuario: JSON crudo).
        let models = if self.provider_draft.models.trim().is_empty() {
            "{}".to_string()
        } else {
            self.provider_draft.models.trim().to_string()
        };
        match crate::jsonx::parse(&models) {
            Ok(crate::jsonx::Value::Object(_)) => {}
            Ok(_) => {
                self.provider_note = "«Modelos» tiene que ser un objeto JSON (con llaves).".into();
                return;
            }
            Err(e) => {
                self.provider_note = format!("«Modelos» no es JSON válido: {e}");
                return;
            }
        }

        let name = self.provider_draft.name.trim().to_string();
        let package = self.provider_draft.package.trim().to_string();
        let base_url = self.provider_draft.base_url.trim().to_string();
        let api_key = self.provider_draft.api_key.trim().to_string();

        let mut list = std::mem::take(&mut self.providers);
        match list
            .iter_mut()
            .find(|p| p.id == self.provider_draft.original_id)
        {
            Some(p) => {
                // No pisar con vacío lo que el proveedor resolvía por `env`
                // u otro mecanismo: sólo se escribe un campo si el usuario puso
                // algo o si el proveedor ya lo tenía.
                let had_package = !p.package.is_empty();
                let had_base_url = !p.base_url.is_empty();
                let had_api_key = !p.api_key.is_empty();
                p.rename(&id);
                p.set_name(&name);
                if !package.is_empty() || had_package {
                    p.set_package(&package);
                }
                if !base_url.is_empty() || had_base_url {
                    p.set_base_url(&base_url);
                }
                if !api_key.is_empty() || had_api_key {
                    p.set_api_key(&api_key);
                }
                p.set_models(&models);
            }
            None => {
                let p = crate::providers::Provider::from_fields(
                    &id, &name, &package, &base_url, &api_key, &models,
                );
                list.push(p);
            }
        }
        match crate::providers::save_into(&crate::providers::config_dir(), &list) {
            Ok(_) => {
                self.provider_open = false;
                self.provider_note.clear();
                self.providers = list;
                // Volver a ver la lista (la sección pudo haber quedado en otra).
                self.settings_section = 4;
            }
            Err(e) => {
                self.provider_note = e;
                self.providers = list;
            }
        }
    }

    /// Pide confirmación antes de eliminar el proveedor del borrador.
    fn provider_delete_request(&mut self) {
        if !self.provider_draft.original_id.is_empty() {
            self.ask = Some(Ask::DeleteProvider(self.provider_draft.original_id.clone()));
        }
    }

    /// Elimina el proveedor de la config global (después de confirmar).
    fn provider_do_delete(&mut self, id: &str) {
        let mut list = std::mem::take(&mut self.providers);
        list.retain(|p| p.id != id);
        match crate::providers::save_into(&crate::providers::config_dir(), &list) {
            Ok(_) => {
                self.provider_open = false;
                self.provider_note.clear();
                self.providers = list;
            }
            Err(e) => {
                self.provider_note = e;
                self.providers = list;
            }
        }
    }

    /// Cierra el formulario sin guardar.
    fn provider_cancel(&mut self) {
        self.provider_open = false;
        self.provider_note.clear();
    }

    /// Teclado del formulario: Esc cancela; el resto no toca el editor.
    fn provider_intercept(&mut self, text: &str, control: bool) -> bool {
        if self.provider_open {
            if !control && text == "\u{1b}" {
                self.provider_cancel();
            }
            true
        } else {
            false
        }
    }

    /// Vuelca un mensaje del hilo de despliegue en la sesión `build`.
    fn deploy_message(&mut self, msg: DeployMsg) {
        let line = match msg {
            DeployMsg::Log(text) => format!("{text}\n"),
            DeployMsg::Sent { path, ok } => {
                format!("{} {}\n", if ok { "  ok  " } else { "  FAIL" }, path)
            }
            DeployMsg::Aborted(text) => {
                self.deploy_busy = false;
                format!("[despliegue abortado: {text}]\n")
            }
            DeployMsg::Finished {
                sent,
                skipped,
                failed,
            } => {
                self.deploy_busy = false;
                format!(
                    "\n[despliegue: {sent} subido(s), {skipped} saltado(s), {failed} fallido(s)]\n"
                )
            }
        };
        self.write_build_output(&line);
    }

    fn prefs_cycle_mono(&mut self) {
        self.mono = next_family(MONO_FONTS, &self.mono);
        self.save_prefs();
    }

    fn prefs_cycle_sans(&mut self) {
        self.sans = next_family(SANS_FONTS, &self.sans);
        self.save_prefs();
    }

    fn prefs_font_delta(&mut self, delta: f32) {
        self.editor_font_size = (self.editor_font_size + delta).clamp(9.0, 24.0);
        self.save_prefs();
    }

    /// Cicla la opacidad de los paneles (Preferencias → Opacidad).
    fn prefs_cycle_alpha(&mut self) {
        let idx = PANEL_ALPHAS
            .iter()
            .position(|a| (*a - self.panel_alpha).abs() < 0.001)
            .unwrap_or(0);
        self.panel_alpha = PANEL_ALPHAS[(idx + 1) % PANEL_ALPHAS.len()];
        self.save_prefs();
    }

    fn prefs_cycle_accent(&mut self) {
        if !ACCENTS.is_empty() {
            self.accent = (self.accent + 1) % ACCENTS.len();
            self.save_prefs();
        }
    }

    /// Empieza (o cancela) la captura de una combinación para `action`.
    fn prefs_capture(&mut self, action: &str) {
        if self.capturing_key.as_deref() == Some(action) {
            self.capturing_key = None;
        } else {
            self.capturing_key = Some(action.to_string());
        }
    }

    /// Color del acento actual.
    fn accent_rgb(&self) -> (u8, u8, u8) {
        ACCENTS
            .get(self.accent)
            .map(|(_, c)| *c)
            .unwrap_or((0x83, 0xd9, 0xcb))
    }

    /// Nombre del acento actual.
    fn accent_name(&self) -> String {
        ACCENTS
            .get(self.accent)
            .map(|(n, _)| (*n).to_string())
            .unwrap_or_default()
    }

    /// Aplica las preferencias visuales a los globales de Slint.
    fn apply_theme(&self, window: &NuIdeWindow) {
        let theme = window.global::<crate::ui::IdeTheme>();
        theme.set_sans(self.sans.clone().into());
        theme.set_mono(self.mono.clone().into());
        theme.set_editor_font_size(self.editor_font_size);
        let (r, g, b) = self.accent_rgb();
        theme.set_mint(slint::Color::from_rgb_u8(r, g, b));
        theme.set_panel_alpha(self.panel_alpha);
    }

    /// Persiste fuentes, tamaño, acento y opacidad de los paneles.
    fn save_prefs(&mut self) {
        self.config.set("font.sans", &self.sans);
        self.config.set("font.mono", &self.mono);
        self.config.set("font.editor-size", self.editor_font_size);
        self.config.set("theme.accent", self.accent);
        self.config.set("theme.panel-alpha", self.panel_alpha);
        self.config.save();
    }

    /// Acción global cuyo atajo coincide con la tecla (si hay).
    fn keymap_action(&self, text: &str, control: bool, shift: bool, alt: bool) -> Option<String> {
        let chord = chord_of(text, control, shift, alt)?;
        self.keymap
            .iter()
            .find(|(_, c)| **c == chord)
            .map(|(a, _)| a.clone())
    }

    /// Ejecuta una acción global.
    fn run_key_action(&mut self, action: &str) {
        match action {
            "palette" => self.palette_open(PaletteKind::Commands),
            "files" => self.palette_open(PaletteKind::Files),
            "search" => self.search_open(),
            "help" => self.spasm_help(),
            _ => {}
        }
    }

    /// Si se está capturando un atajo, lo asigna. Devuelve si consumió la tecla.
    fn capture_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) -> bool {
        let Some(action) = self.capturing_key.clone() else {
            return false;
        };
        if text == "\u{1b}" {
            self.capturing_key = None;
            return true;
        }
        if let Some(chord) = chord_of(text, control, shift, alt) {
            self.keymap.insert(action.clone(), chord.clone());
            self.config.set(&format!("keys.{action}"), &chord);
            self.config.save();
            self.capturing_key = None;
        }
        true
    }

    /// Filas de atajos para Preferencias.
    fn prefs_keys(&self) -> Vec<IdePrefRow> {
        KEY_ACTIONS
            .iter()
            .map(|(id, label, _)| IdePrefRow {
                id: (*id).into(),
                label: (*label).into(),
                value: self.keymap.get(*id).cloned().unwrap_or_default().into(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Sesiones
// ---------------------------------------------------------------------------

impl Ide {
    fn select_session(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.local.len() {
            self.local_active = i as usize;
            self.split = None;
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
        self.split = None;
        self.local.remove(self.local_active);
        if self.local_active >= self.local.len() {
            self.local_active = self.local.len() - 1;
        }
        self.resize_local();
    }

    /// Reinicia la sesión activa: la reemplaza por una shell nueva en la raíz
    /// del proyecto (conserva la posición en la barra de sesiones).
    fn restart_session(&mut self) {
        if self.local.is_empty() {
            self.new_local_session();
            return;
        }
        self.split = None;
        let index = self.local_active;
        self.local.remove(index);
        self.new_local_session(); // agrega al final y la deja activa
        let last = self.local.len() - 1;
        let session = self.local.remove(last);
        let at = index.min(self.local.len());
        self.local.insert(at, session);
        self.local_active = at;
        self.resize_local();
    }

    /// Pide un nombre nuevo para la sesión de terminal activa.
    fn prompt_rename_session(&mut self) {
        if self.local.is_empty() {
            return;
        }
        let current = self.local[self.local_active].name.clone();
        self.prompt = Some(Prompt {
            kind: PromptKind::SessionName,
            target: PathBuf::new(),
            text: current,
        });
    }

    fn split_terminal(&mut self) {
        // Divide el panel en dos vistas (o lo vuelve a una): la segunda usa otra
        // sesión; si no hay, abre una shell nueva.
        if self.split.is_some() {
            self.split = None;
        } else {
            if self.local.len() < 2 {
                self.new_local_session();
            }
            self.split = Some(if self.local_active == 0 { 1 } else { 0 });
        }
        self.resize_local();
    }

    /// Pone el foco en la vista izquierda (0) o derecha (1) de la división.
    fn focus_pane(&mut self, pane: i32) {
        if pane == 1 {
            if let Some(other) = self.split {
                let active = self.local_active;
                self.local_active = other;
                self.split = Some(active);
                self.resize_local();
            }
        }
    }

    /// Avisa a la shell local el tamaño actual del panel. En vista dividida,
    /// cada panel ocupa la mitad del ancho.
    fn resize_local(&mut self) {
        let split = self.split.is_some();
        let cols = if split {
            (self.local_cols / 2).max(20)
        } else {
            self.local_cols
        };
        let rows = self.local_rows;
        let mut indices = vec![self.local_active];
        if let Some(other) = self.split {
            if other != self.local_active {
                indices.push(other);
            }
        }
        for index in indices {
            if let Some(session) = self.local.get_mut(index) {
                session.screen.resize(rows, cols);
                if let Some(pty) = session.pty.as_ref() {
                    let _ = pty.resize(cols, rows);
                }
            }
        }
    }

    /// Avisa al panel del agente el tamaño actual.
    fn resize_agent(&mut self) {
        self.resize_target(Target::Agent);
    }

    /// Aplica a la terminal indicada el tamaño que ya tiene registrado (resize
    /// del emulador y SIGWINCH del PTY).
    fn resize_target(&mut self, target: Target) {
        let (cols, rows) = self.dimensions(target);
        if let Some(session) = self.session_mut(target) {
            session.screen.resize(rows, cols);
            if let Some(pty) = session.pty.as_ref() {
                let _ = pty.resize(cols, rows);
            }
        }
    }

    /// La UI informa el tamaño del área de pantalla de la terminal local (en
    /// píxeles, sin el encabezado).
    fn local_resize(&mut self, w: f32, h: f32) {
        self.local_px = (w, h);
        let (cols, rows) = cells_for(w, h, self.cell_w);
        if cols != self.local_cols || rows != self.local_rows {
            self.local_cols = cols;
            self.local_rows = rows;
            self.resize_local();
        }
    }

    /// Igual que `local_resize`, pero para el panel del agente.
    fn agent_resize(&mut self, w: f32, h: f32) {
        self.agent_px = (w, h);
        let (cols, rows) = cells_for(w, h, self.cell_w);
        if cols != self.agent_cols || rows != self.agent_rows {
            self.agent_cols = cols;
            self.agent_rows = rows;
            self.resize_agent();
        }
    }

    /// La UI mide el ancho real de una celda monoespaciada; cuando cambia, se
    /// recalculan las columnas del PTY para que coincidan con la grilla.
    fn sync_cell_width(&mut self, window: &NuIdeWindow) {
        let width = window.get_cell_width();
        if (4.0..=24.0).contains(&width) && (width - self.cell_w).abs() >= 0.001 {
            self.cell_w = width;
            let (lw, lh) = self.local_px;
            let (aw, ah) = self.agent_px;
            self.local_resize(lw, lh);
            self.agent_resize(aw, ah);
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
        push_recent(&mut self.recent_files, path.clone());
        self.active_file = Some(path.clone());
        if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
            self.active_tab = i;
            self.editor_full = true;
            self.undo.clear();
            self.find_after_tab_change();
            self.lint_active();
            return;
        }
        let (lines, trailing_newline, read_only) = if is_image(&path) {
            (Vec::new(), false, true)
        } else {
            read_lines(&path)
        };
        let is_img = is_image(&path);
        let kind = if is_img {
            DocKind::Image
        } else {
            DocKind::Text
        };
        let image = if is_img {
            slint::Image::load_from_path(&path).ok()
        } else {
            None
        };
        self.tabs.push(OpenTab {
            path: path.clone(),
            kind,
            image,
            image_zoom: 1.0,
            language: highlight::Language::from_path(&path),
            preview: false,
            preview_doc: None,
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
        self.find_after_tab_change();
        self.lint_active();
    }

    fn tab_activated(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.tabs.len() {
            self.active_tab = i as usize;
            self.active_file = Some(self.tabs[self.active_tab].path.clone());
            self.editor_full = true;
            self.undo.clear();
            self.find_after_tab_change();
            self.lint_active();
        }
    }

    /// Con la búsqueda abierta, recalcula las coincidencias al cambiar de
    /// pestaña (las de la anterior ya no valen).
    fn find_after_tab_change(&mut self) {
        if self.find.open {
            self.find_recompute();
        }
    }

    /// Alterna la vista previa (sólo para Markdown).
    fn toggle_preview(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if tab.language == highlight::Language::Markdown {
                tab.preview = !tab.preview;
                if tab.preview {
                    let source = tab.lines.join("\n");
                    tab.preview_doc = Some(markdown_preview(&source));
                } else {
                    tab.preview_doc = None;
                }
            }
        }
    }

    /// Cambia el zoom del visor de imagen (`factor` > 1 acerca).
    fn image_zoom_by(&mut self, factor: f32) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if tab.kind == DocKind::Image {
                tab.image_zoom = (tab.image_zoom * factor).clamp(0.1, 8.0);
            }
        }
    }

    /// Vuelve el zoom al 100% (ajustada al panel).
    fn image_zoom_reset(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if tab.kind == DocKind::Image {
                tab.image_zoom = 1.0;
            }
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
        // Diálogo de entrada, paleta/búsqueda abiertas, captura de atajo y
        // atajos globales configurables; después, la edición normal.
        if self.provider_intercept(text, control) {
            return;
        }
        if self.prompt_intercept(text, control) {
            return;
        }
        if self.palette_intercept(text, control, shift) {
            return;
        }
        if self.search_intercept(text, control, shift) {
            return;
        }
        if self.capture_key(text, control, shift, false) {
            return;
        }
        if let Some(action) = self.keymap_action(text, control, shift, false) {
            self.run_key_action(&action);
            return;
        }
        if self.tabs.is_empty() {
            return;
        }
        // Una imagen no se edita.
        if self.tabs[self.active_tab].kind == DocKind::Image {
            return;
        }
        // Atajos globales del IDE, aun con la búsqueda abierta.
        if text == "\u{F708}" {
            self.run(None);
            return;
        }
        if text == "\u{F709}" {
            // F6: analiza el archivo activo con el parser de SpASM.
            self.lint_active();
            return;
        }
        if text == "\u{F70b}" {
            // F8 / Shift+F8: siguiente / anterior diagnóstico.
            self.diagnostic_step(!shift);
            return;
        }
        if text == "\u{F70f}" {
            // F12: ir a la definición del símbolo bajo el cursor.
            self.go_to_definition();
            return;
        }
        if control && shift && text.eq_ignore_ascii_case("m") {
            // Ctrl+Shift+M: vista previa de Markdown.
            self.toggle_preview();
            return;
        }
        if control && shift && text.eq_ignore_ascii_case("s") {
            // Ctrl+Shift+S: guardar como.
            self.save_as();
            return;
        }
        if control && shift && text.eq_ignore_ascii_case("b") {
            // Ctrl+Shift+B: compilar y ejecutar.
            self.build(true);
            return;
        }
        if control {
            match text.to_ascii_lowercase().as_str() {
                "s" => {
                    self.save_active();
                    return;
                }
                "f" => {
                    let show_replace = self.find.replace_open;
                    self.find_open(show_replace);
                    return;
                }
                "h" => {
                    self.find_open(true);
                    return;
                }
                "b" => {
                    self.build(false);
                    return;
                }
                _ => {}
            }
        }
        // Con la barra de búsqueda abierta, el teclado edita la búsqueda.
        if self.find.open {
            self.find_key(text, control, shift);
            return;
        }
        if control {
            match text.to_ascii_lowercase().as_str() {
                "z" if shift => self.redo_active(),
                "z" => self.undo_active(),
                "y" => self.redo_active(),
                "a" => self.select_all(),
                "c" => self.copy_active(),
                "x" => self.cut_active(),
                "v" => self.paste_active(),
                "n" => self.new_file(),
                "o" => self.pick_file(),
                "w" => self.request_close_tab(self.active_tab as i32),
                "g" => self.palette_open(PaletteKind::GoToLine),
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
                '\t' => self.insert_tab(),
                '\u{F700}' | '\u{F701}' | '\u{F702}' | '\u{F703}' | '\u{F729}' | '\u{F72b}'
                | '\u{F72c}' | '\u{F72d}' => self.move_for_key(ch, shift),
                '\u{1b}' => {}
                c if (' '..'\u{E000}').contains(&c) => self.insert_char(c),
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
            // Al guardar, corre el analizador de SpASM.
            self.lint_active();
            // El archivo cambió: se relee el estado de Git.
            if !cfg!(test) {
                self.git_refresh();
            }
            // Compilar al guardar (opt-in), si el archivo es `.spasm`.
            let is_spasm = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("spasm"));
            if self.run_build_on_save && is_spasm && !cfg!(test) {
                self.build(false);
            }
        }
    }

    /// Corre el analizador de SpASM sobre el archivo activo, si es `.spasm` y
    /// están el intérprete de Python y el frontend. Trabaja en un hilo.
    fn lint_active(&mut self) {
        // En las pruebas no se lanza Python; el linter se cubre con `lint::tests`.
        if cfg!(test) {
            return;
        }
        let Some(path) = self.tabs.get(self.active_tab).map(|t| t.path.clone()) else {
            self.clear_diagnostics();
            return;
        };
        let is_spasm = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("spasm"));
        let tools = self
            .toolchain
            .python
            .clone()
            .zip(self.toolchain.frontend.clone());
        match (is_spasm, tools) {
            (true, Some((python, frontend))) => {
                lint::spawn(python, frontend, path, self.lint_tx.clone());
            }
            _ => self.clear_diagnostics(),
        }
    }

    /// Borra los diagnósticos y el esquema (al cambiar a un archivo que no se
    /// analiza).
    fn clear_diagnostics(&mut self) {
        if !self.diagnostics.is_empty() || !self.outline.is_empty() {
            self.diagnostics.clear();
            self.outline.clear();
            self.diagnostics_path = None;
            self.editor_full = true;
        }
    }

    /// Texto de la barra de estado con el resultado del último análisis.
    fn diagnostics_summary(&self) -> String {
        match self.diagnostics.first() {
            None => String::new(),
            Some(d) if d.line == 0 => format!("error: {}", d.message),
            Some(d) => format!("error (línea {}): {}", d.line, d.message),
        }
    }

    fn undo_active(&mut self) {
        if let Some(ops) = self.undo.pop() {
            // Guardar operaciones inversas para redo
            let mut inverse_ops = Vec::new();
            for op in ops.iter().rev() {
                if let Some(inv) = self.apply_undo_op(op) {
                    inverse_ops.push(inv);
                }
            }
            if !inverse_ops.is_empty() {
                self.redo.push(inverse_ops);
            }
            self.editor_full = true;
        }
    }

    fn redo_active(&mut self) {
        if let Some(ops) = self.redo.pop() {
            // Guardar operaciones inversas para undo
            let mut inverse_ops = Vec::new();
            for op in ops.iter().rev() {
                if let Some(inv) = self.apply_undo_op(op) {
                    inverse_ops.push(inv);
                }
            }
            if !inverse_ops.is_empty() {
                self.undo.push(inverse_ops);
            }
            self.editor_full = true;
        }
    }

    /// Guarda operaciones incrementales para undo (nuevo sistema eficiente).
    fn push_undo_ops(&mut self, ops: Vec<EditOp>) {
        if !ops.is_empty() {
            self.undo.push(ops);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Guarda un snapshot completo (sistema legacy para operaciones complejas).
    fn push_undo(&mut self) {
        let snap = self.tabs[self.active_tab].snapshot();
        self.undo.push(vec![EditOp::Snapshot {
            lines: snap.0,
            cursor: snap.1,
        }]);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Aplica una operación de undo y devuelve la operación inversa para redo.
    fn apply_undo_op(&mut self, op: &EditOp) -> Option<EditOp> {
        let tab = self.tabs.get_mut(self.active_tab)?;
        match op {
            EditOp::InsertChar { line, col, ch } => {
                if let Some(l) = tab.lines.get_mut(*line) {
                    let inverse = EditOp::DeleteChar {
                        line: *line,
                        col: *col,
                        ch: *ch,
                    };
                    l.remove(*col);
                    Some(inverse)
                } else {
                    None
                }
            }
            EditOp::DeleteChar { line, col, ch } => {
                if let Some(l) = tab.lines.get_mut(*line) {
                    let inverse = EditOp::InsertChar {
                        line: *line,
                        col: *col,
                        ch: *ch,
                    };
                    l.insert(*col, *ch);
                    Some(inverse)
                } else {
                    None
                }
            }
            EditOp::InsertLine { after, content } => {
                let inverse = EditOp::DeleteLine { line: after + 1 };
                tab.lines.insert(after + 1, content.clone());
                Some(inverse)
            }
            EditOp::DeleteLine { line } => {
                if *line < tab.lines.len() {
                    let content = tab.lines.remove(*line);
                    let inverse = EditOp::InsertLine {
                        after: line - 1,
                        content,
                    };
                    Some(inverse)
                } else {
                    None
                }
            }
            EditOp::JoinLine { line } => {
                if *line + 1 < tab.lines.len() {
                    let next = tab.lines.remove(*line + 1);
                    tab.lines[*line].push_str(&next);
                    let inverse = EditOp::SplitLine {
                        line: *line,
                        at: tab.lines[*line].len(),
                    };
                    Some(inverse)
                } else {
                    None
                }
            }
            EditOp::SplitLine { line, at } => {
                if let Some(l) = tab.lines.get(*line) {
                    if *at <= l.len() {
                        let rest = l[*at..].to_string();
                        tab.lines[*line] = l[..*at].to_string();
                        tab.lines.insert(*line + 1, rest);
                        let inverse = EditOp::JoinLine { line: *line };
                        Some(inverse)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            EditOp::Snapshot { lines, cursor } => {
                // Snapshot completo: restaurar todo el documento
                let inverse = EditOp::Snapshot {
                    lines: tab.lines.clone(),
                    cursor: tab.cursor,
                };
                tab.lines = lines.clone();
                tab.cursor = *cursor;
                tab.anchor = None;
                tab.dirty = true;
                tab.clamp_cursor();
                Some(inverse)
            }
        }
    }

    /// Abre la barra de búsqueda. Con `show_replace` se muestra también el campo
    /// de reemplazo.
    fn find_open(&mut self, show_replace: bool) {
        self.find.open = true;
        if show_replace {
            self.find.replace_open = true;
        }
        self.find.focus_replace = false;
        self.find.query_cursor = self.find.query.chars().count();
        self.find.replace_cursor = self.find.replace.chars().count();
        self.find_recompute();
        self.editor_full = true;
    }

    /// Cierra la barra y limpia las coincidencias resaltadas.
    fn find_close(&mut self) {
        self.find.open = false;
        self.find.matches.clear();
        self.find.by_line.clear();
        self.find.current = None;
        self.editor_full = true;
    }

    /// Muestra u oculta el campo de reemplazo.
    fn find_toggle_replace(&mut self) {
        self.find.replace_open = !self.find.replace_open;
        if !self.find.replace_open {
            self.find.focus_replace = false;
        }
        self.editor_full = true;
    }

    /// Alterna distinguir mayúsculas/minúsculas.
    fn find_toggle_case(&mut self) {
        self.find.case_sensitive = !self.find.case_sensitive;
        self.find_recompute();
        self.editor_full = true;
    }

    /// Alterna buscar sólo palabras completas.
    fn find_toggle_word(&mut self) {
        self.find.whole_word = !self.find.whole_word;
        self.find_recompute();
        self.editor_full = true;
    }

    /// Alterna buscar con expresiones regulares.
    fn find_toggle_regex(&mut self) {
        self.find.regex = !self.find.regex;
        self.find_recompute();
        self.editor_full = true;
    }

    /// Alterna buscar sólo dentro de la selección (captura el rango al activar).
    fn find_toggle_selection(&mut self) {
        if self.find.in_selection {
            self.find.in_selection = false;
            self.find.selection = None;
        } else if let Some(range) = self.current_selection() {
            self.find.in_selection = true;
            self.find.selection = Some(range);
        }
        self.find_recompute();
        self.editor_full = true;
    }

    /// Guarda la consulta en el historial (sin duplicados, tope 20).
    fn remember_query(&mut self) {
        let q = self.find.query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.find.history.retain(|h| h != &q);
        self.find.history.push(q);
        if self.find.history.len() > 20 {
            self.find.history.remove(0);
        }
        self.find.history_pos = None;
    }

    /// Recorre el historial de consultas (`delta`: -1 arriba, +1 abajo).
    fn find_history(&mut self, delta: i32) {
        if self.find.history.is_empty() {
            return;
        }
        let len = self.find.history.len() as i32;
        let pos = match self.find.history_pos {
            None => {
                if delta < 0 {
                    len - 1
                } else {
                    return;
                }
            }
            Some(p) => (p as i32 + delta).clamp(0, len - 1),
        };
        self.find.history_pos = Some(pos as usize);
        self.find.query = self.find.history[pos as usize].clone();
        self.find.query_cursor = self.find.query.chars().count();
        self.find_recompute();
        self.editor_full = true;
    }

    /// Pone el foco en uno de los dos campos.
    fn find_focus(&mut self, replace: bool) {
        if replace && !self.find.replace_open {
            self.find.replace_open = true;
        }
        self.find.focus_replace = replace;
        if replace {
            self.find.replace_cursor = self.find.replace.chars().count();
        } else {
            self.find.query_cursor = self.find.query.chars().count();
        }
    }

    /// Recalcula las coincidencias de la pestaña activa y lleva el cursor a la
    /// primera.
    fn find_recompute(&mut self) {
        self.find.matches.clear();
        self.find.by_line.clear();
        self.find.current = None;
        let query = self.find.query.clone();
        if query.is_empty() {
            return;
        }
        let case = self.find.case_sensitive;
        let whole = self.find.whole_word;
        let sel = if self.find.in_selection {
            self.find.selection
        } else {
            None
        };
        let re = if self.find.regex {
            let compiled = build_regex(&query, case);
            if compiled.is_none() {
                return; // expresión inválida: sin coincidencias
            }
            compiled
        } else {
            None
        };
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        for (line, text) in tab.lines.iter().enumerate() {
            if let Some((sl, _, el, _)) = sel {
                if line < sl || line > el {
                    continue;
                }
            }
            let mut hits = match &re {
                Some(re) => find_in_line_regex(text, re, whole),
                None => find_in_line(text, &query, case, whole),
            };
            if let Some((sl, sc, el, ec)) = sel {
                hits.retain(|&(a, b)| !(line == sl && a < sc) && !(line == el && b > ec));
            }
            if !hits.is_empty() {
                self.find
                    .matches
                    .extend(hits.iter().map(|&(a, b)| (line, a, b)));
                self.find.by_line.insert(line, hits);
            }
        }
        self.find.current = (!self.find.matches.is_empty()).then_some(0);
        if let Some((line, start, _)) = self.find.matches.first().copied() {
            let old = self.tabs[self.active_tab].cursor.line;
            self.set_cursor(Cursor { line, col: start }, old, false);
        }
    }

    /// Rango normalizado de la selección activa.
    fn current_selection(&self) -> Option<(usize, usize, usize, usize)> {
        let t = self.tabs.get(self.active_tab)?;
        let a = t.anchor?;
        let c = t.cursor;
        if (a.line, a.col) <= (c.line, c.col) {
            Some((a.line, a.col, c.line, c.col))
        } else {
            Some((c.line, c.col, a.line, a.col))
        }
    }

    /// Salta a la coincidencia siguiente o anterior (circular).
    fn find_step(&mut self, forward: bool) {
        self.remember_query();
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        let previous = self.find.current_range();
        let current = self.find.current.unwrap_or(0);
        let next = if forward {
            (current + 1) % n
        } else {
            (current + n - 1) % n
        };
        self.find.current = Some(next);
        let (line, start, _) = self.find.matches[next];
        let old = self.tabs[self.active_tab].cursor.line;
        self.set_cursor(Cursor { line, col: start }, old, false);
        // Redibuja la fila que dejó de ser la actual (las del cursor ya se
        // actualizaron en `set_cursor`).
        if let Some((label, _, _)) = previous {
            self.update_row(label);
        }
    }

    /// El campo de búsqueda o el de reemplazo junto con su cursor, según el foco.
    fn find_field(&mut self) -> (&mut String, &mut usize) {
        if self.find.focus_replace {
            (&mut self.find.replace, &mut self.find.replace_cursor)
        } else {
            (&mut self.find.query, &mut self.find.query_cursor)
        }
    }

    /// Inserta un carácter en el cursor del campo con foco.
    fn find_insert(&mut self, ch: char) {
        self.find.history_pos = None;
        let (text, cursor) = self.find_field();
        let mut chars: Vec<char> = text.chars().collect();
        let at = (*cursor).min(chars.len());
        chars.insert(at, ch);
        *text = chars.into_iter().collect();
        *cursor = at + 1;
    }

    /// Borra el carácter anterior al cursor.
    fn find_backspace(&mut self) {
        let (text, cursor) = self.find_field();
        let mut chars: Vec<char> = text.chars().collect();
        let at = (*cursor).min(chars.len());
        if at == 0 {
            return;
        }
        chars.remove(at - 1);
        *text = chars.into_iter().collect();
        *cursor = at - 1;
    }

    /// Borra el carácter bajo el cursor.
    fn find_delete(&mut self) {
        let (text, cursor) = self.find_field();
        let mut chars: Vec<char> = text.chars().collect();
        let at = (*cursor).min(chars.len());
        if at >= chars.len() {
            return;
        }
        chars.remove(at);
        *text = chars.into_iter().collect();
    }

    /// Mueve el cursor del campo con foco `delta` posiciones (o a un extremo).
    fn find_move_cursor(&mut self, delta: i32, to_end: bool) {
        let (text, cursor) = self.find_field();
        let len = text.chars().count();
        if to_end {
            *cursor = len;
        } else {
            let at = (*cursor).min(len) as i32;
            *cursor = (at + delta).clamp(0, len as i32) as usize;
        }
    }

    /// Teclado mientras la barra de búsqueda está abierta: edita el campo con
    /// foco (con cursor), navega entre coincidencias y cierra.
    fn find_key(&mut self, text: &str, control: bool, shift: bool) {
        if control {
            match text.to_ascii_lowercase().as_str() {
                "z" => {
                    self.undo_active();
                    self.find_recompute();
                }
                "y" => {
                    self.redo_active();
                    self.find_recompute();
                }
                _ => {}
            }
            return;
        }
        let mut edited = false;
        for ch in text.chars() {
            match ch {
                '\u{1b}' => {
                    self.find_close();
                    return;
                }
                '\n' | '\r' => {
                    self.find_step(!shift);
                    return;
                }
                '\t' => {
                    if self.find.replace_open {
                        self.find.focus_replace = !self.find.focus_replace;
                        self.find_move_cursor(0, true);
                    }
                    return;
                }
                '\u{8}' => {
                    self.find_backspace();
                    edited = true;
                }
                '\u{7f}' => {
                    self.find_delete();
                    edited = true;
                }
                '\u{F702}' => self.find_move_cursor(-1, false),
                '\u{F703}' => self.find_move_cursor(1, false),
                '\u{F700}' if !self.find.focus_replace => self.find_history(-1),
                '\u{F701}' if !self.find.focus_replace => self.find_history(1),
                '\u{F729}' => {
                    let (_, cursor) = self.find_field();
                    *cursor = 0;
                }
                '\u{F72b}' => self.find_move_cursor(0, true),
                c if (' '..'\u{E000}').contains(&c) => {
                    self.find_insert(c);
                    edited = true;
                }
                _ => {}
            }
        }
        if edited {
            self.find_recompute();
            self.editor_full = true;
        }
    }

    /// Reemplaza la coincidencia actual por el texto de reemplazo.
    fn replace_current(&mut self) {
        let Some(index) = self.find.current else {
            return;
        };
        let Some(&(line, start, end)) = self.find.matches.get(index) else {
            return;
        };
        let replacement = self.find.replace.clone();
        self.push_undo();
        {
            let t = &mut self.tabs[self.active_tab];
            let Some(text) = t.lines.get_mut(line) else {
                return;
            };
            let chars: Vec<char> = text.chars().collect();
            let start = start.min(chars.len());
            let end = end.min(chars.len());
            let head: String = chars[..start].iter().collect();
            let tail: String = chars[end..].iter().collect();
            *text = format!("{head}{replacement}{tail}");
            t.dirty = true;
        }
        self.find_recompute();
        self.editor_full = true;
    }

    /// Reemplaza todas las coincidencias de la pestaña activa.
    fn replace_all(&mut self) {
        let query = self.find.query.clone();
        if query.is_empty() {
            return;
        }
        let replacement = self.find.replace.clone();
        let case = self.find.case_sensitive;
        let whole = self.find.whole_word;
        let mut changed = false;
        self.push_undo();
        {
            let t = &mut self.tabs[self.active_tab];
            for line in t.lines.iter_mut() {
                let (updated, count) = replace_in_line(line, &query, &replacement, case, whole);
                if count > 0 {
                    *line = updated;
                    changed = true;
                }
            }
            if changed {
                t.dirty = true;
            }
        }
        if changed {
            self.find_recompute();
            self.editor_full = true;
        }
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

    /// Inserta un carácter en la posición del cursor, con cierre automático de
    /// pares (paréntesis/llaves/comillas) y salto sobre un cierre ya presente.
    fn insert_char(&mut self, ch: char) {
        if !self.tabs[self.active_tab].has_selection() {
            let close = match ch {
                '(' => Some(')'),
                '[' => Some(']'),
                '{' => Some('}'),
                '"' => Some('"'),
                '\'' => Some('\''),
                _ => None,
            };
            if let Some(c) = close {
                if self.skip_over(ch) {
                    return;
                }
                let quote = ch == '"' || ch == '\'';
                if quote
                    && (self.char_before_cursor().is_some_and(is_word_char)
                        || self.char_at_cursor().is_some_and(is_word_char))
                {
                    // No cerrar comillas dentro de una palabra (don't).
                    self.insert_str(&ch.to_string());
                    return;
                }
                self.insert_auto_pair(ch, c);
                return;
            }
            if matches!(ch, ')' | ']' | '}' | '"' | '\'') && self.skip_over(ch) {
                return;
            }
        }
        self.insert_str(&ch.to_string());
    }

    /// Inserta el par `open`+`close` y deja el cursor en el medio.
    fn insert_auto_pair(&mut self, open: char, close: char) {
        self.push_undo();
        self.tabs[self.active_tab].delete_selection();
        let line = self.tabs[self.active_tab].cursor.line;
        {
            let t = &mut self.tabs[self.active_tab];
            let col = t.cursor.col;
            let mut chars: Vec<char> = t.lines[line].chars().collect();
            chars.insert(col, open);
            chars.insert(col + 1, close);
            t.lines[line] = chars.into_iter().collect();
            t.cursor.col = col + 1;
            t.anchor = None;
            t.dirty = true;
        }
        self.update_row(line);
    }

    /// Si el carácter bajo el cursor es `ch`, avanza sobre él (no lo duplica).
    fn skip_over(&mut self, ch: char) -> bool {
        if self.char_at_cursor() != Some(ch) {
            return false;
        }
        self.tabs[self.active_tab].cursor.col += 1;
        let line = self.tabs[self.active_tab].cursor.line;
        self.update_row(line);
        true
    }

    /// Carácter bajo el cursor.
    fn char_at_cursor(&self) -> Option<char> {
        let t = self.tabs.get(self.active_tab)?;
        let chars: Vec<char> = t.lines.get(t.cursor.line)?.chars().collect();
        chars.get(t.cursor.col).copied()
    }

    /// Carácter inmediatamente anterior al cursor.
    fn char_before_cursor(&self) -> Option<char> {
        let t = self.tabs.get(self.active_tab)?;
        if t.cursor.col == 0 {
            return None;
        }
        let chars: Vec<char> = t.lines.get(t.cursor.line)?.chars().collect();
        chars.get(t.cursor.col - 1).copied()
    }

    /// Unidad de indentación (según `insert_spaces` y `tab_size`).
    fn indent_unit(&self) -> String {
        if self.insert_spaces {
            " ".repeat(self.tab_size)
        } else {
            "\t".to_string()
        }
    }

    /// Inserta una tabulación: espacios hasta el ancho configurado, o un `\t`.
    fn insert_tab(&mut self) {
        if self.insert_spaces {
            let unit = self.indent_unit();
            self.insert_str(&unit);
        } else {
            self.insert_char('\t');
        }
    }

    /// Cicla el tamaño de tab: 2 → 4 → 8 → 2.
    fn cycle_tab_size(&mut self) {
        self.tab_size = match self.tab_size {
            2 => 4,
            4 => 8,
            8 => 2,
            _ => 4,
        };
        self.config.set("editor.tab-size", self.tab_size);
        self.config.save();
    }

    /// Alterna insertar espacios vs. tabulaciones.
    fn toggle_insert_spaces(&mut self) {
        self.insert_spaces = !self.insert_spaces;
        self.config
            .set("editor.insert-spaces", u8::from(self.insert_spaces));
        self.config.save();
    }

    /// Inserta un fragmento (sin saltos) en la posición del cursor. Si hay
    /// selección, la reemplaza.
    fn insert_str(&mut self, s: &str) {
        // Tomar snapshot ANTES de modificar (para undo con selección)
        let had_selection = self.tabs[self.active_tab].has_selection();
        if had_selection {
            // Guardar estado actual antes de delete_selection
            let snap = self.tabs[self.active_tab].snapshot();
            self.undo.push(vec![EditOp::Snapshot {
                lines: snap.0,
                cursor: snap.1,
            }]);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
        }

        let crossed = self.tabs[self.active_tab].delete_selection();

        // Registrar operaciones de undo incremental (solo si no había selección)
        if !had_selection {
            let line = self.tabs[self.active_tab].cursor.line;
            let col = self.tabs[self.active_tab].cursor.col;
            let ops: Vec<EditOp> = s
                .chars()
                .enumerate()
                .map(|(i, ch)| EditOp::InsertChar {
                    line,
                    col: col + i,
                    ch,
                })
                .collect();
            self.push_undo_ops(ops);
        }

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
        let unit = self.indent_unit();
        {
            let t = &mut self.tabs[self.active_tab];
            let chars: Vec<char> = t.lines[line].chars().collect();
            let head: String = chars[..col].iter().collect();
            let tail: String = chars[col..].iter().collect();
            // Auto-indentación: se copian los espacios/tabs iniciales.
            let base: String = head
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let opens = head
                .trim_end()
                .chars()
                .last()
                .is_some_and(|c| matches!(c, '(' | '[' | '{'));
            t.lines[line] = head;
            if opens {
                let inner = format!("{base}{unit}");
                let tail_trim = tail.trim_start();
                let tail_is_closer = tail_trim
                    .chars()
                    .next()
                    .is_some_and(|c| matches!(c, ')' | ']' | '}'));
                if tail_trim.is_empty() {
                    t.lines.insert(line + 1, inner);
                    t.cursor = Cursor {
                        line: line + 1,
                        col: base.chars().count() + unit.chars().count(),
                    };
                } else if tail_is_closer {
                    // El cierre se alinea con la línea del abridor.
                    t.lines.insert(line + 1, inner);
                    t.lines.insert(line + 2, format!("{base}{tail_trim}"));
                    t.cursor = Cursor {
                        line: line + 1,
                        col: base.chars().count() + unit.chars().count(),
                    };
                } else {
                    let col = base.chars().count() + unit.chars().count();
                    t.lines.insert(line + 1, format!("{base}{unit}{tail}"));
                    t.cursor = Cursor {
                        line: line + 1,
                        col,
                    };
                }
            } else {
                let col = base.chars().count();
                t.lines.insert(line + 1, format!("{base}{tail}"));
                t.cursor = Cursor {
                    line: line + 1,
                    col,
                };
            }
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
            Some(t) => {
                let deco = self.decorations();
                (0..t.lines.len())
                    .map(|i| self.build_row(i, &deco))
                    .collect()
            }
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
            let deco = self.decorations();
            self.code_model
                .set_row_data(line, self.build_row(line, &deco));
        }
    }

    /// Decoraciones que dependen del cursor y del documento: la palabra bajo el
    /// cursor y las llaves correspondientes. Se calculan una vez por pasada.
    /// Decoraciones del editor: palabra bajo el cursor resaltada y llave correspondiente.
    fn decorations(&self) -> Decorations {
        let mut deco = Decorations::default();
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return deco;
        };
        // Palabra bajo el cursor: se resaltan todas sus apariciones.
        if let Some((start, end)) = word_at(&tab.lines[tab.cursor.line], tab.cursor.col) {
            let word: String = tab.lines[tab.cursor.line]
                .chars()
                .skip(start)
                .take(end - start)
                .collect();
            if word.chars().count() > 1 {
                // Limitar búsqueda a viewport (línea actual ± 50 líneas)
                let viewport_start = tab.cursor.line.saturating_sub(50);
                let viewport_end = (tab.cursor.line + 50).min(tab.lines.len());
                for line in viewport_start..viewport_end {
                    let text = &tab.lines[line];
                    let hits = find_word_in_line(text, &word);
                    if !hits.is_empty() {
                        deco.words.insert(line, hits);
                    }
                }
            }
        }
        // Llave correspondiente.
        if let Some((a, b)) = self.bracket_pair() {
            deco.brackets = vec![a, b];
        }
        deco
    }

    /// Si el cursor está sobre (o justo después de) una llave, devuelve las dos
    /// posiciones `(línea, columna)` de la pareja.
    fn bracket_pair(&self) -> Option<((usize, usize), (usize, usize))> {
        let tab = self.tabs.get(self.active_tab)?;
        let line = tab.cursor.line;
        let chars: Vec<char> = tab.lines.get(line)?.chars().collect();
        for col in [tab.cursor.col, tab.cursor.col.wrapping_sub(1)] {
            let Some(&ch) = chars.get(col) else {
                continue;
            };
            if opener(ch).is_some() {
                if let Some(other) = match_forward(tab, line, col, ch) {
                    return Some(((line, col), other));
                }
            } else if closer(ch).is_some() {
                if let Some(other) = match_backward(tab, line, col, ch) {
                    return Some(((line, col), other));
                }
            }
        }
        None
    }

    /// Arma una fila con el resaltado, la selección, la búsqueda, la palabra y
    /// las llaves que le toquen.
    fn build_row(&self, line: usize, deco: &Decorations) -> IdeCodeLine {
        let tab = &self.tabs[self.active_tab];
        let cursor = (line == tab.cursor.line).then_some(tab.cursor.col);
        let mut marks: Vec<(usize, usize, i32)> = Vec::new();
        if let Some(hits) = deco.words.get(&line) {
            for &(a, b) in hits {
                marks.push((a, b, 11));
            }
        }
        for &(bracket_line, col) in &deco.brackets {
            if bracket_line == line {
                marks.push((col, col + 1, 10));
            }
        }
        if let Some(range) = tab.sel_on_line(line) {
            marks.push((range.0, range.1, 7));
        }
        if let Some(hits) = self.find.by_line.get(&line) {
            let current = self.find.current_range();
            for &(a, b) in hits {
                let is_current = current == Some((line, a, b));
                marks.push((a, b, if is_current { 9 } else { 8 }));
            }
        }
        let toks = render_tokens(&tab.lines[line], tab.language, cursor, &marks);
        let error = self.diagnostics.iter().any(|d| d.line == line + 1);
        IdeCodeLine {
            indent: 0.0,
            error,
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
}

// ---------------------------------------------------------------------------
// Sincronización con la ventana
// ---------------------------------------------------------------------------

impl Ide {
    /// Arma el modelo de una terminal para la UI: las filas (con la selección
    /// pintada) y el estado de la barra de scrollback. Solo lo usan las pruebas:
    /// en runtime el vuelcado va por `sync_terms`, que escribe los modelos en
    /// la ventana.
    #[cfg(test)]
    fn term_view(&mut self, target: Target) -> (Vec<TermLine>, i32, f32, f32) {
        match self.session_mut(target) {
            Some(session) => {
                Self::refresh_term_model(session);
                let (total, pos, thumb) = Self::term_scroll(session);
                (session.term_model.snapshot(), total, pos, thumb)
            }
            None => (Vec::new(), 0, 1.0, 1.0),
        }
    }

    /// Deja el modelo de filas de la sesión al día, **en el mismo `Rc`**: sólo
    /// se reemplazan las filas cuya huella cambió, avisando con `row_changed`.
    /// Reconstruir las 24-200 filas con `remove`+`insert` por chunk de salida era
    /// el costo dominante del refresco (Slint recreaba el elemento y su texto).
    fn refresh_term_model(session: &mut LocalSession) {
        let (rows, cols) = session.screen.size();
        let want = rows as usize;
        let sel = session.sel.and_then(|s| s.region(cols));
        if session.term_model.len() != want {
            // Cambió el alto del panel: se reconstruye de una (pasa al
            // redimensionar o al arrancar, no en el refresco normal).
            let (all, _cursor) = session.screen.snapshot(sel);
            let lines: Vec<TermLine> = all.into_iter().map(Self::term_line).collect();
            session.term_model.set_all(lines);
            // Se guardan las huellas de las filas recién pintadas.
            session.screen.changed_rows(sel);
            return;
        }
        for (index, runs) in session.screen.changed_rows(sel) {
            session.term_model.set_row(index, Self::term_line(runs));
        }
    }

    /// Un tramo por fila, con el ancho en celdas ya contado.
    fn term_line(runs: Vec<Run>) -> TermLine {
        TermLine {
            runs: ModelRc::from(Rc::new(VecModel::from(
                runs.into_iter()
                    .map(|run| {
                        let cols = run.text.chars().count() as i32;
                        TermRun {
                            text: run.text.into(),
                            col: run.col as i32,
                            cols,
                            fg: to_color(run.fg),
                            bg: to_color(run.bg),
                            bold: run.bold,
                        }
                    })
                    .collect::<Vec<_>>(),
            ))),
        }
    }

    /// Estado de la barra de scrollback de la sesión: `(total, posición,
    /// pasador)`. La posición va de 0 (lo más viejo arriba) a 1 (el final).
    fn term_scroll(session: &mut LocalSession) -> (i32, f32, f32) {
        let (visible_rows, _cols) = session.screen.size();
        let total = session.screen.scrollback_total();
        let offset = session.screen.scrollback();
        let total_f = total as f32;
        let pos = if total == 0 {
            1.0
        } else {
            (total - offset) as f32 / total_f
        };
        let visible_f = visible_rows as f32;
        let thumb = if total == 0 {
            1.0
        } else {
            (visible_f / (visible_f + total_f)).clamp(0.08, 1.0)
        };
        (total as i32, pos, thumb)
    }

    /// ¿El panel está a la vista? Lo que no se ve no se refresca: cuando el dock
    /// o la región vuelven a abrirse pasa un `sync` completo y lo pone al día.
    fn dock_visible(&self, dock: Dock) -> bool {
        match dock {
            Dock::Hidden => false,
            Dock::Left => self.show_left,
            Dock::Right => self.show_right,
            Dock::Bottom => self.show_bottom,
        }
    }

    /// Refresco **sólo de los paneles de terminal**: filas, barras de
    /// scrollback y banderas de mouse. Es el camino que dispara la salida del
    /// PTY (varias veces por segundo con una TUI), así que no toca el
    /// explorador, la búsqueda, el diff ni el resto de la ventana.
    fn sync_terms(&mut self, w: &NuIdeWindow) {
        // Docks: 1 = agente, 2 = terminal (ver `sync`).
        let term_visible = self.dock_visible(self.docks[2]);
        let agent_visible = self.dock_visible(self.docks[1]);

        if term_visible {
            // Terminal local: la sesión activa.
            if let Some(session) = self.session_mut(Target::Local) {
                Self::refresh_term_model(session);
                let (total, pos, thumb) = Self::term_scroll(session);
                w.set_term_lines(ModelRc::from(session.term_model.clone()));
                w.set_term_scroll_total(total);
                w.set_term_scroll_pos(pos);
                w.set_term_scroll_thumb(thumb);
            } else {
                w.set_term_lines(ModelRc::from(Rc::new(TermRows::default())));
                w.set_term_scroll_total(0);
                w.set_term_scroll_pos(1.0);
                w.set_term_scroll_thumb(1.0);
            }
            // Mouse reporting: sólo la vista que pidió el proceso lo recibe.
            w.set_term_mouse(self.session_mouse(self.local_active));
            w.set_term_mouse_2(self.split.map(|i| self.session_mouse(i)).unwrap_or(false));

            // Segunda vista de la terminal (si está dividida).
            let split_model: Rc<TermRows> = match self.split {
                Some(index) => match self.local.get_mut(index) {
                    Some(session) => {
                        Self::refresh_term_model(session);
                        Rc::clone(&session.term_model)
                    }
                    None => Rc::new(TermRows::default()),
                },
                None => Rc::new(TermRows::default()),
            };
            w.set_term_split(self.split.is_some());
            w.set_term_lines_2(ModelRc::from(split_model));
        }

        if agent_visible {
            if let Some(session) = self.session_mut(Target::Agent) {
                Self::refresh_term_model(session);
                let (total, pos, thumb) = Self::term_scroll(session);
                w.set_agent_lines(ModelRc::from(session.term_model.clone()));
                w.set_agent_scroll_total(total);
                w.set_agent_scroll_pos(pos);
                w.set_agent_scroll_thumb(thumb);
            } else {
                w.set_agent_lines(ModelRc::from(Rc::new(TermRows::default())));
                w.set_agent_scroll_total(0);
                w.set_agent_scroll_pos(1.0);
                w.set_agent_scroll_thumb(1.0);
            }
            w.set_agent_mouse_tracking(
                self.agent
                    .as_ref()
                    .map(|a| a.screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None)
                    .unwrap_or(false),
            );
            w.set_agent_connected(self.agent.as_ref().is_some_and(|a| a.alive));
        }
    }

    /// Vuelca el estado a la ventana. Idempotente.
    fn sync(&mut self, w: &NuIdeWindow) {
        self.sync_cell_width(w);

        // --- paneles (menú Ver) ---
        w.set_show_left(self.show_left);
        w.set_show_right(self.show_right);
        w.set_show_bottom(self.show_bottom);
        w.set_dock_explorer(self.docks[0].code());
        w.set_dock_agent(self.docks[1].code());
        w.set_dock_terminal(self.docks[2].code());
        w.set_dock_git(self.docks[3].code());
        w.set_menu_open(self.menu_open.clone().into());
        let menu_items: Vec<IdeMenuEntry> = match self.menu_open.as_str() {
            "Archivo" => self.file_menu_items(),
            "Editar" => self.edit_menu_items(),
            "Ayuda" => self.help_menu_items(),
            _ => Vec::new(),
        };
        w.set_menu_items(ModelRc::from(Rc::new(VecModel::from(menu_items))));
        // Uso del sistema (pie).
        w.set_cpu_percent((self.metrics.cpu * 100.0).round() as i32);
        w.set_ram_percent((self.metrics.ram * 100.0).round() as i32);
        // Diálogo de cambios sin guardar.
        w.set_confirm_open(self.confirm.is_some());
        if let Some(c) = self.confirm {
            w.set_confirm_title(
                match c {
                    Confirm::CloseTab => "Cerrar pestaña",
                    Confirm::CloseAll => "Cerrar todas las pestañas",
                    Confirm::Quit => "Salir de NuIde",
                    Confirm::RelaunchElevated => "Reiniciar como administrador",
                }
                .into(),
            );
            let verb = match c {
                Confirm::CloseTab => "cerrar",
                Confirm::CloseAll => "cerrar todas",
                Confirm::Quit => "salir",
                Confirm::RelaunchElevated => "reiniciar como administrador",
            };
            w.set_confirm_message(
                format!("Hay cambios sin guardar. ¿Querés guardarlos antes de {verb}?").into(),
            );
        }

        // --- bienvenida del editor ---
        w.set_welcome(self.tabs.is_empty());
        if self.tabs.is_empty() {
            // Sin archivos no hay migas de pan (evita el demo del `.slint`).
            w.set_breadcrumb(ModelRc::from(Rc::new(VecModel::from(Vec::<
                slint::SharedString,
            >::new()))));
        }
        let recent: Vec<IdeRecent> = self
            .recent_entries()
            .into_iter()
            .map(|e| match e {
                RecentEntry::Project(p) => IdeRecent {
                    label: dir_name(&p).into(),
                    detail: p.display().to_string().into(),
                    kind: 1,
                },
                RecentEntry::File(p) => IdeRecent {
                    label: file_name(&p).into(),
                    detail: p.display().to_string().into(),
                    kind: 0,
                },
            })
            .collect();
        w.set_welcome_recent(ModelRc::from(Rc::new(VecModel::from(recent))));

        // --- terminal real ---
        let sessions: Vec<slint::SharedString> =
            self.local.iter().map(|s| s.name.clone().into()).collect();
        w.set_term_sessions(ModelRc::from(Rc::new(VecModel::from(sessions))));
        w.set_active_session(self.local_active as i32);
        // Filas, barras de scroll y mouse: es el mismo refresco que corre cuando
        // sale el PTY, así que se comparte (ver `sync_terms`).
        self.sync_terms(w);

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
        w.set_explorer_show_generated(self.explorer.shows_generated());
        w.set_explorer_context_open(self.explorer_context_open);
        w.set_explorer_context_items(ModelRc::from(Rc::new(VecModel::from(
            self.explorer_context_items.clone(),
        ))));
        w.set_term_context_open(self.term_context_open);
        w.set_term_context_items(ModelRc::from(Rc::new(VecModel::from(
            self.term_context_items.clone(),
        ))));

        // --- diálogos de entrada / borrado ---
        w.set_prompt_open(self.prompt.is_some());
        if let Some(p) = self.prompt.as_ref() {
            let (title, placeholder) = match p.kind {
                PromptKind::NewFile => ("Nuevo archivo", "nombre.spasm"),
                PromptKind::NewFolder => ("Nueva carpeta", "nombre"),
                PromptKind::Rename => ("Renombrar", ""),
                PromptKind::SessionName => ("Renombrar terminal", "sesión"),
                PromptKind::NewBranch => ("Nueva rama", "nombre-rama"),
                PromptKind::RunArgs => ("Argumentos para ejecutar", "--flag valor"),
                PromptKind::SiteField(field) => match field {
                    "user" => ("Usuario del sitio", "ftp-usuario"),
                    "pass" => (
                        "Contraseña del sitio",
                        "se guarda en %APPDATA%, no en el repo",
                    ),
                    "remote" => ("Carpeta remota", "public_html"),
                    "local-sub" => ("Subcarpeta local a subir", "build"),
                    "include" => ("Extensiones a subir", "html,css,js,png,svg"),
                    "script" => ("Script de despliegue", "deploy.spasm"),
                    _ => ("Host del sitio", "ftp.midominio.com"),
                },
                PromptKind::WebField(field) => match field {
                    "port" => ("Puerto del servidor local", "8080"),
                    "root" => ("Carpeta servida", "build"),
                    _ => ("Origen permitido (CORS)", "*"),
                },
            };
            w.set_prompt_title(title.into());
            w.set_prompt_placeholder(placeholder.into());
            w.set_prompt_text(p.text.clone().into());
        }
        w.set_ask_open(self.ask.is_some());
        match self.ask.as_ref() {
            Some(Ask::Delete(path)) => {
                w.set_ask_title("Eliminar".into());
                w.set_ask_message(
                    format!(
                        "¿Eliminar «{}»? Esta acción no se puede deshacer.",
                        file_name(path)
                    )
                    .into(),
                );
                w.set_ask_accept_label("Eliminar".into());
                w.set_ask_danger(true);
            }
            Some(Ask::ReplaceInProject) => {
                let files = {
                    let mut v: Vec<PathBuf> =
                        self.search.hits.iter().map(|h| h.path.clone()).collect();
                    v.sort();
                    v.dedup();
                    v.len()
                };
                w.set_ask_title("Reemplazar en el proyecto".into());
                w.set_ask_message(
                    format!(
                        "Se reemplazará «{}» en {files} archivo(s). ¿Continuar?",
                        self.search.query
                    )
                    .into(),
                );
                w.set_ask_accept_label("Reemplazar".into());
                w.set_ask_danger(false);
            }
            Some(Ask::Discard { path, .. }) => {
                w.set_ask_title("Descartar cambios".into());
                w.set_ask_message(
                    format!("¿Descartar los cambios de «{path}»? No se puede deshacer.").into(),
                );
                w.set_ask_accept_label("Descartar".into());
                w.set_ask_danger(true);
            }
            Some(Ask::DeleteProvider(id)) => {
                w.set_ask_title("Eliminar proveedor".into());
                w.set_ask_message(
                    format!("¿Eliminar el proveedor «{id}» de la config global de OpenCode?")
                        .into(),
                );
                w.set_ask_accept_label("Eliminar".into());
                w.set_ask_danger(true);
            }
            None => {}
        }

        // --- buscar en el proyecto ---
        w.set_search_open(self.search.open);
        w.set_search_replace_open(self.search.replace_open);
        w.set_search_case(self.search.case_sensitive);
        w.set_search_regex(self.search.regex);
        w.set_search_focus_replace(self.search.focus_replace);
        w.set_search_searching(self.search.searching);
        w.set_search_selected(self.search.selected as i32);
        let (qb, qa) = split_at_cursor(&self.search.query, self.search.query_cursor);
        let (rb, ra) = split_at_cursor(&self.search.replace, self.search.replace_cursor);
        w.set_search_query_before(qb.into());
        w.set_search_query_after(qa.into());
        w.set_search_replace_before(rb.into());
        w.set_search_replace_after(ra.into());
        let info = if self.search.query.is_empty() {
            String::new()
        } else if self.search.searching {
            "…".to_string()
        } else {
            let n = self.search.hits.len();
            let extra = if self.search.truncated { "+" } else { "" };
            format!("{n}{extra} resultado(s)")
        };
        w.set_search_info(info.into());
        let root = self.explorer.root().to_path_buf();
        let hits: Vec<IdeSearchHit> = self
            .search
            .hits
            .iter()
            .map(|h| {
                let rel = h.path.strip_prefix(&root).unwrap_or(&h.path);
                IdeSearchHit {
                    path: rel.to_string_lossy().into_owned().into(),
                    line: h.line as i32 + 1,
                    text: h.text.clone().into(),
                }
            })
            .collect();
        w.set_search_hits(ModelRc::from(Rc::new(VecModel::from(hits))));

        // --- preferencias ---
        w.set_prefs_open(self.prefs_open);
        w.set_prefs_mono(self.mono.clone().into());
        w.set_prefs_sans(self.sans.clone().into());
        w.set_prefs_editor_size(format!("{}", self.editor_font_size as i32).into());
        w.set_prefs_accent(self.accent_name().into());
        // Módulos: servidor web y visor de SQLite.
        w.set_prefs_alpha(format!("{} %", (self.panel_alpha * 100.0).round() as i32).into());
        w.set_web_enabled(self.web.enabled);
        w.set_web_url(if self.web.enabled {
            self.web.url().into()
        } else {
            "".into()
        });
        w.set_web_port(self.web.port.to_string().into());
        w.set_web_root(self.web.root_sub.clone().into());
        w.set_web_cors(self.web.cors_origin.clone().into());
        w.set_sqlite_module(self.sqlite_enabled);
        w.set_sqlite_open(self.sqlite_open);
        w.set_sqlite_db(
            self.sqlite_db
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
                .into(),
        );
        w.set_sqlite_tool(
            self.sqlite_tool
                .as_ref()
                .map(|t| t.label().to_string())
                .unwrap_or_default()
                .into(),
        );
        w.set_sqlite_tables(
            self.sqlite_tables
                .iter()
                .cloned()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
        );
        w.set_sqlite_selected(self.sqlite_selected as i32);
        w.set_sqlite_header(self.sqlite_header.clone().into());
        w.set_sqlite_lines(
            self.sqlite_lines
                .iter()
                .cloned()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
        );
        w.set_sqlite_note(self.sqlite_note.clone().into());
        w.set_sqlite_busy(self.sqlite_busy);
        // Pantalla de configuración.
        self.sync_settings(w);
        // Formulario de proveedores de OpenCode.
        w.set_provider_open(self.provider_open);
        w.set_provider_id(self.provider_draft.id.clone().into());
        w.set_provider_name(self.provider_draft.name.clone().into());
        w.set_provider_package(self.provider_draft.package.clone().into());
        w.set_provider_base_url(self.provider_draft.base_url.clone().into());
        w.set_provider_api_key(self.provider_draft.api_key.clone().into());
        w.set_provider_models(self.provider_draft.models.clone().into());
        w.set_provider_note(self.provider_note.clone().into());
        w.set_provider_new(self.provider_draft.original_id.is_empty());

        // Sitio de despliegue del proyecto (Preferencias → Despliegue).
        w.set_site_transport(self.site.transport.label().into());
        w.set_site_host(self.site.host.clone().into());
        w.set_site_user(self.site.user.clone().into());
        w.set_site_pass(self.site.pass.clone().into());
        w.set_site_remote(self.site.remote_root.clone().into());
        w.set_site_local_sub(self.site.local_sub.clone().into());
        w.set_site_include(self.site.include.clone().into());
        w.set_site_script(self.site.script.clone().into());
        let (ar, ag, ab) = self.accent_rgb();
        w.set_prefs_accent_color(slint::Color::from_rgb_u8(ar, ag, ab));
        w.set_prefs_tab_size(self.tab_size.to_string().into());
        w.set_prefs_insert_spaces(self.insert_spaces);
        w.set_prefs_build_on_save(self.run_build_on_save);
        w.set_prefs_capturing(self.capturing_key.clone().unwrap_or_default().into());
        w.set_prefs_keys(ModelRc::from(Rc::new(VecModel::from(self.prefs_keys()))));
        w.set_about_open(self.about_open);
        w.set_about_version(self.version.clone().into());

        // --- Git ---
        w.set_git_repo(self.git_status.is_repo);
        w.set_git_branch(self.git_status.branch.clone().into());
        let sync = match (self.git_status.ahead, self.git_status.behind) {
            (0, 0) => String::new(),
            (a, 0) => format!("↑{a}"),
            (0, b) => format!("↓{b}"),
            (a, b) => format!("↑{a} ↓{b}"),
        };
        w.set_git_sync_info(sync.into());
        w.set_git_note(self.git_note.clone().into());
        w.set_git_error(self.git_error);
        w.set_git_busy(self.git_busy);
        let mut staged: Vec<IdeGitFile> = Vec::new();
        let mut changes: Vec<IdeGitFile> = Vec::new();
        for c in &self.git_status.changes {
            let file = IdeGitFile {
                status: c.status().into(),
                name: c.path.clone().into(),
                path: c.path.clone().into(),
            };
            if c.staged() {
                staged.push(file);
            } else {
                changes.push(file);
            }
        }
        w.set_git_staged(ModelRc::from(Rc::new(VecModel::from(staged))));
        w.set_git_changes(ModelRc::from(Rc::new(VecModel::from(changes))));

        // --- visor de diff ---
        w.set_diff_open(self.diff.is_some());
        if let Some(d) = self.diff.as_ref() {
            w.set_diff_title(format!("diff · {}", d.path).into());
            w.set_diff_note(d.note.clone().into());
            w.set_diff_error(d.error);
            w.set_diff_staged(d.staged);
            w.set_diff_busy(self.git_busy);
            w.set_diff_lines(ModelRc::from(Rc::new(VecModel::from(d.lines.clone()))));
        }

        // --- menú de ramas ---
        w.set_branch_menu_open(self.branch_menu);
        w.set_branch_menu_items(ModelRc::from(Rc::new(VecModel::from(
            self.branch_items.clone(),
        ))));

        // --- paleta de comandos / buscador de archivos ---
        w.set_palette_open(self.palette.open);
        w.set_palette_title(
            match self.palette.kind {
                PaletteKind::Commands => "Comandos",
                PaletteKind::Files => "Archivos del proyecto",
                PaletteKind::GoToLine => "Ir a línea",
                PaletteKind::Help => "Ayuda SpASM",
            }
            .into(),
        );
        w.set_palette_query(self.palette.query.clone().into());
        w.set_palette_selected(self.palette.selected as i32);
        let rows: Vec<IdePaletteItem> = self
            .palette
            .rows
            .iter()
            .map(|r| IdePaletteItem {
                label: r.label.clone().into(),
                detail: r.detail.clone().into(),
            })
            .collect();
        w.set_palette_items(ModelRc::from(Rc::new(VecModel::from(rows))));

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
            let active = self.tabs.get(self.active_tab);
            let language = match active {
                Some(t) if t.kind == DocKind::Image => "Imagen",
                Some(t) => t.language.name(),
                None => "SpASM",
            };
            let is_image = active.is_some_and(|t| t.kind == DocKind::Image);
            let image = active.and_then(|t| t.image.clone()).unwrap_or_default();
            let preview = active.is_some_and(|t| t.preview);
            let preview_doc = active
                .filter(|t| t.preview)
                .and_then(|t| t.preview_doc.clone())
                .unwrap_or_default();
            let is_markdown = active.is_some_and(|t| t.language == highlight::Language::Markdown);
            w.set_language(language.into());
            w.set_diagnostics(self.diagnostics_summary().into());
            let outline: Vec<IdeOutlineItem> = self
                .outline
                .iter()
                .map(|s| IdeOutlineItem {
                    label: s.name.clone().into(),
                    detail: s.detail.clone().into(),
                    line: s.line as i32,
                    kind: s.kind,
                })
                .collect();
            w.set_outline(ModelRc::from(Rc::new(VecModel::from(outline))));
            w.set_show_image(is_image);
            w.set_active_image(image);
            w.set_image_zoom(active.map(|t| t.image_zoom).unwrap_or(1.0));
            w.set_show_preview(preview);
            w.set_preview_styled(preview_doc);
            w.set_is_markdown(is_markdown);

            // Barra de búsqueda del editor.
            w.set_find_open(self.find.open);
            w.set_find_replace_open(self.find.replace_open);
            w.set_find_query(self.find.query.clone().into());
            w.set_find_replace(self.find.replace.clone().into());
            w.set_find_focus_replace(self.find.focus_replace);
            w.set_find_case(self.find.case_sensitive);
            w.set_find_whole_word(self.find.whole_word);
            w.set_find_regex(self.find.regex);
            w.set_find_in_selection(self.find.in_selection);
            let (qb, qa) = split_at_cursor(&self.find.query, self.find.query_cursor);
            let (rb, ra) = split_at_cursor(&self.find.replace, self.find.replace_cursor);
            w.set_find_query_before(qb.into());
            w.set_find_query_after(qa.into());
            w.set_find_replace_before(rb.into());
            w.set_find_replace_after(ra.into());
            let info = if self.find.query.is_empty() {
                String::new()
            } else {
                match self.find.current {
                    Some(i) => format!("{}/{}", i + 1, self.find.matches.len()),
                    None => format!("0/{}", self.find.matches.len()),
                }
            };
            w.set_find_info(info.into());

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
        // Salida nueva de algún PTY: con refrescar los paneles de terminal
        // alcanza, no hace falta volcar los otros ~20 modelos de la ventana.
        let mut terms = false;
        // Medidor de sistema cada ~1.5 s (94 ticks de 16 ms). Son dos números del
        // pie: se escriben directo, sin arrastrar un `sync` completo.
        self.drain_tick = self.drain_tick.wrapping_add(1);
        if self.drain_tick.is_multiple_of(94) {
            self.metrics = self.monitor.sample();
            w.set_cpu_percent((self.metrics.cpu * 100.0).round() as i32);
            w.set_ram_percent((self.metrics.ram * 100.0).round() as i32);
        }
        while let Ok(event) = self.local_rx.try_recv() {
            match &event {
                PtyEvent::Output { .. } => terms = true,
                // El proceso se cayó: cambia la lista de sesiones y el estado del
                // panel, eso sí va por el camino completo.
                PtyEvent::Exit { .. } => got = true,
            }
            self.apply_pty_event(Target::Local, event);
        }
        while let Ok(event) = self.agent_rx.try_recv() {
            match &event {
                PtyEvent::Output { .. } => terms = true,
                PtyEvent::Exit { .. } => got = true,
            }
            self.apply_pty_event(Target::Agent, event);
        }
        while let Ok(result) = self.lint_rx.try_recv() {
            let active = self.tabs.get(self.active_tab).map(|t| t.path.clone());
            if active.as_deref() == Some(result.path.as_path()) {
                self.diagnostics = result.diagnostics;
                self.outline = result.symbols;
                self.diagnostics_path = Some(result.path);
                self.editor_full = true;
            }
            got = true;
        }
        while let Ok(result) = self.build_rx.try_recv() {
            let mut text = result.output;
            text.push_str(&format!("\n[build: código {}]\n", result.code));
            if let Some(note) = self.build_note.take() {
                text.push('\n');
                text.push_str(&note);
                text.push('\n');
            }
            self.write_build_output(&text);
            self.diagnostics = result.diagnostics;
            self.diagnostics_path = self.active_file.clone();
            self.editor_full = true;
            got = true;
        }
        while let Ok(result) = self.git_rx.try_recv() {
            self.git_busy = false;
            self.git_error = result.error;
            self.git_note = result.note;
            self.git_status = result.status;
            if result.committed {
                // El commit se hizo: se limpia el campo de mensaje.
                w.set_git_message("".into());
            }
            got = true;
        }
        while let Ok(result) = self.search_rx.try_recv() {
            // Sólo la última búsqueda importa.
            if result.gen == self.search_gen {
                self.search.hits = result.hits;
                self.search.truncated = result.truncated;
                self.search.searching = false;
                let len = self.search.hits.len();
                self.search.selected = if len == 0 {
                    0
                } else {
                    self.search.selected.min(len - 1)
                };
                got = true;
            }
        }
        // Despliegue: se muestra en la sesión `build`; si terminó, se refresca la
        // barra de estado (el indicador de "ocupado").
        let mut deploy_done = false;
        while let Ok(msg) = self.deploy_rx.try_recv() {
            let finished = matches!(msg, DeployMsg::Finished { .. } | DeployMsg::Aborted(_));
            self.deploy_message(msg);
            deploy_done |= finished;
        }
        // Módulos: mensajes del servidor web y resultados de lecturas de SQLite.
        let mut web_touched = false;
        while let Ok(ev) = self.web_rx.try_recv() {
            match ev {
                WebEvent::Started { url } => {
                    self.write_build_output(&format!("servidor web activo en {url}\n"));
                    web_touched = true;
                }
                WebEvent::StartFailed { message } => {
                    self.web.enabled = false;
                    self.web_server = None;
                    self.write_build_output(&format!("servidor web: {message}\n"));
                    web_touched = true;
                }
                WebEvent::Stopped => {
                    self.write_build_output("servidor web detenido.\n");
                    web_touched = true;
                }
                WebEvent::Served { line } => {
                    self.write_build_output(&format!("{line}\n"));
                    web_touched = true;
                }
            }
        }
        let mut sqlite_touched = false;
        while let Ok(msg) = self.sqlite_rx.try_recv() {
            self.sqlite_loaded(msg);
            sqlite_touched = true;
        }
        if got || web_touched || sqlite_touched {
            self.sync(w);
        } else if terms || deploy_done {
            self.sync_terms(w);
        }
    }

    /// Procesa un evento del PTY: alimenta el emulador y contesta el pedido de
    /// posición del cursor (ConPTY lo necesita para poder renderizar).
    fn apply_pty_event(&mut self, target: Target, event: PtyEvent) {
        match event {
            PtyEvent::Output { session, bytes } => {
                let Some(s) = self.session_by_id_mut(target, session) else {
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
                // Salida nueva con la vista al final: el contenido se corrió y
                // la selección quedaría sobre texto que ya no es el mismo. Si
                // se está mirando el historial, `vt100` ancla la vista y la
                // selección sigue siendo válida.
                if s.screen.scrollback() == 0 {
                    s.sel = None;
                }
            }
            PtyEvent::Exit { session, code } => {
                if let Some(s) = self.session_by_id_mut(target, session) {
                    s.alive = false;
                    s.pty = None;
                    s.screen
                        .process(format!("\r\n[proceso terminado: {code}]\r\n").as_bytes());
                }
                // El panel derecho es una terminal que se recupera sola: si la
                // shell se cae — opencode al salir con Ctrl+C puede arrastrar
                // toda la consola — se relanza para dejar siempre un prompt
                // usable. (La terminal inferior queda muerta a propósito.)
                if matches!(target, Target::Agent) {
                    self.agent = None;
                    self.start_agent();
                    if let Some(s) = self.agent.as_mut() {
                        s.screen.process(
                            "[NuIde] la shell terminó; se relanza la terminal.\r\n".as_bytes(),
                        );
                    }
                }
            }
        }
    }
}

/// ¿La extensión corresponde a una imagen que podemos mostrar?
fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico")
    )
}

/// Convierte Markdown a texto con estilo de Slint. Sanea lo que el parser de
/// Slint no soporta (headings y cercos de código) y cae a texto plano si falla.
fn markdown_preview(source: &str) -> slint::StyledText {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
            let text: String = trimmed.chars().skip(hashes + 1).collect();
            out.push_str("**");
            out.push_str(text.trim());
            out.push_str("**");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    slint::StyledText::from_markdown(&out)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(source))
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

/// Tokeniza una línea, aplica las decoraciones y el cursor, y agrupa tramos
/// consecutivos del mismo kind. Así el caret es un token más.
///
/// `marks` trae rangos **en caracteres de la línea cruda** (con el fin
/// exclusivo); como el resaltado expande los tabs a cuatro espacios, cada índice
/// se mapea a su posición en la línea expandida.
fn render_tokens(
    line: &str,
    lang: highlight::Language,
    cursor: Option<usize>,
    marks: &[(usize, usize, i32)],
) -> Vec<(String, i32)> {
    // Atributo por carácter según el resaltado.
    let mut chars: Vec<(char, i32)> = Vec::new();
    for (text, kind) in highlight::tokenize_line(line, lang) {
        for c in text.chars() {
            chars.push((c, kind));
        }
    }

    let map = tab_map(line);
    let len = chars.len();
    let expand = |raw: usize| map.get(raw).copied().unwrap_or(len);

    for &(start, end, kind) in marks {
        let start = expand(start).min(len);
        let end = expand(end).min(len);
        if start < end {
            for item in chars[start..end].iter_mut() {
                item.1 = kind;
            }
        }
    }

    let cursor = cursor.map(expand).map(|c| c.min(len));

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

/// Para cada carácter **crudo** de la línea, su índice en la línea con los tabs
/// expandidos a cuatro espacios (lo que produce `highlight::tokenize_line`).
fn tab_map(line: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(line.chars().count() + 1);
    let mut expanded = 0;
    for c in line.chars() {
        map.push(expanded);
        expanded += if c == '\t' { 4 } else { 1 };
    }
    map
}

/// Parte `text` en el cursor: lo de antes y lo de después. Sirve para que la UI
/// dibuje el caret en su posición.
fn split_at_cursor(text: &str, cursor: usize) -> (String, String) {
    let before: String = text.chars().take(cursor).collect();
    let after: String = text.chars().skip(cursor).collect();
    (before, after)
}

/// `true` si el carácter es parte de un identificador.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// La palabra (identificador) bajo el cursor de `tab`, si la hay.
fn word_under_cursor(tab: &OpenTab) -> Option<String> {
    let text = tab.lines.get(tab.cursor.line)?;
    let (start, end) = word_at(text, tab.cursor.col)?;
    let word: String = text.chars().skip(start).take(end - start).collect();
    (!word.is_empty()).then_some(word)
}

/// Rango `(inicio, fin)` en caracteres de la palabra bajo (o justo después de)
/// la columna `col`, si la hay.
fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let index = if col < chars.len() && is_word_char(chars[col]) {
        col
    } else if col > 0 && is_word_char(chars[col - 1]) {
        col - 1
    } else {
        return None;
    };
    let mut start = index;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = index + 1;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    Some((start, end))
}

/// Todas las apariciones de `word` como palabra completa en `line`, en índices
/// de carácter.
fn find_word_in_line(line: &str, word: &str) -> Vec<(usize, usize)> {
    let hay: Vec<char> = line.chars().collect();
    let needle: Vec<char> = word.chars().collect();
    let n = needle.len();
    let mut out = Vec::new();
    if n == 0 || n > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + n <= hay.len() {
        let boundary = (i == 0 || !is_word_char(hay[i - 1]))
            && (i + n == hay.len() || !is_word_char(hay[i + n]));
        if boundary && hay[i..i + n] == needle[..] {
            out.push((i, i + n));
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

/// La pareja de una llave que abre: `(`→`)`, etc.
fn opener(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// La pareja de una llave que cierra: `)`→`(`, etc.
fn closer(c: char) -> Option<char> {
    match c {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

/// Busca la llave que cierra que corresponde a la que abre en `(line, col)`.
fn match_forward(tab: &OpenTab, line: usize, col: usize, open: char) -> Option<(usize, usize)> {
    let close = opener(open)?;
    let mut depth = 0i32;
    for (li, text) in tab.lines.iter().enumerate().skip(line) {
        for (ci, ch) in text.chars().enumerate() {
            if li == line && ci < col {
                continue;
            }
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some((li, ci));
                }
            }
        }
    }
    None
}

/// Busca la llave que abre que corresponde a la que cierra en `(line, col)`.
fn match_backward(tab: &OpenTab, line: usize, col: usize, close: char) -> Option<(usize, usize)> {
    let open = closer(close)?;
    let mut depth = 0i32;
    for li in (0..=line).rev() {
        let chars: Vec<char> = tab.lines[li].chars().collect();
        for ci in (0..chars.len()).rev() {
            if li == line && ci > col {
                continue;
            }
            let ch = chars[ci];
            if ch == close {
                depth += 1;
            } else if ch == open {
                depth -= 1;
                if depth == 0 {
                    return Some((li, ci));
                }
            }
        }
    }
    None
}

/// Compara dos caracteres, con o sin distinguir mayúsculas.
fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

/// Apariciones de `query` en `line`, como rangos de caracteres con el fin
/// exclusivo. No se solapan. Con `whole_word`, exige bordes de palabra.
pub(crate) fn find_in_line(
    line: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<(usize, usize)> {
    let hay: Vec<char> = line.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let matches = (0..needle.len()).all(|j| char_eq(hay[i + j], needle[j], case_sensitive));
        let boundary = !whole_word
            || ((i == 0 || !is_word_char(hay[i - 1]))
                && (i + needle.len() == hay.len() || !is_word_char(hay[i + needle.len()])));
        if matches && boundary {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Compila una expresión regular (case-insensitive si `case_sensitive` es false).
pub(crate) fn build_regex(pattern: &str, case_sensitive: bool) -> Option<regex_lite::Regex> {
    regex_lite::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .ok()
}

/// Apariciones de una regex en `line` (rangos de caracteres, fin exclusivo).
pub(crate) fn find_in_line_regex(
    line: &str,
    re: &regex_lite::Regex,
    whole_word: bool,
) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    for m in re.find_iter(line) {
        if m.start() == m.end() {
            continue;
        }
        let start = line[..m.start()].chars().count();
        let end = line[..m.end()].chars().count();
        if whole_word {
            let boundary = (start == 0 || !is_word_char(chars[start - 1]))
                && (end == chars.len() || !is_word_char(chars[end]));
            if !boundary {
                continue;
            }
        }
        out.push((start, end));
    }
    out
}

/// Reemplaza todas las apariciones de `query` en `line`. Devuelve la línea nueva
/// y cuántas reemplazó.
fn replace_in_line(
    line: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> (String, usize) {
    let hits = find_in_line(line, query, case_sensitive, whole_word);
    if hits.is_empty() {
        return (line.to_string(), 0);
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut last = 0;
    for &(start, end) in &hits {
        out.extend(chars[last..start].iter());
        out.push_str(replacement);
        last = end;
    }
    out.extend(chars[last..].iter());
    (out, hits.len())
}

/// Codifica un evento de mouse para la app (SGR, UTF-8 o X10).
fn encode_mouse(
    encoding: vt100::MouseProtocolEncoding,
    code: u32,
    col: u32,
    row: u32,
    release: bool,
) -> Vec<u8> {
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            format!("\x1b[<{code};{};{}{}", col + 1, row + 1, end).into_bytes()
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            push_utf8(&mut out, code + 32);
            push_utf8(&mut out, col + 33);
            push_utf8(&mut out, row + 33);
            out
        }
        vt100::MouseProtocolEncoding::Default => vec![
            0x1b,
            b'[',
            b'M',
            (code + 32).min(255) as u8,
            (col + 33).min(255) as u8,
            (row + 33).min(255) as u8,
        ],
    }
}

/// Agrega `v` como un carácter UTF-8 a `out`.
fn push_utf8(out: &mut Vec<u8>, v: u32) {
    if let Some(c) = char::from_u32(v) {
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
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

/// Nombre de una carpeta (proyecto) para la bienvenida.
fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Separa una lista de rutas guardada con `|` (separador ilegal en Windows).
fn load_recent(config: &Config, key: &str) -> Vec<PathBuf> {
    config
        .get(key)
        .map(|v| {
            v.split('|')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Serializa una lista de rutas para `Config`.
fn save_recent(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("|")
}

/// Mete `path` al frente de la lista de recientes (sin duplicados, tope 8).
fn push_recent(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(8);
}

/// Junta los archivos del proyecto (recursivo, con topes) para el buscador.
/// Se saltea carpetas pesadas o generadas y las que empiezan con punto.
pub(crate) fn collect_files(root: &Path) -> Vec<PathBuf> {
    const MAX: usize = 6000;
    const SKIP: &[&str] = &[
        "target",
        "node_modules",
        "dist",
        "build",
        "out",
        "bin",
        "vendor",
        ".git",
    ];
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= MAX || depth > 24 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else {
                out.push(path);
            }
            if out.len() >= MAX {
                break;
            }
        }
    }
    out.sort();
    out
}

/// Traduce el tamaño de un panel en píxeles a celdas de terminal (columnas ×
/// filas), acotado a un rango razonable para el PTY.
fn cells_for(w: f32, h: f32, cell_w: f32) -> (u16, u16) {
    let cell_w = cell_w.max(1.0);
    let cols = ((w / cell_w).floor() as i64).clamp(20, 500) as u16;
    let rows = ((h / TERM_CELL_H).floor() as i64).clamp(2, 200) as u16;
    (cols, rows)
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
            c if (' '..'\u{E000}').contains(&c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

// ---------------------------------------------------------------------------
// Wiring con Slint
// ---------------------------------------------------------------------------

/// Devuelve la primera familia cuyo archivo de fuente exista; si ninguna, el
/// valor por defecto. Slint necesita un **único** nombre de familia.
/// Familias instaladas de la lista de candidatos (para poder ciclar en
/// Preferencias). En otras plataformas devuelve todas: Slint usa su fallback.
fn installed_families(candidates: &[(&str, &str)]) -> Vec<String> {
    #[cfg(windows)]
    {
        let dirs: Vec<PathBuf> = [
            std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts")),
            std::env::var_os("LOCALAPPDATA").map(|l| {
                PathBuf::from(l)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts")
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        let found: Vec<String> = candidates
            .iter()
            .filter(|(_, file)| dirs.iter().any(|d| d.join(file).is_file()))
            .map(|(n, _)| (*n).to_string())
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    candidates.iter().map(|(n, _)| (*n).to_string()).collect()
}

/// Siguiente familia instalada respecto de `current`.
fn next_family(candidates: &[(&str, &str)], current: &str) -> String {
    let list = installed_families(candidates);
    if list.is_empty() {
        return current.to_string();
    }
    let idx = list
        .iter()
        .position(|f| f == current)
        .map(|i| (i + 1) % list.len())
        .unwrap_or(0);
    list[idx].clone()
}

/// Combinación de teclas (`"Ctrl+Shift+P"`) a partir del evento del editor.
/// `None` si la tecla no sirve como atajo.
fn chord_of(text: &str, control: bool, shift: bool, alt: bool) -> Option<String> {
    let key: String = match text {
        "\u{F704}" => "F1".to_string(),
        "\u{F705}" => "F2".to_string(),
        "\u{F706}" => "F3".to_string(),
        "\u{F707}" => "F4".to_string(),
        "\u{F708}" => "F5".to_string(),
        "\u{F709}" => "F6".to_string(),
        "\u{F70a}" => "F7".to_string(),
        "\u{F70b}" => "F8".to_string(),
        "\u{F70c}" => "F9".to_string(),
        "\u{F70d}" => "F10".to_string(),
        "\u{F70e}" => "F11".to_string(),
        "\u{F70f}" => "F12".to_string(),
        "\u{F700}" => "Up".to_string(),
        "\u{F701}" => "Down".to_string(),
        "\u{F702}" => "Left".to_string(),
        "\u{F703}" => "Right".to_string(),
        "\u{F729}" => "Home".to_string(),
        "\u{F72b}" => "End".to_string(),
        "\u{F72c}" => "PageUp".to_string(),
        "\u{F72d}" => "PageDown".to_string(),
        _ => {
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_graphic() {
                return None;
            }
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
    };
    let mut chord = String::new();
    if control {
        chord.push_str("Ctrl+");
    }
    if shift {
        chord.push_str("Shift+");
    }
    if alt {
        chord.push_str("Alt+");
    }
    chord.push_str(&key);
    Some(chord)
}

fn pick_font(candidates: &[(&str, &str)], default: &str) -> String {
    #[cfg(windows)]
    {
        let dirs: Vec<PathBuf> = [
            std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts")),
            std::env::var_os("LOCALAPPDATA").map(|l| {
                PathBuf::from(l)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts")
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (family, file) in candidates {
            if dirs.iter().any(|d| d.join(file).is_file()) {
                return (*family).to_string();
            }
        }
    }
    let _ = candidates;
    default.to_string()
}

/// Engancha los callbacks de la ventana y arranca el bucle de drenado.
pub fn attach(window: &NuIdeWindow, ide: Rc<RefCell<Ide>>) {
    // Fuentes, tamaño y acento (desde Preferencias).
    ide.borrow().apply_theme(window);

    // Elevación (Windows): se consulta una sola vez y se refleja en el pie.
    window.set_elevated(elevate::is_elevated());

    ide.borrow_mut().sync(window);
    // Restaura el tamaño/posición recordados (si los hay).
    ide.borrow().apply_window(window);

    // Menú superior.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_menu_activated(move |label| {
            if let Some(w) = weak.upgrade() {
                // Controles de ventana: se manejan acá porque necesitan acceso a la ventana.
                match label.as_str() {
                    "Minimizar" => {
                        w.window().set_minimized(true);
                        return;
                    }
                    "Maximizar" => {
                        let win = w.window();
                        win.set_maximized(!win.is_maximized());
                        return;
                    }
                    _ => {}
                }
                let mut e = ide.borrow_mut();
                e.menu(&label);
                e.sync(&w);
            }
        });
    }

    // Elegir carpeta del proyecto.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_open_project(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.pick_project();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_open_file(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.pick_file();
                e.sync(&w);
            }
        });
    }
    // Nuevo archivo (bienvenida).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_new_file(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.new_file();
                e.sync(&w);
            }
        });
    }
    // Recientes (bienvenida): abre un proyecto o un archivo por índice.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_open_recent(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.open_recent(i);
                e.sync(&w);
            }
        });
    }

    // Git: refrescar, inicializar, preparar, commit, push y abrir archivos.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_refresh(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_refresh();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_init(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_init();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_stage_all(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_stage_all();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_toggle(move |path, staged| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_toggle(path.to_string(), staged);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_commit(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                let message = w.get_git_message().to_string();
                e.git_commit(message);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_push(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_push();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_open(move |path| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_open(&path);
                e.sync(&w);
            }
        });
    }
    // Git avanzado: quitar todo, bajar, enmendar, ramas y diff.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_unstage_all(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_unstage_all();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_pull(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_pull();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_amend(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                let message = w.get_git_message().to_string();
                e.git_amend(message);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_branch_click(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_branch_menu();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_branch_menu_command(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_branch_command(&id);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_branch_menu_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.branch_menu = false;
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_git_diff(move |path, staged, untracked| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_diff(&path, staged, untracked);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_diff_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_close_diff();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_diff_open_file(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                if let Some(d) = e.diff.as_ref() {
                    let path = d.path.clone();
                    e.git_open(&path);
                }
                e.git_close_diff();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_diff_toggle_stage(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_diff_toggle_stage();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_diff_discard(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.git_diff_discard();
                e.sync(&w);
            }
        });
    }

    // Paleta de comandos / buscador: aceptar una fila o cerrarla.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_palette_accept(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.palette_accept_at(i);
                e.take_relaunch(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_palette_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.palette_close();
                e.sync(&w);
            }
        });
    }

    // Buscar en el proyecto.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_close();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_toggle_case(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_toggle_case();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_toggle_regex(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_toggle_regex();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_toggle_replace(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_toggle_replace();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_replace_all(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_request_replace_all();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_accept(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search_accept_at(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_search_focus(move |replace| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.search.focus_replace = replace;
                if replace {
                    e.search.replace_cursor = e.search.replace.chars().count();
                } else {
                    e.search.query_cursor = e.search.query.chars().count();
                }
                e.sync(&w);
            }
        });
    }

    // Preferencias.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_close();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_mono(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_cycle_mono();
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_sans(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_cycle_sans();
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_alpha(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_cycle_alpha();
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    // Despliegue del proyecto (Preferencias → Despliegue).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_transport(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_cycle_transport();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_edit_site(move |field| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_edit_site(&field);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_test_site(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.deploy_test();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_upload_site(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.deploy_upload();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_ssh_terminal(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.deploy_ssh_terminal();
                e.sync(&w);
            }
        });
    }
    // Módulos: servidor web y visor de SQLite.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_toggle_web(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.toggle_web();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_edit_web_port(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.edit_web("port");
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_edit_web_root(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.edit_web("root");
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_edit_web_cors(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.edit_web("cors");
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_toggle_sqlite_module(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.toggle_sqlite_module();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_sqlite_pick_db(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.sqlite_pick_db();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_sqlite_refresh(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.sqlite_reload();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_sqlite_table_selected(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.sqlite_select(i.max(0) as usize);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_sqlite_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.sqlite_close();
                e.sync(&w);
            }
        });
    }
    // Pantalla de configuración.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_settings_changed(move |id, value| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.apply_theme(&w);
                e.setting_set(&id, &value);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_settings_activated(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.setting_activate(&id);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_settings_section_selected(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.settings_section = i.max(0) as usize;
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_settings_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.settings_open = false;
                e.capturing_key = None;
                e.sync(&w);
            }
        });
    }
    // Formulario de proveedores de OpenCode.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_provider_changed(move |field, value| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.provider_set(&field, &value);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_provider_save(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.provider_save();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_provider_delete(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.provider_delete_request();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_provider_cancel(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.provider_cancel();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_font_smaller(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_font_delta(-1.0);
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_font_bigger(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_font_delta(1.0);
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_accent(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_cycle_accent();
                e.apply_theme(&w);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_cycle_tab(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.cycle_tab_size();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_toggle_spaces(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.toggle_insert_spaces();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_toggle_build(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.run_toggle_build_on_save();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prefs_capture(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prefs_capture(&id);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_about_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.about_close();
                e.sync(&w);
            }
        });
    }

    // Explorador: menú contextual, refrescar y filtro de generados.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_explorer_context_opened(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.explorer_context(i);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_explorer_context_command(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.explorer_context_command(&id);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_explorer_context_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.explorer_context_open = false;
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_explorer_refresh(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.explorer_refresh();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_explorer_toggle_generated(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.explorer_toggle_generated();
                e.sync(&w);
            }
        });
    }

    // Diálogos de entrada y de borrado.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prompt_accept(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prompt_accept();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_prompt_cancel(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.prompt = None;
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_ask_accept(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.ask_accept();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_ask_cancel(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.ask = None;
                e.sync(&w);
            }
        });
    }

    // Vista previa de Markdown.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_preview_toggle(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.toggle_preview();
                e.sync(&w);
            }
        });
    }

    // Zoom del visor de imagen.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_image_zoom_in(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.image_zoom_by(1.25);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_image_zoom_out(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.image_zoom_by(0.8);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_image_zoom_reset(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.image_zoom_reset();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_goto_line(move |line| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.goto_line(line);
                e.sync(&w);
            }
        });
    }

    // Menú "Ver": mostrar/ocultar regiones y cambiar el dock de cada panel.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_toggle_region(move |kind| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.toggle_region(kind);
                e.sync(&w);
                e.save_state(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_cycle_dock(move |kind| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.cycle_dock(kind);
                e.sync(&w);
                e.save_state(&w);
            }
        });
    }
    // Arrastrar y soltar un panel de un dock a otro: el asa de la cabecera
    // (ui/ide_dock.slint) avisa cuando se suelta; acá se aplica la zona que
    // calculó la ventana y se persiste con `save_state` (`panel.<i>.dock`).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.global::<DockDrag>().on_released(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let dock = w.global::<DockDrag>();
            let panel = dock.get_panel();
            // Fuera de un arrastre real no hay nada que hacer.
            if panel < 0 {
                return;
            }
            dock.set_panel(-1);
            let zone = w.get_drop_zone();
            let mut e = ide.borrow_mut();
            e.dock_drop(panel, zone);
            e.sync(&w);
            e.save_state(&w);
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_menu_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.close_menu();
                e.sync(&w);
            }
        });
    }

    // Acción de un ítem de los menús Archivo/Editar.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_menu_command(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                let quit = e.menu_command(&id);
                e.sync(&w);
                if quit {
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }

    // Diálogo de "cambios sin guardar".
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_confirm_save(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                let quit = e.confirm_save();
                if !quit {
                    e.take_relaunch(&w);
                }
                e.sync(&w);
                if quit {
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_confirm_discard(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                let quit = e.confirm_discard();
                if !quit {
                    e.take_relaunch(&w);
                }
                e.sync(&w);
                if quit {
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_confirm_cancel(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.confirm_cancel();
                e.sync(&w);
            }
        });
    }

    // Cerrar desde el control de la barra superior: guarda el estado y, si hay
    // cambios sin guardar, pide confirmación antes de salir.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_close_requested(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.save_state(&w);
                let quit = e.request_quit();
                e.sync(&w);
                if quit {
                    let _ = slint::quit_event_loop();
                }
            }
        });
    }

    // Teclado de la terminal.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_key(move |text, control, shift, alt| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                if e.local_key(&text, control, shift, alt) {
                    e.sync(&w);
                } else {
                    e.sync_terms(&w);
                }
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
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_scroll(move |dy| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.wheel(Target::Local, dy);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_selection_start(move |y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_start(Target::Local, y, x);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_selection_update(move |y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_update(Target::Local, y, x);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_selection_end(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_end(Target::Local);
                e.sync_terms(&w);
            }
        });
    }
    // Mouse reporting de la terminal local.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_mouse(move |kind, button, y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.forward_mouse(Target::Local, mouse_kind(kind, button), y, x);
                e.sync_terms(&w);
            }
        });
    }
    // Pegar en la terminal local (botón del medio).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_paste(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.paste_into(Target::Local);
                e.sync_terms(&w);
            }
        });
    }

    // Menú contextual de las terminales (clic derecho): copiar, pegar, elevar.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_term_context_opened(move |target| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.terminal_context(if target == 1 {
                    Target::Agent
                } else {
                    Target::Local
                });
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_term_context_command(move |id| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.terminal_context_command(&id);
                e.sync(&w);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_term_context_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.term_context_open = false;
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
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_focus_pane(move |pane| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.focus_pane(pane);
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
                e.request_close_tab(i);
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
        window.on_find_next(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_step(true);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_prev(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_step(false);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_close();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_toggle_replace(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_toggle_replace();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_toggle_case(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_toggle_case();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_toggle_word(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_toggle_word();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_toggle_regex(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_toggle_regex();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_toggle_selection(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_toggle_selection();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_find_focus(move |replace| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.find_focus(replace);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_replace_one(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.replace_current();
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_replace_all(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.replace_all();
                e.sync(&w);
            }
        });
    }
    // Panel del agente (mismos gestos que la terminal local).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_key(move |text, control, shift, alt| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                if e.agent_key(&text, control, shift, alt) {
                    e.sync(&w);
                } else {
                    e.sync_terms(&w);
                }
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_resize(move |w_px, h_px| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.agent_resize(w_px, h_px);
                e.sync(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_scroll(move |dy| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.wheel(Target::Agent, dy);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_selection_start(move |y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_start(Target::Agent, y, x);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_selection_update(move |y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_update(Target::Agent, y, x);
                e.sync_terms(&w);
            }
        });
    }
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_selection_end(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.select_end(Target::Agent);
                e.sync_terms(&w);
            }
        });
    }
    // Mouse reporting del panel del agente.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_mouse(move |kind, button, y, x| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.forward_mouse(Target::Agent, mouse_kind(kind, button), y, x);
                e.sync_terms(&w);
            }
        });
    }
    // Pegar en el panel del agente (botón del medio).
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_agent_paste(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.paste_into(Target::Agent);
                e.sync_terms(&w);
            }
        });
    }

    // Bucle de drenado. El closure usa un `Weak`: si guardara el `Rc` habría un
    // ciclo `Ide -> Timer -> Ide`.
    let timer = Timer::default();
    {
        let weak_ide = Rc::downgrade(&ide);
        let weak_win = window.as_weak();
        // 16 ms = ~60 fps para las terminales. El renderer software de Slint
        // hace *partial rendering*, así que solo se redibuja lo que cambió.
        timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
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

    /// La tecla de una terminal sólo toca la terminal: con `sync_terms` alcanza.
    /// Los atajos que abren paneles ajenos (paleta) o interceptan el diálogo
    /// necesitan el vuelcado completo. Es el contrato que devuelve `pty_key`.
    #[test]
    fn las_teclas_de_la_terminal_no_piden_vuelcado_completo() {
        let mut ide = Ide::new();
        // Sin sesión creada: la letra, y también copiar/pegar de terminal.
        assert!(!ide.local_key("a", false, false, false));
        assert!(!ide.local_key("c", true, true, false));
        assert!(!ide.local_key("v", true, true, false));
        // El atajo global de la paleta cambia otros paneles: vuelve `true`.
        assert!(ide.local_key("p", true, true, false));
        assert!(ide.palette.open);
    }

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
        let lang = highlight::Language::Spasm;
        let joined = |col| -> String {
            render_tokens("let x = 1", lang, Some(col), &[])
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        assert_eq!(joined(4), "let |x = 1");
        assert_eq!(joined(0), "|let x = 1");
        assert_eq!(joined(9), "let x = 1|");

        let toks = render_tokens("ab", lang, Some(2), &[]);
        assert!(toks.iter().any(|(t, k)| t == "|" && *k == 6));

        // La selección se marca como kind 7.
        let toks = render_tokens("hola", lang, None, &[(0, 2, 7)]);
        assert!(toks.iter().any(|(t, k)| t == "ho" && *k == 7));
    }

    #[test]
    fn el_tab_se_expande_para_mapear_el_cursor_y_la_seleccion() {
        let lang = highlight::Language::Spasm;
        // El cursor tras el tab (columna cruda 1) cae tras los cuatro espacios.
        let joined: String = render_tokens("\tx", lang, Some(1), &[])
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(joined, "    |x");

        // Seleccionar el tab crudo (0..1) pinta los cuatro espacios.
        let toks = render_tokens("\tx", lang, None, &[(0, 1, 7)]);
        assert!(toks.iter().any(|(t, k)| t == "    " && *k == 7));
    }

    #[test]
    fn encuentra_y_reemplaza_en_una_linea() {
        assert_eq!(
            find_in_line("hola hola", "hola", true, false),
            vec![(0, 4), (5, 9)]
        );
        // Sin distinguir mayúsculas.
        assert_eq!(find_in_line("HoLa", "hola", false, false), vec![(0, 4)]);
        assert_eq!(
            find_in_line("HoLa", "hola", true, false),
            Vec::<(usize, usize)>::new()
        );
        // Palabra completa: "hola" no coincide dentro de "holanda".
        assert_eq!(
            find_in_line("hola holanda", "hola", true, true),
            vec![(0, 4)]
        );
        assert_eq!(
            find_in_line("hola holanda", "hola", true, false),
            vec![(0, 4), (5, 9)]
        );

        let (line, count) = replace_in_line("uno dos uno", "uno", "X", true, false);
        assert_eq!(line, "X dos X");
        assert_eq!(count, 2);
    }

    #[test]
    fn resalta_la_palabra_bajo_el_cursor_y_las_llaves() {
        let dir = temp_dir("deco");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "foo = bar(foo)\nbar(foo)").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());

        // Cursor dentro del primer "foo" (línea 0, col 1): aparecen las 3 veces.
        ide.tabs[0].cursor = Cursor { line: 0, col: 1 };
        let deco = ide.decorations();
        let total: usize = deco.words.values().map(Vec::len).sum();
        assert_eq!(total, 3);
        assert!(deco.words.get(&0).is_some_and(|v| v.contains(&(0, 3))));

        // Cursor sobre el "(" (línea 0, col 9): marca el par con el ")".
        ide.tabs[0].cursor = Cursor { line: 0, col: 9 };
        let deco = ide.decorations();
        assert_eq!(deco.brackets, vec![(0, 9), (0, 13)]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_busqueda_encuentra_cuenta_y_reemplaza() {
        let dir = temp_dir("find");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "uno dos uno\ntres uno").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());

        ide.find.query = "uno".to_string();
        ide.find.replace = "X".to_string();
        ide.find_recompute();
        assert_eq!(ide.find.matches.len(), 3);
        // El cursor saltó a la primera coincidencia.
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 0, col: 0 });

        ide.find_step(true);
        assert_eq!(ide.tabs[0].cursor, Cursor { line: 0, col: 8 });

        ide.replace_all();
        assert_eq!(ide.tabs[0].lines, vec!["X dos X", "tres X"]);
        assert!(ide.tabs[0].dirty);
        // Ya no quedan coincidencias.
        assert!(ide.find.matches.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
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
    fn la_region_de_seleccion_ordena_y_hace_exclusivo_el_fin() {
        let sel = TermSel {
            anchor: (1, 4),
            head: (0, 2),
        };
        assert_eq!(sel.region(80), Some((0, 2, 1, 5)));
        // Un clic sin arrastre no selecciona nada.
        let click = TermSel {
            anchor: (2, 3),
            head: (2, 3),
        };
        assert_eq!(click.region(80), None);
        // El fin se acota al ancho de la terminal.
        let end = TermSel {
            anchor: (0, 0),
            head: (0, 79),
        };
        assert_eq!(end.region(80), Some((0, 0, 0, 80)));
    }

    #[test]
    fn la_celda_se_traduce_desde_pixeles_y_se_acota() {
        let mut ide = Ide::new();
        ide.local_cols = 80;
        ide.local_rows = 24;
        // Fila 1, columna 2 (celda de 16 px de alto y ~7.2 de ancho).
        assert_eq!(ide.term_cell(Target::Local, 20.0, 15.0), (1, 2));
        // Fuera del área se acota a la primera / última celda.
        assert_eq!(ide.term_cell(Target::Local, -5.0, -5.0), (0, 0));
        assert_eq!(ide.term_cell(Target::Local, 9_999.0, 9_999.0), (23, 79));
        // El agente tiene su propio tamaño.
        assert_eq!(ide.term_cell(Target::Agent, 0.0, 0.0), (0, 0));
        assert_eq!(
            ide.term_cell(Target::Agent, 9_999.0, 9_999.0),
            (AGENT_ROWS - 1, AGENT_COLS - 1)
        );
    }

    #[test]
    fn el_panel_del_agente_arranca_con_una_pantalla() {
        let mut ide = Ide::new();
        ide.start_agent();
        {
            let agent = ide.agent.as_ref().expect("el agente queda inicializado");
            assert!(agent.name.ends_with("agente"));
        }
        // La pantalla del agente tiene exactamente las filas del panel.
        let (lines, total, pos, thumb) = ide.term_view(Target::Agent);
        assert_eq!(lines.len(), ide.agent_rows as usize);
        assert_eq!(total, 0);
        assert_eq!(pos, 1.0);
        assert_eq!(thumb, 1.0);
    }

    #[test]
    fn edita_el_campo_de_busqueda_con_cursor() {
        let mut ide = Ide::new();
        ide.find_open(false);
        // Se escribe "ac", se vuelve atrás y se corrige en el medio.
        ide.find_insert('a');
        ide.find_insert('c');
        ide.find_move_cursor(-1, false);
        ide.find_insert('b');
        assert_eq!(ide.find.query, "abc");
        assert_eq!(ide.find.query_cursor, 2);

        ide.find_backspace();
        assert_eq!(ide.find.query, "ac");
        assert_eq!(ide.find.query_cursor, 1);

        ide.find_move_cursor(0, true);
        assert_eq!(ide.find.query_cursor, 2);

        // El toggle de palabra completa queda activo.
        ide.find_toggle_word();
        assert!(ide.find.whole_word);
    }

    #[test]
    fn el_menu_ver_alterna_los_paneles() {
        let mut ide = Ide::new();
        assert!(ide.show_left && ide.show_right && ide.show_bottom);
        assert!(ide.menu_open.is_empty());

        ide.menu("Ver");
        assert_eq!(ide.menu_open, "Ver");
        // Volver a abrir el mismo menú lo cierra.
        ide.menu("Ver");
        assert!(ide.menu_open.is_empty());
        // Archivo y Editar abren su menú.
        ide.menu("Archivo");
        assert_eq!(ide.menu_open, "Archivo");
        assert!(!ide.file_menu_items().is_empty());

        ide.toggle_region(0);
        assert!(!ide.show_left);
        ide.toggle_region(1);
        assert!(!ide.show_right);
        ide.toggle_region(2);
        assert!(!ide.show_bottom);
        // Un tipo desconocido no rompe.
        ide.toggle_region(9);

        ide.close_menu();
        assert!(ide.menu_open.is_empty());
    }

    #[test]
    fn el_dock_de_los_paneles_cicla_y_se_apila() {
        let mut ide = Ide::new();
        // Por defecto: explorador a la izquierda, agente a la derecha, terminal
        // abajo y Git apilado a la izquierda con el explorador.
        assert_eq!(
            ide.docks,
            [Dock::Left, Dock::Right, Dock::Bottom, Dock::Left]
        );

        // El explorador cicla izquierda → derecha → abajo → oculto → izquierda.
        ide.cycle_dock(0);
        assert_eq!(ide.docks[0], Dock::Right);
        ide.cycle_dock(0);
        assert_eq!(ide.docks[0], Dock::Bottom);
        ide.cycle_dock(0);
        assert_eq!(ide.docks[0], Dock::Hidden);
        ide.cycle_dock(0);
        assert_eq!(ide.docks[0], Dock::Left);

        // Si el agente va a la izquierda, los dos quedan apilados ahí.
        ide.cycle_dock(1); // agente a abajo
        ide.cycle_dock(1); // agente a oculto
        ide.cycle_dock(1); // agente a izquierda
        assert_eq!(ide.docks[1], Dock::Left);

        // El panel de Git también cicla su dock.
        ide.cycle_dock(3);
        assert_eq!(ide.docks[3], Dock::Right);
    }

    /// Soltar el asa de una cabecera mueve el panel a la zona y prende la
    /// región si estaba apagada (si no, el gesto parecería no hacer nada).
    #[test]
    fn soltar_un_panel_arrastrado_lo_cambia_de_dock() {
        let mut ide = Ide::new();
        assert_eq!(ide.docks[2], Dock::Bottom);

        // Terminal al borde derecho: se va a la derecha y prende esa región.
        ide.show_right = false;
        ide.dock_drop(2, 1);
        assert_eq!(ide.docks[2], Dock::Right);
        assert!(ide.show_right, "la región del destino se enciende");

        // Explorador al centro: se oculta (y no toca las regiones).
        ide.dock_drop(0, 3);
        assert_eq!(ide.docks[0], Dock::Hidden);

        // Agente abajo.
        ide.dock_drop(1, 2);
        assert_eq!(ide.docks[1], Dock::Bottom);

        // Un panel inexistente no rompe nada.
        ide.dock_drop(9, 0);
        assert_eq!(ide.docks.len(), 4);
    }

    #[test]
    fn detecta_imagenes_y_alterna_la_vista_previa() {
        assert!(is_image(Path::new("foto.PNG")));
        assert!(is_image(Path::new("a.jpeg")));
        assert!(!is_image(Path::new("a.spasm")));
        assert!(!is_image(Path::new("a.md")));

        let dir = temp_dir("viewer");
        let md = dir.join("README.md");
        std::fs::write(&md, "# Hola\n\ntexto").unwrap();
        let mut ide = Ide::new();
        ide.open_file(md.clone());
        assert_eq!(ide.tabs[0].language, highlight::Language::Markdown);
        assert!(!ide.tabs[0].preview);
        ide.toggle_preview();
        assert!(ide.tabs[0].preview);

        // Un `.spasm` no tiene vista previa.
        let sp = dir.join("a.spasm");
        std::fs::write(&sp, "end;").unwrap();
        ide.open_file(sp);
        ide.toggle_preview();
        assert!(!ide.tabs[1].preview);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_zoom_de_imagen_se_acota_y_se_reinicia() {
        let dir = temp_dir("zoom");
        let img = dir.join("foto.png");
        // El contenido no hace falta que sea una imagen válida: lo que importa
        // es que se abra como `DocKind::Image`.
        std::fs::write(&img, b"no-es-png").unwrap();
        let mut ide = Ide::new();
        ide.open_file(img.clone());
        assert_eq!(ide.tabs[0].kind, DocKind::Image);
        assert_eq!(ide.tabs[0].image_zoom, 1.0);

        ide.image_zoom_by(1.25);
        assert!((ide.tabs[0].image_zoom - 1.25).abs() < 1e-6);
        ide.image_zoom_by(100.0);
        assert_eq!(ide.tabs[0].image_zoom, 8.0); // tope superior
        ide.image_zoom_by(0.0001);
        assert_eq!(ide.tabs[0].image_zoom, 0.1); // tope inferior
        ide.image_zoom_reset();
        assert_eq!(ide.tabs[0].image_zoom, 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f8_salta_entre_diagnosticos() {
        let dir = temp_dir("diag-nav");
        let f = dir.join("a.spasm");
        std::fs::write(&f, "l1\nl2\nl3\nl4\nl5\n").unwrap();
        let mut ide = Ide::new();
        ide.open_file(f.clone());
        ide.diagnostics = vec![
            lint::Diagnostic {
                line: 2,
                message: "a".into(),
            },
            lint::Diagnostic {
                line: 5,
                message: "b".into(),
            },
        ];
        // Desde la línea 0, F8 va a la 2 (índice 1); otra vez, a la 5; otra, circular.
        ide.diagnostic_step(true);
        assert_eq!(ide.tabs[0].cursor.line, 1);
        ide.diagnostic_step(true);
        assert_eq!(ide.tabs[0].cursor.line, 4);
        ide.diagnostic_step(true);
        assert_eq!(ide.tabs[0].cursor.line, 1);
        // Hacia atrás desde la 2: a la 5.
        ide.diagnostic_step(false);
        assert_eq!(ide.tabs[0].cursor.line, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ir_a_definicion_con_el_esquema() {
        let dir = temp_dir("goto");
        let f = dir.join("a.spasm");
        std::fs::write(
            &f,
            "programa \"x\"\nmain {\n    let total = 0\n    let x = total\n}\n",
        )
        .unwrap();
        let mut ide = Ide::new();
        ide.open_file(f);
        ide.outline = vec![lint::Symbol {
            kind: 1,
            name: "total".into(),
            line: 3,
            detail: String::new(),
        }];
        // Cursor sobre "total" en la línea 4 (índice 3, columna 13).
        ide.tabs[0].cursor = Cursor { line: 3, col: 13 };
        ide.go_to_definition();
        assert_eq!(ide.tabs[0].cursor.line, 2); // línea 3 (índice 2)
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dividir_la_terminal_togglea_y_enfoca() {
        let mut ide = Ide::new();
        assert!(ide.split.is_none());
        ide.split_terminal();
        assert!(ide.split.is_some());
        assert!(ide.local.len() >= 2);
        let primary = ide.local_active;
        let secondary = ide.split.unwrap();
        assert_ne!(primary, secondary);

        // Enfocar el panel 1 intercambia activo y dividido.
        ide.focus_pane(1);
        assert_eq!(ide.local_active, secondary);
        assert_eq!(ide.split, Some(primary));

        // Volver a dividir quita la división.
        ide.split_terminal();
        assert!(ide.split.is_none());
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

    #[test]
    fn push_recent_dedup_y_limita_a_ocho() {
        let mut list = Vec::new();
        for i in 0..10 {
            push_recent(&mut list, PathBuf::from(format!("p{i}")));
        }
        assert_eq!(list.len(), 8);
        assert_eq!(list[0], PathBuf::from("p9"));

        // Reinsertar uno existente lo sube al frente sin duplicar.
        push_recent(&mut list, PathBuf::from("p5"));
        assert_eq!(list[0], PathBuf::from("p5"));
        assert_eq!(list.iter().filter(|p| p.as_os_str() == "p5").count(), 1);
    }

    #[test]
    fn load_recent_separa_por_barra_vertical_e_ignora_vacios() {
        let mut cfg = Config::load();
        cfg.set("recent.files", "a|b||c");
        assert_eq!(
            load_recent(&cfg, "recent.files"),
            vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        assert!(load_recent(&cfg, "falta").is_empty());
    }

    #[test]
    fn save_recent_hace_round_trip() {
        let paths = vec![PathBuf::from("F:\\uno\\dos"), PathBuf::from("F:\\tres")];
        let mut cfg = Config::load();
        cfg.set("k", save_recent(&paths));
        assert_eq!(load_recent(&cfg, "k"), paths);
    }

    #[test]
    fn la_paleta_filtra_comandos_por_etiqueta_y_atajo() {
        assert_eq!(filter_commands("").len(), Command::ALL.len());

        let git = filter_commands("git");
        assert!(git.contains(&Command::GitPush));
        assert!(git
            .iter()
            .all(|c| c.label().to_lowercase().contains("git") || c.hint() == "git"));

        // El atajo también cuenta como match.
        assert!(filter_commands("F5").contains(&Command::Run));
        assert!(filter_commands("zzz").is_empty());
    }

    #[test]
    fn collect_files_entra_en_subcarpetas_y_saltea_generados() {
        let dir = temp_dir("palette-files");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("src").join("main.spasm"), "end;").unwrap();
        std::fs::write(dir.join("target").join("basura.o"), "x").unwrap();
        std::fs::write(dir.join(".git").join("config"), "x").unwrap();

        let names: Vec<String> = collect_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "main.spasm"));
        assert!(!names.iter().any(|n| n == "basura.o"));
        assert!(!names.iter().any(|n| n == "config"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ctrl_p_abre_la_paleta_y_esc_la_cierra() {
        let mut ide = Ide::new();
        assert!(!ide.palette.open);

        // Raíz de prueba: así el buscador no recorre todo el repo.
        let dir = temp_dir("palette-ctrl");
        std::fs::write(dir.join("a.spasm"), "end;").unwrap();
        ide.explorer = Explorer::new(dir.clone());

        // Ctrl+Shift+P: paleta de comandos (vía el atajo configurable).
        let action = ide.keymap_action("P", true, true, false).unwrap();
        ide.run_key_action(&action);
        assert!(ide.palette.open);
        assert_eq!(ide.palette.kind, PaletteKind::Commands);
        assert_eq!(ide.palette.rows.len(), Command::ALL.len());

        // Escribir filtra en vivo.
        ide.palette_key("g", false);
        ide.palette_key("i", false);
        ide.palette_key("t", false);
        assert!(ide.palette.rows.iter().all(|r| match &r.action {
            PaletteAction::Command(c) => c.label().to_lowercase().contains("git"),
            _ => false,
        }));

        // Esc cierra.
        ide.palette_key("\u{1b}", false);
        assert!(!ide.palette.open);

        // Ctrl+P: buscador de archivos.
        let action = ide.keymap_action("p", true, false, false).unwrap();
        ide.run_key_action(&action);
        assert_eq!(ide.palette.kind, PaletteKind::Files);
        assert_eq!(ide.palette.rows.len(), 1);

        // Mientras está abierta, las teclas no llegan al editor.
        assert!(ide.palette_key("x", false));
        assert_eq!(ide.palette.query, "x");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cerrar_pestana_con_cambios_pide_confirmacion() {
        let dir = temp_dir("confirm");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "end;").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a.clone());

        // Sin cambios: cierra directo.
        ide.request_close_tab(0);
        assert!(ide.tabs.is_empty());

        // Con cambios: pide confirmación.
        ide.open_file(a.clone());
        ide.tabs[0].dirty = true;
        ide.request_close_tab(0);
        assert_eq!(ide.confirm, Some(Confirm::CloseTab));
        assert_eq!(ide.tabs.len(), 1);

        // Cancelar no cierra.
        ide.confirm_cancel();
        assert!(ide.confirm.is_none());
        assert_eq!(ide.tabs.len(), 1);

        // Descartar cierra.
        ide.request_close_tab(0);
        assert!(!ide.confirm_discard());
        assert!(ide.tabs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirmar_guardar_escribe_el_archivo() {
        let dir = temp_dir("confirm-save");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "end;").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a.clone());
        ide.tabs[0].lines = vec!["end;".to_string(), "; nuevo".to_string()];
        ide.tabs[0].dirty = true;

        ide.request_close_tab(0);
        assert_eq!(ide.confirm, Some(Confirm::CloseTab));
        assert!(!ide.confirm_save());
        assert!(ide.tabs.is_empty());
        assert!(std::fs::read_to_string(&a).unwrap().contains("; nuevo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_paleta_ir_a_linea_mueve_el_cursor() {
        let dir = temp_dir("palette-goto");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "l1\nl2\nl3\nl4\n").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a.clone());

        ide.run_command(Command::GoToLine);
        assert_eq!(ide.palette.kind, PaletteKind::GoToLine);
        assert!(ide.palette.rows.is_empty());

        ide.palette_key("3", false);
        assert_eq!(ide.palette.rows.len(), 1);
        ide.palette_key("\n", false);
        assert!(!ide.palette.open);
        assert_eq!(ide.tabs[0].cursor.line, 2); // línea 3 → índice 2
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_menu_ejecuta_y_cierra() {
        let mut ide = Ide::new();
        ide.menu("Editar");
        assert_eq!(ide.menu_open, "Editar");
        assert!(!ide.edit_menu_items().is_empty());

        ide.menu_command("edit.find");
        assert!(ide.menu_open.is_empty());
        assert!(ide.find.open);
    }

    #[test]
    fn valid_name_rechaza_separadores_y_vacios() {
        assert!(valid_name("main.spasm"));
        assert!(valid_name("con espacio.txt"));
        assert!(!valid_name(""));
        assert!(!valid_name("."));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
        assert!(!valid_name("a:b"));
        assert!(!valid_name("a*b"));
    }

    #[test]
    fn el_menu_contextual_deshabilita_la_raiz() {
        let dir = temp_dir("ctx");
        std::fs::write(dir.join("a.spasm"), "end;").unwrap();
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());

        // La raíz no se puede renombrar ni eliminar.
        ide.explorer_context(0);
        assert!(ide.explorer_context_open);
        let rename = ide
            .explorer_context_items
            .iter()
            .find(|e| e.id == "ctx.rename")
            .expect("hay renombrar");
        assert!(!rename.enabled);

        // Un archivo sí.
        let idx = ide
            .explorer
            .nodes()
            .iter()
            .position(|n| n.label == "a.spasm")
            .unwrap();
        ide.explorer_context(idx as i32);
        let rename = ide
            .explorer_context_items
            .iter()
            .find(|e| e.id == "ctx.rename")
            .unwrap();
        assert!(rename.enabled);

        ide.explorer_context_command("ctx.refresh");
        assert!(!ide.explorer_context_open);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_prompt_crea_archivo_y_carpeta() {
        let dir = temp_dir("prompt-new");
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());
        ide.explorer_selected = 0; // raíz

        ide.explorer_context_command("ctx.new-folder");
        ide.prompt.as_mut().unwrap().text = "sub".to_string();
        ide.prompt_accept();
        assert!(dir.join("sub").is_dir());

        ide.explorer_selected = 0;
        ide.explorer_context_command("ctx.new-file");
        ide.prompt.as_mut().unwrap().text = "nuevo.spasm".to_string();
        ide.prompt_accept();
        assert!(dir.join("nuevo.spasm").is_file());
        assert!(ide.tabs.iter().any(|t| t.path == dir.join("nuevo.spasm")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renombrar_actualiza_las_pestanas() {
        let dir = temp_dir("prompt-rename");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "end;").unwrap();
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());
        ide.open_file(a.clone());

        let idx = ide
            .explorer
            .nodes()
            .iter()
            .position(|n| n.label == "a.spasm")
            .unwrap();
        ide.explorer_selected = idx as i32;
        ide.explorer_context_command("ctx.rename");
        ide.prompt.as_mut().unwrap().text = "b.spasm".to_string();
        ide.prompt_accept();

        assert!(dir.join("b.spasm").is_file());
        assert!(!a.exists());
        assert_eq!(ide.tabs[0].path, dir.join("b.spasm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eliminar_cierra_la_pestana() {
        let dir = temp_dir("ask-delete");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "end;").unwrap();
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());
        ide.open_file(a.clone());

        let idx = ide
            .explorer
            .nodes()
            .iter()
            .position(|n| n.label == "a.spasm")
            .unwrap();
        ide.explorer_selected = idx as i32;
        ide.explorer_context_command("ctx.delete");
        match ide.ask.as_ref() {
            Some(Ask::Delete(p)) => assert_eq!(p.as_path(), a.as_path()),
            _ => panic!("esperaba Ask::Delete"),
        }
        ide.ask_accept();

        assert!(!a.exists());
        assert!(ide.tabs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_encuentra_rangos_en_la_linea() {
        let re = build_regex(r"\d+", false).unwrap();
        assert_eq!(
            find_in_line_regex("a12 b3", &re, false),
            vec![(1, 3), (5, 6)]
        );
        // Inválida no compila.
        assert!(build_regex("(", false).is_none());
    }

    #[test]
    fn el_editor_cierra_pares_y_salta_el_cierre() {
        let dir = temp_dir("autopair");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "x\n").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a);

        ide.tabs[0].cursor = Cursor { line: 0, col: 1 };
        ide.insert_char('(');
        assert_eq!(ide.tabs[0].lines[0], "x()");
        assert_eq!(ide.tabs[0].cursor.col, 2);

        // Escribir el cierre ya presente sólo avanza.
        ide.insert_char(')');
        assert_eq!(ide.tabs[0].lines[0], "x()");
        assert_eq!(ide.tabs[0].cursor.col, 3);

        // Comilla dentro de palabra no se autocierra.
        ide.tabs[0].lines[0] = "don".to_string();
        ide.tabs[0].cursor = Cursor { line: 0, col: 3 };
        ide.insert_char('\'');
        assert_eq!(ide.tabs[0].lines[0], "don'");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enter_entre_parentesis_indenta_y_alinea_el_cierre() {
        let dir = temp_dir("autoindent");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "func()\n").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a);

        // Cursor entre "(" y ")".
        ide.tabs[0].cursor = Cursor { line: 0, col: 5 };
        ide.newline();
        assert_eq!(ide.tabs[0].lines, vec!["func(", "    ", ")"]);
        assert_eq!(ide.tabs[0].cursor.line, 1);
        assert_eq!(ide.tabs[0].cursor.col, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn buscar_en_la_seleccion_acota_las_coincidencias() {
        let dir = temp_dir("find-sel");
        let a = dir.join("sel.spasm");
        std::fs::write(&a, "foo\nbar\nfoo\n").unwrap();
        let mut ide = Ide::new();
        ide.open_file(a);
        ide.find_open(false);
        ide.find.query = "foo".to_string();
        // Selecciona sólo la primera línea.
        ide.tabs[0].anchor = Some(Cursor { line: 0, col: 0 });
        ide.tabs[0].cursor = Cursor { line: 1, col: 0 };
        ide.find_toggle_selection();

        assert!(ide.find.in_selection);
        assert_eq!(ide.find.matches.len(), 1);
        assert_eq!(ide.find.matches[0].0, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_busqueda_recuerda_el_historial() {
        let mut ide = Ide::new();
        ide.find_open(false);
        ide.find.query = "uno".to_string();
        ide.remember_query();
        ide.find.query = "dos".to_string();
        ide.remember_query();

        ide.find.query.clear();
        ide.find_history(-1);
        assert_eq!(ide.find.query, "dos");
        ide.find_history(-1);
        assert_eq!(ide.find.query, "uno");
    }

    #[test]
    fn codifica_eventos_de_mouse() {
        use vt100::MouseProtocolEncoding as E;
        // SGR: `\x1b[<code;col;row` + M/m (1-based).
        assert_eq!(
            encode_mouse(E::Sgr, 0, 4, 2, false),
            b"\x1b[<0;5;3M".to_vec()
        );
        assert_eq!(
            encode_mouse(E::Sgr, 0, 4, 2, true),
            b"\x1b[<0;5;3m".to_vec()
        );
        // Rueda arriba (64) y abajo (65).
        assert_eq!(
            encode_mouse(E::Sgr, 64, 0, 0, false),
            b"\x1b[<64;1;1M".to_vec()
        );
        // X10 por defecto: 3 bytes con +32.
        assert_eq!(
            encode_mouse(E::Default, 0, 0, 0, false),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }

    #[test]
    fn mapea_eventos_de_mouse() {
        assert_eq!(mouse_kind(0, 0), MouseKind::Press(0));
        assert_eq!(mouse_kind(1, 2), MouseKind::Release(2));
        assert_eq!(mouse_kind(2, 3), MouseKind::Move);
        assert_eq!(mouse_kind(3, 0), MouseKind::Wheel(true));
        assert_eq!(mouse_kind(4, 0), MouseKind::Wheel(false));
    }

    #[test]
    fn reiniciar_la_sesion_conserva_la_posicion() {
        let mut ide = Ide::new();
        ide.new_session();
        let total = ide.local.len();
        ide.local_active = 0;
        let old_id = ide.local[0].id;

        ide.restart_session();
        assert_eq!(ide.local.len(), total);
        assert_eq!(ide.local_active, 0);
        assert_ne!(ide.local[0].id, old_id);
    }

    #[test]
    fn renombrar_la_sesion_de_terminal() {
        let mut ide = Ide::new();
        ide.prompt_rename_session();
        assert!(matches!(
            ide.prompt.as_ref().map(|p| p.kind),
            Some(PromptKind::SessionName)
        ));
        ide.prompt.as_mut().unwrap().text = "servidor".to_string();
        ide.prompt_accept();
        assert_eq!(ide.local[ide.local_active].name, "servidor");
    }

    #[test]
    fn ctrl_shift_f_abre_la_busqueda_del_proyecto() {
        let dir = temp_dir("search-open");
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());

        assert!(!ide.search.open);
        let action = ide.keymap_action("F", true, true, false).unwrap();
        ide.run_key_action(&action);
        assert!(ide.search.open);
        ide.search_key("x", false, false);
        assert_eq!(ide.search.query, "x");
        ide.search_key("\u{1b}", false, false);
        assert!(!ide.search.open);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abrir_un_resultado_salta_a_la_linea() {
        let dir = temp_dir("search-accept");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "l1\nl2\nl3\n").unwrap();
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());

        ide.search.open = true;
        ide.search.hits = vec![crate::search::SearchHit {
            path: a.clone(),
            line: 2,
            col: 0,
            text: "l3".to_string(),
        }];
        ide.search.selected = 0;
        ide.search_accept();

        assert!(!ide.search.open);
        assert_eq!(ide.tabs[0].path, a);
        assert_eq!(ide.tabs[0].cursor.line, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reemplazar_en_el_proyecto_escribe_los_archivos() {
        let dir = temp_dir("search-replace");
        let a = dir.join("a.spasm");
        std::fs::write(&a, "foo\nbar\nfoo\n").unwrap();
        let mut ide = Ide::new();
        ide.explorer = Explorer::new(dir.clone());
        ide.search.query = "foo".to_string();
        ide.search.replace = "baz".to_string();
        ide.search.hits = vec![
            crate::search::SearchHit {
                path: a.clone(),
                line: 0,
                col: 0,
                text: "foo".to_string(),
            },
            crate::search::SearchHit {
                path: a.clone(),
                line: 2,
                col: 0,
                text: "foo".to_string(),
            },
        ];

        ide.replace_in_project();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "baz\nbar\nbaz\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clasifica_lineas_de_diff() {
        let d = parse_diff_lines(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-viejo\n+nuevo\n mismo\n",
        );
        let kinds: Vec<i32> = d.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, vec![4, 4, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn separa_argumentos_de_ejecucion() {
        assert_eq!(
            shell_split("--a b"),
            vec!["--a".to_string(), "b".to_string()]
        );
        assert_eq!(
            shell_split("--x \"a b\" --y"),
            vec!["--x".to_string(), "a b".to_string(), "--y".to_string()]
        );
        assert!(shell_split("   ").is_empty());
    }

    #[test]
    fn arma_combinaciones_de_teclas() {
        assert_eq!(chord_of("p", true, false, false).as_deref(), Some("Ctrl+P"));
        assert_eq!(
            chord_of("P", true, true, false).as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(
            chord_of("\u{F704}", false, false, false).as_deref(),
            Some("F1")
        );
        assert_eq!(chord_of("a", false, false, false).as_deref(), Some("A"));
        assert!(chord_of("\u{1b}", false, false, false).is_none());
    }

    #[test]
    fn reasigna_un_atajo_global() {
        let mut ide = Ide::new();
        // Por defecto, Ctrl+Shift+P abre la paleta.
        assert_eq!(
            ide.keymap_action("P", true, true, false).as_deref(),
            Some("palette")
        );

        ide.prefs_capture("palette");
        ide.capture_key("k", true, true, false); // Ctrl+Shift+K
        assert_eq!(
            ide.keymap.get("palette").map(String::as_str),
            Some("Ctrl+Shift+K")
        );
        assert_eq!(
            ide.keymap_action("K", true, true, false).as_deref(),
            Some("palette")
        );
        assert!(ide.keymap_action("P", true, true, false).is_none());
    }
}
