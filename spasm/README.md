# Lógica de NuIde en SpASM

Los `*.spasm` de acá son el **dialecto compilado (AOT)** de SpASM
(`programa` + `main {}`): la lógica pura de la IDE escrita en SpASM y
enlazada al binario Rust por FFI. La KB del lenguaje (obligatoria antes de
escribir código `.spasm`) vive en `F:\SpAMS-Oficial\skills\spasm-architect\`.

## Flujo

```text
spasm/*.spasm ── spasmc.py --target c ──▶ spasm/generated/*.c  (commiteado)
spasm/ffi/*_ffi.c   a mano: incluye el generado y exporta los símbolos nuide_*
build.rs (crate cc) ──▶ staticlib enlazado a nuide.exe
src/spasm_logic.rs   extern "C" + wrappers seguros (mutex + CString)
```

- **Regenerar el C**: `powershell -ExecutionPolicy Bypass -File tools/regen_spasm.ps1`
  (requiere Python y `spasmc.py`; rutas con `NUIDE_SPASMC` / `NUIDE_PYTHON`).
  El test `spasm_logic::tests::el_c_generado_esta_al_dia` falla si quedó viejo.
- Un clone limpio **no** necesita Python: el C va commiteado y `build.rs` sólo
  necesita un compilador de C (el mismo que ya pide Rust en Windows).

## Reglas del puente (las impone el backend C)

1. `func` sólo **retorna int/float**. Los strings salen por **globales del
   módulo** (los `let` de `main`): el wrapper los expone y Rust los copia
   dentro del mutex, antes de la siguiente llamada.
2. Cada builtin de string (`str_slice`, `str_trim`, `+`) usa un **buffer
   estático**: nunca guardar el puntero que devuelve — el siguiente slice lo
   pisa. Cortar siempre del string original (el parámetro) y consumir el
   resultado de inmediato. `str_slice`/`str_trim` tienen 4 KiB; el concat, 8 KiB.
3. **Thread-safety**: el C generado no tiene; `src/spasm_logic.rs` serializa
   todos los llamados con un `Mutex`.
4. `str_trim` recorta sólo ASCII (Rust recorta todo el whitespace Unicode):
   se asume ASCII en la config y en la salida del toolchain.
5. Estilo del ecosistema: comentarios en **español**, identificadores en
   **inglés**.

## Módulos

| Módulo | Migrado desde | Exporta |
|---|---|---|
| `diagnostics.spasm` | `src/build.rs` (`parse_diagnostics`) | `nuide_diag_parse`, `nuide_diag_message` |
| `configkv.spasm` | `src/config.rs` (`parse`) | `nuide_config_skip`, `nuide_config_key_of`, `nuide_config_value_of`, `nuide_config_out` |

## Cómo agregar un módulo

1. Crear `spasm/<nombre>.spasm` (con `main {}`; las globales del módulo van
   como `let` dentro de `main` y deben prefijarse para no chocar símbolos).
2. Correr `tools/regen_spasm.ps1`.
3. Escribir `spasm/ffi/<nombre>_ffi.c` con los exports `nuide_*` (copiar la
   estructura de los existentes: `#define main …_aot_main` + `#include`).
4. Declarar los símbolos y los wrappers seguros en `src/spasm_logic.rs`.
5. `cargo test` (la frescura del C se verifica sola; los tests de paridad
   tienen que seguir pasando).
