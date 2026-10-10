/* generado por spasm-c-gen (no editar a mano) */
/* programa "nucoder_router" */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

static int64_t spasm_func_task_wants_power(const char * task);
static int64_t spasm_func_is_excluded(int64_t idx, const char * excluded);
static int64_t spasm_func_router_pick_ex(const char * task, const char * cands, const char * excluded);
static int64_t spasm_func_router_pick(const char * task, const char * cands);
static int64_t spasm_func_router_is_power_task(const char * task);

static int64_t spasm_func_task_wants_power(const char * task)
{
    if (((strcmp(task, "refactor") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(task, "debug") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(task, "explain") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(task, "architect") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(task, "test") == 0) == 1)) {
        return 1;
    }
    return 0;
}

static int64_t spasm_func_is_excluded(int64_t idx, const char * excluded)
{
    int64_t n;
    int64_t i;
    int64_t cur;
    int64_t has;
    int64_t is_sep;
    int64_t b;

    n = (int64_t)strlen(excluded);
    if ((n == 0)) {
        return 0;
    }
    i = 0;
    cur = 0;
    has = 0;
    is_sep = 0;
    b = 0;
    while ((i <= n)) {
        is_sep = 0;
        if ((i == n)) {
            is_sep = 1;
        }
        if ((i < n)) {
            if ((((int64_t)(unsigned char)(excluded)[(i)]) == 44)) {
                is_sep = 1;
            }
        }
        if ((is_sep == 1)) {
            if ((has == 1)) {
                if ((cur == idx)) {
                    return 1;
                }
            }
            cur = 0;
            has = 0;
        } else {
            b = ((int64_t)(unsigned char)(excluded)[(i)]);
            if ((b >= 48)) {
                if ((b <= 57)) {
                    cur = ((cur * 10) + (b - 48));
                    has = 1;
                }
            }
        }
        i += 1;
    }
    return 0;
}

static int64_t spasm_func_router_pick_ex(const char * task, const char * cands, const char * excluded)
{
    int64_t wants;
    int64_t n;
    int64_t i;
    int64_t rec;
    int64_t field;
    int64_t cur;
    int64_t has;
    int64_t free;
    int64_t power;
    int64_t cost;
    int64_t best_idx;
    int64_t best_free;
    int64_t best_power;
    int64_t best_cost;
    int64_t is_sep;
    int64_t ch;
    int64_t better;

    wants = spasm_func_task_wants_power(task);
    n = (int64_t)strlen(cands);
    i = 0;
    rec = 0;
    field = 0;
    cur = 0;
    has = 0;
    free = 0;
    power = 0;
    cost = 0;
    best_idx = (-1);
    best_free = 0;
    best_power = 0;
    best_cost = 0;
    is_sep = 0;
    ch = 0;
    better = 0;
    while ((i <= n)) {
        is_sep = 0;
        if ((i == n)) {
            is_sep = 1;
        }
        if ((i < n)) {
            ch = ((int64_t)(unsigned char)(cands)[(i)]);
            if ((ch == 59)) {
                is_sep = 1;
            }
        }
        if ((is_sep == 1)) {
            cost = cur;
            if ((has == 1)) {
                if ((spasm_func_is_excluded(rec, excluded) == 0)) {
                    if ((best_idx == (-1))) {
                        best_idx = rec;
                        best_free = free;
                        best_power = power;
                        best_cost = cost;
                    } else {
                        better = 0;
                        if ((free != best_free)) {
                            if ((free > best_free)) {
                                better = 1;
                            }
                        } else {
                            if ((wants == 1)) {
                                if ((power != best_power)) {
                                    if ((power > best_power)) {
                                        better = 1;
                                    }
                                } else {
                                    if ((cost < best_cost)) {
                                        better = 1;
                                    }
                                }
                            } else {
                                if ((cost != best_cost)) {
                                    if ((cost < best_cost)) {
                                        better = 1;
                                    }
                                } else {
                                    if ((power > best_power)) {
                                        better = 1;
                                    }
                                }
                            }
                        }
                        if ((better == 1)) {
                            best_idx = rec;
                            best_free = free;
                            best_power = power;
                            best_cost = cost;
                        }
                    }
                }
            }
            rec += 1;
            field = 0;
            cur = 0;
            has = 0;
            free = 0;
            power = 0;
            cost = 0;
        } else {
            if ((ch == 44)) {
                if ((field == 0)) {
                    free = cur;
                }
                if ((field == 1)) {
                    power = cur;
                }
                field += 1;
                cur = 0;
                has = 0;
            } else {
                if ((ch >= 48)) {
                    if ((ch <= 57)) {
                        cur = ((cur * 10) + (ch - 48));
                        has = 1;
                    }
                }
            }
        }
        i += 1;
    }
    return best_idx;
}

static int64_t spasm_func_router_pick(const char * task, const char * cands)
{
    return spasm_func_router_pick_ex(task, cands, "");
}

static int64_t spasm_func_router_is_power_task(const char * task)
{
    return spasm_func_task_wants_power(task);
}

int main(void)
{
    return 0;
}

