//! Compiles the icon, manifest and version info into the executable with
//! windres (MinGW). Without windres the app still builds, just without them.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/AppIcon.ico");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app_res.o");
    let status = Command::new("windres")
        .current_dir("res")
        .args(["--input", "app.rc", "--output-format=coff", "--output"])
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", out.display()),
        _ => println!("cargo:warning=windres not found; building without icon/manifest"),
    }
}
