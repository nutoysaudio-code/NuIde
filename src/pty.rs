//! Terminal real sobre un pseudo-terminal (ConPTY en Windows, PTY en Unix).
//!
//! A diferencia de la consola lineal de `terminal.rs`, acá hay un **proceso
//! persistente** (la shell) hablando por un PTY. Un hilo lee los bytes crudos
//! que emite y los manda por un canal; el emulador de `vt.rs` los interpreta y
//! mantiene la pantalla. Escribir en el PTY es mandarle teclas a la shell.
//!
//! Los eventos llevan el `session` para poder multiplexar varias terminales por
//! el mismo canal.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

/// Eventos que mandan los hilos del PTY al hilo de la UI.
pub enum PtyEvent {
    /// Bytes crudos leídos del PTY (van al emulador de pantalla).
    Output { session: u64, bytes: Vec<u8> },
    /// El proceso terminó.
    Exit { session: u64, code: i32 },
}

/// Un PTY abierto con su proceso. Mantiene el `master` vivo: si se cae, se
/// cierra el PTY y muere la shell.
pub struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

impl Pty {
    /// Abre un PTY, lanza `shell` con `args` en `cwd` y arranca los hilos de
    /// lectura y de espera.
    pub fn spawn(
        session: u64,
        shell: &str,
        args: &[String],
        cwd: &Path,
        cols: u16,
        rows: u16,
        tx: Sender<PtyEvent>,
    ) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        let mut cmd = CommandBuilder::new(shell);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);

        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = master.take_writer().map_err(|e| e.to_string())?;

        let tx_out = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx_out
                            .send(PtyEvent::Output {
                                session,
                                bytes: buf[..n].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        thread::spawn(move || {
            let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
            let _ = tx.send(PtyEvent::Exit { session, code });
        });

        Ok(Self { writer, master })
    }

    /// Le manda bytes al proceso (teclas).
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.writer.write_all(bytes).map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())
    }

    /// Avisa del tamaño nuevo de la ventana.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }
}

/// Shell por defecto: `NUIDE_SHELL`, si no el primer PowerShell del `PATH`, y si
/// no `cmd.exe`.
pub fn default_shell() -> (String, Vec<String>) {
    if let Ok(shell) = std::env::var("NUIDE_SHELL") {
        if !shell.trim().is_empty() {
            return (shell, Vec::new());
        }
    }
    #[cfg(windows)]
    {
        if let Some(p) = on_path(&["pwsh.exe", "pwsh"]) {
            return (p, vec!["-NoLogo".to_string()]);
        }
        if let Some(p) = on_path(&["powershell.exe", "powershell"]) {
            return (p, vec!["-NoLogo".to_string()]);
        }
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        (comspec, Vec::new())
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        (shell, Vec::new())
    }
}

/// `true` si la shell parece PowerShell (para cotizar comandos distinto).
pub fn is_powershell(shell: &str) -> bool {
    let name = shell.to_ascii_lowercase();
    name.contains("pwsh") || name.contains("powershell")
}

/// Busca uno de los nombres en el `PATH`.
pub fn on_path(names: &[&str]) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full.display().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    /// Prueba real de ConPTY: lanza `cmd /C echo` y espera leer la salida.
    ///
    /// ConPTY emite `\x1b[6n` (pedido de posición del cursor) y espera la
    /// respuesta `\x1b[fila;columnaR`; sin contestar, la salida no llega.
    #[cfg(windows)]
    #[test]
    fn el_pty_devuelve_la_salida_del_proceso() {
        let (tx, rx) = channel();
        let mut pty = Pty::spawn(
            0,
            "cmd.exe",
            &["/C".to_string(), "echo hola-pty".to_string()],
            &std::env::temp_dir(),
            80,
            24,
            tx,
        )
        .expect("abrir el PTY");

        let mut out: Vec<u8> = Vec::new();
        let mut exited = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        while !exited && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(PtyEvent::Output { bytes, .. }) => {
                    if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = pty.write(b"\x1b[1;1R");
                    }
                    out.extend_from_slice(&bytes);
                }
                Ok(PtyEvent::Exit { .. }) => exited = true,
                Err(_) => {}
            }
        }
        drop(pty);
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("hola-pty"),
            "el PTY no devolvió la salida: {text:?}"
        );
    }

    /// Prueba de humo: `opencode` (un `.cmd` de npm en Windows) arranca dentro
    /// de un PTY y emite algo. Se saltea si no está instalado.
    #[cfg(windows)]
    #[test]
    fn el_pty_lanza_opencode() {
        let Some(agent) = crate::spasm::Toolchain::discover().agent else {
            eprintln!("test salteado: no se encontró opencode");
            return;
        };
        let (tx, rx) = channel();
        let mut pty = Pty::spawn(
            0,
            &agent.display().to_string(),
            &[],
            &std::env::temp_dir(),
            100,
            30,
            tx,
        )
        .expect("abrir el PTY con opencode");

        let mut out: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while out.len() < 64 && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(PtyEvent::Output { bytes, .. }) => {
                    if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = pty.write(b"\x1b[1;1R");
                    }
                    out.extend_from_slice(&bytes);
                }
                Ok(PtyEvent::Exit { code, .. }) => {
                    panic!("opencode terminó demasiado pronto: código {code}");
                }
                Err(_) => {}
            }
        }
        drop(pty);
        assert!(!out.is_empty(), "opencode no emitió salida en el PTY");
    }

    /// Prueba de ida y vuelta con una shell interactiva: se le mandan teclas y
    /// la salida vuelve.
    #[cfg(windows)]
    #[test]
    fn el_pty_acepta_teclas() {
        let (tx, rx) = channel();
        let mut pty =
            Pty::spawn(0, "cmd.exe", &[], &std::env::temp_dir(), 80, 24, tx).expect("abrir el PTY");

        let mut out = Vec::new();
        let mut sent = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(PtyEvent::Output { bytes, .. }) => {
                    if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = pty.write(b"\x1b[1;1R");
                    }
                    if !sent {
                        // La shell ya arrancó: mandamos el comando.
                        let _ = pty.write(b"echo hola-teclas\r");
                        sent = true;
                    }
                    out.extend_from_slice(&bytes);
                    if String::from_utf8_lossy(&out).contains("hola-teclas") {
                        break;
                    }
                }
                Ok(PtyEvent::Exit { .. }) => break,
                Err(_) => {}
            }
        }
        drop(pty);
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("hola-teclas"),
            "la shell no devolvió lo escrito: {text:?}"
        );
    }
}
