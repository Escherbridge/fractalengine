//! Per-verse timeseries fabric settings surface state (M2/F6 — A13).
//!
//! The settings themselves are **per-verse** and persisted on the verse row's
//! `ts_*` columns (fe-runtime `VerseTimeseriesSettings`); the UI never invents
//! parallel state. This module is the fe-ui mirror of that truth: a
//! verse-id → settings map fed from the hierarchy load and from the
//! authoritative `DbResult::VerseTimeseriesSettingsSet` echo, plus the pure
//! helpers the Settings section's controls use. Same pattern as
//! `terrain_map::PetalMapState` (per-petal state mirrored from the DB).
//!
//! Writes go out as `DbCommand::SetVerseTimeseriesSettings`; the handler
//! sanitizes, persists, and echoes `VerseTimeseriesSettingsSet`, which the
//! `db_results` handler folds back here — so the surface always displays the
//! persisted values, never an optimistic guess.

use bevy::prelude::*;
use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings, DEFAULT_BUCKET_WIDTH_MS};
use std::collections::HashMap;

/// The largest replication factor the slider offers. R is a *request*: the
/// placement planner clamps it to the peers a shard can actually reach
/// (`fe-sync/src/placement.rs`), so this ceiling only bounds the control, not
/// the durability. Kept modest so the slider stays usable on a small fleet.
pub const MAX_REPLICATION_FACTOR: u32 = 16;

/// Per-verse timeseries settings, mirrored from the DB (verse manifest /
/// `VerseTimeseriesSettingsSet`). See the module docs.
#[derive(Resource, Debug, Default)]
pub struct TimeseriesSettingsState {
    /// verse_id → persisted settings. Absent means "not loaded yet" (the UI
    /// falls back to the mirror defaults for a verse it has never seen).
    by_verse: HashMap<String, VerseTimeseriesSettings>,
    /// Bucket-width edit buffer (ms) for the active verse, so a partially
    /// typed value is not written on every keystroke.
    bucket_width_buf: u64,
    /// The verse the buffer belongs to (a navigation change reseeds it).
    bucket_width_verse: Option<String>,
}

impl TimeseriesSettingsState {
    /// The persisted settings for a verse, defaulting to mirror (a verse whose
    /// row carries no `ts_*` columns behaves exactly like a pre-F6 verse).
    pub fn settings_for(&self, verse_id: &str) -> VerseTimeseriesSettings {
        self.by_verse.get(verse_id).copied().unwrap_or_default()
    }

    /// Record settings loaded from the DB (hierarchy load or the set echo).
    pub fn set(&mut self, verse_id: &str, settings: VerseTimeseriesSettings) {
        self.by_verse.insert(verse_id.to_string(), settings);
    }

    /// Fold a `VerseHierarchyData` timeseries payload in (hierarchy load).
    pub fn set_from_hierarchy(&mut self, verse_id: &str, settings: VerseTimeseriesSettings) {
        self.set(verse_id, settings);
    }

    /// Reseed the bucket-width edit buffer for `verse_id` when the active
    /// verse changes or the persisted value arrives. Returns the buffer to
    /// render.
    pub fn bucket_width_buffer(&mut self, verse_id: &str) -> u64 {
        if self.bucket_width_verse.as_deref() != Some(verse_id) {
            self.bucket_width_verse = Some(verse_id.to_string());
            self.bucket_width_buf = self.settings_for(verse_id).bucket_width_ms;
        }
        self.bucket_width_buf
    }

    /// Update the bucket-width buffer from a widget edit.
    pub fn set_bucket_width_buffer(&mut self, verse_id: &str, value: u64) {
        self.bucket_width_verse = Some(verse_id.to_string());
        self.bucket_width_buf = value;
    }

    /// The bucket-width value to persist (buffer, floored at 1 ms so the
    /// sanitizer never rejects it).
    pub fn bucket_width_to_commit(&self) -> u64 {
        self.bucket_width_buf.max(1)
    }
}

/// The next mode in the UI cycle (mirror → sharded → balanced → mirror).
pub fn next_mode(mode: TimeseriesMode) -> TimeseriesMode {
    match mode {
        TimeseriesMode::Mirror => TimeseriesMode::Sharded,
        TimeseriesMode::Sharded => TimeseriesMode::Balanced,
        TimeseriesMode::Balanced => TimeseriesMode::Mirror,
    }
}

/// Human label for a mode (the settings surface's combo entries).
pub fn mode_label(mode: TimeseriesMode) -> &'static str {
    match mode {
        TimeseriesMode::Mirror => "Mirror — every peer holds all shards",
        TimeseriesMode::Sharded => "Sharded — each peer hosts its portion",
        TimeseriesMode::Balanced => "Balanced — R peers per shard",
    }
}

/// Clamp a requested replication factor into the slider's `1..=MAX` range.
pub fn clamp_replication_factor(r: u32) -> u32 {
    r.clamp(1, MAX_REPLICATION_FACTOR)
}

/// Whether the R slider is meaningful in this mode (only `balanced` uses R;
/// `sharded` is R=1 by contract, `mirror` ignores it).
pub fn replication_slider_enabled(mode: TimeseriesMode) -> bool {
    matches!(mode, TimeseriesMode::Balanced)
}

/// Bucket-width presets offered beside the numeric field (ms).
pub const BUCKET_WIDTH_PRESETS: [(u64, &str); 4] = [
    (3_600_000, "1 hour"),
    (86_400_000, "1 day"),
    (604_800_000, "1 week"),
    (2_592_000_000, "30 days"),
];

/// The bucket width a fresh verse uses (mirrors the fe-runtime default).
pub const DEFAULT_BUCKET_WIDTH: u64 = DEFAULT_BUCKET_WIDTH_MS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_verse_defaults_to_mirror() {
        let state = TimeseriesSettingsState::default();
        let s = state.settings_for("v-unknown");
        assert_eq!(s.mode, TimeseriesMode::Mirror);
        assert_eq!(s.replication_factor, 1);
        assert_eq!(s.bucket_width_ms, DEFAULT_BUCKET_WIDTH_MS);
    }

    #[test]
    fn set_then_read_back_round_trips() {
        let mut state = TimeseriesSettingsState::default();
        state.set(
            "v1",
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Balanced,
                replication_factor: 3,
                bucket_width_ms: 3_600_000,
            },
        );
        let s = state.settings_for("v1");
        assert_eq!(s.mode, TimeseriesMode::Balanced);
        assert_eq!(s.replication_factor, 3);
    }

    #[test]
    fn bucket_width_buffer_reseeds_on_verse_change() {
        let mut state = TimeseriesSettingsState::default();
        state.set(
            "v1",
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Mirror,
                replication_factor: 1,
                bucket_width_ms: 3_600_000,
            },
        );
        assert_eq!(state.bucket_width_buffer("v1"), 3_600_000);
        // A widget edit sticks while the verse is unchanged.
        state.set_bucket_width_buffer("v1", 7_200_000);
        assert_eq!(state.bucket_width_buffer("v1"), 7_200_000);
        // Navigating to another verse reseeds from that verse's persisted value.
        state.set(
            "v2",
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Mirror,
                replication_factor: 1,
                bucket_width_ms: 604_800_000,
            },
        );
        assert_eq!(state.bucket_width_buffer("v2"), 604_800_000);
        // And back reseeds v1 from its persisted (not buffered) value.
        assert_eq!(state.bucket_width_buffer("v1"), 3_600_000);
    }

    #[test]
    fn bucket_width_to_commit_floors_at_one() {
        let mut state = TimeseriesSettingsState::default();
        state.set_bucket_width_buffer("v1", 0);
        assert_eq!(state.bucket_width_to_commit(), 1);
    }

    #[test]
    fn mode_cycle_visits_every_mode() {
        assert_eq!(next_mode(TimeseriesMode::Mirror), TimeseriesMode::Sharded);
        assert_eq!(next_mode(TimeseriesMode::Sharded), TimeseriesMode::Balanced);
        assert_eq!(next_mode(TimeseriesMode::Balanced), TimeseriesMode::Mirror);
    }

    #[test]
    fn replication_factor_clamps_into_slider_range() {
        assert_eq!(clamp_replication_factor(0), 1);
        assert_eq!(clamp_replication_factor(5), 5);
        assert_eq!(clamp_replication_factor(9_999), MAX_REPLICATION_FACTOR);
    }

    #[test]
    fn replication_slider_only_enabled_for_balanced() {
        assert!(!replication_slider_enabled(TimeseriesMode::Mirror));
        assert!(!replication_slider_enabled(TimeseriesMode::Sharded));
        assert!(replication_slider_enabled(TimeseriesMode::Balanced));
    }

    #[test]
    fn every_mode_has_a_label() {
        for mode in [
            TimeseriesMode::Mirror,
            TimeseriesMode::Sharded,
            TimeseriesMode::Balanced,
        ] {
            assert!(!mode_label(mode).is_empty());
        }
    }
}
