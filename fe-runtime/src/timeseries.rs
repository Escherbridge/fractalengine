//! Per-verse timeseries replication settings (M2/F6) — see
//! `fe-runtime/src/AGENTS.md` §timeseries.
//!
//! The canonical enum lives here because every layer needs it and fe-runtime
//! is the lowest common dependency: the DB thread persists and re-emits the
//! verse row carrying these fields, fe-sync's fabric/placement/retention
//! dispatch on them, and fe-ui renders them on the settings surface. The same
//! split was used for `RoleLevel` (fe-policy) and `ReplicationMode`
//! (fe-database) — one definition, no drift, no dependency cycle.

/// Default shard bucket width: 1 day, aligned to the epoch.
pub const DEFAULT_BUCKET_WIDTH_MS: u64 = 86_400_000;

/// How a verse's `iot_reading` shards are placed across peers (D2: hybrid,
/// switchable, with a durability slider).
///
/// `mirror` preserves the pre-F6 behavior exactly (every peer holds every
/// shard), so a fresh verse is byte-for-byte compatible with F5-era peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeseriesMode {
    /// Every peer holds all shards (full replication).
    #[default]
    Mirror,
    /// Each peer hosts its declared portion only — one host per shard,
    /// capacity-aware.
    Sharded,
    /// Each shard lives on `replication_factor` peers (1..N), capacity-aware.
    Balanced,
}

impl TimeseriesMode {
    /// Stable lowercase label (verse-row column, ledger rows, UI).
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeseriesMode::Mirror => "mirror",
            TimeseriesMode::Sharded => "sharded",
            TimeseriesMode::Balanced => "balanced",
        }
    }

    /// Parse the verse-row column label; `None` on anything else so callers
    /// can reject (handler) or default (row parse) by context.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mirror" => Some(TimeseriesMode::Mirror),
            "sharded" => Some(TimeseriesMode::Sharded),
            "balanced" => Some(TimeseriesMode::Balanced),
            _ => None,
        }
    }
}

/// The verse's tunable timeseries-fabric parameters (persisted on the `verse`
/// row's `ts_*` columns, replicated with the verse manifest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VerseTimeseriesSettings {
    /// Placement mode (D2 #1).
    pub mode: TimeseriesMode,
    /// Durability slider R (D2 #2): how many peers host each shard in
    /// `balanced` mode. Clamped at placement time to the peers a shard can
    /// actually reach — R is a request, never a hard requirement.
    pub replication_factor: u32,
    /// Shard bucket width in milliseconds, epoch-aligned: a reading maps to
    /// exactly one shard by `(anchor, recorded_at_ms)` integer-divided by
    /// this width.
    pub bucket_width_ms: u64,
}

impl Default for VerseTimeseriesSettings {
    fn default() -> Self {
        Self {
            mode: TimeseriesMode::Mirror,
            replication_factor: 1,
            bucket_width_ms: DEFAULT_BUCKET_WIDTH_MS,
        }
    }
}

impl VerseTimeseriesSettings {
    /// Validate raw settings input (settings surface, inbound rows).
    ///
    /// Returns the sanitized settings or a human-readable reason — the
    /// caller decides whether to reject (DB handler: error) or clamp (row
    /// parse: fall back to defaults).
    pub fn sanitized(
        mode: &str,
        replication_factor: u32,
        bucket_width_ms: u64,
    ) -> Result<Self, String> {
        let mode = TimeseriesMode::parse(mode)
            .ok_or_else(|| format!("unknown timeseries mode '{mode}' (mirror|sharded|balanced)"))?;
        if replication_factor == 0 {
            return Err("replication_factor must be >= 1".to_string());
        }
        if bucket_width_ms == 0 {
            return Err("bucket_width_ms must be > 0".to_string());
        }
        Ok(Self {
            mode,
            replication_factor,
            bucket_width_ms,
        })
    }

    /// Parse the settings out of a `verse` row's JSON (any shape a peer or an
    /// older version may have written). Missing fields fall back to defaults
    /// — a pre-F6 verse row carries no `ts_*` columns and must keep behaving
    /// exactly like a mirror verse. Invalid values clamp the same way: the
    /// row is data, not a command, so it must never crash the sync plane.
    pub fn from_verse_row(row: &serde_json::Value) -> Self {
        let fallback = Self::default();
        let mode = row
            .get("ts_mode")
            .and_then(|v| v.as_str())
            .and_then(TimeseriesMode::parse)
            .unwrap_or(fallback.mode);
        let replication_factor = row
            .get("ts_replication_factor")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as u32)
            .unwrap_or(fallback.replication_factor);
        let bucket_width_ms = row
            .get("ts_bucket_width_ms")
            .and_then(|v| v.as_u64())
            .filter(|v| *v > 0)
            .unwrap_or(fallback.bucket_width_ms);
        Self {
            mode,
            replication_factor,
            bucket_width_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_mirror_r1_one_day() {
        let s = VerseTimeseriesSettings::default();
        assert_eq!(s.mode, TimeseriesMode::Mirror);
        assert_eq!(s.replication_factor, 1);
        assert_eq!(s.bucket_width_ms, DEFAULT_BUCKET_WIDTH_MS);
    }

    #[test]
    fn mode_labels_round_trip() {
        for mode in [
            TimeseriesMode::Mirror,
            TimeseriesMode::Sharded,
            TimeseriesMode::Balanced,
        ] {
            assert_eq!(TimeseriesMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(TimeseriesMode::parse("MIRROR"), None);
        assert_eq!(TimeseriesMode::parse(""), None);
    }

    #[test]
    fn sanitized_accepts_every_valid_combination() {
        for mode in ["mirror", "sharded", "balanced"] {
            let s = VerseTimeseriesSettings::sanitized(mode, 3, 3_600_000)
                .unwrap_or_else(|e| panic!("{mode} must sanitize: {e}"));
            assert_eq!(s.replication_factor, 3);
            assert_eq!(s.bucket_width_ms, 3_600_000);
        }
    }

    #[test]
    fn sanitized_rejects_bad_input() {
        assert!(VerseTimeseriesSettings::sanitized("replicated", 1, 1).is_err());
        assert!(VerseTimeseriesSettings::sanitized("mirror", 0, 1).is_err());
        assert!(VerseTimeseriesSettings::sanitized("mirror", 1, 0).is_err());
    }

    #[test]
    fn pre_f6_verse_row_parses_as_pure_mirror_defaults() {
        let s = VerseTimeseriesSettings::from_verse_row(&json!({
            "verse_id": "v", "name": "n", "default_access": "viewer"
        }));
        assert_eq!(s, VerseTimeseriesSettings::default());
    }

    #[test]
    fn verse_row_with_ts_fields_parses() {
        let s = VerseTimeseriesSettings::from_verse_row(&json!({
            "verse_id": "v",
            "ts_mode": "balanced",
            "ts_replication_factor": 2,
            "ts_bucket_width_ms": 3_600_000
        }));
        assert_eq!(s.mode, TimeseriesMode::Balanced);
        assert_eq!(s.replication_factor, 2);
        assert_eq!(s.bucket_width_ms, 3_600_000);
    }

    #[test]
    fn verse_row_with_invalid_ts_fields_clamps_to_defaults() {
        let s = VerseTimeseriesSettings::from_verse_row(&json!({
            "ts_mode": "banana",
            "ts_replication_factor": 0,
            "ts_bucket_width_ms": 0
        }));
        assert_eq!(s, VerseTimeseriesSettings::default());
    }

    #[test]
    fn settings_serde_round_trips() {
        let s = VerseTimeseriesSettings {
            mode: TimeseriesMode::Balanced,
            replication_factor: 4,
            bucket_width_ms: 7_200_000,
        };
        let json = serde_json::to_value(s).expect("serialize");
        assert_eq!(json["mode"], serde_json::json!("balanced"));
        let back: VerseTimeseriesSettings = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn partial_settings_doc_fills_defaults() {
        // `#[serde(default)]`: a ledger/dump document written by an older
        // version must still load.
        let back: VerseTimeseriesSettings =
            serde_json::from_value(json!({"mode": "sharded"})).expect("deserialize");
        assert_eq!(back.mode, TimeseriesMode::Sharded);
        assert_eq!(back.replication_factor, 1);
        assert_eq!(back.bucket_width_ms, DEFAULT_BUCKET_WIDTH_MS);
    }
}
