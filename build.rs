//! Detecta `libmpv` en tiempo de compilación.
//!
//! Si existe -> se emite `cfg(mpv)` y se linkea dinámicamente contra `libmpv.so`.
//! Si no existe -> la app compila igual y usa el backend de audio 100% Rust.
//!
//! Override manual:
//!   SONIDO_NO_MPV=1              -> forzar el backend Rust
//!   MPV_LIB_DIR=/usr/lib/x86_64-linux-gnu   -> dónde buscar libmpv.so
//!   MPV_INCLUDE_DIR=/usr/include -> dónde buscar mpv/client.h

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-env-changed=SONIDO_NO_MPV");
    println!("cargo::rerun-if-env-changed=MPV_LIB_DIR");
    println!("cargo::rerun-if-env-changed=MPV_INCLUDE_DIR");
    println!("cargo::rerun-if-changed=build.rs");

    // Declarado para que `cargo` no avise por el cfg personalizado.
    println!("cargo::rustc-check-cfg=cfg(mpv)");

    let want = std::env::var("CARGO_FEATURE_MPV").is_ok();
    let disabled = std::env::var("SONIDO_NO_MPV").is_ok();

    if !want || disabled {
        println!("cargo::warning=sonido: compilando SIN soporte libmpv (backend de audio Rust)");
        return;
    }

    // Solo Linux por ahora (la app es nativa Linux).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        println!("cargo::warning=sonido: libmpv solo se auto-configura en Linux");
        return;
    }

    match find_mpv() {
        Some(libdir) => {
            println!("cargo::rustc-cfg=mpv");
            println!("cargo::rustc-link-search=native={}", libdir.display());
            println!("cargo::rustc-link-lib=dylib=mpv");
            println!(
                "cargo::warning=sonido: libmpv encontrada en {} -> gapless + FLAC Hi-Res + Atmos",
                libdir.display()
            );
        }
        None => {
            println!("cargo::warning=sonido: no se encontró libmpv (instalá libmpv-dev). Se usará el backend de audio Rust.");
        }
    }
}

fn find_mpv() -> Option<PathBuf> {
    // 1) pkg-config (lo más confiable)
    if let Some(dir) = via_pkg_config() {
        return Some(dir);
    }
    // 2) directorio explícito
    if let Ok(dir) = std::env::var("MPV_LIB_DIR") {
        let p = PathBuf::from(dir);
        if has_libmpv(&p) {
            return Some(p);
        }
    }
    // 3) rutas típicas de las distros
    let candidates = [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/usr/local/lib",
        "/usr/local/lib/x86_64-linux-gnu",
        "/lib/x86_64-linux-gnu",
    ];
    for c in candidates {
        let p = Path::new(c);
        if has_libmpv(p) {
            return Some(p.to_path_buf());
        }
    }
    None
}

fn via_pkg_config() -> Option<PathBuf> {
    let out = Command::new("pkg-config")
        .args(["--variable=libdir", "mpv"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    let p = PathBuf::from(s);
    // Confirmar que el linker realmente va a encontrar el .so
    if has_libmpv(&p) {
        Some(p)
    } else {
        Some(p)
    }
}

fn has_libmpv(dir: &Path) -> bool {
    for name in ["libmpv.so", "libmpv.so.2", "libmpv.so.1"] {
        if dir.join(name).exists() {
            return true;
        }
    }
    false
}
