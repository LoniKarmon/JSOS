#pragma once
#include <stddef.h>
#define noreturn __attribute__((noreturn))
#define alloca __builtin_alloca
void *malloc(size_t size);
void free(void *ptr);
void *realloc(void *ptr, size_t new_size);
void *calloc(size_t nmemb, size_t size);
long strtol(const char *s, char **endptr, int base);
unsigned long strtoul(const char *s, char **endptr, int base);
long long strtoll(const char *s, char **endptr, int base);
unsigned long long strtoull(const char *s, char **endptr, int base);
double strtod(const char *s, char **endptr);
int atoi(const char *s);
long atol(const char *s);
noreturn void abort(void);
void exit(int status);
#define EXIT_FAILURE 1
#define EXIT_SUCCESS 0
#define RAND_MAX 32767
static inline int abs(int x) { return x < 0 ? -x : x; }
static inline long labs(long x) { return x < 0 ? -x : x; }
static inline long long llabs(long long x) { return x < 0 ? -x : x; }
