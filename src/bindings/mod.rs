use core::ffi::c_int;

// --- Memory Algorithms ---
#[no_mangle]
pub unsafe extern "C" fn memchr(s: *const u8, c: c_int, n: usize) -> *const u8 {
    let ch = c as u8;
    for i in 0..n {
        if *s.add(i) == ch {
            return s.add(i);
        }
    }
    core::ptr::null()
}

// --- String Algorithms ---
#[no_mangle]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    while *s.add(len) != 0 {
        len += 1;
    }
    len
}

#[no_mangle]
pub unsafe extern "C" fn strcmp(mut s1: *const u8, mut s2: *const u8) -> c_int {
    while *s1 != 0 && *s1 == *s2 {
        s1 = s1.add(1);
        s2 = s2.add(1);
    }
    (*s1 as c_int) - (*s2 as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn strncmp(mut s1: *const u8, mut s2: *const u8, mut n: usize) -> c_int {
    while n > 0 && *s1 != 0 && *s1 == *s2 {
        s1 = s1.add(1);
        s2 = s2.add(1);
        n -= 1;
    }
    if n == 0 { 0 } else { (*s1 as c_int) - (*s2 as c_int) }
}

#[no_mangle]
pub unsafe extern "C" fn strchr(mut s: *const u8, c: c_int) -> *const u8 {
    let ch = c as u8;
    while *s != 0 {
        if *s == ch { return s; }
        s = s.add(1);
    }
    if ch == 0 { s } else { core::ptr::null() }
}

#[no_mangle]
pub unsafe extern "C" fn strrchr(mut s: *const u8, c: c_int) -> *const u8 {
    let ch = c as u8;
    let mut last = core::ptr::null();
    while *s != 0 {
        if *s == ch { last = s; }
        s = s.add(1);
    }
    if ch == 0 { s } else { last }
}

// --- Math Algorithms ---
#[no_mangle]
pub extern "C" fn scalbn(x: f64, n: c_int) -> f64 { 
    libm::scalbn(x, n) 
}
