/* Punto de entrada FFI del módulo SpASM `spasm/codeassist.spasm`.
 *
 * Ver router_ffi.c: el C generado se incluye y el `main` AOT se renombra.
 *
 * Contrato: `nucoder_codeassist_keywords` deja la lista en la global
 * `codeassist_out`; hay que copiarla antes de la siguiente llamada FFI
 * (buffer estático). Serializado por src/ffi.rs con un mutex.
 */
#include <stdint.h>

#define main nucoder_codeassist_aot_main
#include "codeassist.c"
#undef main

/* Keywords de completado para `lang`; deja la lista en `codeassist_out`. */
int64_t nucoder_codeassist_keywords(const char *lang)
{
    return spasm_func_keywords_for(lang);
}

/* Última lista de keywords (nunca NULL). */
const char *nucoder_codeassist_out(void)
{
    return codeassist_out ? codeassist_out : "";
}

/* `main` del programa AOT (expuesto para evitar el warning de sin uso). */
int nucoder_codeassist_run_main(void)
{
    return (int)nucoder_codeassist_aot_main();
}
