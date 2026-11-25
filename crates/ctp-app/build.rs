use std::path::Path;

fn main() {
    embed_kit();
    tauri_build::build()
}

/// Снимок agent-kit (`tools/kit-snapshot.ps1`, ADR-015) — в `OUT_DIR`, откуда его берёт
/// `include_bytes!`. Нет снимка (GitHub CI, чужая машина) — пустой файл: PGC без agent-kit.
fn embed_kit() {
    let kit = Path::new("kit");
    let (tar, version) = (kit.join("agent-kit.tar"), kit.join("agent-kit.version"));
    println!("cargo:rerun-if-changed={}", tar.display());
    println!("cargo:rerun-if-changed={}", version.display());
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let bytes = std::fs::read(&tar).unwrap_or_default();
    // Rewritten only when it changes: a fresh mtime would make include_bytes! rebuild the crate
    // on every cargo run where there is no snapshot (rerun-if-changed on a missing file).
    let dst = Path::new(&out).join("agent-kit.tar");
    if std::fs::read(&dst).ok().as_deref() != Some(&bytes[..]) {
        std::fs::write(&dst, &bytes).expect("write agent-kit.tar");
    }
    let version = std::fs::read_to_string(&version).unwrap_or_default();
    let version = if bytes.is_empty() { "" } else { version.trim() };
    println!("cargo:rustc-env=PGC_KIT_VERSION={version}");
}
