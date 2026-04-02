// build.rs - Compile QuickJS-NG C source for bare-metal x86_64
fn main() {
    let mut build = cc::Build::new();

    // Find clang and llvm-ar
    let clang = if cfg!(windows) { "clang.exe" } else { "clang" };
    let ar = if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" };

    // Try to find clang
    if let Ok(_) = std::process::Command::new(clang).arg("--version").output() {
        build.compiler(clang);
    } else {
        // Fallback for Windows if not in PATH but in common location
        if cfg!(windows) {
            let llvm_bin = "C:\\Program Files\\LLVM\\bin";
            let clang_path = format!("{}\\clang.exe", llvm_bin);
            if std::path::Path::new(&clang_path).exists() {
                build.compiler(&clang_path);
            }
        }
    }

    // Try to find llvm-ar
    if let Ok(_) = std::process::Command::new(ar).arg("--version").output() {
        build.archiver(ar);
    } else {
        // Fallback for Windows if not in PATH but in common location
        if cfg!(windows) {
            let llvm_bin = "C:\\Program Files\\LLVM\\bin";
            let ar_path = format!("{}\\llvm-ar.exe", llvm_bin);
            if std::path::Path::new(&ar_path).exists() {
                build.archiver(&ar_path);
            }
        }
    }

    // Set target for freestanding x86_64
    build.target("x86_64-unknown-none");
    build.flag("-target");
    build.flag("x86_64-unknown-none");

    build.flag("-std=c11");
    build.flag("-ffreestanding");
    build.flag("-nostdlib");
    build.flag("-fno-stack-protector");
    build.flag("-mno-red-zone");
    build.flag("-fno-exceptions");
    build.flag("-fno-builtin");
    build.flag("-O2");

    // Use absolute paths to avoid "file not found" errors under some build environments
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let qjs_dir = std::path::Path::new(&manifest_dir).join("quickjs");

    // Use our freestanding include directory for system headers
    build.include(qjs_dir.join("include"));
    // Include quickjs source dir for its own headers
    build.include(&qjs_dir);

    // Defines
    build.define("CONFIG_VERSION", "\"0.8.0\"");
    build.define("_GNU_SOURCE", None);
    build.define("CONFIG_BIGNUM", None);
    build.define("NDEBUG", None);
    // Disable features requiring OS
    build.define("__JSOS_FREESTANDING__", None);

    // Compile QuickJS core
    let files = [
        "quickjs.c", "libregexp.c", "libunicode.c", "dtoa.c", "freestanding.c", "setjmp.S"
    ];
    for f in &files {
        let p = qjs_dir.join(f);
        println!("cargo:warning=Checking file {:?}: exists={}", p, p.exists());
        build.file(p);
    }

    build.compile("quickjs");

    println!("cargo:rerun-if-changed=quickjs/");
}
