#pragma once
#include <stddef.h>
#include <stdarg.h>
typedef struct { int dummy; } FILE;
extern FILE *stderr;
extern FILE *stdout;
#define EOF (-1)
#define NULL ((void*)0)
int printf(const char *fmt, ...);
int fprintf(FILE *f, const char *fmt, ...);
int sprintf(char *buf, const char *fmt, ...);
int snprintf(char *buf, size_t size, const char *fmt, ...);
int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int fputs(const char *s, FILE *f);
int fputc(int c, FILE *f);
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);
int fflush(FILE *f);
int putchar(int c);
int puts(const char *s);

