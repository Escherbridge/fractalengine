//! Embeds assets/branding/icon.ico as the Windows .exe icon (cosmetic: a host
//! without a resource compiler only warns) — see src/AGENTS.md §window-chrome.

fn main() {
    println!("cargo:rerun-if-changed=assets/branding/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    embed_windows_icon();
}

/// Write a one-line `.rc` naming the icon by absolute path, then compile + link it.
#[cfg(windows)]
fn embed_windows_icon() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let icon = manifest_dir
        .join("assets")
        .join("branding")
        .join("icon.ico");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let rc_path = out_dir.join("fractalengine-icon.rc");
    let escaped_icon = icon.display().to_string().replace('\\', "\\\\");
    if let Err(error) = std::fs::write(&rc_path, format!("1 ICON \"{escaped_icon}\"\n")) {
        println!(
            "cargo:warning=could not write {}: {error}",
            rc_path.display()
        );
        return;
    }
    if let Err(error) = embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional()
    {
        println!("cargo:warning=exe icon not embedded: {error:?}");
    }
}
