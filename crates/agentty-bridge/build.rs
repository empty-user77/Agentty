//! Links clang's runtime library on macOS builds with on-device voice.
//!
//! whisper.cpp's Metal backend checks the macOS version with `@available`. Built for an older
//! deployment target than the SDK (release builds target macOS 13), clang turns those checks into
//! calls to `__isPlatformVersionAtLeast`, which lives in `libclang_rt.osx.a`. rustc links with
//! `-nodefaultlibs` and never adds that library, so the final link fails without this.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos");
    let voice = std::env::var_os("CARGO_FEATURE_VOICE").is_some();
    if !(macos && voice) {
        return;
    }
    // The Xcode toolchain's clang first (what cc and cmake use), else whatever `clang` is on PATH.
    let runtime_dir = [&["xcrun", "clang", "--print-runtime-dir"][..], &["clang", "--print-runtime-dir"][..]]
        .iter()
        .filter_map(|argv| {
            let output = Command::new(argv[0]).args(&argv[1..]).output().ok()?;
            output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .find(|dir| std::path::Path::new(dir).join("libclang_rt.osx.a").is_file());
    match runtime_dir {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        None => println!("cargo:warning=libclang_rt.osx.a not found: a release build for an older macOS may fail to link"),
    }
}
