//! Typed affine route changes preserve timestamps and opaque trailing metadata.

use serde_json::Value;

pub(super) fn validate_restore(before: &Value, after: &Value) -> anyhow::Result<()> {
    let old = before
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("stored route missing"))?;
    let new = after
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("restored route missing"))?;
    anyhow::ensure!(
        old.len() == new.len() && !new.is_empty(),
        "whole-route restore cannot change topology"
    );
    for (old, new) in old.iter().zip(new) {
        let old = old
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid old route"))?;
        let new = new
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid new route"))?;
        anyhow::ensure!(
            old.len() == new.len() && new.len() >= 4,
            "restore cannot change route metadata shape"
        );
        for (index, (previous, value)) in old.iter().zip(new).enumerate() {
            let coordinate = index < 3
                || ((4..7).contains(&index) && old.len() >= 7)
                || ((7..10).contains(&index) && old.len() >= 10);
            if coordinate {
                anyhow::ensure!(
                    value
                        .as_f64()
                        .is_some_and(|v| v.is_finite() && (v as f32).is_finite()),
                    "restored route vector must be finite"
                );
            } else {
                anyhow::ensure!(
                    previous == value,
                    "restore changed timestamp or opaque route metadata"
                );
            }
        }
    }
    Ok(())
}
