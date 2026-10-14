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
        // S6: also report integrity/layout — a cheap, real end-to-end
        // exercise of the S1-S6 stack (header verification + chunk
        // container detection) against every sample the repo ships, every
        // time this example runs.
        let index = fe_format::HexonArchiveIndex::open(&path)?;
        let integrity = match index.integrity_status() {
            fe_format::IntegrityStatus::Sealed { publisher_did } => {
                format!("sealed({publisher_did})")
            }
            fe_format::IntegrityStatus::Unsealed => "unsealed".to_string(),
        };
        let layout = match index.layout() {
            fe_format::Layout::Monolithic => "monolithic".to_string(),
            fe_format::Layout::Container { chunk_count } => format!("container({chunk_count})"),
        };
        println!(
            "built {} -> {} ({} bytes; entries={}, nodes={}, gpx={}, elevation_tiles={}, satellite_tiles={}; integrity={integrity}, layout={layout})",
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
