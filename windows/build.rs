//! Compiles the icon, manifest and version info into the executable with
//! windres (MinGW). Without windres the app still builds, just without them.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/AppIcon.ico");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app_res.o");
    // Plain `windres` on Windows; the target-prefixed one when cross-compiling (Homebrew mingw-w64).
    let ok = ["windres", "x86_64-w64-mingw32-windres"].iter().any(|tool| {
        Command::new(tool)
            .current_dir("res")
            .args(["--input", "app.rc", "--output-format=coff", "--output"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success())
    });
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=windres not found; building without icon/manifest");
    }
}
