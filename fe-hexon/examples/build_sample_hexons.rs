//! Packs the repo-root `sample-hexons/` sources into `sample-hexons/dist/*.hexon`.

#[path = "../tests/support/sample_hexons.rs"]
mod sample_hexons;

fn main() -> anyhow::Result<()> {
    let root = sample_hexons::samples_root();
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist)?;

    for (name, bytes) in sample_hexons::build_all(&root)? {
        let path = dist.join(format!("{name}.hexon"));
        std::fs::write(&path, &bytes)?;

        let data = fe_format::HexonArchive::import(&bytes)?;
        println!(
            "built {} -> {} ({} bytes; entries={}, nodes={}, gpx={}, elevation_tiles={}, satellite_tiles={})",
            name,
            path.display(),
            bytes.len(),
            data.entries.len(),
            data.nodes.len(),
            data.gpx_files.len(),
            data.elevation_tiles.len(),
            data.satellite_tiles.len(),
        );
    }
    Ok(())
}
