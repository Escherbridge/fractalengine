//! In-process fe-hexon-registry on an ephemeral port, plus a post-seal tamper helper.

use std::io::{Cursor, Read, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fe_hexon_registry::{build_router, scan_dir, RegistryState};

/// Serve the registry over whatever `.hexon` files `dir` holds (placed on
/// disk directly, bypassing publish — exactly how a tampered file would land).
pub async fn spawn_registry(dir: &Path) -> SocketAddr {
    let state = Arc::new(RegistryState::new(
        dir.to_path_buf(),
        None,
        false,
        scan_dir(dir),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    addr
}

/// Rewrite `archive` with member `name`'s bytes replaced (valid zip, valid
/// CRCs, untouched signed manifest) — a seal broken only at the digest level.
pub fn tamper_member(archive: &[u8], name: &str, replacement: &[u8]) -> Vec<u8> {
    let mut input = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for index in 0..input.len() {
        let mut file = input.by_index(index).unwrap();
        let file_name = file.name().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        output.start_file(file_name.as_str(), stored).unwrap();
        let bytes = if file_name == name {
            replacement
        } else {
            &data
        };
        output.write_all(bytes).unwrap();
    }
    output.finish().unwrap().into_inner()
}
