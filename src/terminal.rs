//! Consola local de NuIde.
//!
//! La terminal **no** emula una VT100: es un modelo lineal. Cada proceso que se
//! lanza escribe líneas en stdout/stderr; esas líneas se agregan al historial de
//! su sesión y la UI las dibuja una debajo de otra. El usuario escribe un
//! comando y, al apretar Enter, se ejecuta un proceso nuevo con el `cwd` de su
//! sesión.
//!
//! La decisión es deliberada, no un atajo: sin un parser ANSI, un PTY real
//! ensuciaría el panel con secuencias de escape (`\x1b[2K`, `\r`), y todavía no
//! hay una fila de entrada con edición de línea completa. El PTY del Skill
//! `pty` de SpASM queda como el siguiente paso, cuando el runtime lo soporte.
//!
//! Los procesos corren en hilos aparte y avisan al hilo de la UI por un canal
//! (`Sender<Msg>`). La UI drena ese canal con un `Timer` y vuelca el estado.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

/// Tope de líneas por sesión. Sin esto, un proceso que escupe sin parar come
/// memoria sin techo; el scrollback más viejo se descarta.
const MAX_LINES: usize = 2000;

/// Una línea de la terminal.
///
/// `kind` es el mismo índice que usa la UI (`IdeTermLine`): 0 normal, 1 apagado,
/// 2 añadido, 3 quitado (errores), 4 menta (comandos), 5 ámbar (avisos).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub kind: i32,
}

/// Una sesión de terminal: nombre, carpeta de trabajo y su scrollback.
#[derive(Debug)]
pub struct Session {
    pub name: String,
    pub cwd: PathBuf,
    pub lines: VecDeque<Line>,
    /// Hay un proceso en curso en esta sesión.
    pub running: bool,
}

impl Session {
    fn new(name: String, cwd: PathBuf) -> Self {
        Self {
            name,
            cwd,
            lines: VecDeque::new(),
            running: false,
        }
    }

    fn push(&mut self, line: Line) {
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }
}

/// Mensajes que mandan los hilos lectores al hilo de la UI.
pub enum Msg {
    /// Una línea nueva de stdout (`kind` 0) o stderr (`kind` 3).
    Line {
        session: usize,
        text: String,
        kind: i32,
    },
    /// El proceso terminó, con su código de salida.
    Exit { session: usize, code: i32 },
}

/// Todas las sesiones de la terminal.
pub struct Terminal {
    pub sessions: Vec<Session>,
    pub active: usize,
}

impl Terminal {
    /// Arranca con una sesión en `cwd`.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            sessions: vec![Session::new(default_shell_name().to_string(), cwd)],
            active: 0,
        }
    }

    pub fn active(&self) -> &Session {
        &self.sessions[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
    }

    /// Agrega una línea a la sesión indicada, si sigue existiendo.
    pub fn push(&mut self, session: usize, line: Line) {
        if let Some(s) = self.sessions.get_mut(session) {
            s.push(line);
        }
    }

    pub fn push_active(&mut self, line: Line) {
        self.push(self.active, line);
    }

    /// Aplica un mensaje de los hilos lectores.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Line {
                session,
                text,
                kind,
            } => self.push(session, Line { text, kind }),
            Msg::Exit { session, code } => {
                if let Some(s) = self.sessions.get_mut(session) {
                    s.running = false;
                    // Solo se anota la salida cuando es un error: un `[salida 0]`
                    // después de cada comando es ruido.
                    if code != 0 {
                        s.push(Line {
                            text: format!("[salida {code}]"),
                            kind: 1,
                        });
                    }
                }
            }
        }
    }

    pub fn clear_active(&mut self) {
        self.active_mut().lines.clear();
    }

    /// Agrega una sesión y la deja activa.
    pub fn add_session(&mut self, name: String, cwd: PathBuf) {
        self.sessions.push(Session::new(name, cwd));
        self.active = self.sessions.len() - 1;
    }

    /// Cierra una sesión. Nunca deja la terminal vacía: la última no se cierra.
    pub fn close_session(&mut self, idx: usize) {
        if self.sessions.len() <= 1 || idx >= self.sessions.len() {
            return;
        }
        self.sessions.remove(idx);
        if self.active >= self.sessions.len() {
            self.active = self.sessions.len() - 1;
        }
    }

    /// Cambia la carpeta de la sesión activa. Sin argumento, va al home.
    pub fn set_cwd(&mut self, arg: &str) -> Result<PathBuf, String> {
        let target = {
            let s = self.active();
            let arg = arg.trim();
            if arg.is_empty() {
                std::env::var_os("USERPROFILE")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| s.cwd.clone())
            } else {
                let p = PathBuf::from(arg);
                if p.is_absolute() {
                    p
                } else {
                    s.cwd.join(p)
                }
            }
        };
        if !target.is_dir() {
            return Err(format!("no es una carpeta: {}", target.display()));
        }
        let canon = target.canonicalize().unwrap_or(target);
        self.active_mut().cwd = canon.clone();
        Ok(canon)
    }

    /// El prompt de la sesión activa: el nombre de la carpeta actual.
    pub fn prompt(&self) -> String {
        let s = self.active();
        let name = s
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.cwd.display().to_string());
        format!("{name}> ")
    }
}

/// Nombre por defecto de una sesión de shell según la plataforma.
pub fn default_shell_name() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "bash"
    }
}

/// Lanza `program args` en `cwd` y transmite su salida por `tx`.
///
/// stdout y stderr se leen en hilos separados y se etiquetan con `kind` distinto
/// (0 y 3). Un tercer hilo espera al proceso, une a los lectores y recién ahí
/// avisa `Exit`, para que las últimas líneas lleguen antes del cierre.
pub fn spawn_streaming(
    session: usize,
    cwd: &Path,
    program: &str,
    args: &[String],
    tx: Sender<Msg>,
) -> std::io::Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // En Windows una consola nueva parpadearía en cada comando.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let out_thread = stdout.map(|out| {
        let tx = tx.clone();
        thread::spawn(move || pump(out, session, 0, tx))
    });
    let err_thread = stderr.map(|err| {
        let tx = tx.clone();
        thread::spawn(move || pump(err, session, 3, tx))
    });

    thread::spawn(move || {
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        if let Some(h) = out_thread {
            let _ = h.join();
        }
        if let Some(h) = err_thread {
            let _ = h.join();
        }
        let _ = tx.send(Msg::Exit { session, code });
    });

    Ok(())
}

/// Lee un stream línea por línea y manda cada una por el canal.
fn pump(stream: impl Read, session: usize, kind: i32, tx: Sender<Msg>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let raw = String::from_utf8_lossy(&buf);
                let text = strip_ansi(raw.trim_end_matches(['\n', '\r']));
                if tx
                    .send(Msg::Line {
                        session,
                        text,
                        kind,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Quita secuencias ANSI/control de una línea antes de mostrarla.
///
/// Soporta CSI (`ESC [ ... letra`) y OSC (`ESC ] ... BEL | ESC \`). Se conserva
/// el tabulador (indentación de salidas de compilador); el resto de controles,
/// como `\r`, se descartan para que un progreso con retorno de carro no rompa la
/// línea.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    // CSI: termina en un byte del rango '@'..='~'.
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: termina en BEL o en ST (ESC \).
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
        } else if c == '\t' || c >= ' ' {
            out.push(c);
        }
    }
    out
}

/// Parte un comando en argumentos respetando comillas dobles.
///
/// Es suficiente para las rutas y banderas que usa la IDE. No interpreta
/// comillas simples ni escapes: eso es trabajo del shell, y cuando se delega al
/// shell se le pasa la línea entera.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in input.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_quita_color() {
        assert_eq!(strip_ansi("\u{1b}[31mrojo\u{1b}[0m"), "rojo");
        assert_eq!(strip_ansi("\u{1b}[2K\u{1b}[1Gtexto"), "texto");
    }

    #[test]
    fn strip_ansi_conserva_tabs_y_descarta_cr() {
        assert_eq!(strip_ansi("a\tb"), "a\tb");
        assert_eq!(strip_ansi("progreso\rfinal"), "progresofinal");
    }

    #[test]
    fn strip_ansi_quita_osc() {
        // Título de ventana OSC, terminado en BEL.
        assert_eq!(strip_ansi("\u{1b}]0;título\u{7}ok"), "ok");
    }

    #[test]
    fn tokenize_respeta_comillas() {
        assert_eq!(tokenize("a b"), vec!["a", "b"]);
        assert_eq!(
            tokenize("spasm \"mi archivo.spasm\" --x"),
            vec!["spasm", "mi archivo.spasm", "--x"]
        );
        assert_eq!(tokenize("  "), Vec::<String>::new());
    }

    #[test]
    fn la_ultima_sesion_no_se_cierra() {
        let mut t = Terminal::new(PathBuf::from("."));
        t.close_session(0);
        assert_eq!(t.sessions.len(), 1);
    }

    #[test]
    fn el_tope_de_lineas_descarta_lo_viejo() {
        let mut t = Terminal::new(PathBuf::from("."));
        for i in 0..(MAX_LINES + 10) {
            t.push_active(Line {
                text: i.to_string(),
                kind: 0,
            });
        }
        assert_eq!(t.active().lines.len(), MAX_LINES);
        assert_eq!(
            t.active().lines.front().map(|l| l.text.as_str()),
            Some("10")
        );
    }

    /// Prueba real de punta a punta del lanzador de procesos: un `echo` del shell
    /// tiene que llegar como línea y el cierre tiene que anunciarse.
    #[cfg(windows)]
    #[test]
    fn spawn_streaming_entrega_stdout_y_exit() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cwd = std::env::temp_dir();
        let args = vec!["/D".to_string(), "/C".to_string(), "echo hola".to_string()];
        spawn_streaming(0, &cwd, "cmd.exe", &args, tx).expect("lanzar cmd");

        let mut lines = Vec::new();
        let mut exited = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !exited && std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(Msg::Line { text, .. }) => lines.push(text),
                Ok(Msg::Exit { .. }) => exited = true,
                Err(_) => {}
            }
        }
        assert!(exited, "no llegó el mensaje de cierre");
        assert!(
            lines.iter().any(|l| l.contains("hola")),
            "la salida no trajo el eco: {lines:?}"
        );
    }
}
