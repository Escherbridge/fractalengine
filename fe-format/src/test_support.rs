//! Test-only archive tampering that models a realistic attacker.

use std::io::{Cursor, Read, Write};

/// Flip the first byte of the first member matching `target` and re-zip it with a FRESH
/// CRC (same compression method, same position); every other member is raw-copied. A
/// tamperer can always recompute a zip CRC, so only the signed digests can catch this —
/// flipping bytes in place would trip zip's own CRC check before the integrity layer runs.
pub(crate) fn tamper_member(bytes: &[u8], target: impl Fn(&str, &[u8]) -> bool) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(Cursor::new(bytes)).expect("source zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut tampered = false;
    for index in 0..source.len() {
        let (name, method, mut data) = {
            let mut file = source.by_index(index).expect("member");
            let mut data = Vec::new();
            file.read_to_end(&mut data).expect("member bytes");
            (file.name().to_string(), file.compression(), data)
        };
        if !tampered && !data.is_empty() && target(&name, &data) {
            data[0] ^= 0xFF;
            let options = zip::write::SimpleFileOptions::default().compression_method(method);
            out.start_file(name.as_str(), options)
                .expect("start member");
            out.write_all(&data).expect("write member");
            tampered = true;
        } else {
            out.raw_copy_file(source.by_index_raw(index).expect("raw member"))
                .expect("raw copy");
        }
    }
    assert!(tampered, "no member matched the tamper target");
    out.finish().expect("finish zip").into_inner()
}
