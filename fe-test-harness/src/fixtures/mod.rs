//! Test fixtures for the fractalengine test harness.

/// Create a minimal valid GLB (glTF Binary) file.
///
/// Structure:
/// - 12-byte header: magic ("glTF"), version (2), total length
/// - 8-byte JSON chunk header: chunk length, chunk type (JSON = 0x4E4F534A)
/// - JSON payload: `{"asset":{"version":"2.0"}}`
/// - Padding to 4-byte alignment (spaces, per glTF spec)
pub fn create_minimal_glb() -> Vec<u8> {
    let json = br#"{"asset":{"version":"2.0"}}"#;
    let json_padded_len = (json.len() + 3) & !3; // pad to 4-byte alignment
    let total_len = 12 + 8 + json_padded_len; // header + chunk header + chunk data

    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(b"glTF"); // magic
    buf.extend_from_slice(&2u32.to_le_bytes()); // version
    buf.extend_from_slice(&(total_len as u32).to_le_bytes()); // length
    buf.extend_from_slice(&(json_padded_len as u32).to_le_bytes()); // chunk length
    buf.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // chunk type: JSON
    buf.extend_from_slice(json);
    buf.resize(total_len, 0x20); // pad with spaces
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_glb_has_correct_magic() {
        let glb = create_minimal_glb();
        assert!(glb.len() >= 12);
        assert_eq!(&glb[0..4], b"glTF");
    }

    #[test]
    fn minimal_glb_has_version_2() {
        let glb = create_minimal_glb();
        let version = u32::from_le_bytes([glb[4], glb[5], glb[6], glb[7]]);
        assert_eq!(version, 2);
    }

    #[test]
    fn minimal_glb_length_is_consistent() {
        let glb = create_minimal_glb();
        let stated_len = u32::from_le_bytes([glb[8], glb[9], glb[10], glb[11]]) as usize;
        assert_eq!(stated_len, glb.len());
    }

    #[test]
    fn minimal_glb_is_4byte_aligned() {
        let glb = create_minimal_glb();
        assert_eq!(glb.len() % 4, 0);
    }
}
