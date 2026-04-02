#pragma once
typedef void (*sighandler_t)(int);
#define SIGABRT 6
static inline sighandler_t signal(int sig, sighandler_t handler) { (void)sig; (void)handler; return (sighandler_t)0; }

