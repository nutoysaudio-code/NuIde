//! Lanzar procesos hijos **sin ventana de consola** en Windows.
//!
//! NuCoder corre **dentro de NuIde**, que es un binario de subsistema *ventanas*
//! (sin consola). Cuando lanza un proceso de consola (`curl.exe`, la shell de
//! `bash`…) Windows le abre una consola nueva salvo que se pase
//! `CREATE_NO_WINDOW`. Todas las corridas de herramientas externas pasan por
//! [`hidden`] para que no parpadee ninguna ventana. Fuera de Windows es
//! simplemente `Command::new`.

use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Un `Command` para un programa externo que **no** abre consola.
pub fn hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `curl` (o `NUCODER_CURL`) sin consola.
pub fn curl() -> Command {
    hidden(std::env::var("NUCODER_CURL").unwrap_or_else(|_| "curl".to_string()))
}
