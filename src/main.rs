//! NuIde.
//!
//! Arranque en tres pasos: se crea la ventana, se construye el estado de la IDE
//! y se enganchan los callbacks, y se corre. El estado vive en `editor::Ide`;
//! `main` no guarda nada.

// En release, el binario es una app de ventanas (sin consola): así, al abrirlo
// desde el Explorador o el menú Inicio no aparece una terminal vacía. En `debug`
// se deja la consola para ver la salida al desarrollar.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use nuide::editor::{self, Ide};
use nuide::ui::NuIdeWindow;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        attach_console();
        println!("NuIde {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        attach_console();
        println!(
            "NuIde {} — IDE para SpASM con terminales y OpenCode CLI\n\nUso: nuide [--version] [--help]",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let window = NuIdeWindow::new()?;
    let ide = Ide::shared();
    editor::attach(&window, ide);
    window.run()
}

/// Con el subsistema de ventanas (release) no hay consola: para `--version` /
/// `--help` se engancha la consola del proceso padre (si la hay) y se redirige
/// la salida, así el comando sigue imprimiendo al correrlo desde una terminal.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(dw_process_id: u32) -> i32;
            fn SetStdHandle(n_std_handle: u32, handle: *mut c_void) -> i32;
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                sa: *mut c_void,
                disposition: u32,
                flags: u32,
                template: *mut c_void,
            ) -> *mut c_void;
        }

        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ_WRITE: u32 = 3;
        const OPEN_EXISTING: u32 = 3;
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;

        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            let name: Vec<u16> = std::ffi::OsStr::new("CONOUT$")
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let handle = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle as isize != -1 {
                SetStdHandle(STD_OUTPUT_HANDLE, handle);
                SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
    }
}
