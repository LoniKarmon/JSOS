#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <stdarg.h>
#include <math.h>

/* ===== Exported from Rust (js_runtime.rs) ===== */
extern void rust_serial_print(const char *s, size_t len);
extern void *rust_alloc(size_t size, size_t align);
extern void rust_dealloc(void *ptr, size_t size, size_t align);
extern void *rust_realloc(void *ptr, size_t old_size, size_t new_size, size_t align);
extern uint64_t rust_get_ticks(void);

extern double rust_floor(double x);
extern double rust_ceil(double x);
extern double rust_sqrt(double x);
extern double rust_fabs(double x);
extern double rust_fmod(double x, double y);
extern double rust_pow(double x, double y);
extern double rust_log(double x);
extern double rust_log2(double x);
extern double rust_log10(double x);
extern double rust_exp(double x);
extern double rust_expm1(double x);
extern double rust_log1p(double x);
extern double rust_sin(double x);
extern double rust_cos(double x);
extern double rust_tan(double x);
extern double rust_asin(double x);
extern double rust_acos(double x);
extern double rust_atan(double x);
extern double rust_atan2(double y, double x);
extern double rust_sinh(double x);
extern double rust_cosh(double x);
extern double rust_tanh(double x);
extern double rust_asinh(double x);
extern double rust_acosh(double x);
extern double rust_atanh(double x);
extern double rust_round(double x);
extern double rust_trunc(double x);
extern float rust_floorf(float x);
extern float rust_ceilf(float x);
extern float rust_sqrtf(float x);
extern float rust_fabsf(float x);
extern double rust_modf(double x, double *iptr);

/* ===== Memory Allocation ===== */
typedef struct {
    size_t size;
} AllocHeader;

void *malloc(size_t size) {
    if (size == 0) return NULL;
    AllocHeader *h = (AllocHeader *)rust_alloc(size + sizeof(AllocHeader), 16);
    if (!h) return NULL;
    h->size = size;
    return (void *)(h + 1);
}

void free(void *ptr) {
    if (!ptr) return;
    AllocHeader *h = (AllocHeader *)ptr - 1;
    rust_dealloc(h, h->size + sizeof(AllocHeader), 16);
}

void *realloc(void *ptr, size_t size) {
    if (!ptr) return malloc(size);
    if (size == 0) { free(ptr); return NULL; }
    AllocHeader *h = (AllocHeader *)ptr - 1;
    AllocHeader *new_h = (AllocHeader *)rust_realloc(h, h->size + sizeof(AllocHeader), size + sizeof(AllocHeader), 16);
    if (!new_h) return NULL;
    new_h->size = size;
    return (void *)(new_h + 1);
}

void *calloc(size_t nmemb, size_t size) {
    size_t total = nmemb * size;
    void *ptr = malloc(total);
    if (ptr) memset(ptr, 0, total);
    return ptr;
}

/* ===== String / Ctype ===== */
long strtol(const char *s, char **endptr, int base) {
    long result = 0;
    while (*s == ' ') s++;
    int neg = (*s == '-');
    if (neg || *s == '+') s++;
    while (*s >= '0' && *s <= '9') {
        result = result * base + (*s - '0');
        s++;
    }
    if (endptr) *endptr = (char *)s;
    return neg ? -result : result;
}

unsigned long strtoul(const char *s, char **endptr, int base) {
    unsigned long result = 0;
    while (*s == ' ') s++;
    if (*s == '+') s++;
    while (*s >= '0' && *s <= '9') {
        result = result * base + (*s - '0');
        s++;
    }
    if (endptr) *endptr = (char *)s;
    return result;
}

long long strtoll(const char *s, char **endptr, int base) {
    return (long long)strtol(s, endptr, base);
}

unsigned long long strtoull(const char *s, char **endptr, int base) {
    return (unsigned long long)strtoul(s, endptr, base);
}

double strtod(const char *s, char **endptr) {
    double result = 0.0;
    while (*s == ' ') s++;
    int negative = 0;
    if (*s == '-') { negative = 1; s++; }
    else if (*s == '+') s++;
    while (*s >= '0' && *s <= '9') { result = result * 10.0 + (*s - '0'); s++; }
    if (*s == '.') {
        s++;
        double frac = 0.1;
        while (*s >= '0' && *s <= '9') { result += (*s - '0') * frac; frac /= 10.0; s++; }
    }
    if (*s == 'e' || *s == 'E') {
        s++;
        int exp_neg = 0;
        if (*s == '-') { exp_neg = 1; s++; }
        else if (*s == '+') s++;
        int exp = 0;
        while (*s >= '0' && *s <= '9') { exp = exp * 10 + (*s - '0'); s++; }
        double mult = 1.0;
        for (int i = 0; i < exp; i++) mult *= 10.0;
        if (exp_neg) result /= mult; else result *= mult;
    }
    if (endptr) *endptr = (char *)s;
    return negative ? -result : result;
}

int atoi(const char *s) { return (int)strtol(s, NULL, 10); }
long atol(const char *s) { return strtol(s, NULL, 10); }

/* ===== I/O Redirection ===== */
typedef struct FILE { int dummy; } FILE;
FILE *stdout = NULL;
FILE *stderr = NULL;
FILE *stdin = NULL;

int vsnprintf(char *str, size_t size, const char *format, va_list ap) {
    // Ultra minimal vsnprintf for basic types used by QuickJS
    size_t i = 0, j = 0;
    while (format[i] && j < size - 1) {
        if (format[i] == '%' && format[i+1]) {
            i++;
            if (format[i] == 's') {
                const char *s = va_arg(ap, const char *);
                while (*s && j < size - 1) str[j++] = *s++;
            } else if (format[i] == 'd') {
                int d = va_arg(ap, int);
                if (d < 0) { if (j < size-1) str[j++] = '-'; d = -d; }
                char b[16]; int k = 0;
                do { b[k++] = (d % 10) + '0'; d /= 10; } while (d > 0);
                while (k > 0 && j < size-1) str[j++] = b[--k];
            } else if (format[i] == 'u') {
                unsigned int d = va_arg(ap, unsigned int);
                char b[16]; int k = 0;
                do { b[k++] = (d % 10) + '0'; d /= 10; } while (d > 0);
                while (k > 0 && j < size-1) str[j++] = b[--k];
            } else if (format[i] == 'x' || format[i] == 'p') {
                uintptr_t d = (format[i] == 'p') ? va_arg(ap, uintptr_t) : va_arg(ap, unsigned int);
                char b[20]; int k = 0;
                do { int rem = d % 16; b[k++] = (rem < 10) ? rem + '0' : rem - 10 + 'a'; d /= 16; } while (d > 0);
                while (k > 0 && j < size-1) str[j++] = b[--k];
            } else {
                str[j++] = format[i];
            }
        } else {
            str[j++] = format[i];
        }
        i++;
    }
    str[j] = '\0';
    return (int)j;
}

int vprintf(const char *format, va_list ap) {
    char buf[256];
    int len = vsnprintf(buf, sizeof(buf), format, ap);
    rust_serial_print(buf, (len < (int)sizeof(buf)) ? len : sizeof(buf) - 1);
    return len;
}

int printf(const char *format, ...) {
    char buf[256];
    va_list args;
    va_start(args, format);
    int len = vsnprintf(buf, sizeof(buf), format, args);
    va_end(args);
    rust_serial_print(buf, (len < (int)sizeof(buf)) ? len : sizeof(buf) - 1);
    return len;
}

int fprintf(FILE *stream, const char *format, ...) {
    (void)stream;
    va_list args;
    va_start(args, format);
    int r = vprintf(format, args);
    va_end(args);
    return r;
}

int sprintf(char *str, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int r = vsnprintf(str, 1024, format, args);
    va_end(args);
    return r;
}

int snprintf(char *str, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int r = vsnprintf(str, size, format, args);
    va_end(args);
    return r;
}



int fputs(const char *s, FILE *stream) { (void)stream; int len = strlen(s); rust_serial_print(s, len); return len; }
int fputc(int c, FILE *stream) { (void)stream; char ch = c; rust_serial_print(&ch, 1); return c; }
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream) { (void)stream; rust_serial_print(ptr, size * nmemb); return nmemb; }
int fflush(FILE *stream) { (void)stream; return 0; }
int putchar(int c) { char ch = c; rust_serial_print(&ch, 1); return c; }
int puts(const char *s) { int len = strlen(s); rust_serial_print(s, len); rust_serial_print("\n", 1); return len; }

/* ===== Time ===== */
typedef long time_t;
struct timeval { long tv_sec; long tv_usec; };
struct timespec { long tv_sec; long tv_nsec; };
struct tm { int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst; long tm_gmtoff; const char *tm_zone; };

time_t time(time_t *t) { time_t val = (time_t)(rust_get_ticks() / 100); if (t) *t = val; return val; }
long clock(void) { return (long)rust_get_ticks(); }
int gettimeofday(struct timeval *tv, void *tz) { (void)tz; if (tv) { uint64_t ticks = rust_get_ticks(); tv->tv_sec = (long)(ticks / 100); tv->tv_usec = (long)((ticks % 100) * 10000); } return 0; }
int clock_gettime(int clk_id, struct timespec *tp) { (void)clk_id; if (tp) { uint64_t ticks = rust_get_ticks(); tp->tv_sec = (long)(ticks / 100); tp->tv_nsec = (long)((ticks % 100) * 10000000); } return 0; }

struct tm *localtime(const time_t *t) { static struct tm tm_res; (void)t; memset(&tm_res, 0, sizeof(tm_res)); tm_res.tm_year = 70; tm_res.tm_mday = 1; return &tm_res; }
struct tm *localtime_r(const time_t *t, struct tm *result) { (void)t; memset(result, 0, sizeof(struct tm)); result->tm_year = 70; result->tm_mday = 1; return result; }
struct tm *gmtime(const time_t *t) { return localtime(t); }
size_t strftime(char *s, size_t max, const char *format, const struct tm *tm) { (void)format; (void)tm; if (max > 0) s[0] = '\0'; return 0; }
time_t mktime(struct tm *tm) { (void)tm; return 0; }

/* ===== Math ===== */
double floor(double x) { return rust_floor(x); }
double ceil(double x) { return rust_ceil(x); }
double sqrt(double x) { return rust_sqrt(x); }
double fabs(double x) { return rust_fabs(x); }
double fmod(double x, double y) { return rust_fmod(x, y); }
double pow(double x, double y) { return rust_pow(x, y); }
double log(double x) { return rust_log(x); }
double log2(double x) { return rust_log2(x); }
double log10(double x) { return rust_log10(x); }
double exp(double x) { return rust_exp(x); }
double expm1(double x) { return rust_expm1(x); }
double log1p(double x) { return rust_log1p(x); }
double sin(double x) { return rust_sin(x); }
double cos(double x) { return rust_cos(x); }
double tan(double x) { return rust_tan(x); }
double asin(double x) { return rust_asin(x); }
double acos(double x) { return rust_acos(x); }
double atan(double x) { return rust_atan(x); }
double atan2(double y, double x) { return rust_atan2(y, x); }
double sinh(double x) { return rust_sinh(x); }
double cosh(double x) { return rust_cosh(x); }
double tanh(double x) { return rust_tanh(x); }
double asinh(double x) { return rust_asinh(x); }
double acosh(double x) { return rust_acosh(x); }
double atanh(double x) { return rust_atanh(x); }
double round(double x) { return rust_round(x); }
double trunc(double x) { return rust_trunc(x); }
float floorf(float x) { return rust_floorf(x); }
float ceilf(float x) { return rust_ceilf(x); }
float sqrtf(float x) { return rust_sqrtf(x); }
float fabsf(float x) { return rust_fabsf(x); }
double modf(double x, double *iptr) { return rust_modf(x, iptr); }
double copysign(double x, double y) { union { double d; uint64_t u; } ux = { x }, uy = { y }; ux.u = (ux.u & 0x7FFFFFFFFFFFFFFFULL) | (uy.u & 0x8000000000000000ULL); return ux.d; }
double frexp(double x, int *exp) { if (x == 0.0) { *exp = 0; return 0.0; } union { double d; uint64_t u; } u = { x }; int e = (int)((u.u >> 52) & 0x7FF) - 1022; *exp = e; u.u = (u.u & 0x800FFFFFFFFFFFFFULL) | 0x3FE0000000000000ULL; return u.d; }
double ldexp(double x, int exp) { while (exp > 0) { x *= 2.0; exp--; } while (exp < 0) { x *= 0.5; exp++; } return x; }

double logb(double x) { return log2(fabs(x)); }
int ilogb(double x) { return (int)logb(x); }
long lrint(double x) { return (long)round(x); }

// Long double stubs (forward to double)
long double sinl(long double x) { return (long double)sin((double)x); }
long double cosl(long double x) { return (long double)cos((double)x); }
long double tanl(long double x) { return (long double)tan((double)x); }
long double asinl(long double x) { return (long double)asin((double)x); }
long double acosl(long double x) { return (long double)acos((double)x); }
long double atanl(long double x) { return (long double)atan((double)x); }
long double atan2l(long double y, long double x) { return (long double)atan2((double)y, (double)x); }
long double sqrtl(long double x) { return (long double)sqrt((double)x); }
long double fabsl(long double x) { return (long double)fabs((double)x); }
long double floorl(long double x) { return (long double)floor((double)x); }
long double ceill(long double x) { return (long double)ceil((double)x); }
long double powl(long double x, long double y) { return (long double)pow((double)x, (double)y); }
long double expl(long double x) { return (long double)exp((double)x); }
long double logl(long double x) { return (long double)log((double)x); }
long double log10l(long double x) { return (long double)log10((double)x); }
long double modfl(long double x, long double *iptr) { double i; double r = modf((double)x, &i); *iptr = (long double)i; return (long double)r; }
long double fmodl(long double x, long double y) { return (long double)fmod((double)x, (double)y); }
long double copysignl(long double x, long double y) { return (long double)copysign((double)x, (double)y); }
long double nanl(const char *tagp) { (void)tagp; return (long double)NAN; }

/* ===== Misc ===== */
_Noreturn void abort(void) { rust_serial_print("ABORT!\n", 7); while(1) {} }
void exit(int status) { (void)status; abort(); }
_Noreturn void __assert_fail(const char *expr, const char *file, int line, const char *func) {
    (void)file; (void)line; (void)func;
    rust_serial_print("ASSERT FAILED: ", 15);
    rust_serial_print(expr, strlen(expr));
    rust_serial_print("\n", 1);
    abort();
}

/* ===== setjmp / longjmp ===== */
typedef unsigned long long jmp_buf[8]; 
extern int setjmp(jmp_buf env);
extern _Noreturn void longjmp(jmp_buf env, int val);

/* ===== errno ===== */
int errno;
