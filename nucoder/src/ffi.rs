//! FFI bindings a los modulos SpASM AOT.
//!
//! `build.rs` compila los wrappers de `ffi/*.c` (que incluyen el C generado de
//! `spasm/generated/`) en una sola libreria estatica que Cargo enlaza.
//!
//! Thread-safety: el C generado usa globales y buffers estaticos para los
//! builtins de string, asi que **todos** los llamados se serializan con un
//! mutex (igual que `src/spasm_logic.rs` de NuIde).

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

extern "C" {
    fn nucoder_router_pick_ex(
        task: *const c_char,
        cands: *const c_char,
        excluded: *const c_char,
    ) -> i64;
    fn nucoder_router_is_power_task(task: *const c_char) -> i64;
    fn nucoder_codeassist_keywords(lang: *const c_char) -> i64;
    fn nucoder_codeassist_out() -> *const c_char;
    fn nucoder_cook_label_kind(s: *const c_char) -> i64;
    fn nucoder_cook_is_rule(s: *const c_char) -> i64;
}

static FFI: Mutex<()> = Mutex::new(());

/// Elige el indice del candidato segun `task` y la lista codificada
/// `free,power,cost;...`. Descarta los indices de `excluded` (rotacion por
/// cuota agotada). `None` si no queda ningun candidato valido.
pub fn router_pick(task: &str, cands: &str, excluded: &[usize]) -> Option<usize> {
    let task = CString::new(task).ok()?;
    let cands = CString::new(cands).ok()?;
    let ex = excluded
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let ex = CString::new(ex).ok()?;
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    let idx = unsafe { nucoder_router_pick_ex(task.as_ptr(), cands.as_ptr(), ex.as_ptr()) };
    (idx >= 0).then_some(idx as usize)
}

/// `true` si la tarea pide un modelo potente (lo decide el modulo SpASM).
pub fn router_is_power_task(task: &str) -> bool {
    let Ok(task) = CString::new(task) else {
        return false;
    };
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { nucoder_router_is_power_task(task.as_ptr()) != 0 }
}

/// Lista de keywords de completado para `lang` (la produce SpASM). Disponible
/// para la TUI; hoy la usa el modulo `codeassist` de SpASM.
#[allow(dead_code)]
pub fn codeassist_keywords(lang: &str) -> Vec<String> {
    let Ok(lang) = CString::new(lang) else {
        return Vec::new();
    };
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        nucoder_codeassist_keywords(lang.as_ptr());
        let ptr = nucoder_codeassist_out();
        if ptr.is_null() {
            return Vec::new();
        }
        // Copiar bajo el mutex, antes de la siguiente llamada FFI.
        CStr::from_ptr(ptr)
            .to_string_lossy()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

/// Índice de la etiqueta canónica de NuCook (0..10) o `None` si no es canónica.
/// Lo decide **SpASM** (`spasm/cook.spasm`, dialecto AOT).
pub fn cook_label_kind(s: &str) -> Option<usize> {
    let s = CString::new(s).ok()?;
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    let r = unsafe { nucoder_cook_label_kind(s.as_ptr()) };
    (r >= 0).then_some(r as usize)
}

/// `true` si `s` es una regla horizontal exacta (`---`, `***`, …). Lo decide SpASM.
pub fn cook_is_rule(s: &str) -> bool {
    let Ok(s) = CString::new(s) else {
        return false;
    };
    let _guard = FFI.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { nucoder_cook_is_rule(s.as_ptr()) != 0 }
}
