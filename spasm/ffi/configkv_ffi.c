/* Punto de entrada FFI del módulo SpASM `spasm/configkv.spasm`.
 *
 * Ver el comentario de spasm/ffi/diagnostics_ffi.c: el C generado se incluye
 * para ver las funciones `static` y el `main` AOT se renombra.
 *
 * Contrato: `nuide_config_key_of` / `nuide_config_value_of` dejan el resultado
 * en la global `cfg_out`; hay que copiarlo antes de la siguiente llamada
 * (buffer estático). Serializado por src/spasm_logic.rs con un mutex.
 */
#include <stdint.h>

#define main nuide_configkv_aot_main
#include "configkv.c"
#undef main

/* ¿La línea se saltea (vacía o comentario)? 1 = sí. */
int64_t nuide_config_skip(const char *line)
{
    return spasm_func_cfg_skip(line);
}

/* Clave de la línea; 1 si tiene '='. El resultado queda en `cfg_out`. */
int64_t nuide_config_key_of(const char *line)
{
    return spasm_func_cfg_key_of(line);
}

/* Valor de la línea (llamar después de la clave con la misma línea). */
int64_t nuide_config_value_of(const char *line)
{
    return spasm_func_cfg_value_of(line);
}

/* Último resultado de clave/valor (nunca NULL). */
const char *nuide_config_out(void)
{
    return cfg_out ? cfg_out : "";
}

/* `main` del programa AOT (expuesto para evitar el warning de sin uso). */
int64_t nuide_configkv_run_main(void)
{
    return nuide_configkv_aot_main();
}
