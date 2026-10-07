/* generado por spasm-c-gen (no editar a mano) */
/* programa "nuide_configkv" */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

static int64_t spasm_str_index_of(const char *s, const char *sub)
{
    const char *p = strstr(s, sub);
    return p ? (int64_t)(p - s) : -1;
}

static const char *spasm_str_slice(const char *s, int64_t start, int64_t len)
{
    static char buf[4096];
    int64_t n = (int64_t)strlen(s);
    int64_t i;
    if (start < 0) start = 0;
    if (start > n) start = n;
    if (len < 0) len = 0;
    if (start + len > n) len = n - start;
    for (i = 0; i < len; i++) buf[i] = s[start + i];
    buf[i] = 0;
    return buf;
}

static const char *spasm_str_trim(const char *s)
{
    static char buf[4096];
    int64_t n = (int64_t)strlen(s);
    int64_t a = 0, b = n, i;
    while (a < b && (s[a] == ' ' || s[a] == '\t' || s[a] == '\n' || s[a] == '\r')) a++;
    while (b > a && (s[b-1] == ' ' || s[b-1] == '\t' || s[b-1] == '\n' || s[b-1] == '\r')) b--;
    for (i = 0; i < b - a; i++) buf[i] = s[a + i];
    buf[i] = 0;
    return buf;
}

const char * cfg_out;

static int64_t spasm_func_cfg_skip(const char * line);
static int64_t spasm_func_cfg_key_of(const char * line);
static int64_t spasm_func_cfg_value_of(const char * line);

static int64_t spasm_func_cfg_skip(const char * line)
{
    const char * t;

    t = spasm_str_trim(line);
    if (((int64_t)strlen(t) == 0)) {
        return 1;
    }
    if ((((int64_t)(unsigned char)(t)[(0)]) == 35)) {
        return 1;
    }
    return 0;
}

static int64_t spasm_func_cfg_key_of(const char * line)
{
    const char * t;
    int64_t eq;

    t = spasm_str_trim(line);
    eq = spasm_str_index_of(t, "=");
    if ((eq < 0)) {
        return 0;
    }
    cfg_out = spasm_str_trim(spasm_str_slice(t, 0, eq));
    return 1;
}

static int64_t spasm_func_cfg_value_of(const char * line)
{
    const char * t;
    int64_t eq;

    t = spasm_str_trim(line);
    eq = spasm_str_index_of(t, "=");
    if ((eq < 0)) {
        return 0;
    }
    cfg_out = spasm_str_trim(spasm_str_slice(t, (eq + 1), (int64_t)strlen(t)));
    return 1;
}

int main(void)
{
    cfg_out = "";
    return 0;
}

