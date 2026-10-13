/* generado por spasm-c-gen (no editar a mano) */
/* programa "nucoder_cook" */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

int64_t cook_ok;

static int64_t spasm_func_label_kind(const char * s);
static int64_t spasm_func_is_rule(const char * s);

static int64_t spasm_func_label_kind(const char * s)
{
    if (((strcmp(s, "when") == 0) == 1)) {
        return 0;
    }
    if (((strcmp(s, "not") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(s, "want") == 0) == 1)) {
        return 2;
    }
    if (((strcmp(s, "have") == 0) == 1)) {
        return 3;
    }
    if (((strcmp(s, "do") == 0) == 1)) {
        return 4;
    }
    if (((strcmp(s, "keep") == 0) == 1)) {
        return 5;
    }
    if (((strcmp(s, "map") == 0) == 1)) {
        return 6;
    }
    if (((strcmp(s, "prove") == 0) == 1)) {
        return 7;
    }
    if (((strcmp(s, "eg") == 0) == 1)) {
        return 8;
    }
    if (((strcmp(s, "risk") == 0) == 1)) {
        return 9;
    }
    if (((strcmp(s, "rest") == 0) == 1)) {
        return 10;
    }
    return (0 - 1);
}

static int64_t spasm_func_is_rule(const char * s)
{
    if (((strcmp(s, "---") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(s, "----") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(s, "***") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(s, "___") == 0) == 1)) {
        return 1;
    }
    if (((strcmp(s, "===") == 0) == 1)) {
        return 1;
    }
    return 0;
}

int main(void)
{
    cook_ok = 0;
    return 0;
}

