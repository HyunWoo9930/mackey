use std::env;
use std::path::PathBuf;
use std::process::Command;

// Embed the Windows resources (requireAdministrator manifest + tray icons).
// Uses windres directly so the build works both natively on Windows (MSYS/GNU)
// and when cross-compiling from macOS/Linux with mingw-w64.
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/mackey.ico");
    println!("cargo:rerun-if-changed=res/mackey_off.ico");

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return; // host-side `cargo test` on macOS/Linux: nothing to embed
    }

    let windres = env::var("WINDRES").unwrap_or_else(|_| {
        if target.contains("gnu") && env::consts::OS != "windows" {
            "x86_64-w64-mingw32-windres".to_string()
        } else {
            "windres".to_string()
        }
    });

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app_res.o");
    let status = Command::new(&windres)
        .args(["res/app.rc", "-O", "coff", "-o"])
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {windres}: {e}"));
    assert!(status.success(), "{windres} failed");

    println!("cargo:rustc-link-arg-bins={}", out.display());
}
