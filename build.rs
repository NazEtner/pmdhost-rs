// build.rs
// libx86emu(C)+ shim.c を同梱コンパイルしつつ、lymag-rs等と同じビルド情報の環境変数も埋める。
use chrono::Local;
use std::process::Command;

fn main() {
    build_native();
    emit_build_info();
}

// libx86emu と csrc/shim.c を cc でコンパイルして静的リンクする。
// cross(x86_64-pc-windows-gnu)では cc が自動的に x86_64-w64-mingw32-gcc を使う。
fn build_native() {
    let mut build = cc::Build::new();
    build
        .include("vendor/libx86emu/include")
        .file("vendor/libx86emu/api.c")
        .file("vendor/libx86emu/decode.c")
        .file("vendor/libx86emu/mem.c")
        .file("vendor/libx86emu/ops.c")
        .file("vendor/libx86emu/ops2.c")
        .file("vendor/libx86emu/prim_ops.c")
        .file("csrc/shim.c")
        .warnings(false);
    build.compile("x86emu_shim");

    println!("cargo:rerun-if-changed=csrc/shim.c");
    println!("cargo:rerun-if-changed=vendor/libx86emu");
}

fn emit_build_info() {
    let build_time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);

    let commit_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_COMMIT_HASH={}", commit_hash);

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_TARGET={}", target);

    let rustc_version = std::env::var("RUSTC")
        .ok()
        .and_then(|rustc| Command::new(rustc).arg("--version").output().ok())
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RUSTC_VERSION={}", rustc_version);
}
