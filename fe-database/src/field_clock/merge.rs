//! Pure per-key merge fold (no DB); see field_clock/AGENTS.md §merge.

use super::wire::FieldVersion;
use serde_json::{Map, Value};
use std::cmp::Ordering;

/// Recursively key-sorted copy of `value` (stable whether or not serde_json preserves order).
pub(crate) fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key.clone(), canonical_value(value));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

/// Canonical value bytes; a deleted version (no value) is the empty string and sorts lowest.
pub(crate) fn canonical_bytes(version: &FieldVersion) -> Vec<u8> {
    match (&version.value, version.deleted) {
        (Some(value), false) => serde_json::to_vec(&canonical_value(value)).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Total order `(hlc, author bytes, canonical value bytes)` (spec §3.4).
pub(crate) fn version_order(left: &FieldVersion, right: &FieldVersion) -> Ordering {
    left.hlc
        .cmp(&right.hlc)
        .then_with(|| left.author.as_bytes().cmp(right.author.as_bytes()))
        .then_with(|| canonical_bytes(left).cmp(&canonical_bytes(right)))
}

/// Winner among the versions of ONE key: the maximum admissible version under [`version_order`].
pub(crate) fn winner<'a>(
    versions: &'a [FieldVersion],
    admissible: impl Fn(&FieldVersion) -> bool,
) -> Option<&'a FieldVersion> {
    versions
        .iter()
        .filter(|version| admissible(version))
        .max_by(|left, right| version_order(left, right))
}

/// Keep-newer per (key, author): `Some(incoming)` when it should replace `existing`, else `None`.
pub(crate) fn upsert_own(existing: Option<&FieldVersion>, incoming: FieldVersion) -> Option<FieldVersion> {
    match existing {
        None => Some(incoming),
        Some(existing) => {
            debug_assert!(
                existing.field == incoming.field && existing.author == incoming.author,
                "upsert_own compares one author's versions of one key"
            );
            (version_order(&incoming, existing) == Ordering::Greater).then_some(incoming)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn version(author: &str, hlc: u64, value: Option<Value>) -> FieldVersion {
        FieldVersion {
            field: "name".into(),
            author: author.into(),
            hlc,
            deleted: value.is_none(),
            value,
        }
    }

    /// Every ordering of `items` (pick-one-then-recurse).
    fn permutations(items: &[FieldVersion]) -> Vec<Vec<FieldVersion>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for index in 0..items.len() {
            let mut rest = items.to_vec();
            let first = rest.remove(index);
            for mut tail in permutations(&rest) {
                tail.insert(0, first.clone());
                out.push(tail);
            }
        }
        out
    }

    /// Arrival in any order through the per-author store, then the fold, yields one winner.
    #[test]
    fn fold_is_order_independent() {
        let alice_new = version("did:alice", 10, Some(json!("alice-10")));
        let alice_old = version("did:alice", 7, Some(json!("alice-7")));
        let bob_tie = version("did:bob", 10, Some(json!("bob-10")));
        let carol_deleted = version("did:carol", 12, None);
        let equal_low = version("did:alice", 5, Some(json!({"b": 1, "a": 2})));
        let equal_high = version("did:alice", 5, Some(json!({"a": 3})));
        let bob_low = version("did:bob", 4, Some(json!("bob-4")));
        let carol_newer = version("did:carol", 9, Some(json!("carol-9")));

        let standard = [alice_new.clone(), alice_old.clone(), bob_tie.clone(), carol_deleted.clone()];
        let byte_tie = [equal_low.clone(), equal_high.clone(), bob_low.clone(), carol_newer.clone()];
        // (versions, admitted authors, expected winner)
        let cases: [(&[FieldVersion], &[&str], Option<&FieldVersion>); 6] = [
            (&standard, &["did:alice", "did:bob", "did:carol"], Some(&carol_deleted)),
            (&standard, &["did:alice", "did:bob"], Some(&bob_tie)),
            (&standard, &["did:alice"], Some(&alice_new)),
            (&standard, &[], None),
            (&byte_tie, &["did:alice", "did:bob"], Some(&equal_high)),
            (&byte_tie, &["did:bob"], Some(&bob_low)),
        ];
        for (versions, admitted, expected) in cases {
            for arrival in permutations(versions) {
                let mut stored: BTreeMap<String, FieldVersion> = BTreeMap::new();
                for incoming in arrival {
                    let author = incoming.author.clone();
                    if let Some(kept) = upsert_own(stored.get(&author), incoming) {
                        stored.insert(author, kept);
                    }
                }
                let stored: Vec<FieldVersion> = stored.into_values().collect();
                let won = winner(&stored, |version| admitted.contains(&version.author.as_str()));
                assert_eq!(won, expected, "admitted {admitted:?}, stored {stored:?}");
            }
        }
    }
}
