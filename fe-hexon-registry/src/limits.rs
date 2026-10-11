//! Bounded zip-entry decompression (zip-bomb guard) — see `AGENTS.md` §decompression limits.
//!
//! Mirrors `fe_format::limits` (same constants and error text); duplicated because
//! fe-format depends on fe-runtime (bevy), which the registry container must not pull in.

use std::io::Read;

use anyhow::{bail, Result};

/// Largest uncompressed size accepted for one binary archive entry (64 MiB).
pub const MAX_ENTRY_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
/// Largest total uncompressed bytes in one archive (1 GiB).
pub const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest uncompressed size accepted for one JSON metadata entry (8 MiB).
pub const MAX_JSON_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
/// Largest number of entries an archive may list in its central directory.
pub const MAX_ARCHIVE_ENTRIES: usize = 200_000;

/// Error text every limit refusal carries.
pub const DECOMPRESSION_LIMIT_ERROR: &str = "archive entry exceeds decompression limit";

/// Per-entry cap for `name`: JSON metadata gets the small cap, everything else the binary cap.
pub fn entry_cap(name: &str) -> u64 {
    if name.ends_with(".json") {
        MAX_JSON_ENTRY_BYTES
    } else {
        MAX_ENTRY_UNCOMPRESSED_BYTES
    }
}

/// Read one zip entry, refusing it when its declared OR actual size exceeds
/// `cap` or the remaining shared `budget`; decrements `budget` by the larger of
/// the declared and actual size (the buffer reserves `declared`).
pub fn read_entry_capped<R: Read>(
    entry: R,
    declared: u64,
    cap: u64,
    budget: &mut u64,
) -> Result<Vec<u8>> {
    let limit = check_declared(declared, cap, *budget)?;
    let mut buf = Vec::with_capacity(declared as usize);
    entry.take(limit + 1).read_to_end(&mut buf)?;
    charge(buf.len() as u64, limit, budget)?;
    // The reservation counts too (an over-declared entry keeps `declared` capacity).
    *budget -= declared.saturating_sub(buf.len() as u64);
    Ok(buf)
}

/// Decompress one entry into a sink under the same limits, without buffering it.
pub fn drain_entry_capped<R: Read>(
    entry: R,
    declared: u64,
    cap: u64,
    budget: &mut u64,
) -> Result<()> {
    let limit = check_declared(declared, cap, *budget)?;
    let actual = std::io::copy(&mut entry.take(limit + 1), &mut std::io::sink())?;
    charge(actual, limit, budget)
}

/// Effective limit for this entry; refuses a declared size above it before any allocation.
fn check_declared(declared: u64, cap: u64, budget: u64) -> Result<u64> {
    let limit = cap.min(budget);
    if declared > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (declared {declared} bytes, limit {limit})");
    }
    Ok(limit)
}

/// Refuse output beyond `limit` (a lying header), else drain `budget`.
fn charge(actual: u64, limit: u64, budget: &mut u64) -> Result<()> {
    if actual > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (more than {limit} bytes decompressed)");
    }
    *budget -= actual;
    Ok(())
}

/// Refuse an archive whose central directory lists more than [`MAX_ARCHIVE_ENTRIES`].
pub fn check_entry_count(count: usize) -> Result<()> {
    if count > MAX_ARCHIVE_ENTRIES {
        bail!("archive lists {count} entries (limit {MAX_ARCHIVE_ENTRIES})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An over-declared entry is charged its reservation, so many tiny entries
    /// declaring the cap cannot retain far more memory than the budget.
    #[test]
    fn over_declared_entry_charges_its_reservation() {
        let mut budget = 100;
        let buf = read_entry_capped([7u8].as_slice(), 10, 64, &mut budget).expect("within limits");
        assert_eq!(buf, [7u8]);
        assert_eq!(budget, 90);
        let mut drained = 100;
        drain_entry_capped([7u8].as_slice(), 10, 64, &mut drained).expect("within limits");
        assert_eq!(
            drained, 99,
            "a drain buffers nothing, so only output is charged"
        );
    }
}
