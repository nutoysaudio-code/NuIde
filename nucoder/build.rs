//! Build script de NuCoder.
//!
//! Compila los wrappers FFI de `ffi/*.c`. Cada wrapper incluye el C generado
//! de `spasm/generated/` (producido por `spasmc.py --target c`) y exporta los
//! símbolos `nuide_*` que declara `src/ffi.rs`.
//!
//! Regenerar el C (requiere Python + spasmc.py):
//!   tools/regen_spasm.ps1
//! El C se commitea para que un clone limpio compile sin Python.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=spasm");
    println!("cargo:rerun-if-changed=ffi");
    compilar_logica_spasm();
}

/// Compila los wrappers FFI de `ffi/*.c`. Si no hay wrappers, el build falla
/// explícito: la lógica declarada en `spasm/*.spasm` no puede quedar sin enlazar.
fn compilar_logica_spasm() {
    let mut build = cc::Build::new();
    // El generado primero: el `#include "router.c"` de los wrappers tiene que
    // resolver al C de `spasm/generated/`, no a otra copia en `ffi/`.
    build.include("spasm/generated").include("ffi");

    let mut wrappers = 0;
    if let Ok(entries) = std::fs::read_dir("ffi") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "c") {
                build.file(&path);
                wrappers += 1;
            }
        }
    }
    assert!(
        wrappers > 0,
        "ffi/ no tiene archivos .c: falta el wrapper FFI de los módulos SpASM"
    );
    build.flag_if_supported("-std=c99").compile("nucoder_spasm");
}
