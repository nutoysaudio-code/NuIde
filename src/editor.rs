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
//! terminal local corre una shell; en el panel del agente corre `opencode` (su
//! TUI interactiva). Los menús Ejecutar/Compilar escriben la orden en la shell
//! local, así la salida aparece ahí.
//!
//! Los procesos corren en hilos y avisan por canales. Como los callbacks de
//! Slint viven en el hilo de la UI, un `Timer` de 60 ms drena los canales y
//! vuelca el estado con `sync`. Ese timer se guarda en `Ide` para que no se caiga
//! al salir de `attach`; su closure usa un `Weak` para no crear un ciclo.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::build;
use crate::config::Config;
use crate::explorer::{Action, Explorer};
use crate::git;
use crate::highlight;
use crate::lint;
use crate::pty::{self, Pty, PtyEvent};
use crate::spasm::Toolchain;
use crate::ui::{
    IdeCodeLine, IdeGitFile, IdeOutlineItem, IdePaletteItem, IdeRecent, IdeTab, IdeToken,
    IdeTreeNode, NuIdeWindow, TermLine, TermRun,
};
use crate::vt::{Screen, Selection};

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
const SANS_FONTS: &[(&str, &str)] = &[
    ("Montserrat", "Montserrat-Regular.ttf"),
    ("Century Gothic", "GOTHIC.TTF"),
    ("Segoe UI", "segoeui.ttf"),
];
const MONO_FONTS: &[(&str, &str)] = &[
    ("JetBrains Mono", "JetBrainsMono-Regular.ttf"),
    ("Cascadia Mono", "CascadiaMono.ttf"),
    ("Consolas", "consola.ttf"),
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
    Lint,
    NextError,
    Find,
    Replace,
    GotoDef,
    Preview,
    Save,
    OpenProject,
    OpenFile,
    NewFile,
    FindFiles,
    NewTerminal,
    SplitTerminal,
    ToggleExplorer,
    ToggleAgent,
    ToggleTerminal,
    ToggleGit,
    GitRefresh,
    GitStageAll,
    GitPush,
}

impl Command {
    /// Todos, en el orden en que se muestran.
    const ALL: [Command; 22] = [
        Command::Run,
        Command::Build,
        Command::Lint,
        Command::NextError,
        Command::Find,
        Command::Replace,
        Command::GotoDef,
        Command::Preview,
        Command::Save,
        Command::FindFiles,
        Command::OpenProject,
        Command::OpenFile,
        Command::NewFile,
        Command::NewTerminal,
        Command::SplitTerminal,
        Command::ToggleExplorer,
        Command::ToggleAgent,
        Command::ToggleTerminal,
        Command::ToggleGit,
        Command::GitRefresh,
        Command::GitStageAll,
        Command::GitPush,
    ];

    fn label(self) -> &'static str {
        match self {
            Command::Run => "Ejecutar el programa",
            Command::Build => "Compilar (spasmc)",
            Command::Lint => "Analizar / diagnóstico",
            Command::NextError => "Siguiente error",
            Command::Find => "Buscar",
            Command::Replace => "Buscar y reemplazar",
            Command::GotoDef => "Ir a definición",
            Command::Preview => "Vista previa de Markdown",
            Command::Save => "Guardar",
            Command::FindFiles => "Buscar archivos",
            Command::OpenProject => "Abrir carpeta del proyecto…",
            Command::OpenFile => "Abrir archivo…",
            Command::NewFile => "Nuevo archivo…",
            Command::NewTerminal => "Nueva terminal",
            Command::SplitTerminal => "Dividir la terminal",
            Command::ToggleExplorer => "Ver: Explorador",
            Command::ToggleAgent => "Ver: OpenCode CLI",
            Command::ToggleTerminal => "Ver: Terminal",
            Command::ToggleGit => "Ver: Control de código",
            Command::GitRefresh => "Git: refrescar",
            Command::GitStageAll => "Git: preparar todo",
            Command::GitPush => "Git: push",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Command::Run => "F5",
            Command::Build => "Ctrl+B",
            Command::Lint => "F6",
            Command::NextError => "F8",
            Command::Find => "Ctrl+F",
            Command::Replace => "Ctrl+H",
            Command::GotoDef => "F12",
            Command::Preview => "Ctrl+Shift+M",
            Command::Save => "Ctrl+S",
            Command::FindFiles => "Ctrl+P",
            Command::NewTerminal | Command::SplitTerminal => "terminal",
            Command::ToggleExplorer
            | Command::ToggleAgent
            | Command::ToggleTerminal
            | Command::ToggleGit => "panel",
            Command::GitRefresh | Command::GitStageAll | Command::GitPush => "git",
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

/// Tipo de paleta abierta.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum PaletteKind {
    #[default]
    Commands,
    Files,
}

/// Acción de una fila de la paleta.
#[derive(Clone)]
enum PaletteAction {
    Command(Command),
    File(PathBuf),
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
    /// El menú desplegable de "Ver" está abierto.
    view_menu_open: bool,
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
    /// Pila de deshacer del archivo activo.
    undo: Vec<(Vec<String>, Cursor)>,
    /// Pila de rehacer. Se vacía con cada edición nueva.
    redo: Vec<(Vec<String>, Cursor)>,
    /// Estado de la búsqueda del editor (barra, coincidencias).
    find: Find,
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
    /// Git del proyecto: estado, nota, hilo de operaciones.
    git_status: git::Status,
    git_note: String,
    git_error: bool,
    git_busy: bool,
    git_tx: Sender<git::GitResult>,
    git_rx: Receiver<git::GitResult>,
    /// Panel del agente: `opencode` en un PTY con su propia pantalla. Canal
    /// aparte para no mezclar la salida con la terminal local.
    agent: Option<LocalSession>,
    agent_tx: Sender<PtyEvent>,
    agent_rx: Receiver<PtyEvent>,
    /// Tamaño actual del panel del agente (columnas × filas).
    agent_cols: u16,
    agent_rows: u16,
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
        let config = Config::load();
        // Si hay un proyecto recordado y sigue existiendo, se arranca ahí.
        let root = config.project().unwrap_or(cwd);
        let recent_projects = load_recent(&config, "recent.projects");
        let recent_files = load_recent(&config, "recent.files");
        let show_left = config.bool("show.left", true);
        let show_right = config.bool("show.right", true);
        let show_bottom = config.bool("show.bottom", true);
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
        // En las pruebas no se consulta Git (mantiene los tests rápidos y ajenos
        // al estado del repo; el parseo se cubre en `git::tests`).
        let git_status = if cfg!(test) {
            git::Status::default()
        } else {
            git::status(&root)
        };

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
            tabs: Vec::new(),
            active_tab: 0,
            recent_projects,
            recent_files,
            show_left,
            show_right,
            show_bottom,
            docks,
            view_menu_open: false,
            palette: Palette::default(),
            config,
            code_model: Rc::new(VecModel::default()),
            model_attached: false,
            editor_full: false,
            undo: Vec::new(),
            redo: Vec::new(),
            find: Find::default(),
            diagnostics: Vec::new(),
            outline: Vec::new(),
            diagnostics_path: None,
            lint_tx,
            lint_rx,
            build_tx,
            build_rx,
            build_session: None,
            git_status,
            git_note: String::new(),
            git_error: false,
            git_busy: false,
            git_tx,
            git_rx,
            agent: None,
            agent_tx,
            agent_rx,
            agent_cols: AGENT_COLS,
            agent_rows: AGENT_ROWS,
            agent_version: std::env::var("NUIDE_AGENT_VERSION")
                .unwrap_or_else(|_| "OpenCode CLI".to_string()),
            agent_model: std::env::var("NUIDE_AGENT_MODEL")
                .unwrap_or_else(|_| "por defecto".to_string()),
            active_file: None,
            timer: None,
        };
        ide.new_local_session();
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
        }
        self.config.set("active_tab", self.active_tab);
        self.config
            .set("recent.projects", save_recent(&self.recent_projects));
        self.config
            .set("recent.files", save_recent(&self.recent_files));
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
    /// Lanza `opencode` en un PTY para el panel del agente.
    ///
    /// Si no está el CLI, o el PTY no se abre, deja la pantalla con el motivo.
    fn start_agent(&mut self) {
        let mut screen = Screen::new(self.agent_rows, self.agent_cols);
        let Some(bin) = self.toolchain.agent.clone() else {
            screen.process(
                b"CLI de OpenCode no encontrado (defini NUIDE_AGENT).\r\n\
                  \r\nopencode: https://opencode.ai\r\n"
                    .as_ref(),
            );
            self.agent = Some(LocalSession {
                id: 0,
                name: "opencode".to_string(),
                pty: None,
                screen,
                alive: false,
                sel: None,
            });
            return;
        };
        let cwd = self.explorer.root().to_path_buf();
        let pty = match Pty::spawn(
            0,
            &bin.display().to_string(),
            &[],
            &cwd,
            self.agent_cols,
            self.agent_rows,
            self.agent_tx.clone(),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                screen.process(
                    format!("no se pudo abrir opencode ({}): {e}\r\n", bin.display()).as_bytes(),
                );
                None
            }
        };
        let alive = pty.is_some();
        self.agent = Some(LocalSession {
            id: 0,
            name: "opencode".to_string(),
            pty,
            screen,
            alive,
            sel: None,
        });
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
            const TEMPLATE: &str = "; nuevo archivo SpASM\n\nprograma \"nuevo\"\n\nfunc suma(a : int) -> int {\n    let b = a\n    return b\n}\n\nmain {\n    let c = suma(1)\n}\n";
            if std::fs::write(&path, TEMPLATE).is_err() {
                return;
            }
            self.explorer.rebuild();
        }
        self.open_file(path);
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

    /// Cambia la carpeta del agente sin reiniciar el proceso: le manda `/cd` al
    /// PTY de `opencode`. Si no está vivo, lo arranca en la raíz nueva.
    fn retarget_agent(&mut self, dir: &Path) {
        let live = self
            .agent
            .as_ref()
            .is_some_and(|s| s.alive && s.pty.is_some());
        if live {
            let command = format!("/cd {}\r", dir.display());
            if let Some(session) = self.agent.as_mut() {
                if let Some(pty) = session.pty.as_mut() {
                    let _ = pty.write(command.as_bytes());
                }
            }
        } else {
            self.start_agent();
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
            alive: true,
            sel: None,
        });
        self.local_active = self.local.len() - 1;
    }
}

// ---------------------------------------------------------------------------
// Entrada y comandos
// ---------------------------------------------------------------------------

impl Ide {
    /// Teclas de la terminal local.
    fn local_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) {
        self.pty_key(Target::Local, text, control, shift, alt);
    }

    /// Teclas del panel del agente.
    fn agent_key(&mut self, text: &str, control: bool, shift: bool, alt: bool) {
        self.pty_key(Target::Agent, text, control, shift, alt);
    }

    /// Teclado compartido por las dos terminales.
    ///
    /// Antes de mandar las teclas al PTY se atienden los atajos que no son del
    /// proceso: `Ctrl+Shift+C/V` copian/pegan la selección y `Shift+PageUp/Down`
    /// recorren el scrollback. Cualquier otra tecla vuelve la vista al final.
    fn pty_key(&mut self, target: Target, text: &str, control: bool, shift: bool, alt: bool) {
        // Paleta de comandos / buscador de archivos (global).
        if self.palette_intercept(text, control, shift) {
            return;
        }
        if control && shift {
            match text.to_ascii_lowercase().as_str() {
                "c" => {
                    self.copy_selection(target);
                    return;
                }
                "v" => {
                    self.paste_into(target);
                    return;
                }
                _ => {}
            }
        }
        if shift && matches!(text, "\u{F72c}" | "\u{F72d}") {
            let half = (self.dimensions(target).1 / 2).max(1) as i32;
            let delta = if text == "\u{F72c}" { half } else { -half };
            self.scroll_session(target, delta);
            return;
        }
        // Cualquier tecla normal abandona el historial y la selección.
        if let Some(session) = self.session_mut(target) {
            session.sel = None;
            session.screen.scroll_to_end();
        }
        if let Some(bytes) = encode_key(text, control, shift, alt) {
            self.write_to(target, &bytes);
        }
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

    /// Corre el toolchain en segundo plano (build/run) y muestra la salida en la
    /// sesión "build" de la terminal.
    fn spawn_build(&mut self, program: PathBuf, args: Vec<String>) {
        let cwd = self.explorer.root().to_path_buf();
        let index = self.ensure_build_session();
        // La salida queda a la vista.
        self.local_active = index;
        self.resize_local();
        build::spawn(program, args, cwd, self.build_tx.clone());
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

    /// Ejecuta el archivo activo con el intérprete de SpASM (capturando la
    /// salida en la sesión "build").
    fn run(&mut self, _arg: Option<&str>) {
        let Some(file) = self.active_file.clone() else {
            return;
        };
        let Ok(exe) = self.toolchain.interpreter_required() else {
            return;
        };
        self.open_file(file.clone());
        self.spawn_build(exe, vec![file.display().to_string()]);
    }

    /// Transpila el archivo activo a C con `spasmc.py` (y lo construye si hay
    /// compilador de C), capturando la salida y los diagnósticos.
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
            self.spawn_build(program, args);
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
            "Compilar" => self.build(""),
            // "Ver" abre/cierra el desplegable de paneles.
            "Ver" => self.view_menu_open = !self.view_menu_open,
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

    /// Cierra el menú desplegable de "Ver".
    fn close_view_menu(&mut self) {
        self.view_menu_open = false;
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
                    self.palette.query.push_str(text);
                    self.palette_recompute();
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
        if control && text.eq_ignore_ascii_case("p") {
            self.palette_open(if shift {
                PaletteKind::Commands
            } else {
                PaletteKind::Files
            });
            return true;
        }
        false
    }

    /// Ejecuta la fila seleccionada y cierra la paleta.
    fn palette_accept(&mut self) {
        let action = self.palette.rows.get(self.palette.selected).cloned();
        self.palette.open = false;
        match action {
            Some(row) => match row.action {
                PaletteAction::Command(c) => self.run_command(c),
                PaletteAction::File(p) => self.open_file(p),
            },
            None => {}
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
            Command::Build => self.build(""),
            Command::Lint => self.lint_active(),
            Command::NextError => self.diagnostic_step(true),
            Command::Find => self.find_open(false),
            Command::Replace => self.find_open(true),
            Command::GotoDef => self.go_to_definition(),
            Command::Preview => self.toggle_preview(),
            Command::Save => self.save_active(),
            Command::FindFiles => self.palette_open(PaletteKind::Files),
            Command::OpenProject => self.pick_project(),
            Command::OpenFile => self.pick_file(),
            Command::NewFile => self.new_file(),
            Command::NewTerminal => {
                self.new_local_session();
                self.resize_local();
            }
            Command::SplitTerminal => self.split_terminal(),
            Command::ToggleExplorer => self.toggle_panel(0),
            Command::ToggleAgent => self.toggle_panel(1),
            Command::ToggleTerminal => self.toggle_panel(2),
            Command::ToggleGit => self.toggle_panel(3),
            Command::GitRefresh => self.git_refresh(),
            Command::GitStageAll => self.git_stage_all(),
            Command::GitPush => self.git_push(),
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
        // Paleta de comandos / buscador de archivos (global).
        if self.palette_intercept(text, control, shift) {
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
                    self.build("");
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
            // Al guardar, corre el analizador de SpASM.
            self.lint_active();
            // El archivo cambió: se relee el estado de Git.
            if !cfg!(test) {
                self.git_refresh();
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
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        for (line, text) in tab.lines.iter().enumerate() {
            let hits = find_in_line(text, &query, case, whole);
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

    /// Salta a la coincidencia siguiente o anterior (circular).
    fn find_step(&mut self, forward: bool) {
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
                '\u{F729}' => {
                    let (_, cursor) = self.find_field();
                    *cursor = 0;
                }
                '\u{F72b}' => self.find_move_cursor(0, true),
                c if c >= ' ' && c < '\u{E000}' => {
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
                for (line, text) in tab.lines.iter().enumerate() {
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
    /// pintada) y el estado de la barra de scrollback. Lo comparten la terminal
    /// local y el panel del agente.
    fn term_view(&mut self, target: Target) -> (Vec<TermLine>, i32, f32, f32) {
        match self.session_mut(target) {
            Some(session) => Self::term_view_of(session),
            None => (Vec::new(), 0, 1.0, 1.0),
        }
    }

    /// Arma el modelo de una sesión de terminal (filas + estado del scrollback).
    fn term_view_of(session: &mut LocalSession) -> (Vec<TermLine>, i32, f32, f32) {
        let (visible_rows, cols) = session.screen.size();
        let sel = session.sel.and_then(|s| s.region(cols));
        let total = session.screen.scrollback_total();
        let offset = session.screen.scrollback();
        let (rows, _cursor) = session.screen.snapshot(sel);
        let lines: Vec<TermLine> = rows
            .into_iter()
            .map(|runs| TermLine {
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
            })
            .collect();
        // Posición de la barra: 0 arriba (lo más viejo), 1 abajo.
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
        (lines, total as i32, pos, thumb)
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
        w.set_view_open(self.view_menu_open);

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

        let (term_lines, scroll_total, scroll_pos, scroll_thumb) = self.term_view(Target::Local);
        w.set_term_lines(ModelRc::from(Rc::new(VecModel::from(term_lines))));
        w.set_term_scroll_total(scroll_total);
        w.set_term_scroll_pos(scroll_pos);
        w.set_term_scroll_thumb(scroll_thumb);

        // Segunda vista de la terminal (si está dividida).
        let split_lines: Vec<TermLine> = match self.split {
            Some(index) => match self.local.get_mut(index) {
                Some(session) => Self::term_view_of(session).0,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        w.set_term_split(self.split.is_some());
        w.set_term_lines_2(ModelRc::from(Rc::new(VecModel::from(split_lines))));

        // --- panel del agente ---
        let (agent_lines, agent_total, agent_pos, agent_thumb) = self.term_view(Target::Agent);
        w.set_agent_lines(ModelRc::from(Rc::new(VecModel::from(agent_lines))));
        w.set_agent_scroll_total(agent_total);
        w.set_agent_scroll_pos(agent_pos);
        w.set_agent_scroll_thumb(agent_thumb);
        w.set_agent_connected(self.agent.as_ref().is_some_and(|a| a.alive));
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

        // --- Git ---
        w.set_git_repo(self.git_status.is_repo);
        w.set_git_branch(self.git_status.branch.clone().into());
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

        // --- paleta de comandos / buscador de archivos ---
        w.set_palette_open(self.palette.open);
        w.set_palette_title(
            match self.palette.kind {
                PaletteKind::Commands => "Comandos",
                PaletteKind::Files => "Archivos del proyecto",
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
        while let Ok(event) = self.local_rx.try_recv() {
            self.apply_pty_event(Target::Local, event);
            got = true;
        }
        while let Ok(event) = self.agent_rx.try_recv() {
            self.apply_pty_event(Target::Agent, event);
            got = true;
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
        if got {
            self.sync(w);
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
fn find_in_line(
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
fn collect_files(root: &Path) -> Vec<PathBuf> {
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
            c if c >= ' ' && c < '\u{E000}' => {
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
    // Slint no soporta listas de fuentes separadas por comas: `font-family` es un
    // único nombre. Se elige la mejor familia instalada.
    {
        let theme = window.global::<crate::ui::IdeTheme>();
        theme.set_sans(pick_font(SANS_FONTS, "Segoe UI").into());
        theme.set_mono(pick_font(MONO_FONTS, "Consolas").into());
    }

    ide.borrow_mut().sync(window);
    // Restaura el tamaño/posición recordados (si los hay).
    ide.borrow().apply_window(window);

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

    // Paleta de comandos / buscador: aceptar una fila o cerrarla.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_palette_accept(move |i| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.palette_accept_at(i);
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
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_view_menu_close(move || {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.close_view_menu();
                e.sync(&w);
            }
        });
    }

    // Cerrar desde el control de la barra superior: guarda el estado.
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_close_requested(move || {
            if let Some(w) = weak.upgrade() {
                if let Ok(mut e) = ide.try_borrow_mut() {
                    e.save_state(&w);
                }
            }
            let _ = slint::quit_event_loop();
        });
    }

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
    {
        let ide = Rc::clone(&ide);
        let weak = window.as_weak();
        window.on_terminal_scroll(move |dy| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.wheel(Target::Local, dy);
                e.sync(&w);
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
                e.sync(&w);
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
                e.sync(&w);
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
                e.agent_key(&text, control, shift, alt);
                e.sync(&w);
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
                e.sync(&w);
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
                e.sync(&w);
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
                e.sync(&w);
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
            assert_eq!(agent.name, "opencode");
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
        assert!(!ide.view_menu_open);

        ide.menu("Ver");
        assert!(ide.view_menu_open);
        ide.toggle_region(0);
        assert!(!ide.show_left);
        ide.toggle_region(1);
        assert!(!ide.show_right);
        ide.toggle_region(2);
        assert!(!ide.show_bottom);
        // Un tipo desconocido no rompe.
        ide.toggle_region(9);

        ide.close_view_menu();
        assert!(!ide.view_menu_open);
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
        assert!(git.iter().all(|c| matches!(
            c,
            Command::GitRefresh | Command::GitStageAll | Command::GitPush
        )));

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

        // Ctrl+Shift+P: paleta de comandos.
        assert!(ide.palette_intercept("P", true, true));
        assert!(ide.palette.open);
        assert_eq!(ide.palette.kind, PaletteKind::Commands);
        assert_eq!(ide.palette.rows.len(), Command::ALL.len());

        // Escribir filtra en vivo.
        ide.palette_key("g", false);
        ide.palette_key("i", false);
        ide.palette_key("t", false);
        assert!(ide.palette.rows.iter().all(|r| matches!(
            r.action,
            PaletteAction::Command(Command::GitRefresh | Command::GitStageAll | Command::GitPush)
        )));

        // Esc cierra.
        ide.palette_key("\u{1b}", false);
        assert!(!ide.palette.open);

        // Ctrl+P: buscador de archivos.
        assert!(ide.palette_intercept("p", true, false));
        assert_eq!(ide.palette.kind, PaletteKind::Files);
        assert_eq!(ide.palette.rows.len(), 1);

        // Mientras está abierta, las teclas no llegan al editor.
        assert!(ide.palette_key("x", false));
        assert_eq!(ide.palette.query, "x");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
