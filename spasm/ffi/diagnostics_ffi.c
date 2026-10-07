/* Punto de entrada FFI del módulo SpASM `spasm/diagnostics.spasm`.
 *
 * El C generado se commitea en spasm/generated/ (regenerarlo con
 * tools/regen_spasm.ps1). Se incluye acá —en vez de compilarlo aparte—
 * porque las funciones del programa son `static`: sólo son visibles dentro
 * de este TU. El `main` del programa AOT se renombra con un macro para no
 * chocar con el `main` de Rust.
 *
 * Contrato: `nuide_diag_parse` deja el mensaje en la global `diag_msg`; hay
 * que copiarlo antes de la siguiente llamada FFI (buffer estático). Todos los
 * llamados los serializa src/spasm_logic.rs con un mutex.
 */
#include <stdint.h>

#define main nuide_diagnostics_aot_main
#include "diagnostics.c"
#undef main

/* Número de línea del diagnóstico de `line`, o -1 si no lo es. */
int64_t nuide_diag_parse(const char *line)
{
    return spasm_func_diag_parse(line);
}

/* Mensaje del último `nuide_diag_parse` exitoso (nunca NULL). */
const char *nuide_diag_message(void)
{
    return diag_msg ? diag_msg : "";
}

/* `main` del programa AOT: lo expone el wrapper para que el compilador no lo
   marque como sin uso (inicializa las globales del módulo). */
int64_t nuide_diagnostics_run_main(void)
{
    return nuide_diagnostics_aot_main();
}
