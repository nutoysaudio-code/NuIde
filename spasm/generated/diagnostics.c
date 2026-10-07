/* generado por spasm-c-gen (no editar a mano) */
/* programa "nuide_diagnostics" */
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

const char * diag_msg;

static int64_t spasm_func_diag_parse(const char * line);
static int64_t spasm_func_parse_uint(const char * s);
static int64_t spasm_func_contains_ci(const char * hay, const char * needle);

static int64_t spasm_func_diag_parse(const char * line)
{
    int64_t n;
    int64_t pos;
    int64_t colon;
    int64_t num;
    int64_t seg_start;
    int64_t i;
    const char * seg;
    const char * r;
    int64_t is_sep;

    n = (int64_t)strlen(line);
    pos = spasm_str_index_of(line, "linea ");
    colon = 0;
    num = 0;
    if ((pos >= 0)) {
        colon = spasm_str_index_of(spasm_str_slice(line, (pos + 6), n), ":");
        if ((colon >= 0)) {
            num = spasm_func_parse_uint(spasm_str_slice(line, (pos + 6), colon));
            if ((num >= 0)) {
                diag_msg = spasm_str_trim(spasm_str_slice(line, (((pos + 6) + colon) + 1), n));
                return num;
            }
        }
    }
    seg_start = 0;
    i = 0;
    seg = "";
    r = "";
    while ((i <= n)) {
        is_sep = 0;
        if ((i == n)) {
            is_sep = 1;
        }
        if ((i < n)) {
            if ((((int64_t)(unsigned char)(line)[(i)]) == 58)) {
                is_sep = 1;
            }
        }
        if ((is_sep == 1)) {
            seg = spasm_str_slice(line, seg_start, (i - seg_start));
            num = spasm_func_parse_uint(seg);
            if ((num >= 0)) {
                r = spasm_str_slice(line, (i + 1), n);
                if ((spasm_func_contains_ci(r, "error") == 1)) {
                    diag_msg = spasm_str_trim(r);
                    return num;
                }
                if ((spasm_func_contains_ci(r, "warning") == 1)) {
                    diag_msg = spasm_str_trim(r);
                    return num;
                }
            }
            seg_start = (i + 1);
        }
        i += 1;
    }
    return (-1);
}

static int64_t spasm_func_parse_uint(const char * s)
{
    const char * t;
    int64_t n;
    int64_t v;
    int64_t i;
    int64_t b;

    t = spasm_str_trim(s);
    n = (int64_t)strlen(t);
    if ((n == 0)) {
        return (-1);
    }
    if ((n > 18)) {
        return (-1);
    }
    v = 0;
    i = 0;
    b = 0;
    while ((i < n)) {
        b = ((int64_t)(unsigned char)(t)[(i)]);
        if ((b < 48)) {
            return (-1);
        }
        if ((b > 57)) {
            return (-1);
        }
        v = ((v * 10) + (b - 48));
        i += 1;
    }
    return v;
}

static int64_t spasm_func_contains_ci(const char * hay, const char * needle)
{
    int64_t nh;
    int64_t nn;
    int64_t i;
    int64_t j;
    int64_t ok;
    int64_t a;
    int64_t b;

    nh = (int64_t)strlen(hay);
    nn = (int64_t)strlen(needle);
    if ((nn == 0)) {
        return 1;
    }
    if ((nn > nh)) {
        return 0;
    }
    i = 0;
    j = 0;
    ok = 0;
    a = 0;
    b = 0;
    while ((i <= (nh - nn))) {
        ok = 1;
        j = 0;
        while ((j < nn)) {
            a = ((int64_t)(unsigned char)(hay)[((i + j))]);
            b = ((int64_t)(unsigned char)(needle)[(j)]);
            if ((a >= 65)) {
                if ((a <= 90)) {
                    a = (a + 32);
                }
            }
            if ((b >= 65)) {
                if ((b <= 90)) {
                    b = (b + 32);
                }
            }
            if ((a != b)) {
                ok = 0;
            }
            j += 1;
        }
        if ((ok == 1)) {
            return 1;
        }
        i += 1;
    }
    return 0;
}

int main(void)
{
    diag_msg = "";
    return 0;
}

