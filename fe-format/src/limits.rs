//! Bounded zip-entry decompression (zip-bomb guard) — see `fe-format/AGENTS.md` §decompression-limits.

use std::io::Read;

use anyhow::{bail, Result};

/// Largest uncompressed size accepted for one binary archive entry (64 MiB).
pub const MAX_ENTRY_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
/// Largest total uncompressed bytes read out of one archive (1 GiB).
pub const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest uncompressed size accepted for one JSON/text metadata entry (8 MiB).
pub const MAX_JSON_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
/// Largest number of entries an archive may list in its central directory.
pub const MAX_ARCHIVE_ENTRIES: usize = 200_000;

/// Error text every limit refusal carries (tests and callers match on it).
pub const DECOMPRESSION_LIMIT_ERROR: &str = "archive entry exceeds decompression limit";

/// Read one zip entry, refusing it when its declared OR actual size exceeds
/// `cap` or the remaining shared `budget`; decrements `budget` by the larger of
/// the declared and actual size (the buffer reserves `declared`).
pub fn read_entry_capped<R: Read>(
    entry: R,
    declared: u64,
    cap: u64,
    budget: &mut u64,
) -> Result<Vec<u8>> {
    let limit = cap.min(*budget);
    if declared > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (declared {declared} bytes, limit {limit})");
    }
    // `declared <= limit`, so this pre-allocation is bounded even for hostile headers.
    let mut buf = Vec::with_capacity(declared as usize);
    // `take(limit + 1)` bounds the real output when the declared size lies.
    entry.take(limit + 1).read_to_end(&mut buf)?;
    let actual = buf.len() as u64;
    if actual > limit {
        bail!("{DECOMPRESSION_LIMIT_ERROR} (more than {limit} bytes decompressed)");
    }
    // Charge the reservation too: a header declaring 64 MiB over a 1-byte body
    // would otherwise retain 64 MiB per entry while draining almost nothing.
    *budget -= actual.max(declared);
    Ok(buf)
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

    #[test]
    fn limits_apply_to_declared_and_actual_sizes_and_drain_the_budget() {
        // (payload length, declared, cap, budget, expect ok, budget after)
        let cases: &[(usize, u64, u64, u64, bool, u64)] = &[
            (10, 10, 10, 100, true, 90),   // exactly at cap
            (11, 11, 10, 100, false, 100), // declared over cap
            (11, 1, 10, 100, false, 100),  // lying header: actual over cap
            (10, 10, 64, 9, false, 9),     // over remaining budget
            (5, 0, 64, 9, true, 4),        // under-declared but within limits
            (1, 10, 64, 100, true, 90),    // over-declared: the reservation is charged
        ];
        for &(len, declared, cap, budget, ok, after) in cases {
            let mut remaining = budget;
            let payload = vec![7u8; len];
            let result = read_entry_capped(payload.as_slice(), declared, cap, &mut remaining);
            assert_eq!(result.is_ok(), ok, "case {len}/{declared}/{cap}/{budget}");
            if let Err(e) = &result {
                assert!(e.to_string().contains(DECOMPRESSION_LIMIT_ERROR));
            } else {
                assert_eq!(result.unwrap(), payload);
            }
            assert_eq!(remaining, after, "budget after case {len}/{declared}");
        }
    }
}
