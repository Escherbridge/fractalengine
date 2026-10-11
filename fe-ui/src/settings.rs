//! Application settings surface (D-78): the single home for the residency
//! ledger's tunable knobs, persisted as RON under the platform config dir, plus
//! the session-only per-petal render-distance override. See
//! `fe-ui/src/AGENTS.md` §app-settings.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

/// Default configurable entity ceiling. Mirrors `verse_manager::MAX_PETAL_NODES`
/// (that const is the HARD backstop; this is the soft default).
const DEFAULT_ENTITY_CAP: usize = 10_000;

/// Default configurable per-petal stamp ceiling. Mirrors
/// `verse_manager::MAX_STAMPS_PER_PETAL` (the hard backstop).
const DEFAULT_STAMP_CEILING: usize = 65_536;

/// Upper clamp for `render_distance` (world units).
pub const RENDER_DISTANCE_MAX: f32 = 50_000.0;

/// Camera multiplier clamp range (sensitivity and zoom speed).
pub const CAMERA_MULTIPLIER_RANGE: std::ops::RangeInclusive<f32> = 0.1..=10.0;

/// `OrbitCameraController::default().sensitivity` — the base the multiplier scales.
pub const BASE_CAMERA_SENSITIVITY: f32 = 0.005;

/// `OrbitCameraController::default().zoom_speed` — the base the multiplier scales.
pub const BASE_CAMERA_ZOOM_SPEED: f32 = 1.0;

/// Quiet period after the last settings change before it is written to disk.
pub const SAVE_DEBOUNCE_SECS: f64 = 1.0;

/// Default relay spec (`fe_sync::relay_config::RelayConfig::parse` keyword).
const DEFAULT_RELAY_SPEC: &str = "default";

/// Where terrain tiles are sourced from (D-78). `Offline` deliberately skips the
/// online-origin disk cache; `Hybrid` prefers local then falls back to network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileSourceMode {
    Offline,
    Online,
    #[default]
    Hybrid,
}

/// User-tunable application settings (D-78). The global defaults for the
/// residency ledger; `PetalRenderOverride` carries the per-petal override.
#[derive(Resource, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Global render distance (world units); soft residency horizon. `0` halts
    /// distance-ranked spawning entirely.
    pub render_distance: f32,
    /// Live mirror of `MeshInstanceBudget.ceiling` (synced by
    /// `sync_app_settings_to_mesh_budget`). Seeded from `MAX_MESH_INSTANCES`.
    pub mesh_budget_ceiling: usize,
    /// Configurable soft cap on scene entities per petal (hard-backstopped by
    /// `MAX_PETAL_NODES` at the spawn sites).
    pub entity_cap: usize,
    /// Configurable soft cap on path-asset stamps per petal pass (hard-backstopped
    /// by `MAX_STAMPS_PER_PETAL`).
    pub stamp_ceiling: usize,
    /// Terrain tile source mode; `Offline` is a global no-network override applied
    /// by the main binary's terrain bridge, otherwise the petal's own mode governs.
    pub tile_mode: TileSourceMode,
    /// Camera orbit sensitivity multiplier (× `BASE_CAMERA_SENSITIVITY`).
    pub camera_sensitivity: f32,
    /// Camera zoom speed multiplier (× `BASE_CAMERA_ZOOM_SPEED`).
    pub camera_zoom_speed: f32,
    /// Camera focus easing factor (0 = snap, 1 = full easing). No consumer yet.
    pub camera_easing: f32,
    /// Opt-in adaptive metric world grid (hexon_scale FR-10); off by default.
    pub show_world_grid: bool,
    /// Bottom-left scale bar (hexon_scale FR-8); on by default.
    pub show_scale_bar: bool,
    /// P2P relay servers: `default` | `disabled` | comma-separated relay URLs.
    /// Read once at startup; `FE_SYNC_RELAY` overrides it.
    pub relay: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            render_distance: 500.0,
            mesh_budget_ceiling: crate::plugin::MAX_MESH_INSTANCES,
            entity_cap: DEFAULT_ENTITY_CAP,
            stamp_ceiling: DEFAULT_STAMP_CEILING,
            tile_mode: TileSourceMode::Hybrid,
            camera_sensitivity: 1.0,
            camera_zoom_speed: 1.0,
            camera_easing: 1.0,
            show_world_grid: false,
            show_scale_bar: true,
            relay: DEFAULT_RELAY_SPEC.to_string(),
        }
    }
}

/// Clamps every knob into its valid range; non-finite floats fall back to the
/// default and an unparseable relay spec falls back to `default`.
pub fn sanitize(settings: &mut AppSettings) {
    let defaults = AppSettings::default();
    settings.render_distance = if settings.render_distance.is_finite() {
        settings.render_distance.clamp(0.0, RENDER_DISTANCE_MAX)
    } else {
        defaults.render_distance
    };
    settings.mesh_budget_ceiling = settings
        .mesh_budget_ceiling
        .clamp(1, crate::plugin::MAX_MESH_INSTANCES);
    settings.entity_cap = settings
        .entity_cap
        .clamp(1, crate::verse_manager::MAX_PETAL_NODES);
    settings.stamp_ceiling = settings
        .stamp_ceiling
        .clamp(1, crate::verse_manager::MAX_STAMPS_PER_PETAL);
    settings.camera_sensitivity = clamp_multiplier(settings.camera_sensitivity);
    settings.camera_zoom_speed = clamp_multiplier(settings.camera_zoom_speed);
    settings.camera_easing = if settings.camera_easing.is_finite() {
        settings.camera_easing.clamp(0.0, 1.0)
    } else {
        defaults.camera_easing
    };
    if fe_sync::relay_config::RelayConfig::parse(&settings.relay).is_err() {
        warn!(relay = %settings.relay, "invalid relay spec in settings; using default relays");
        settings.relay = defaults.relay;
    }
}

fn clamp_multiplier(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(
            *CAMERA_MULTIPLIER_RANGE.start(),
            *CAMERA_MULTIPLIER_RANGE.end(),
        )
    } else {
        1.0
    }
}

/// `<platform config dir>/fractalengine/settings.ron`, or `None` when the
/// platform has no config dir.
pub fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("fractalengine").join("settings.ron"))
}

/// What `load_settings_from` found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file yet — defaults.
    Missing,
    /// Parsed (missing fields filled from defaults, values sanitized).
    Loaded,
    /// Unreadable or unparseable — defaults; the bad file was moved to `backup`
    /// when the move succeeded.
    Corrupt { backup: Option<PathBuf> },
}

/// Loads settings from `path`: missing → defaults, partial → defaults fill the
/// gaps, corrupt → defaults + the bad file kept as `settings.ron.corrupt-<ts>`.
pub fn load_settings_from(path: &Path) -> (AppSettings, LoadOutcome) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (AppSettings::default(), LoadOutcome::Missing);
        }
        Err(err) => {
            // Not UTF-8 is a corrupt file (kept aside so the next save cannot
            // overwrite it); any other read error leaves the file in place.
            let backup = (err.kind() == std::io::ErrorKind::InvalidData)
                .then(|| backup_corrupt_file(path))
                .flatten();
            warn!(path = %path.display(), error = %err, backup = ?backup, "settings file unreadable; using defaults");
            return (AppSettings::default(), LoadOutcome::Corrupt { backup });
        }
    };
    match ron::from_str::<AppSettings>(&text) {
        Ok(mut settings) => {
            sanitize(&mut settings);
            (settings, LoadOutcome::Loaded)
        }
        Err(err) => {
            let backup = backup_corrupt_file(path);
            warn!(
                path = %path.display(),
                error = %err,
                backup = ?backup,
                "settings file corrupt; using defaults"
            );
            (AppSettings::default(), LoadOutcome::Corrupt { backup })
        }
    }
}

fn backup_corrupt_file(path: &Path) -> Option<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".corrupt-{stamp}"));
    let backup = path.with_file_name(name);
    std::fs::rename(path, &backup).ok().map(|()| backup)
}

/// Writes `settings` to `path` atomically (temp file + rename), creating the
/// parent directory if needed.
pub fn save_settings_to(path: &Path, settings: &AppSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default())
        .map_err(std::io::Error::other)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Startup load for the GUI binary: resolves `settings_path()`, loads, and logs
/// the outcome. Returns the settings and the path to persist to (if any).
pub fn load_startup_settings() -> (AppSettings, Option<PathBuf>) {
    let Some(path) = settings_path() else {
        warn!("no platform config dir; settings are session-only");
        return (AppSettings::default(), None);
    };
    let (settings, outcome) = load_settings_from(&path);
    info!(path = %path.display(), ?outcome, "application settings loaded");
    (settings, Some(path))
}

/// Present only when settings should be written to disk (inserted by the GUI
/// binary); hosts and tests without it never touch the user's config dir.
#[derive(Resource, Debug, Clone)]
pub struct SettingsPersistence {
    pub path: PathBuf,
}

/// Trailing-edge debounce for settings saves: the first observation is the
/// loaded state; a change is saved once it has been stable for
/// `SAVE_DEBOUNCE_SECS`.
#[derive(Debug, Default)]
pub struct SettingsSaveDebounce {
    saved: Option<AppSettings>,
    seen: Option<AppSettings>,
    last_change_secs: f64,
}

impl SettingsSaveDebounce {
    /// Returns `true` when `current` should be written now.
    pub fn poll(&mut self, current: &AppSettings, now_secs: f64) -> bool {
        let Some(saved) = self.saved.as_ref() else {
            self.saved = Some(current.clone());
            self.seen = Some(current.clone());
            return false;
        };
        if self.seen.as_ref() != Some(current) {
            self.seen = Some(current.clone());
            self.last_change_secs = now_secs;
        }
        if saved != current && now_secs - self.last_change_secs >= SAVE_DEBOUNCE_SECS {
            self.saved = Some(current.clone());
            return true;
        }
        false
    }
}

/// Debounced save of `AppSettings` to `SettingsPersistence.path`.
pub fn save_app_settings_debounced(
    settings: Res<AppSettings>,
    persistence: Option<Res<SettingsPersistence>>,
    time: Res<Time>,
    mut debounce: Local<SettingsSaveDebounce>,
) {
    let Some(persistence) = persistence else {
        return;
    };
    if debounce.poll(&settings, time.elapsed_secs_f64()) {
        if let Err(err) = save_settings_to(&persistence.path, &settings) {
            warn!(path = %persistence.path.display(), error = %err, "failed to save settings");
        }
    }
}

/// Mirrors `AppSettings.mesh_budget_ceiling` into the live `MeshInstanceBudget`
/// gate so a settings change re-ceilings the watchdog without a restart. Cheap
/// (single compare) — runs every frame. See AGENTS.md §app-settings.
pub fn sync_app_settings_to_mesh_budget(
    settings: Res<AppSettings>,
    mut budget: ResMut<crate::plugin::MeshInstanceBudget>,
) {
    if budget.ceiling != settings.mesh_budget_ceiling {
        budget.ceiling = settings.mesh_budget_ceiling;
    }
}

/// Applies the camera multipliers to every `OrbitCameraController` (base × multiplier).
pub fn sync_app_settings_to_camera(
    settings: Res<AppSettings>,
    mut cameras: Query<&mut fe_renderer::camera::OrbitCameraController>,
) {
    let sensitivity = BASE_CAMERA_SENSITIVITY * clamp_multiplier(settings.camera_sensitivity);
    let zoom_speed = BASE_CAMERA_ZOOM_SPEED * clamp_multiplier(settings.camera_zoom_speed);
    for mut controller in &mut cameras {
        if controller.sensitivity != sensitivity {
            controller.sensitivity = sensitivity;
        }
        if controller.zoom_speed != zoom_speed {
            controller.zoom_speed = zoom_speed;
        }
    }
}

/// Session-only per-petal render-distance override, set by a `PetalManifest`
/// save for the active petal and cleared when the active petal changes.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct PetalRenderOverride {
    pub petal_id: Option<String>,
    pub render_distance: Option<f32>,
}

impl PetalRenderOverride {
    /// The override for `petal_id`, if one is set for exactly that petal.
    pub fn for_petal(&self, petal_id: &str) -> Option<f32> {
        match self.petal_id.as_deref() {
            Some(id) if id == petal_id => self.render_distance,
            _ => None,
        }
    }
}

/// The render distance the residency ledger uses: a finite, positive per-petal
/// override (clamped to `RENDER_DISTANCE_MAX`) wins, otherwise the global setting.
pub fn effective_render_distance(global: f32, petal_override: Option<f32>) -> f32 {
    match petal_override {
        Some(distance) if distance.is_finite() && distance > 0.0 => {
            distance.min(RENDER_DISTANCE_MAX)
        }
        _ => global,
    }
}

/// Drops the override once the petal it was set for is no longer active.
pub fn clear_petal_render_override_on_nav_change(
    nav: Res<crate::navigation_manager::NavigationManager>,
    mut render_override: ResMut<PetalRenderOverride>,
) {
    if render_override.petal_id.is_some() && render_override.petal_id != nav.active_petal_id {
        *render_override = PetalRenderOverride::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_seeds_ceilings_from_the_hard_caps() {
        let s = AppSettings::default();
        assert_eq!(s.render_distance, 500.0);
        assert_eq!(s.mesh_budget_ceiling, crate::plugin::MAX_MESH_INSTANCES);
        assert_eq!(s.entity_cap, DEFAULT_ENTITY_CAP);
        assert_eq!(s.stamp_ceiling, DEFAULT_STAMP_CEILING);
        assert_eq!(s.tile_mode, TileSourceMode::Hybrid);
        assert_eq!(s.relay, "default");
        // ui_ux §7 calm defaults: the scale bar shows, the grid is opt-in.
        assert!(s.show_scale_bar);
        assert!(!s.show_world_grid);
    }

    #[test]
    fn camera_base_consts_match_the_controller_defaults() {
        let controller = fe_renderer::camera::OrbitCameraController::default();
        assert_eq!(BASE_CAMERA_SENSITIVITY, controller.sensitivity);
        assert_eq!(BASE_CAMERA_ZOOM_SPEED, controller.zoom_speed);
    }

    #[test]
    fn saved_settings_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fractalengine").join("settings.ron");
        let edited = AppSettings {
            render_distance: 750.0,
            entity_cap: 2_000,
            stamp_ceiling: 1_024,
            tile_mode: TileSourceMode::Offline,
            camera_sensitivity: 2.5,
            camera_zoom_speed: 0.5,
            show_world_grid: true,
            relay: "disabled".into(),
            ..Default::default()
        };
        save_settings_to(&path, &edited).unwrap();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(loaded, edited);
        assert!(
            !path.with_file_name("settings.ron.tmp").exists(),
            "temp file renamed away"
        );
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (loaded, outcome) = load_settings_from(&dir.path().join("settings.ron"));
        assert_eq!(outcome, LoadOutcome::Missing);
        assert_eq!(loaded, AppSettings::default());
    }

    #[test]
    fn corrupt_file_loads_defaults_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ron");
        std::fs::write(&path, "(render_distance: oops").unwrap();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(loaded, AppSettings::default());
        let LoadOutcome::Corrupt {
            backup: Some(backup),
        } = outcome
        else {
            panic!("expected a corrupt outcome with a backup, got {outcome:?}");
        };
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "(render_distance: oops"
        );
        assert!(
            !path.exists(),
            "bad file moved aside so the next save starts clean"
        );
    }

    #[test]
    fn non_utf8_file_loads_defaults_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ron");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x28]).unwrap();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(loaded, AppSettings::default());
        let LoadOutcome::Corrupt {
            backup: Some(backup),
        } = outcome
        else {
            panic!("expected a corrupt outcome with a backup, got {outcome:?}");
        };
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            vec![0xff, 0xfe, 0x00, 0x28]
        );
        assert!(
            !path.exists(),
            "a later save must not overwrite the bad bytes"
        );
    }

    #[test]
    fn partial_file_fills_missing_fields_from_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ron");
        std::fs::write(&path, "(render_distance: 300.0, tile_mode: offline)").unwrap();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(loaded.render_distance, 300.0);
        assert_eq!(loaded.tile_mode, TileSourceMode::Offline);
        assert_eq!(loaded.entity_cap, DEFAULT_ENTITY_CAP);
        assert_eq!(loaded.relay, "default");
        assert!(loaded.show_scale_bar);
    }

    #[test]
    fn loaded_values_are_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ron");
        std::fs::write(&path, "(entity_cap: 99999999, camera_zoom_speed: 500.0)").unwrap();
        let (loaded, _) = load_settings_from(&path);
        assert_eq!(loaded.entity_cap, crate::verse_manager::MAX_PETAL_NODES);
        assert_eq!(loaded.camera_zoom_speed, 10.0);
    }

    #[test]
    fn sanitize_clamps_every_knob() {
        let mesh_cap = crate::plugin::MAX_MESH_INSTANCES;
        let node_cap = crate::verse_manager::MAX_PETAL_NODES;
        let stamp_cap = crate::verse_manager::MAX_STAMPS_PER_PETAL;
        // (input, expected) per field family.
        let cases: Vec<(AppSettings, AppSettings)> = vec![
            (
                AppSettings {
                    render_distance: f32::NAN,
                    camera_sensitivity: f32::INFINITY,
                    camera_easing: f32::NAN,
                    ..Default::default()
                },
                AppSettings::default(),
            ),
            (
                AppSettings {
                    render_distance: -5.0,
                    mesh_budget_ceiling: 0,
                    entity_cap: 0,
                    stamp_ceiling: 0,
                    camera_sensitivity: 0.0,
                    camera_zoom_speed: 0.01,
                    camera_easing: -1.0,
                    ..Default::default()
                },
                AppSettings {
                    render_distance: 0.0,
                    mesh_budget_ceiling: 1,
                    entity_cap: 1,
                    stamp_ceiling: 1,
                    camera_sensitivity: 0.1,
                    camera_zoom_speed: 0.1,
                    camera_easing: 0.0,
                    ..Default::default()
                },
            ),
            (
                AppSettings {
                    render_distance: 1e9,
                    mesh_budget_ceiling: usize::MAX,
                    entity_cap: usize::MAX,
                    stamp_ceiling: usize::MAX,
                    camera_sensitivity: 50.0,
                    camera_zoom_speed: 11.0,
                    camera_easing: 3.0,
                    ..Default::default()
                },
                AppSettings {
                    render_distance: RENDER_DISTANCE_MAX,
                    mesh_budget_ceiling: mesh_cap,
                    entity_cap: node_cap,
                    stamp_ceiling: stamp_cap,
                    camera_sensitivity: 10.0,
                    camera_zoom_speed: 10.0,
                    camera_easing: 1.0,
                    ..Default::default()
                },
            ),
            (
                AppSettings {
                    relay: "not a url".into(),
                    ..Default::default()
                },
                AppSettings::default(),
            ),
            (
                AppSettings {
                    render_distance: 1_200.0,
                    entity_cap: 500,
                    relay: "https://relay.example.com".into(),
                    ..Default::default()
                },
                AppSettings {
                    render_distance: 1_200.0,
                    entity_cap: 500,
                    relay: "https://relay.example.com".into(),
                    ..Default::default()
                },
            ),
        ];
        for (mut input, expected) in cases {
            sanitize(&mut input);
            assert_eq!(input, expected);
        }
    }

    #[test]
    fn debounce_saves_once_after_the_quiet_period() {
        let mut debounce = SettingsSaveDebounce::default();
        let loaded = AppSettings::default();
        assert!(
            !debounce.poll(&loaded, 0.0),
            "loaded state is never re-saved"
        );
        assert!(
            !debounce.poll(&loaded, 5.0),
            "unchanged settings never save"
        );

        // A slider drag: a new value every frame never saves mid-drag.
        let mut dragging = loaded.clone();
        for (frame, distance) in [(10.0, 510.0), (10.3, 520.0), (10.6, 530.0), (10.9, 540.0)] {
            dragging.render_distance = distance;
            assert!(!debounce.poll(&dragging, frame));
        }
        assert!(
            !debounce.poll(&dragging, 11.5),
            "only 0.6 s since the last change"
        );
        assert!(
            debounce.poll(&dragging, 12.0),
            "1.1 s after the last change → save"
        );
        assert!(!debounce.poll(&dragging, 20.0), "already saved");

        // Reverting to the last-saved value before the delay cancels the save.
        let mut reverted = dragging.clone();
        reverted.entity_cap = 42;
        assert!(!debounce.poll(&reverted, 30.0));
        assert!(!debounce.poll(&dragging, 30.2));
        assert!(!debounce.poll(&dragging, 40.0));
    }

    #[test]
    fn debounced_save_writes_only_with_persistence_configured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ron");
        let mut app = App::new();
        app.init_resource::<AppSettings>()
            .init_resource::<Time>()
            .add_systems(Update, save_app_settings_debounced);
        app.update();
        app.world_mut().resource_mut::<AppSettings>().entity_cap = 77;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(5));
        app.update();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(5));
        app.update();
        assert!(!path.exists(), "no SettingsPersistence → no disk write");

        app.insert_resource(SettingsPersistence { path: path.clone() });
        app.update(); // baseline: the state at startup is the saved state
        assert!(!path.exists(), "unchanged settings are not rewritten");
        app.world_mut().resource_mut::<AppSettings>().entity_cap = 88;
        app.update();
        assert!(!path.exists(), "still inside the debounce window");
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(2));
        app.update();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(loaded.entity_cap, 88);
    }

    #[test]
    fn effective_render_distance_prefers_a_valid_override() {
        let cases = [
            (500.0, None, 500.0),
            (500.0, Some(120.0), 120.0),
            (500.0, Some(0.0), 500.0),
            (500.0, Some(-3.0), 500.0),
            (500.0, Some(f32::NAN), 500.0),
            (500.0, Some(f32::INFINITY), 500.0),
            (0.0, Some(80.0), 80.0),
            (500.0, Some(1.0e9), RENDER_DISTANCE_MAX),
        ];
        for (global, petal_override, expected) in cases {
            assert_eq!(
                effective_render_distance(global, petal_override),
                expected,
                "global {global}, override {petal_override:?}"
            );
        }
    }

    #[test]
    fn camera_multipliers_scale_the_live_controller() {
        let mut app = App::new();
        app.insert_resource(AppSettings {
            camera_sensitivity: 2.0,
            camera_zoom_speed: 0.5,
            ..Default::default()
        })
        .add_systems(Update, sync_app_settings_to_camera);
        let camera = app
            .world_mut()
            .spawn(fe_renderer::camera::OrbitCameraController::default())
            .id();
        app.update();
        let controller = app
            .world()
            .get::<fe_renderer::camera::OrbitCameraController>(camera)
            .unwrap();
        assert_eq!(controller.sensitivity, BASE_CAMERA_SENSITIVITY * 2.0);
        assert_eq!(controller.zoom_speed, BASE_CAMERA_ZOOM_SPEED * 0.5);
    }
}
