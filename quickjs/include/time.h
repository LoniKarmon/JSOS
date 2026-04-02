#pragma once
#include <stddef.h>
typedef long time_t;
typedef long clock_t;
#define CLOCKS_PER_SEC 100
struct timespec { long tv_sec; long tv_nsec; };
struct tm { 
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;
    const char *tm_zone;
};
time_t time(time_t *t);
clock_t clock(void);
struct tm *localtime(const time_t *t);
struct tm *localtime_r(const time_t *t, struct tm *result);
struct tm *gmtime(const time_t *t);
size_t strftime(char *s, size_t max, const char *format, const struct tm *tm);
time_t mktime(struct tm *tm);

#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
int clock_gettime(int clk_id, struct timespec *tp);
