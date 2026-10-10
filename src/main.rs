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
    pick_renderer();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        attach_console();
        print(&format!("NuIde {}\n", env!("CARGO_PKG_VERSION")));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        attach_console();
        print(&format!(
            "NuIde {} — IDE multi-lenguaje\n\nUso: nuide [--version] [--help]\n",
            env!("CARGO_PKG_VERSION")
        ));
        return Ok(());
    }

    let window = NuIdeWindow::new()?;

    // i18n: fija el idioma ANTES de construir el estado (los `.slint` con `@tr`
    // y el `i18n::t()` de Rust usan el mismo catálogo). `select_bundled_translation`
    // exige que ya exista el primer componente.
    let lang = nuide::config::Config::load()
        .get("ui.language")
        .unwrap_or(nuide::i18n::DEFAULT)
        .to_string();
    let _ = slint::select_bundled_translation(&lang);
    nuide::i18n::set_locale(&lang);

    let ide = Ide::shared();
    editor::attach(&window, ide);
    window.run()
}

/// Elige el renderer por defecto: **FemtoVG** (GPU/OpenGL).
///
/// El renderer software de Slint tiene un bug al maximizar: la ventana se vuelve
/// transparente durante el resize grande. FemtoVG no tiene ese problema.
///
/// Medido en NuIde con el panel de OpenCode abierto (una TUI que redibuja varias
/// veces por segundo), mismo binario, cambiando sólo `SLINT_BACKEND`:
///
/// | Renderer            | CPU en reposo | Memoria |
/// |---------------------|--------------:|--------:|
/// | `winit-femtovg`     |        ~77 %  | 113 MB  |
/// | `winit-software`    |        ~10 %  |  30 MB  |
///
/// FemtoVG repinta la escena **completa** ante cualquier cambio; el renderer
/// software trae *partial rendering* (sólo la región dañada) y es mucho más
/// barato en CPU, pero tiene el bug de transparencia al maximizar.
///
/// Si preferís menos CPU y podés vivir con el bug de transparencia, usá:
/// `$env:SLINT_BACKEND = "winit-software"`.
fn pick_renderer() {
    if std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", "winit-femtovg");
    }
}

/// Escribe en stdout sin pifiarla si no hay nadie del otro lado: con el
/// subsistema de ventanas el handle puede no existir (app lanzada desde el
/// Explorador) y `println!` entraría en panic.
fn print(text: &str) {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(text.as_bytes()).and_then(|()| lock.flush());
}

/// Con el subsistema de ventanas (release) no hay consola: para `--version` /
/// `--help` se engancha la consola del proceso padre, así el comando sigue
/// imprimiendo al correrlo desde una terminal.
///
/// Sólo cuando los handles estándar *no* existen: si ya vienen puestos
/// (redirección a archivo o caño, como en la CI), son la salida real y
/// enganchan `CONOUT$` se perdería esa salida.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(dw_process_id: u32) -> i32;
            fn GetStdHandle(n_std_handle: u32) -> *mut c_void;
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

        let invalid = |h: *mut c_void| h.is_null() || h == (-1isize as *mut c_void);
        if !invalid(GetStdHandle(STD_OUTPUT_HANDLE)) {
            return; // ya hay salida asignada (consola heredada o redirección)
        }

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
            if !invalid(handle) {
                SetStdHandle(STD_OUTPUT_HANDLE, handle);
                SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
    }
}
