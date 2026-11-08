use std::{env, path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTC_LINKER");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android")
        || env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("aarch64") { return; }
    // Rust's -nodefaultlibs omits the compiler runtime providing __clear_cache on Android.
    let linker = env::var_os("RUSTC_LINKER").unwrap_or_else(|| "cc".into());
    let output = Command::new(linker).arg("--print-libgcc-file-name").output()
        .expect("Android JIT needs the NDK/Termux linker to locate compiler-rt");
    assert!(output.status.success(), "Android linker could not locate compiler-rt");
    let name = String::from_utf8(output.stdout).expect("compiler-rt path must be UTF-8");
    let runtime = Path::new(name.trim());
    assert!(runtime.is_file(), "Android compiler runtime not found: {}", runtime.display());
    let library = runtime.file_name().unwrap().to_str().unwrap().strip_prefix("lib").unwrap().strip_suffix(".a").unwrap();
    println!("cargo:rustc-link-search=native={}", runtime.parent().unwrap().display());
    println!("cargo:rustc-link-lib=static={library}");
}
