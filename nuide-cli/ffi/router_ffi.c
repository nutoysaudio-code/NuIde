/* Punto de entrada FFI del modulo SpASM `spasm/router.spasm`.
 *
 * El C generado se incluye aca (no se compila aparte) porque las funciones del
 * programa son `static`: solo son visibles en este TU. El `main` del programa
 * AOT se renombra para no chocar con el de Rust.
 *
 * Contrato:
 *   nuide_router_pick(task, cands)              -> indice elegido (-1 si nada)
 *   nuide_router_pick_ex(task, cands, excluded) -> idem, saltando `excluded`
 *   nuide_router_is_power_task(task)            -> 1 si la tarea pide potencia
 *
 * `cands` = "free,power,cost;free,power,cost;..." ; `excluded` = "0,2,5".
 * No usa buffers estaticos para devolver (solo retorna enteros); igual se
 * serializa en src/ffi.rs porque el modulo incluye globales de los builtins.
 */
#include <stdint.h>

#define main nuide_router_aot_main
#include "router.c"
#undef main

/* Indice del candidato elegido por tarea+costo (-1 si ninguno valido). */
int64_t nuide_router_pick(const char *task, const char *cands)
{
    return spasm_func_router_pick(task, cands);
}

/* Idem, saltando los indices de `excluded` (rotacion por cuota agotada). */
int64_t nuide_router_pick_ex(const char *task, const char *cands, const char *excluded)
{
    return spasm_func_router_pick_ex(task, cands, excluded);
}

/* 1 si la tarea pide un modelo potente. */
int64_t nuide_router_is_power_task(const char *task)
{
    return spasm_func_router_is_power_task(task);
}

/* `main` del programa AOT (expuesto para evitar el warning de sin uso). */
int nuide_router_run_main(void)
{
    return (int)nuide_router_aot_main();
}
