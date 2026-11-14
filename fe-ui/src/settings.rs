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

/// Stable vector symbol for a local hierarchy display preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeIcon {
    Circle,
    Diamond,
    Square,
    Triangle,
    Star,
    Hexagon,
    Cross,
    Ring,
}

/// Per-entry local icon and RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntryAppearance {
    pub icon: TreeIcon,
    pub color: [u8; 3],
}

/// User-tunable application settings (D-78). The global defaults for the
/// residency ledger; `PetalRenderOverride` carries the per-petal override.
#[derive(Resource, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Local display preferences keyed by hierarchy kind and stable entry id.
    pub tree_appearance: std::collections::BTreeMap<String, EntryAppearance>,
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
    /// Base URL of a hosted `fe-hexon-registry` to install maps from (PAS-P5
    /// S7b, A6). `None` means no registry is configured — the Hexon Manager's
    /// "Install from registry" control is disabled rather than faked
    /// (ui_ux.md §6). `FE_HEXON_REGISTRY_URL` overrides it; see
    /// `resolve_hexon_registry_url`.
    pub hexon_registry_url: Option<String>,
    /// OD-13: a covered advertised relay may become home automatically (local only); read at startup.
    pub auto_home_relay: bool,
    /// Answer dial-back reachability checks for verse peers (local only); read at startup.
    pub act_as_prober: bool,
    /// Serve a members-only relay for my verses; needs the `relay-server` build and a passing self-test; read at startup.
    pub serve_relay: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            tree_appearance: Default::default(),
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
            hexon_registry_url: None,
            auto_home_relay: true,
            act_as_prober: false,
            serve_relay: false,
        }
    }
}

/// Whether this binary was built with the relay server; registered once at startup by the host binary.
static RELAY_SERVE_BUILT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Latest reachability self-test verdict label (`public`, `behind_nat`, ...); `None` while pending.
static LAST_SELF_TEST_VERDICT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Verses (id, display name) this device would serve if relay serving is switched on.
static RELAY_SERVABLE_VERSES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Host binary registers whether relay serving is compiled in.
pub fn set_relay_serve_built(built: bool) {
    RELAY_SERVE_BUILT.store(built, std::sync::atomic::Ordering::Relaxed);
}

/// True when this binary can serve a relay.
pub fn relay_serve_built() -> bool {
    RELAY_SERVE_BUILT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Host binary records the newest self-test verdict label for the Settings reason line.
pub fn record_self_test_verdict(label: Option<String>) {
    match LAST_SELF_TEST_VERDICT.lock() {
        Ok(mut guard) => *guard = label,
        Err(poisoned) => *poisoned.into_inner() = label,
    }
}

/// Host binary records the verses the consent step lists.
pub fn record_relay_servable_verses(verses: Vec<(String, String)>) {
    match RELAY_SERVABLE_VERSES.lock() {
        Ok(mut guard) => *guard = verses,
        Err(poisoned) => *poisoned.into_inner() = verses,
    }
}

/// Verses (id, display name) the consent step lists.
pub fn relay_servable_verses() -> Vec<(String, String)> {
    match RELAY_SERVABLE_VERSES.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// One consent line per verse (name, id when the name is blank), capped with a trailing "+N more".
pub fn consent_verse_lines(verses: &[(String, String)], cap: usize) -> Vec<String> {
    let mut lines: Vec<String> = verses
        .iter()
        .take(cap)
        .map(|(id, name)| if name.trim().is_empty() { id.clone() } else { name.clone() })
        .collect();
    if verses.len() > cap {
        lines.push(format!("+{} more", verses.len() - cap));
    }
    lines
}

/// Seconds between self-test verdict requests while no verdict is recorded.
const SELF_TEST_REQUEST_INTERVAL_SECS: f32 = 10.0;

/// Bevy system: asks the sync thread for diagnostics every 10 s until a self-test verdict is recorded.
pub fn request_self_test_verdict(
    sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    time: Res<Time>,
    mut last_request_secs: Local<f32>,
) {
    let Some(sender) = sender else { return };
    if last_self_test_verdict().is_some() {
        return;
    }
    let now = time.elapsed_secs();
    if *last_request_secs != 0.0 && now - *last_request_secs < SELF_TEST_REQUEST_INTERVAL_SECS {
        return;
    }
    *last_request_secs = now.max(f32::MIN_POSITIVE);
    // The diagnostics state resource carries the result, so the direct reply is dropped.
    let (reply, _unused_receiver) = crossbeam::channel::bounded(1);
    if sender
        .0
        .send(fe_sync::SyncCommand::GetNetworkDiagnostics { detail: false, reply })
        .is_err()
    {
        warn!("Sync channel closed; GetNetworkDiagnostics not sent");
    }
}

/// Bevy system: copies a changed diagnostics snapshot's self-test verdict into the Settings reason line.
pub fn mirror_self_test_verdict(diagnostics: Option<Res<fe_sync::status::NetworkDiagnosticsState>>) {
    let Some(diagnostics) = diagnostics else { return };
    if !diagnostics.is_changed() {
        return;
    }
    let Some(snapshot) = diagnostics.0.as_ref() else { return };
    let verdict = snapshot.node.reachability_verdict.as_str();
    if verdict != "pending" && last_self_test_verdict().as_deref() != Some(verdict) {
        record_self_test_verdict(Some(verdict.to_owned()));
    }
}

/// Latest recorded self-test verdict label.
pub fn last_self_test_verdict() -> Option<String> {
    match LAST_SELF_TEST_VERDICT.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Why the relay toggle cannot be switched on, or `None` when it may be (the consent step still applies).
pub fn relay_serve_blocker(built: bool, verdict: Option<&str>) -> Option<&'static str> {
    if !built {
        return Some("This build cannot serve a relay.");
    }
    match verdict {
        Some("public") => None,
        Some(_) => Some("Reachability check says this device is not publicly reachable."),
        None => Some("Waiting for the reachability check."),
    }
}

/// Environment override for [`AppSettings::hexon_registry_url`] (PAS-P5 S7b, A6).
pub const HEXON_REGISTRY_URL_ENV_VAR: &str = "FE_HEXON_REGISTRY_URL";

/// Registry-URL precedence, mirroring `fe_sync::relay_config::resolve`: the
/// environment override wins over the persisted setting; absent both →
/// `None` (no registry configured). A blank/whitespace-only value at either
/// source is treated as absent.
pub fn resolve_hexon_registry_url(env: Option<&str>, settings: Option<&str>) -> Option<String> {
    let non_blank = |raw: &str| {
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    env.and_then(non_blank)
        .or_else(|| settings.and_then(non_blank))
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
    // A blank/whitespace-only URL is the same as "not configured" — keeps the
    // Installed/Available UI's "is a registry configured?" check a plain
    // `is_some()` rather than also checking for blankness everywhere it reads this.
    settings.hexon_registry_url = settings
        .hexon_registry_url
        .take()
        .and_then(|url| (!url.trim().is_empty()).then(|| url.trim().to_string()));
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
    let qa_root = match fe_identity::file_fallback::qa_storage_root() {
        Ok(root) => root,
        Err(error) => {
            warn!(%error, "invalid QA storage root; settings persistence disabled");
            return None;
        }
    };
    resolve_settings_path(qa_root.as_deref(), dirs::config_dir().as_deref())
}

fn resolve_settings_path(qa_root: Option<&Path>, config_root: Option<&Path>) -> Option<PathBuf> {
    qa_root
        .map(|root| root.join("config").join("settings.ron"))
        .or_else(|| config_root.map(|root| root.join("fractalengine").join("settings.ron")))
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

/// Disk-save receipts and the startup baseline for local Settings UI.
#[derive(Resource, Debug)]
pub struct SettingsSession {
    startup_relay: Option<String>,
    persisted: Option<AppSettings>,
    pub persistence_available: bool,
    pub saved_at: Option<f64>,
    pub error: Option<String>,
    pub retry_requested: bool,
    pub(crate) reset: DatabaseResetState,
    pub(crate) reset_confirmation: String,
    pub(crate) reset_confirmation_scope: Option<u64>,
    pub(crate) reset_confirmation_revision: u64,
    reset_permit: fe_database::admin::DevelopmentResetPermit,
    reset_host: Option<fe_runtime::maintenance::ResetRequestSender>,
}

#[derive(Debug, Default)]
pub(crate) enum DatabaseResetState {
    #[default]
    Ready,
    Awaiting { request_id: String, sent_at: f64 },
    Unconfirmed { request_id: String },
    Finished { cleared: bool, error: Option<String> },
}

impl Default for SettingsSession {
    fn default() -> Self {
        Self {
            startup_relay: None, persisted: None, persistence_available: false,
            saved_at: None, error: None, retry_requested: false,
            reset: DatabaseResetState::Ready, reset_confirmation: String::new(),
            reset_confirmation_scope: None, reset_confirmation_revision: 0,
            reset_permit: fe_database::admin::DevelopmentResetPermit::capture(),
            reset_host: None,
        }
    }
}

impl SettingsSession {
    pub fn developer_tools_enabled(&self) -> bool { self.reset_permit.is_enabled() }

    pub fn terminal_reset_available(&self) -> bool { self.reset_host.is_some() }

    pub fn install_reset_host(&mut self, sender: fe_runtime::maintenance::ResetRequestSender) {
        self.reset_host = Some(sender);
    }

    pub(crate) fn request_terminal_reset(&self, request_id: String, confirmation: String) -> Result<(), &'static str> {
        if !self.developer_tools_enabled() || confirmation != "RESET" { return Err("Reset confirmation is unavailable"); }
        self.reset_host.as_ref().ok_or("Terminal reset host is unavailable")?.0
            .try_send(fe_runtime::maintenance::ResetRequest { request_id, confirmation })
            .map_err(|_| "Terminal reset request was not admitted")
    }

    pub fn needs_restart(&self, settings: &AppSettings) -> bool {
        self.startup_relay.as_ref().is_some_and(|relay| relay != &settings.relay)
    }

    pub fn is_saved(&self, settings: &AppSettings) -> bool {
        self.persistence_available && self.persisted.as_ref() == Some(settings)
    }
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
    mut session: ResMut<SettingsSession>,
) {
    if session.startup_relay.is_none() {
        session.startup_relay = Some(settings.relay.clone());
        if persistence.is_some() {
            session.persisted = Some(settings.clone());
        }
    }
    session.persistence_available = persistence.is_some();
    let Some(persistence) = persistence else { return; };
    let now = time.elapsed_secs_f64();
    let retry = std::mem::take(&mut session.retry_requested);
    if debounce.poll(&settings, now) || retry {
        match save_settings_to(&persistence.path, &settings) {
            Ok(()) => {
                session.persisted = Some(settings.clone());
                session.saved_at = Some(now);
                session.error = None;
            }
            Err(err) => {
                warn!(path = %persistence.path.display(), error = %err, "failed to save settings");
                session.error = Some(format!("Settings not saved: {err}"));
            }
        }
    }
}

/// Consume only this session's exact reset reply and retain unknown outcomes.
pub fn observe_development_reset(
    mut results: MessageReader<fe_runtime::messages::DbResult>,
    mut session: ResMut<SettingsSession>,
    time: Res<crate::notice::NoticeClock>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut nav: ResMut<crate::navigation_manager::NavigationManager>,
    mut node_mgr: ResMut<crate::node_manager::NodeManager>,
) {
    for result in results.read() {
        if matches!(result, fe_runtime::messages::DbResult::DatabaseReset { .. }) {
            nav.back_from_verse();
            node_mgr.deselect();
        }
        let fe_runtime::messages::DbResult::DevelopmentDatabaseReset { request_id, cleared, error } = result else { continue; };
        let pending_id = match &session.reset {
            DatabaseResetState::Awaiting { request_id, .. } | DatabaseResetState::Unconfirmed { request_id } => Some(request_id),
            _ => None,
        };
        if pending_id != Some(request_id) { continue; }
        session.reset_confirmation.clear();
        session.reset = DatabaseResetState::Finished { cleared: *cleared, error: error.clone() };
        if let Some(error) = error { ui_mgr.show_error(error.clone(), time.elapsed_secs_f64()); }
        else if *cleared { ui_mgr.show_toast("Database reset", time.elapsed_secs_f64()); }
    }
    if let DatabaseResetState::Awaiting { request_id, sent_at } = &session.reset {
        if time.elapsed_secs_f64() - sent_at >= 30.0 {
            session.reset = DatabaseResetState::Unconfirmed { request_id: request_id.clone() };
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

/// The render distance the residency ledger uses, in priority order (p2p_asset_streaming
/// S6, campaign plan §3 step 6): the session override (`PetalRenderOverride`, a
/// local-only knob for viewers on weak machines) beats the petal's durable
/// `render_distance` column, which beats the global `AppSettings.render_distance`.
/// Each candidate is skipped unless finite and positive; the winner is clamped
/// to `RENDER_DISTANCE_MAX` (the ceiling also binds the petal column —
/// `fe-ui/src/AGENTS.md:716-718`).
pub fn effective_render_distance(
    global: f32,
    petal_column: Option<f32>,
    session_override: Option<f32>,
) -> f32 {
    let valid = |d: Option<f32>| d.filter(|v| v.is_finite() && *v > 0.0);
    valid(session_override)
        .or_else(|| valid(petal_column))
        .map(|v| v.min(RENDER_DISTANCE_MAX))
        .unwrap_or(global)
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
        let normal = dir.path().join("user-config");
        let isolated = dir.path().join("qa-run");
        let path = resolve_settings_path(Some(&isolated), Some(&normal)).unwrap();
        assert_eq!(path, isolated.join("config/settings.ron"));
        let edited = AppSettings {
            tree_appearance: std::collections::BTreeMap::from([
                (
                    "verse:shared-id".into(),
                    EntryAppearance {
                        icon: TreeIcon::Star,
                        color: [0, 255, 128],
                    },
                ),
                (
                    "node:shared-id".into(),
                    EntryAppearance {
                        icon: TreeIcon::Triangle,
                        color: [255, 0, 64],
                    },
                ),
            ]),
            render_distance: 750.0,
            entity_cap: 2_000,
            stamp_ceiling: 1_024,
            tile_mode: TileSourceMode::Offline,
            camera_sensitivity: 2.5,
            camera_zoom_speed: 0.5,
            show_world_grid: true,
            relay: "disabled".into(),
            hexon_registry_url: Some("https://hexons.example.com".into()),
            auto_home_relay: false,
            act_as_prober: true,
            serve_relay: true,
            ..Default::default()
        };
        save_settings_to(&path, &edited).unwrap();
        let (loaded, outcome) = load_settings_from(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(loaded, edited);
        assert!(
            !normal.exists(),
            "QA settings must not touch the user config tree"
        );
        assert_eq!(
            resolve_settings_path(None, Some(&normal)),
            Some(normal.join("fractalengine/settings.ron"))
        );
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
        assert!(loaded.auto_home_relay);
        assert!(!loaded.act_as_prober);
        assert!(!loaded.serve_relay);
        assert!(relay_serve_blocker(false, Some("public")).is_some());
        assert!(relay_serve_blocker(true, Some("behind_nat")).is_some());
        assert!(relay_serve_blocker(true, None).is_some());
        assert!(relay_serve_blocker(true, Some("public")).is_none());

        // (verses, cap, expected consent lines)
        let verse = |id: &str, name: &str| (id.to_owned(), name.to_owned());
        let table = [
            (vec![], 8, vec![]),
            (vec![verse("v1", "Alpha"), verse("v2", "")], 8, vec!["Alpha".to_owned(), "v2".to_owned()]),
            (
                vec![verse("v1", "A"), verse("v2", "B"), verse("v3", "C")],
                2,
                vec!["A".to_owned(), "B".to_owned(), "+1 more".to_owned()],
            ),
        ];
        for (verses, cap, expected) in table {
            assert_eq!(consent_verse_lines(&verses, cap), expected);
        }
        record_relay_servable_verses(vec![verse("v9", "Nine")]);
        assert_eq!(relay_servable_verses(), vec![verse("v9", "Nine")]);
        record_relay_servable_verses(Vec::new());
        assert!(relay_servable_verses().is_empty());
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
    fn hexon_registry_url_resolve_precedence_table() {
        // (env, settings, expected) — mirrors fe_sync::relay_config's table shape.
        let cases: [(Option<&str>, Option<&str>, Option<&str>); 6] = [
            (None, None, None),
            (
                None,
                Some("https://settings.example.com"),
                Some("https://settings.example.com"),
            ),
            (
                Some("https://env.example.com"),
                Some("https://settings.example.com"),
                Some("https://env.example.com"),
            ),
            (
                Some("https://env.example.com"),
                None,
                Some("https://env.example.com"),
            ),
            (
                Some("   "),
                Some("https://settings.example.com"),
                Some("https://settings.example.com"),
            ),
            (None, Some("   "), None),
        ];
        for (env, settings, expected) in cases {
            let resolved = resolve_hexon_registry_url(env, settings);
            assert_eq!(
                resolved.as_deref(),
                expected,
                "env={env:?} settings={settings:?}"
            );
        }
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
            // PAS-P5 S7b: blank/whitespace-only is "not configured", a real
            // URL is kept trimmed.
            (
                AppSettings {
                    hexon_registry_url: Some("   ".into()),
                    ..Default::default()
                },
                AppSettings::default(),
            ),
            (
                AppSettings {
                    hexon_registry_url: Some("  https://hexons.example.com  ".into()),
                    ..Default::default()
                },
                AppSettings {
                    hexon_registry_url: Some("https://hexons.example.com".into()),
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
            .init_resource::<SettingsSession>()
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
        assert!(app.world().resource::<SettingsSession>().is_saved(app.world().resource::<AppSettings>()));
        assert!(app.world().resource::<SettingsSession>().saved_at.is_some());
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, "fixture").unwrap();
        app.world_mut().resource_mut::<SettingsPersistence>().path = blocked.join("settings.ron");
        app.world_mut().resource_mut::<AppSettings>().entity_cap = 99;
        app.update();
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs(2));
        app.update();
        assert!(app.world().resource::<SettingsSession>().error.is_some());
        assert!(!app.world().resource::<SettingsSession>().is_saved(app.world().resource::<AppSettings>()));
        app.world_mut().resource_mut::<SettingsPersistence>().path = path.clone();
        app.world_mut().resource_mut::<SettingsSession>().retry_requested = true;
        app.update();
        assert!(app.world().resource::<SettingsSession>().error.is_none());
        assert_eq!(load_settings_from(&path).0.entity_cap, 99);
        app.world_mut().resource_mut::<AppSettings>().relay = "disabled".into();
        assert!(app.world().resource::<SettingsSession>().needs_restart(app.world().resource::<AppSettings>()));

        app.add_message::<fe_runtime::messages::DbResult>()
            .init_resource::<crate::actions::UiManager>()
            .init_resource::<crate::navigation_manager::NavigationManager>()
            .init_resource::<crate::node_manager::NodeManager>()
            .init_resource::<crate::notice::NoticeClock>()
            .add_systems(Update, observe_development_reset);
        app.world_mut().resource_mut::<SettingsSession>().reset = DatabaseResetState::Awaiting { request_id: "owned-reset".into(), sent_at: 0.0 };
        app.world_mut().write_message(fe_runtime::messages::DbResult::DevelopmentDatabaseReset { request_id: "other-reset".into(), cleared: true, error: None });
        app.update();
        assert!(matches!(app.world().resource::<SettingsSession>().reset, DatabaseResetState::Awaiting { .. }));
        app.world_mut().resource_mut::<crate::notice::NoticeClock>().advance_by(std::time::Duration::from_secs(31));
        app.update();
        assert!(matches!(app.world().resource::<SettingsSession>().reset, DatabaseResetState::Unconfirmed { .. }));
        app.world_mut().write_message(fe_runtime::messages::DbResult::DevelopmentDatabaseReset { request_id: "owned-reset".into(), cleared: true, error: Some("Local database cleared, default content unavailable".into()) });
        app.update();
        assert!(matches!(app.world().resource::<SettingsSession>().reset, DatabaseResetState::Finished { cleared: true, error: Some(_) }));

        let (reset_tx, reset_rx) = crossbeam::channel::bounded(1);
        assert!(!app.world().resource::<SettingsSession>().terminal_reset_available());
        app.world_mut().resource_mut::<SettingsSession>().install_reset_host(fe_runtime::maintenance::ResetRequestSender(reset_tx));
        assert!(app.world().resource::<SettingsSession>().terminal_reset_available());
        let request_id = format!("ui-reset-{}", ulid::Ulid::new());
        assert!(app.world().resource::<SettingsSession>().request_terminal_reset(request_id.clone(), "reset".into()).is_err());
        assert!(reset_rx.try_recv().is_err());
        app.world().resource::<SettingsSession>().request_terminal_reset(request_id.clone(), "RESET".into()).unwrap();
        assert!(app.world().resource::<SettingsSession>().request_terminal_reset("second".into(), "RESET".into()).is_err(), "one bounded terminal request only");
        let admitted = reset_rx.try_recv().unwrap();
        assert_eq!(admitted.request_id, request_id);
        assert_eq!(admitted.confirmation, "RESET");
        app.init_resource::<crate::verse_manager::VerseManager>()
            .init_resource::<crate::plugin::InspectorFormState>();
        app.world_mut().resource_mut::<crate::navigation_manager::NavigationManager>().navigate_to_verse("old", "Old");
        app.world_mut().resource_mut::<crate::verse_manager::VerseManager>().verses.push(crate::verse_manager::VerseEntry { id: "old".into(), name: "Old".into(), ..Default::default() });
        {
            let mut inspector = app.world_mut().resource_mut::<crate::plugin::InspectorFormState>();
            inspector.generated_api_token = Some("old-secret-sentinel".into());
            inspector.query_sql_buf = "SELECT old FROM node".into();
            inspector.node_properties = serde_json::json!({ "old": true });
        }
        crate::maintenance::clear_read_models(app.world_mut());
        assert!(app.world().resource::<crate::navigation_manager::NavigationManager>().active_verse_id.is_none());
        assert!(app.world().resource::<crate::verse_manager::VerseManager>().verses.is_empty());
        let inspector = app.world().resource::<crate::plugin::InspectorFormState>();
        assert!(inspector.generated_api_token.is_none());
        assert!(inspector.query_sql_buf.is_empty());
        assert_eq!(inspector.node_properties, crate::plugin::InspectorFormState::default().node_properties);
    }

    #[test]
    fn effective_render_distance_prefers_session_over_petal_over_global() {
        // (global, petal_column, session_override, expected)
        let cases = [
            // No petal column, no session override: global wins.
            (500.0, None, None, 500.0),
            // Petal column alone beats global.
            (500.0, Some(200.0), None, 200.0),
            // Session override beats BOTH petal column and global.
            (500.0, Some(200.0), Some(50.0), 50.0),
            (500.0, None, Some(50.0), 50.0),
            // Invalid (non-finite/non-positive) petal column is skipped,
            // falling through to global.
            (500.0, Some(0.0), None, 500.0),
            (500.0, Some(-3.0), None, 500.0),
            (500.0, Some(f32::NAN), None, 500.0),
            (500.0, Some(f32::INFINITY), None, 500.0),
            // Invalid session override is skipped, falling through to a
            // valid petal column, then global.
            (500.0, Some(200.0), Some(f32::NAN), 200.0),
            (500.0, None, Some(-1.0), 500.0),
            // A valid petal column with an empty global.
            (0.0, Some(80.0), None, 80.0),
            // The winner is clamped to RENDER_DISTANCE_MAX, whichever source.
            (500.0, Some(1.0e9), None, RENDER_DISTANCE_MAX),
            (500.0, None, Some(1.0e9), RENDER_DISTANCE_MAX),
        ];
        for (global, petal_column, session_override, expected) in cases {
            assert_eq!(
                effective_render_distance(global, petal_column, session_override),
                expected,
                "global {global}, petal_column {petal_column:?}, session_override {session_override:?}"
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
