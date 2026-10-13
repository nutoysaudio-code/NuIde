/* Punto de entrada FFI del módulo SpASM `spasm/cook.spasm` (NuCook).
 *
 * Ver router_ffi.c: el C generado se incluye y el `main` AOT se renombra.
 *
 * Contrato: funciones puras sobre un `const char*` (sin buffers estáticos), así
 * que no hace falta copiar nada antes de la próxima llamada. Serializado igual
 * por src/ffi.rs con un mutex (por uniformidad con el resto del puente).
 */
#include <stdint.h>

#define main nucoder_cook_aot_main
#include "cook.c"
#undef main

/* Índice de la etiqueta canónica (0..10) o -1. */
int64_t nucoder_cook_label_kind(const char *s)
{
    return spasm_func_label_kind(s);
}

/* 1 si `s` es una regla horizontal exacta. */
int64_t nucoder_cook_is_rule(const char *s)
{
    return spasm_func_is_rule(s);
}

/* `main` del programa AOT (expuesto para evitar el warning de sin uso). */
int nucoder_cook_run_main(void)
{
    return (int)nucoder_cook_aot_main();
}
