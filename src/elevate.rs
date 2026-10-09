//! Elevación de privilegios (UAC) para las terminales, en Windows.
//!
//! Un proceso **no elevado no puede** meter un proceso elevado dentro de su
//! propio ConPTY: por eso la terminal embebida no puede "elevarse" sola. Hay dos
//! caminos, y la IDE ofrece los dos:
//!
//! - **Ventana aparte** ([`run_as_admin`]): abre una shell elevada en su propia
//!   ventana, sin tocar la IDE ni perder estado.
//! - **Reiniciar elevada** ([`relaunch_as_admin`]): relanza NuIde con UAC; a
//!   partir de ahí todas las terminales del panel corren como administrador.
//!
//! Fuera de Windows no hay UAC: [`is_elevated`] devuelve `false` y los comandos
//! de elevación fallan con un mensaje claro.

use std::path::Path;

/// `true` si el proceso actual corre con privilegios de administrador.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        imp::is_elevated()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Lanza `file` con `args`, pidiendo elevación (UAC) y con `dir` como carpeta de
/// trabajo. Devuelve `Err` si el usuario cancela el diálogo o falla el arranque.
pub fn run_as_admin(file: &str, args: &[String], dir: Option<&Path>) -> Result<(), String> {
    #[cfg(windows)]
    {
        imp::run_as_admin(file, args, dir)
    }
    #[cfg(not(windows))]
    {
        let _ = (file, args, dir);
        Err("la elevación sólo está disponible en Windows".to_string())
    }
}

/// Relanza esta misma aplicación elevada (UAC).
pub fn relaunch_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().ok();
    run_as_admin(&exe.display().to_string(), &[], cwd.as_deref())
}

/// `true` si el error corresponde a que el usuario canceló el UAC (no hay que
/// avisar nada: cancelar es una acción válida).
pub fn cancelled(err: &str) -> bool {
    err.contains("cancel")
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut c_void;
        fn IsUserAnAdmin() -> i32;
    }

    /// `SW_SHOWNORMAL`.
    const SW_SHOWNORMAL: i32 = 1;
    /// `ERROR_CANCELLED`: el usuario cerró el diálogo de UAC.
    const ERROR_CANCELLED: isize = 1223;

    /// Cadena UTF-16 terminada en `NUL`.
    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn is_elevated() -> bool {
        // `IsUserAnAdmin` (shell32) devuelve distinto de cero si el proceso es
        // de administrador. Está marcada como obsoleta pero sigue funcionando y
        // es la forma más directa de consultarlo.
        unsafe { IsUserAnAdmin() != 0 }
    }

    pub fn run_as_admin(file: &str, args: &[String], dir: Option<&Path>) -> Result<(), String> {
        let op = wide("runas");
        let file_w = wide(file);
        let params = (!args.is_empty()).then(|| wide(&join_args(args)));
        let dir_w = dir.map(|p| wide(&p.display().to_string()));

        let ret = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                file_w.as_ptr(),
                params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                dir_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                SW_SHOWNORMAL,
            )
        };
        // `ShellExecuteW` devuelve un `HINSTANCE`; un valor <= 32 es un error.
        let code = ret as isize;
        if code <= 32 {
            if code == ERROR_CANCELLED {
                Err(format!("elevación cancelada ({code})"))
            } else {
                Err(format!("no se pudo elevar «{file}» ({code})"))
            }
        } else {
            Ok(())
        }
    }

    /// Une argumentos en una línea, citando los que tengan espacios.
    fn join_args(args: &[String]) -> String {
        args.iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}
