//! Lógica de NuIde escrita en **SpASM** (dialecto compilado AOT), enlazada al
//! binario por FFI.
//!
//! Cómo está armado:
//!
//! - `spasm/*.spasm` es el fuente (dialecto `programa`/`main`; la KB del
//!   ecosistema está en `F:\SpAMS-Oficial\skills\spasm-architect\`).
//! - `spasm/generated/*.c` es el C que produce `spasmc.py --target c`. Está
//!   **commiteado** para que un clone limpio compile sin Python; se
//!   regenera con `tools/regen_spasm.ps1` y el test
//!   `el_c_generado_esta_al_dia` avisa si quedó viejo.
//! - `spasm/ffi/*_ffi.c` incluye el generado (las funciones del programa son
//!   `static`, sólo visibles en su TU) y exporta los símbolos `nuide_*` que
//!   declara este módulo. `build.rs` los compila con `cc` y los enlaza.
//! - Ver `spasm/README.md` para las reglas del puente y cómo agregar módulos.
//!
//! Reglas que impone el backend C (por eso existe este módulo):
//!
//! 1. Las funciones SpASM **sólo retornan enteros**; los strings salen por
//!    globales del módulo (`diag_msg`, `cfg_out`) y hay que **copiarlos antes
//!    de la siguiente llamada**: cada builtin de string usa un buffer
//!    estático. Por eso todos los llamados FFI de este módulo corren bajo un
//!    único mutex.
//! 2. `str_slice`/`str_trim` tienen buffers de 4 KiB: las líneas de
//!    longitud ≥ [`MAX_LINE`] se descartan (no deberían existir: son salidas
//!    del compilador o líneas de config).
//! 3. El trim del backend C recorta sólo ASCII (Rust recorta todo el
//!    whitespace Unicode); sólo se nota con no-ASCII exótico, que NuIde no
//!    escribe en su config ni produce su toolchain.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

use crate::lint::Diagnostic;

// Símbolos exportados por `spasm/ffi/*.c`.
extern "C" {
    fn nuide_diag_parse(line: *const c_char) -> i64;
    fn nuide_diag_message() -> *const c_char;
    fn nuide_config_skip(line: *const c_char) -> i64;
    fn nuide_config_key_of(line: *const c_char) -> i64;
    fn nuide_config_value_of(line: *const c_char) -> i64;
    fn nuide_config_out() -> *const c_char;
}

/// Límite de las líneas que van al FFI (buffers estáticos de 4 KiB).
const MAX_LINE: usize = 4096;

/// Serializa el FFI: el código C generado usa globales y buffers estáticos
/// sin protección. Si un hilo panicara con el lock, el guard se recupera
/// (`into_inner`): las llamadas C no panickean, no hay estado que sanear.
static FFI: Mutex<()> = Mutex::new(());

fn with_ffi<T>(f: impl FnOnce() -> T) -> T {
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// CString sin NULs intermedios: un NUL cortaría la cadena en C (los NUL
/// sólo aparecen en salida binaria, que nunca es un diagnóstico ni config).
fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("la cadena depurada no tiene NUL")
}

/// Copia una global de string del módulo (válida sólo hasta la siguiente
/// llamada FFI). Debe invocarse con el mutex tomado; `ptr` viene del bloque
/// `extern "C"` y apunta a la estática/global del wrapper.
fn copy(ptr: *const c_char) -> String {
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Diagnóstico en una línea de salida del toolchain (formato SpASM
/// `linea N: …` o gcc/clang `archivo:N:col: error: …`). La lógica vive en
/// `spasm/diagnostics.spasm`.
pub fn diagnostic_of(line: &str) -> Option<Diagnostic> {
    if line.len() >= MAX_LINE {
        return None;
    }
    let c = cstr(line);
    with_ffi(|| {
        // SAFETY: cadena C válida mientras `c` vive; sin punteros compartidos.
        let n = unsafe { nuide_diag_parse(c.as_ptr()) };
        if n < 0 {
            return None;
        }
        Some(Diagnostic {
            line: n as usize,
            // SAFETY: la global del wrapper es válida mientras se sostiene el
            // mutex; se copia acá, antes de la siguiente llamada C.
            message: copy(unsafe { nuide_diag_message() }),
        })
    })
}

/// ¿La línea de config se saltea (vacía o comentario `#`)? — `spasm/configkv.spasm`.
pub fn config_skip(line: &str) -> bool {
    let c = cstr(line);
    with_ffi(|| unsafe { nuide_config_skip(c.as_ptr()) != 0 })
}

/// Divide una línea `clave=valor` como `split_once` + trims. `None` si no
/// tiene `=` o si la línea es demasiado larga para el backend (4 KiB).
/// — `spasm/configkv.spasm`.
pub fn config_split(line: &str) -> Option<(String, String)> {
    if line.len() >= MAX_LINE {
        return None;
    }
    let c = cstr(line);
    with_ffi(|| {
        if unsafe { nuide_config_key_of(c.as_ptr()) } == 0 {
            return None;
        }
        // SAFETY: igual que `nuide_diag_message`, copia bajo el mutex.
        let key = copy(unsafe { nuide_config_out() });
        if unsafe { nuide_config_value_of(c.as_ptr()) } == 0 {
            return None;
        }
        let value = copy(unsafe { nuide_config_out() });
        Some((key, value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_el_formato_de_spasm() {
        let d = diagnostic_of("error: linea 5: funcion no declarada: foo").unwrap();
        assert_eq!(d.line, 5);
        assert!(d.message.contains("no declarada"));
    }

    #[test]
    fn parsea_el_formato_de_gcc() {
        let d = diagnostic_of("build/x.c:12:5: error: expected ';' before '}'").unwrap();
        assert_eq!(d.line, 12);
        assert!(d.message.contains("expected"));
    }

    #[test]
    fn ignora_la_salida_normal() {
        assert!(diagnostic_of("ok: build/x.c").is_none());
        assert!(diagnostic_of("siguiente: gcc x.c -o x").is_none());
    }

    #[test]
    fn parsea_warnings_y_las_bordes_de_las_dos_ramas() {
        let d = diagnostic_of("x.c:3:9: warning: unused variable 'z'").unwrap();
        assert_eq!(d.line, 3);
        assert_eq!(d.message, "9: warning: unused variable 'z'");
        // La rama SpASM tiene prioridad sobre gcc, como en el original.
        let d = diagnostic_of("warning: linea 2: warn con linea").unwrap();
        assert_eq!(d.line, 2);
        assert_eq!(d.message, "warn con linea");
        assert!(diagnostic_of("error: linea abc: sin número").is_none());
        assert!(diagnostic_of("algo linea 5 sin dos puntos").is_none());
    }

    #[test]
    fn config_hace_lo_mismo_que_split_once_con_trims() {
        assert!(!config_skip("project=F:\\x"));
        assert!(config_skip(""));
        assert!(config_skip("   # comentario"));
        assert_eq!(
            config_split("project = F:\\x "),
            Some(("project".to_string(), "F:\\x".to_string()))
        );
        assert_eq!(config_split("sin_igual"), None);
        assert_eq!(
            config_split("a=b=c"),
            Some(("a".to_string(), "b=c".to_string()))
        );
        assert_eq!(
            config_split("=vacia"),
            Some(("".to_string(), "vacia".to_string()))
        );
    }

    #[test]
    fn lineas_largas_se_descartan() {
        let larga = format!("error: linea 5: {}", "x".repeat(MAX_LINE));
        assert!(diagnostic_of(&larga).is_none());
        assert!(config_split(&format!("{}=1", "x".repeat(MAX_LINE))).is_none());
    }

    /// Varios hilos contra el mismo FFI: el mutex es parte del contrato.
    #[test]
    fn el_ffi_es_thread_safe() {
        let mut hilos = Vec::new();
        for _ in 0..4 {
            hilos.push(std::thread::spawn(|| {
                for i in 1..200 {
                    let d = diagnostic_of(&format!("build/x.c:{i}:1: error: e")).unwrap();
                    assert_eq!(d.line, i);
                    assert_eq!(
                        config_split("k = v"),
                        Some(("k".to_string(), "v".to_string()))
                    );
                }
            }));
        }
        for hilo in hilos {
            hilo.join().expect("un hilo del FFI falló");
        }
    }

    /// El C commiteado en `spasm/generated/` tiene que coincidir con una
    /// transpilación fresca. Se saltea si no hay Python ni spasmc.py.
    #[test]
    fn el_c_generado_esta_al_dia() {
        let toolchain = crate::spasm::Toolchain::discover();
        let (Some(python), Some(compiler)) = (toolchain.python.clone(), toolchain.compiler.clone())
        else {
            eprintln!("test salteado: falta Python o spasmc.py");
            return;
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::temp_dir().join(format!("nuide-spasm-gen-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let mut fuentes = std::fs::read_dir(root.join("spasm"))
            .expect("leer spasm/")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "spasm"))
            .collect::<Vec<_>>();
        fuentes.sort();
        assert!(!fuentes.is_empty(), "no hay *.spasm en spasm/");

        for src in fuentes {
            let stem = src.file_stem().unwrap().to_string_lossy().into_owned();
            let fresco = dir.join(format!("{stem}.c"));
            let mut args = vec![compiler.display().to_string()];
            args.extend(crate::spasm::compile_args(&src, &fresco, false));
            let out = std::process::Command::new(&python)
                .args(&args)
                .output()
                .expect("correr spasmc.py");
            assert!(
                out.status.success(),
                "spasmc.py falló para {stem}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let igual = |texto: &str| texto.replace("\r\n", "\n");
            let a = igual(&std::fs::read_to_string(&fresco).expect("leer el C fresco"));
            let commiteado = root
                .join("spasm")
                .join("generated")
                .join(format!("{stem}.c"));
            let b = igual(&std::fs::read_to_string(&commiteado).unwrap_or_else(|_| {
                panic!("falta spasm/generated/{stem}.c: corré tools/regen_spasm.ps1")
            }));
            assert_eq!(
                a, b,
                "spasm/generated/{stem}.c está desactualizado: corré tools/regen_spasm.ps1"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
