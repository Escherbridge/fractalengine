//! Installs TerrainTileset `.hexon` files into the local hexon store so the
//! in-app Map picker has tilesets available. Idempotent; honors `FE_HEXON_DIR`.
//! Non-tileset hexon types (Scene, GpxCollection, ...) are skipped with a note.
//!
//! Run: `cargo run -p fe-terrain --example install_sample_hexons [-- <file-or-dir>...]`
//! Default source: `sample-hexons/dist/` (build via fe-hexon's build_sample_hexons).

use fe_terrain::tiles::HexonStore;

fn main() -> anyhow::Result<()> {
    let mut sources: Vec<std::path::PathBuf> =
        std::env::args().skip(1).map(Into::into).collect();
    if sources.is_empty() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("fe-terrain lives under the workspace root")
            .join("sample-hexons")
            .join("dist");
        anyhow::ensure!(
            dist.is_dir(),
            "{} not found — run `cargo run -p fe-hexon --example build_sample_hexons` first",
            dist.display()
        );
        sources.push(dist);
    }

    let mut files = Vec::new();
    for src in sources {
        if src.is_dir() {
            for entry in std::fs::read_dir(&src)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) == Some("hexon") {
                    files.push(path);
                }
            }
        } else {
            anyhow::ensure!(src.is_file(), "no such file or directory: {}", src.display());
            files.push(src);
        }
    }

    let store = HexonStore::new()?;
    let installed: Vec<String> = store
        .list_installed()
        .into_iter()
        .map(|t| t.hexon_id)
        .collect();
    println!("store: {}", store.base_dir().display());

    for path in files {
        let bytes = std::fs::read(&path)?;
        match store.install_tileset(&bytes) {
            Ok(t) if installed.contains(&t.hexon_id) => {
                println!("refreshed {} ({})", t.hexon_id, t.region_name)
            }
            Ok(t) => println!("installed {} ({})", t.hexon_id, t.region_name),
            Err(e) => eprintln!("skipped {}: {e}", path.display()),
        }
    }

    println!("{} tileset(s) now installed", store.list_installed().len());
    Ok(())
}
