//! Per-verse relay-record warehouse settings (D-CL43) — see
//! `fe-runtime/src/AGENTS.md` §timeseries for the shared-lowest-crate rationale.
//! The durability floor R is `ts_replication_factor` and is deliberately not held here (D-CL39).

/// Whether the verse packs its canonical log into sealed segments held by record relays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarehouseMode {
    /// Local dual-emit only; nothing is seeded to relays.
    #[default]
    Off,
    /// Segments are seeded to the listed record relays.
    On,
}

impl WarehouseMode {
    /// Stable lowercase label (verse-row column, UI).
    pub fn as_str(&self) -> &'static str {
        match self {
            WarehouseMode::Off => "off",
            WarehouseMode::On => "on",
        }
    }

    /// Parse the column label; `None` on anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(WarehouseMode::Off),
            "on" => Some(WarehouseMode::On),
            _ => None,
        }
    }
}

/// Whether a record relay may hold the scope key (blind relays only ever see sealed bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayKeyCustody {
    /// The relay never receives a key wrap.
    #[default]
    Blind,
    /// The relay receives the scope key through wrap delivery.
    Keyholder,
}

impl RelayKeyCustody {
    /// Stable lowercase label (verse-row column, UI).
    pub fn as_str(&self) -> &'static str {
        match self {
            RelayKeyCustody::Blind => "blind",
            RelayKeyCustody::Keyholder => "keyholder",
        }
    }

    /// Parse the column label; `None` on anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "blind" => Some(RelayKeyCustody::Blind),
            "keyholder" => Some(RelayKeyCustody::Keyholder),
            _ => None,
        }
    }
}

/// The verse's warehouse knobs (persisted on the `verse` row's `wh_*` columns, replicated with the manifest).
/// Deserializing runs the same relay-DID checks as [`VerseWarehouseSettings::sanitized`].
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "UncheckedVerseWarehouseSettings")]
pub struct VerseWarehouseSettings {
    /// Warehouse on/off.
    pub warehouse_mode: WarehouseMode,
    /// Blind or keyholder relays.
    pub relay_key_custody: RelayKeyCustody,
    /// Record-relay principal DIDs, unique, in the order given.
    pub record_relays: Vec<String>,
    /// Peers may prune sealed history once the durability floor is met.
    pub peer_prune_after_floor: bool,
}

/// Verse-row column for [`WarehouseMode`]; shared so handlers and row parsing cannot drift.
pub const COLUMN_WAREHOUSE_MODE: &str = "wh_mode";
/// Verse-row column for [`RelayKeyCustody`].
pub const COLUMN_RELAY_KEY_CUSTODY: &str = "wh_relay_key_custody";
/// Verse-row column for the record-relay DID list.
pub const COLUMN_RECORD_RELAYS: &str = "wh_record_relays";
/// Verse-row column for `peer_prune_after_floor`.
pub const COLUMN_PEER_PRUNE_AFTER_FLOOR: &str = "wh_peer_prune_after_floor";

/// True for a `did:key:z<base58btc>` principal (no `0`, `O`, `I`, `l`); the only DID form relays are listed by.
pub fn is_valid_relay_did(did: &str) -> bool {
    did.strip_prefix("did:key:z").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() && !matches!(c, '0' | 'O' | 'I' | 'l'))
    })
}

/// Refuses a relay entry that is not a did:key principal or is listed twice.
fn check_record_relays(record_relays: &[String]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for did in record_relays {
        if !is_valid_relay_did(did) {
            return Err(format!("record relay '{did}' is not a did:key principal"));
        }
        if !seen.insert(did.as_str()) {
            return Err(format!("record relay '{did}' is listed more than once"));
        }
    }
    Ok(())
}

/// Serde's view of the settings before the relay checks; missing fields take defaults.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct UncheckedVerseWarehouseSettings {
    warehouse_mode: WarehouseMode,
    relay_key_custody: RelayKeyCustody,
    record_relays: Vec<String>,
    peer_prune_after_floor: bool,
}

impl TryFrom<UncheckedVerseWarehouseSettings> for VerseWarehouseSettings {
    type Error = String;

    fn try_from(unchecked: UncheckedVerseWarehouseSettings) -> Result<Self, Self::Error> {
        check_record_relays(&unchecked.record_relays)?;
        Ok(Self {
            warehouse_mode: unchecked.warehouse_mode,
            relay_key_custody: unchecked.relay_key_custody,
            record_relays: unchecked.record_relays,
            peer_prune_after_floor: unchecked.peer_prune_after_floor,
        })
    }
}

impl VerseWarehouseSettings {
    /// Validate raw settings input (settings surface, inbound commands); refuses a bad or duplicate relay DID.
    pub fn sanitized(
        warehouse_mode: &str,
        relay_key_custody: &str,
        record_relays: &[String],
        peer_prune_after_floor: bool,
    ) -> Result<Self, String> {
        let warehouse_mode = WarehouseMode::parse(warehouse_mode)
            .ok_or_else(|| format!("unknown warehouse mode '{warehouse_mode}' (off|on)"))?;
        let relay_key_custody = RelayKeyCustody::parse(relay_key_custody).ok_or_else(|| {
            format!("unknown relay key custody '{relay_key_custody}' (blind|keyholder)")
        })?;
        check_record_relays(record_relays)?;
        Ok(Self {
            warehouse_mode,
            relay_key_custody,
            record_relays: record_relays.to_vec(),
            peer_prune_after_floor,
        })
    }

    /// Parse settings out of a `verse` row's JSON; missing or invalid fields fall back to defaults and
    /// bad or duplicate relay entries are dropped — the row is data, never a crash.
    pub fn from_verse_row(row: &serde_json::Value) -> Self {
        let warehouse_mode = row
            .get(COLUMN_WAREHOUSE_MODE)
            .and_then(|v| v.as_str())
            .and_then(WarehouseMode::parse)
            .unwrap_or_default();
        let relay_key_custody = row
            .get(COLUMN_RELAY_KEY_CUSTODY)
            .and_then(|v| v.as_str())
            .and_then(RelayKeyCustody::parse)
            .unwrap_or_default();
        let mut record_relays: Vec<String> = Vec::new();
        if let Some(items) = row.get(COLUMN_RECORD_RELAYS).and_then(|v| v.as_array()) {
            for did in items.iter().filter_map(|v| v.as_str()) {
                if is_valid_relay_did(did) && !record_relays.iter().any(|d| d == did) {
                    record_relays.push(did.to_string());
                }
            }
        }
        let peer_prune_after_floor = row
            .get(COLUMN_PEER_PRUNE_AFTER_FLOOR)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            warehouse_mode,
            relay_key_custody,
            record_relays,
            peer_prune_after_floor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn warehouse_settings_sanitize_row_parse_and_serde() {
        // (mode, custody, relays, prune, expect_ok)
        let cases: Vec<(&str, &str, Vec<String>, bool, bool)> = vec![
            ("off", "blind", dids(&[]), false, true),
            ("on", "keyholder", dids(&["did:key:z6MkA", "did:key:z6MkB"]), true, true),
            ("maybe", "blind", dids(&[]), false, false),
            ("on", "trusted", dids(&[]), false, false),
            ("on", "blind", dids(&["did:web:example.org"]), false, false),
            ("on", "blind", dids(&["did:key:z"]), false, false),
            ("on", "blind", dids(&["did:key:z6Mk0OIl"]), false, false),
            ("on", "blind", dids(&["did:key:z6MkA", "did:key:z6MkA"]), false, false),
        ];
        for (mode, custody, relays, prune, expect_ok) in cases {
            let result = VerseWarehouseSettings::sanitized(mode, custody, &relays, prune);
            assert_eq!(result.is_ok(), expect_ok, "{mode}/{custody}/{relays:?}: {result:?}");
            if let Ok(settings) = result {
                assert_eq!(settings.record_relays, relays);
                assert_eq!(settings.peer_prune_after_floor, prune);
                let back: VerseWarehouseSettings =
                    serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
                assert_eq!(back, settings);
            }
        }

        // A pre-warehouse verse row is off / blind / no relays.
        assert_eq!(
            VerseWarehouseSettings::from_verse_row(&json!({"verse_id": "v"})),
            VerseWarehouseSettings::default()
        );
        // Invalid values clamp; bad and duplicate relays are dropped, order kept.
        let parsed = VerseWarehouseSettings::from_verse_row(&json!({
            "wh_mode": "on",
            "wh_relay_key_custody": "keyholder",
            "wh_record_relays": ["did:key:z6MkA", "nope", "did:key:z6MkA", "did:key:z6MkB", 7],
            "wh_peer_prune_after_floor": true
        }));
        assert_eq!(parsed.warehouse_mode, WarehouseMode::On);
        assert_eq!(parsed.relay_key_custody, RelayKeyCustody::Keyholder);
        assert_eq!(parsed.record_relays, dids(&["did:key:z6MkA", "did:key:z6MkB"]));
        assert!(parsed.peer_prune_after_floor);
        let clamped = VerseWarehouseSettings::from_verse_row(&json!({
            "wh_mode": "banana", "wh_relay_key_custody": 3, "wh_record_relays": "x"
        }));
        assert_eq!(clamped, VerseWarehouseSettings::default());
        // A partial serde document fills defaults.
        let partial: VerseWarehouseSettings =
            serde_json::from_value(json!({"warehouse_mode": "on"})).unwrap();
        assert_eq!(partial.warehouse_mode, WarehouseMode::On);
        assert_eq!(partial.relay_key_custody, RelayKeyCustody::Blind);
        // Deserializing is not a side door past the relay checks.
        for bad in [json!(["did:web:example.org"]), json!(["did:key:z6MkA", "did:key:z6MkA"])] {
            assert!(
                serde_json::from_value::<VerseWarehouseSettings>(json!({"record_relays": bad}))
                    .is_err()
            );
        }
    }
}
