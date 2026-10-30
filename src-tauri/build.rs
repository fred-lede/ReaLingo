use std::process::Command;

fn main() {
    link_clang_runtime();
    tauri_build::build()
}

fn link_clang_runtime() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" || std::env::consts::OS != "macos" {
        return;
    }

    let output = Command::new("xcrun")
        .args(["--find", "clang"])
        .output()
        .expect("failed to locate Xcode clang with xcrun");
    assert!(output.status.success(), "xcrun could not locate clang");

    let clang = String::from_utf8(output.stdout).expect("xcrun returned a non-UTF-8 clang path");
    let output = Command::new(clang.trim())
        .arg("-print-file-name=libclang_rt.osx.a")
        .output()
        .expect("failed to locate the macOS Clang runtime");
    assert!(
        output.status.success(),
        "clang could not locate libclang_rt.osx.a"
    );

    let runtime =
        String::from_utf8(output.stdout).expect("clang returned a non-UTF-8 runtime path");
    let runtime = runtime.trim();
    assert!(
        std::path::Path::new(runtime).is_file(),
        "Clang runtime archive does not exist: {runtime}"
    );
    println!("cargo:rustc-link-arg={runtime}");
}
