#pragma once
typedef unsigned long long jmp_buf[8];
int setjmp(jmp_buf env);
_Noreturn void longjmp(jmp_buf env, int val);

