//! Estado de NuIde: el puente entre la ventana Slint y la lógica real.
//!
//! Este módulo mantiene el toolchain de SpASM, la consola local, el árbol de
//! archivos, las pestañas del editor y el archivo activo, y cablea los callbacks
//! de la ventana.
//!
//! ### Modelo de ejecución
//!
//! La terminal es *lineal* (ver `terminal`). El usuario escribe en la línea de
//! entrada; al apretar Enter, `Ide::submit` refleja el comando, lo despacha y, si
//! no es una orden propia de la IDE, lo pasa al shell del sistema.
//!
//! Los procesos corren en hilos y avisan por un canal. Como los callbacks de
//! Slint viven en el hilo de la UI, un `Timer` de 60 ms drena el canal y vuelca
//! el estado con `sync`. Ese timer se guarda en `Ide` para que no se caiga al
//! salir de `attach`; su closure usa un `Weak` para no crear un ciclo.
//!
//! ### Comandos propios
//!
//! `help`, `cd`, `clear`, `open`, `run`/`spasm` y `build`/`spasmc`. Cualquier
//! otra línea va al shell (`cmd /C` en Windows, `sh -c` en el resto). Así la IDE
//! puede lanzar `spasm.exe` y `spasmc.py` sin depender de que estén en el `PATH`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::explorer::{Action, Explorer};
use crate::highlight;
use crate::spasm::Toolchain;
use crate::terminal::{self, Line, Msg, Terminal};
use crate::ui::{IdeCodeLine, IdeTab, IdeTermLine, IdeToken, IdeTreeNode, NuIdeWindow};

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

/// Estado completo de la IDE.
pub struct Ide {
    toolchain: Toolchain,
    terminal: Terminal,
    /// Árbol de archivos, con raíz en la carpeta de la sesión activa.
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
    /// sesiones con la terminal local.
    agent: Terminal,
    agent_input: String,
    agent_tx: Sender<Msg>,
    agent_rx: Receiver<Msg>,
    /// Encabezado del panel de agente.
    agent_version: String,
    agent_model: String,
    /// Extremo emisor que se clona a cada proceso lanzado.
    tx: Sender<Msg>,
    /// Extremo receptor que el timer drena.
    rx: Receiver<Msg>,
    /// Texto que el usuario está escribiendo en la línea de entrada.
    input: String,
    /// `.spasm` sobre el que actúan los menús Ejecutar/Compilar.
    active_file: Option<PathBuf>,
    /// Timer del bucle de drenado.
    timer: Option<Timer>,
}

impl Ide {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (tx, rx) = mpsc::channel();
        let (agent_tx, agent_rx) = mpsc::channel();
        let mut agent = Terminal::new(cwd.clone());
        agent.sessions[0].name = "opencode".to_string();
        Self {
            toolchain: Toolchain::discover(),
            terminal: Terminal::new(cwd.clone()),
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
            tx,
            rx,
            input: String::new(),
            active_file: None,
            timer: None,
        }
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
    /// Cuenta qué encontró y qué no. No es decorativo: si el intérprete no está,
    /// el usuario tiene que enterarse antes de tocar «Ejecutar».
    fn banner(&mut self) {
        self.terminal.push_active(Line {
            text: "NuIde — SpASM Studio".into(),
            kind: 4,
        });
        self.terminal.push_active(Line {
            text: format!("carpeta: {}", self.terminal.active().cwd.display()),
            kind: 1,
        });
        match &self.toolchain.interpreter {
            Some(p) => self.terminal.push_active(Line {
                text: format!("intérprete: {}", p.display()),
                kind: 0,
            }),
            None => self.terminal.push_active(Line {
                text: "intérprete spasm.exe: no encontrado (definí NUIDE_SPASM)".into(),
                kind: 5,
            }),
        }
        match &self.toolchain.compiler {
            Some(p) => self.terminal.push_active(Line {
                text: format!("compilador: {}", p.display()),
                kind: 0,
            }),
            None => self.terminal.push_active(Line {
                text: "compilador spasmc.py: no encontrado (definí NUIDE_SPASMC)".into(),
                kind: 5,
            }),
        }
        self.terminal.push_active(Line {
            text: "escribí 'help' para ver los comandos propios".into(),
            kind: 1,
        });

        // Panel del agente: banner y versión real del CLI.
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
}

// ---------------------------------------------------------------------------
// Entrada y comandos
// ---------------------------------------------------------------------------

impl Ide {
    /// Procesa una pulsación de tecla de la terminal.
    ///
    /// En Slint los controles llegan como caracteres: Backspace es `\u{8}`, Enter
    /// es `\n`. Cuando `control` es verdadero se trata como atajo y no como
    /// texto.
    fn key(&mut self, text: &str, control: bool) {
        if control {
            match text.to_ascii_lowercase().as_str() {
                "c" => {
                    if !self.input.is_empty() {
                        self.input.clear();
                    }
                    self.terminal.push_active(Line {
                        text: "^C".into(),
                        kind: 1,
                    });
                }
                "l" => self.terminal.clear_active(),
                "u" => self.input.clear(),
                _ => {}
            }
            return;
        }

        if apply_text_key(&mut self.input, text) {
            self.submit();
        }
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

    /// Envía la línea de entrada. Refleja el comando con el prompt y lo despacha.
    fn submit(&mut self) {
        let raw = std::mem::take(&mut self.input);
        let prompt = self.terminal.prompt();
        self.terminal.push_active(Line {
            text: format!("{prompt}{raw}"),
            kind: 4,
        });
        let cmd = raw.trim().to_string();
        if !cmd.is_empty() {
            self.dispatch(&cmd);
        }
    }

    fn dispatch(&mut self, cmd: &str) {
        let (head, rest) = split_first(cmd);
        match head {
            "clear" | "cls" => self.terminal.clear_active(),
            "help" => self.help(),
            "cd" => match self.terminal.set_cwd(rest) {
                Ok(cwd) => {
                    // El explorador sigue a la carpeta de la sesión.
                    self.explorer.set_root(cwd.clone());
                    self.explorer_selected = -1;
                    self.terminal.push_active(Line {
                        text: cwd.display().to_string(),
                        kind: 1,
                    });
                }
                Err(e) => self.terminal.push_active(Line { text: e, kind: 3 }),
            },
            "open" => self.open(rest),
            "run" | "spasm" => {
                let arg = (!rest.trim().is_empty()).then_some(rest);
                self.run(arg)
            }
            "build" | "spasmc" => self.build(rest),
            _ => self.run_shell(cmd),
        }
    }

    fn help(&mut self) {
        let rows = [
            ("help", "esta ayuda"),
            ("cd <dir>", "cambia la carpeta de la sesión"),
            ("clear", "limpia la terminal"),
            ("open <archivo>", "marca el .spasm activo para los menús"),
            ("run [archivo]", "ejecuta un .spasm con el intérprete"),
            (
                "build [archivo]",
                "transpila un .spasm a C (y lo compila si hay cc)",
            ),
            ("<comando>", "se pasa al shell del sistema"),
        ];
        for (cmd, desc) in rows {
            self.terminal.push_active(Line {
                text: format!("  {cmd:<16} {desc}"),
                kind: 0,
            });
        }
    }

    fn open(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            self.terminal.push_active(Line {
                text: "uso: open <archivo.spasm>".into(),
                kind: 5,
            });
            return;
        }
        let path = self.resolve(arg);
        if path.is_file() {
            self.open_file(path);
        } else {
            self.terminal.push_active(Line {
                text: format!("no existe: {}", path.display()),
                kind: 3,
            });
        }
    }

    /// Ejecuta `arg` (o el archivo activo) con el intérprete de SpASM.
    fn run(&mut self, arg: Option<&str>) {
        let Some(file) = self.target(arg) else { return };
        // El intérprete corre el dialecto de intérprete (top-level). Un archivo
        // con `programa`/`main` es del dialecto compilado: avisar es mejor que
        // dejar que reviente con "Error de sintaxis".
        if uses_compiled_dialect(&file) {
            self.terminal.push_active(Line {
                text: "este archivo usa el dialecto compilado ('programa'/'main'); el intérprete \
                       no lo corre. Usá 'build' o el menú Compilar."
                    .into(),
                kind: 5,
            });
            return;
        }
        let exe = match self.toolchain.interpreter_required() {
            Ok(exe) => exe,
            Err(e) => {
                self.terminal.push_active(Line { text: e, kind: 5 });
                return;
            }
        };
        self.open_file(file.clone());
        self.spawn(exe, vec![file.display().to_string()]);
    }

    /// Transpila `arg` (o el archivo activo) a C con `spasmc.py` y, si hay un
    /// compilador de C disponible, construye el binario.
    fn build(&mut self, arg: &str) {
        let arg = (!arg.trim().is_empty()).then_some(arg);
        let Some(file) = self.target(arg) else { return };
        self.open_file(file.clone());

        // El `.c` sale en `build/<stem>.c`. `spasmc` no crea la carpeta: la
        // creamos acá o falla al escribir.
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string());
        let out_dir = self.terminal.active().cwd.join("build");
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            self.terminal.push_active(Line {
                text: format!("no se pudo crear {}: {e}", out_dir.display()),
                kind: 3,
            });
            return;
        }
        let out_c = out_dir.join(format!("{stem}.c"));

        let (program, args, build) = match self.toolchain.compile_command(&file, &out_c) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.terminal.push_active(Line { text: e, kind: 5 });
                return;
            }
        };
        if !build {
            self.terminal.push_active(Line {
                text: "no hay compilador C (CC/gcc/cc) en el PATH: se genera el .c, no el binario"
                    .into(),
                kind: 5,
            });
        }
        self.spawn(program, args);
    }

    /// Resuelve el archivo objetivo: el argumento o, si no hay, el activo.
    fn target(&mut self, arg: Option<&str>) -> Option<PathBuf> {
        let file = match arg.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => self.resolve(s),
            None => match self.active_file.clone() {
                Some(p) => p,
                None => {
                    self.terminal.push_active(Line {
                        text: "no hay archivo activo; usá 'run <archivo>' o 'open <archivo>'"
                            .into(),
                        kind: 5,
                    });
                    return None;
                }
            },
        };
        if !file.is_file() {
            self.terminal.push_active(Line {
                text: format!("no existe: {}", file.display()),
                kind: 3,
            });
            return None;
        }
        Some(file)
    }

    fn resolve(&self, arg: &str) -> PathBuf {
        let p = PathBuf::from(arg);
        if p.is_absolute() {
            p
        } else {
            self.terminal.active().cwd.join(p)
        }
    }

    /// Delega al shell del sistema una línea que no es comando propio.
    fn run_shell(&mut self, cmd: &str) {
        let (program, args) = shell_command(cmd);
        self.spawn(program, args);
    }

    /// Lanza un proceso en la sesión activa.
    fn spawn(&mut self, program: PathBuf, args: Vec<String>) {
        let session = self.terminal.active;
        let cwd = self.terminal.active().cwd.clone();
        let display = program.display().to_string();
        self.terminal.sessions[session].running = true;
        let tx = self.tx.clone();
        if let Err(e) = terminal::spawn_streaming(session, &cwd, &display, &args, tx) {
            self.terminal.push(
                session,
                Line {
                    text: format!("no se pudo ejecutar {display}: {e}"),
                    kind: 3,
                },
            );
            self.terminal.sessions[session].running = false;
        }
    }

    /// Acciones del menú superior.
    fn menu(&mut self, label: &str) {
        match label {
            "Ejecutar" => self.run(None),
            "Compilar" => self.build(""),
            _ => self.terminal.push_active(Line {
                text: format!("[{label}] todavía no está cableado"),
                kind: 1,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Sesiones
// ---------------------------------------------------------------------------

impl Ide {
    fn select_session(&mut self, i: i32) {
        if i >= 0 && (i as usize) < self.terminal.sessions.len() {
            self.terminal.active = i as usize;
            self.sync_explorer_root();
        }
    }

    fn new_session(&mut self) {
        let cwd = self.terminal.active().cwd.clone();
        let n = self.terminal.sessions.len() + 1;
        let name = format!("{} {n}", terminal::default_shell_name());
        self.terminal.add_session(name, cwd);
        self.sync_explorer_root();
    }

    fn close_session(&mut self) {
        self.terminal.close_session(self.terminal.active);
        self.sync_explorer_root();
    }

    fn split_terminal(&mut self) {
        self.terminal.push_active(Line {
            text: "dividir la terminal todavía no está implementado".into(),
            kind: 5,
        });
    }

    /// El explorador sigue a la carpeta de la sesión activa.
    fn sync_explorer_root(&mut self) {
        let cwd = self.terminal.active().cwd.clone();
        self.explorer.set_root(cwd);
        self.explorer_selected = -1;
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
            self.terminal.push_active(Line {
                text: format!("no se puede abrir: {}", path.display()),
                kind: 3,
            });
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
        self.terminal.push_active(Line {
            text: format!("abierto: {}", path.display()),
            kind: 4,
        });
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
            self.terminal.push_active(Line {
                text: "este archivo no es texto editable; no se guarda".into(),
                kind: 5,
            });
            return;
        }
        match std::fs::write(&path, data) {
            Ok(()) => {
                self.tabs[self.active_tab].dirty = false;
                self.terminal.push_active(Line {
                    text: format!("guardado: {}", path.display()),
                    kind: 4,
                });
            }
            Err(e) => self.terminal.push_active(Line {
                text: format!("no se pudo guardar {}: {e}", path.display()),
                kind: 3,
            }),
        }
    }

    fn undo_active(&mut self) {
        match self.undo.pop() {
            Some((lines, cursor)) => {
                let current = self.tabs[self.active_tab].snapshot();
                self.redo.push(current);
                self.apply_snapshot(lines, cursor);
            }
            None => self.terminal.push_active(Line {
                text: "nada que deshacer".into(),
                kind: 1,
            }),
        }
    }

    fn redo_active(&mut self) {
        match self.redo.pop() {
            Some((lines, cursor)) => {
                let current = self.tabs[self.active_tab].snapshot();
                self.undo.push(current);
                self.apply_snapshot(lines, cursor);
            }
            None => self.terminal.push_active(Line {
                text: "nada que rehacer".into(),
                kind: 1,
            }),
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

    /// Escribe el portapapeles del sistema. Avisa en la terminal si falla.
    fn write_clipboard(&mut self, text: &str) -> bool {
        match clipboard_write(text) {
            Ok(()) => true,
            Err(e) => {
                self.terminal.push_active(Line {
                    text: format!("portapapeles: {e}"),
                    kind: 3,
                });
                false
            }
        }
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
        // --- terminal ---
        let sessions: Vec<slint::SharedString> = self
            .terminal
            .sessions
            .iter()
            .map(|s| s.name.clone().into())
            .collect();
        w.set_term_sessions(ModelRc::from(Rc::new(VecModel::from(sessions))));
        w.set_active_session(self.terminal.active as i32);

        let lines: Vec<IdeTermLine> = self
            .terminal
            .active()
            .lines
            .iter()
            .map(|l| IdeTermLine {
                text: l.text.clone().into(),
                kind: l.kind,
            })
            .collect();
        w.set_term_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
        w.set_term_input(self.input.clone().into());
        w.set_term_prompt(self.terminal.prompt().into());

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
        while let Ok(msg) = self.rx.try_recv() {
            self.terminal.apply(msg);
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

/// Parte la primera palabra de un comando y devuelve `(palabra, resto)`.
fn split_first(cmd: &str) -> (&str, &str) {
    let cmd = cmd.trim();
    match cmd.find(char::is_whitespace) {
        Some(i) => (&cmd[..i], cmd[i..].trim_start()),
        None => (cmd, ""),
    }
}

/// ¿El archivo declara el dialecto compilado (`programa "..."`)?
///
/// Es una heurística de la primera palabra útil de una línea: alcanza para no
/// mandarle al intérprete un archivo que no va a entender.
fn uses_compiled_dialect(file: &std::path::Path) -> bool {
    std::fs::read_to_string(file)
        .map(|s| hay_dialecto_compilado(&s))
        .unwrap_or(false)
}

/// La parte pura de `uses_compiled_dialect`, para poder testearla sin disco.
fn hay_dialecto_compilado(source: &str) -> bool {
    source.lines().map(str::trim_start).any(|l| {
        l.starts_with("programa") && (l.len() == 8 || !l.as_bytes()[8].is_ascii_alphanumeric())
    })
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
        window.on_terminal_key(move |text, control| {
            if let Some(w) = weak.upgrade() {
                let mut e = ide.borrow_mut();
                e.key(&text, control);
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
    fn split_first_parte_por_el_primer_espacio() {
        assert_eq!(split_first("run a.spasm"), ("run", "a.spasm"));
        assert_eq!(split_first("  help  "), ("help", ""));
        assert_eq!(split_first(""), ("", ""));
    }

    #[test]
    fn escribir_y_borrar_en_la_linea_de_entrada() {
        let mut ide = Ide::new();
        ide.key("hola", false);
        assert_eq!(ide.input, "hola");
        ide.key("\u{8}", false); // backspace
        assert_eq!(ide.input, "hol");
        ide.key("a", false);
        assert_eq!(ide.input, "hola");
    }

    #[test]
    fn ctrl_u_limpia_la_entrada() {
        let mut ide = Ide::new();
        ide.key("xyz", false);
        ide.key("u", true);
        assert!(ide.input.is_empty());
    }

    #[test]
    fn detecta_el_dialecto_compilado() {
        assert!(hay_dialecto_compilado("programa \"demo\"\n\nmain {\n}\n"));
        assert!(hay_dialecto_compilado("  programa \"x\""));
        // Dialecto de intérprete: no.
        assert!(!hay_dialecto_compilado("print \"hola\";\nend;\n"));
        // Palabras que sólo empiezan igual no cuentan.
        assert!(!hay_dialecto_compilado("programacion = 1\n"));
        assert!(!hay_dialecto_compilado("; programa \"x\"\nend;\n"));
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
