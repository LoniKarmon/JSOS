#pragma once
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))
#define HUGE_VAL (__builtin_inf())

double floor(double x);
double ceil(double x);
double sqrt(double x);
double fabs(double x);
double fmod(double x, double y);
double pow(double x, double y);

double sin(double x);
double cos(double x);
double tan(double x);
double asin(double x);
double acos(double x);
double atan(double x);
double atan2(double y, double x);

double sinh(double x);
double cosh(double x);
double tanh(double x);
double asinh(double x);
double acosh(double x);
double atanh(double x);

double log(double x);
double log2(double x);
double log10(double x);
double exp(double x);
double exp2(double x);
double expm1(double x);
double log1p(double x);

double round(double x);
double trunc(double x);
double rint(double x);
double nearbyint(double x);
long lrint(double x);

double modf(double x, double *iptr);
double logb(double x);
int ilogb(double x);

double copysign(double x, double y);
double frexp(double x, int *exp);
double ldexp(double x, int exp);
double scalbn(double x, int n);
double scalbln(double x, long n);
double hypot(double x, double y);
double cbrt(double x);
double remainder(double x, double y);
double remquo(double x, double y, int *quo);
double nextafter(double x, double y);
double fmax(double x, double y);
double fmin(double x, double y);
double fdim(double x, double y);
double fma(double x, double y, double z);

float floorf(float x);
float ceilf(float x);
float sqrtf(float x);
float fabsf(float x);
float sinf(float x);
float cosf(float x);
float tanf(float x);
float modff(float x, float *iptr);

// Long double versions
long double sinl(long double x);
long double cosl(long double x);
long double tanl(long double x);
long double asinl(long double x);
long double acosl(long double x);
long double atanl(long double x);
long double atan2l(long double y, long double x);
long double sqrtl(long double x);
long double fabsl(long double x);
long double floorl(long double x);
long double ceill(long double x);
long double powl(long double x, long double y);
long double expl(long double x);
long double logl(long double x);
long double log10l(long double x);
long double modfl(long double x, long double *iptr);
long double fmodl(long double x, long double y);
long double copysignl(long double x, long double y);
long double nanl(const char *tagp);

#define fpclassify(x) __builtin_fpclassify(FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO, x)
#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x) __builtin_signbit(x)

#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4

#define M_PI 3.14159265358979323846
#define M_E  2.71828182845904523536
#define M_LN2 0.693147180559945309417
#define M_LN10 2.30258509299404568402
#define M_LOG2E 1.44269504088896340736
#define M_LOG10E 0.434294481903251827651
#define DBL_MAX 1.7976931348623157e+308
#define DBL_MIN 2.2250738585072014e-308
#define DBL_EPSILON 2.2204460492503131e-16
