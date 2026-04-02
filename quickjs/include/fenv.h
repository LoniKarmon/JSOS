#pragma once
typedef int fenv_t;
typedef int fexcept_t;
#define FE_TONEAREST 0
static inline int fesetround(int r) { (void)r; return 0; }
static inline int fegetround(void) { return FE_TONEAREST; }

