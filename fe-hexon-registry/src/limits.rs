//! Bounded zip-entry decompression (zip-bomb guard) — see `AGENTS.md` §decompression limits.
//!
//! The shared constants and the core capped-read now come straight from
//! `fe_format::limits` (PAS-P5 S3 §1): the only reason this was duplicated
//! (fe-format pulling in bevy via fe-runtime) is gone since S1 made the
//! `runtime` feature opt-in. `entry_cap`/`drain_entry_capped` stay local:
//! they are registry-specific helpers (extension-based cap selection, and a
//! drain-without-buffering sweep for `validate_package`'s whole-archive scan)
//! that fe-format has no reason to carry.

use std::io::Read;

use anyhow::{bail, Result};

pub use fe_format::limits::{
    check_entry_count, read_entry_capped, DECOMPRESSION_LIMIT_ERROR, MAX_ARCHIVE_ENTRIES,
    MAX_ARCHIVE_UNCOMPRESSED_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES, MAX_JSON_ENTRY_BYTES,
};

/// Per-entry cap for `name`: JSON metadata gets the small cap, everything else the binary cap.
pub fn entry_cap(name: &str) -> u64 {
    if name.ends_with(".json") {
        MAX_JSON_ENTRY_BYTES
    } else {
        MAX_ENTRY_UNCOMPRESSED_BYTES
    }
}

/// Decompress one entry into a sink under the same limits, without buffering it
/// (`validate_package`'s whole-archive bomb sweep drains every non-manifest
/// entry this way instead of keeping it).
pub fn drain_entry_capped<R: Read>(
    entry: R,
    declared: u64,
    cap: u64,
    budget: &mut u64,
) -> Result<()> {
    let limit = cap.min(*budget);
    if declared > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (declared {declared} bytes, limit {limit})");
    }
    // `take(limit + 1)` bounds real output even for a lying declared size.
    let actual = std::io::copy(&mut entry.take(limit + 1), &mut std::io::sink())?;
    if actual > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (more than {limit} bytes decompressed)");
    }
    *budget -= actual.max(declared);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_cap_picks_json_vs_binary() {
        assert_eq!(entry_cap("manifest.json"), MAX_JSON_ENTRY_BYTES);
        assert_eq!(entry_cap("chunks/index.json"), MAX_JSON_ENTRY_BYTES);
        assert_eq!(entry_cap("assets/deadbeef"), MAX_ENTRY_UNCOMPRESSED_BYTES);
    }

    #[test]
    fn drain_entry_capped_charges_the_budget_and_refuses_over_cap() {
        let mut budget = 100;
        drain_entry_capped([7u8; 10].as_slice(), 10, 64, &mut budget).expect("within limits");
        assert_eq!(budget, 90);

        let mut budget = 5;
        let err = drain_entry_capped([7u8; 10].as_slice(), 10, 64, &mut budget)
            .expect_err("over the remaining budget");
        assert!(err.to_string().contains(DECOMPRESSION_LIMIT_ERROR));
    }
}
