//! The one path-component guard for untrusted content ids (tileset/hexon ids);
//! see `fe-runtime/src/AGENTS.md` §safe-id.

/// Longest accepted id.
pub const MAX_SAFE_ID_LEN: usize = 128;

/// Windows device names that must never become a file or directory stem.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether `id` is safe as one path component, file stem and tag suffix.
pub fn is_safe_path_id(id: &str) -> bool {
    let stem = id.split('.').next().unwrap_or(id).to_ascii_lowercase();
    !id.is_empty()
        && id.len() <= MAX_SAFE_ID_LEN
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !RESERVED_DEVICE_NAMES.contains(&stem.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_path_id_table() {
        for ok in [
            "usgs-z12",
            "a",
            "tileset_1.v2",
            "A-b_c.d",
            "console",
            "nul2",
        ] {
            assert!(is_safe_path_id(ok), "{ok:?} should be accepted");
        }
        let long = "x".repeat(MAX_SAFE_ID_LEN + 1);
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "C:x",
            "a b",
            "é",
            "CON",
            "nul.txt",
            "Lpt1",
            long.as_str(),
        ] {
            assert!(!is_safe_path_id(bad), "{bad:?} should be refused");
        }
    }
}
