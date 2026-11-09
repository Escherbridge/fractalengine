//! Bindings view of the Extensions panel (bindings.md section 9, BND-9): the hook points at the active petal grouped by
//! family, each with its winning scope, this host's state and at most one offer, plus the Manager+ edit form generated
//! from the provider's `config_schema` (the `args_schema` subset). Pure state and egui; the bridge fills the snapshot
//! and executes the queued ops. See `fe-ui/src/extensions/AGENTS.md` §bindings-view.

use bevy_egui::egui;
use fe_runtime::extension_binding::{
    hook_spec, BindingMode, BindingOffer, BindingResolution, BindingScopeLevel, HookFamily, UnresolvedReason,
};
use serde_json::{Map, Number, Value};

use super::panel::{ExtensionOp, ExtensionsPanelState};
use crate::accessibility::ResponseExt;
use crate::theme;

/// A scope addressed by ids (the REST/DB `BindingTarget` shape; fe-ui never builds scope strings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeTarget {
    pub verse_id: String,
    pub fractal_id: Option<String>,
    pub petal_id: Option<String>,
}

/// One scope a binding can be written at, nearest first (petal, fractal, verse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeOption {
    pub level: BindingScopeLevel,
    /// Display label, e.g. `Petal Quay`.
    pub label: String,
    pub target: ScopeTarget,
}

/// One hook point at the active petal: the winning binding (if any) and this host's resolution of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingPointRow {
    pub hook: String,
    /// `""` for keyless hooks.
    pub key: String,
    /// Display label of the scope the winning row sits at; empty when nothing is bound.
    pub scope_label: String,
    /// Where the winning row sits (Remove writes `inherit` here); `None` when nothing is bound.
    pub target: Option<ScopeTarget>,
    /// `Inherit` = nothing bound, the built-in serves.
    pub mode: BindingMode,
    pub package_id: Option<String>,
    pub version_req: Option<String>,
    pub config: Value,
    /// Placement is carried through edits unchanged (the form does not set it).
    pub runs_on: Option<String>,
    /// This host's state; `None` when the host did not report one.
    pub resolution: Option<BindingResolution>,
}

/// An installed extension that provides a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOption {
    pub hook: String,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub config_schema: Option<Value>,
}

/// Everything the view shows (a snapshot; fe-ui never reaches the host). The bridge attaches a point's `offer` only
/// when bindings.md section 5.2 allows it (Install and Update need a source with a fitting build).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BindingsSnapshot {
    pub verse_id: String,
    /// Display name of the active petal.
    pub petal_label: String,
    /// The local DID holds Manager+ at the petal: the edit form opens.
    pub can_manage: bool,
    /// The local DID is the verse Owner: the kill-list Override offer shows (bindings.md section 5.7).
    pub local_is_owner: bool,
    /// Scopes the local DID may write at (Manager+ by subtree authority), nearest first; the form offers no other.
    pub scopes: Vec<ScopeOption>,
    pub points: Vec<BindingPointRow>,
    pub providers: Vec<ProviderOption>,
}

impl BindingsSnapshot {
    /// The edit form is available: Manager+ with at least one writable scope.
    pub fn manages(&self) -> bool {
        self.can_manage && !self.scopes.is_empty()
    }

    /// True when the local DID may write a row at `target`.
    pub fn writes_at(&self, target: &ScopeTarget) -> bool {
        self.manages() && self.scopes.iter().any(|scope| scope.target == *target)
    }

    /// The one offer point `index` shows to this DID: Edit binding needs Manager+, Override needs the verse Owner.
    pub fn visible_offer(&self, index: usize) -> Option<BindingOffer> {
        let row = self.points.get(index)?;
        let Some(BindingResolution::Unresolved { offer: Some(offer), .. }) = &row.resolution else { return None };
        match offer {
            BindingOffer::EditBinding if !self.manages() => None,
            BindingOffer::Override if !self.local_is_owner => None,
            offer => Some(*offer),
        }
    }

    /// Manifest name of an installed provider of `package_id`, else the id.
    fn package_label<'a>(&'a self, package_id: &'a str) -> &'a str {
        self.providers.iter().find(|p| p.package_id == package_id).map_or(package_id, |p| p.name.as_str())
    }
}

/// Kind of one generated form field.
#[derive(Debug, Clone, PartialEq)]
enum FieldKind {
    Text,
    Choice(Vec<String>),
    Integer,
    Number,
    Toggle,
    /// Array of strings, entered comma separated.
    List,
    /// Outside what the form renders; the original value is written back untouched.
    Kept,
}

/// One form field generated from a `properties` entry.
#[derive(Debug, Clone, PartialEq)]
struct FormField {
    name: String,
    description: String,
    required: bool,
    kind: FieldKind,
    text: String,
    flag: bool,
    min: Option<f64>,
    max: Option<f64>,
    min_len: Option<usize>,
    max_len: Option<usize>,
    kept: Option<Value>,
}

/// The open edit form (one at a time).
#[derive(Debug, Clone, PartialEq)]
pub struct EditForm {
    hook: String,
    key: String,
    /// The chosen scope, by target: a refreshed snapshot can never move the write to another scope.
    scope: Option<ScopeTarget>,
    mode: BindingMode,
    /// Index into `BindingsSnapshot::providers`.
    provider: Option<usize>,
    version_req: String,
    runs_on: Option<String>,
    original_config: Value,
    original_package: Option<String>,
    fields: Vec<FormField>,
    error: Option<String>,
}

/// View state held by the panel.
#[derive(Debug, Default)]
pub struct BindingsViewState {
    pub snapshot: Option<BindingsSnapshot>,
    pub edit: Option<EditForm>,
    requested: bool,
}

impl BindingsViewState {
    /// Replace the snapshot; an open form closes when its point vanished or the local DID no longer manages here.
    pub fn apply(&mut self, snapshot: BindingsSnapshot) {
        if let Some(form) = &self.edit {
            let point_gone = !snapshot.points.iter().any(|row| row.hook == form.hook && row.key == form.key);
            if point_gone || !snapshot.manages() {
                self.edit = None;
            }
        }
        self.snapshot = Some(snapshot);
    }

    /// Drop the snapshot and any open form; the next draw asks again (the active petal changed).
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }

    /// Open the form for point `index` (Manager+ only), seeded from its current binding, writing at the nearest scope.
    pub fn start_edit(&mut self, index: usize) {
        let Some(snapshot) = &self.snapshot else { return };
        if !snapshot.manages() {
            return;
        }
        let Some(row) = snapshot.points.get(index) else { return };
        let provider = row
            .package_id
            .as_ref()
            .and_then(|package| snapshot.providers.iter().position(|p| p.hook == row.hook && &p.package_id == package))
            .or_else(|| snapshot.providers.iter().position(|p| p.hook == row.hook));
        let schema = provider.and_then(|i| snapshot.providers[i].config_schema.as_ref());
        self.edit = Some(EditForm {
            hook: row.hook.clone(),
            key: row.key.clone(),
            scope: snapshot.scopes.first().map(|scope| scope.target.clone()),
            mode: if row.mode == BindingMode::Inherit { BindingMode::Extension } else { row.mode },
            provider,
            version_req: row.version_req.clone().unwrap_or_default(),
            runs_on: row.runs_on.clone(),
            original_config: row.config.clone(),
            original_package: row.package_id.clone(),
            fields: form_fields(schema, &row.config),
            error: None,
        });
    }

    /// Switch the provider; the fields are rebuilt, seeded from the saved config only for the same package.
    pub fn choose_provider(&mut self, provider: usize) {
        let (Some(snapshot), Some(form)) = (&self.snapshot, &mut self.edit) else { return };
        let Some(option) = snapshot.providers.get(provider) else { return };
        let seed = if form.original_package.as_deref() == Some(option.package_id.as_str()) {
            form.original_config.clone()
        } else {
            Value::Object(Map::new())
        };
        form.provider = Some(provider);
        form.fields = form_fields(option.config_schema.as_ref(), &seed);
        form.error = None;
    }

    /// Build the write op for the open form, or the message to show; nothing is queued here.
    pub fn build_save(&mut self) -> Option<ExtensionOp> {
        let snapshot = self.snapshot.as_ref()?;
        if !snapshot.manages() {
            self.edit = None;
            return None;
        }
        let form = self.edit.as_mut()?;
        let Some(target) = form.scope.clone().filter(|target| snapshot.writes_at(target)) else {
            form.error = Some("Choose where to apply the binding".into());
            return None;
        };
        let (package_id, version_req, config) = match form.mode {
            BindingMode::Extension => {
                let Some(provider) = form.provider.and_then(|i| snapshot.providers.get(i)) else {
                    form.error = Some("Choose an extension".into());
                    return None;
                };
                let config = match config_from_fields(&form.fields) {
                    Ok(config) => config,
                    Err(message) => {
                        form.error = Some(message);
                        return None;
                    }
                };
                let range = form.version_req.trim();
                (Some(provider.package_id.clone()), (!range.is_empty()).then(|| range.to_owned()), config)
            }
            _ => (None, None, Value::Object(Map::new())),
        };
        let runs_on = if form.mode == BindingMode::Extension { form.runs_on.clone() } else { None };
        let op = ExtensionOp::SetBinding {
            target,
            hook: form.hook.clone(),
            key: form.key.clone(),
            mode: if form.mode == BindingMode::Inherit { BindingMode::Builtin } else { form.mode },
            package_id,
            version_req,
            config,
            runs_on,
        };
        self.edit = None;
        Some(op)
    }
}

impl ExtensionsPanelState {
    /// Ask for a fresh Bindings snapshot of the active petal.
    pub fn request_bindings_refresh(&mut self) {
        self.bindings.requested = true;
        self.queue(ExtensionOp::RefreshBindings, "Loading bindings");
    }

    /// Queue the open form's write; a validation failure stays on the form.
    pub fn request_binding_save(&mut self) {
        if let Some(op) = self.bindings.build_save() {
            self.queue(op, "Saving binding");
        }
    }

    /// Write `inherit` for point `index` where its winning row sits, when the local DID manages that scope.
    pub fn request_binding_clear(&mut self, index: usize) {
        let op = {
            let Some(snapshot) = &self.bindings.snapshot else { return };
            let Some(row) = snapshot.points.get(index) else { return };
            let writable = |target: &&ScopeTarget| row.mode != BindingMode::Inherit && snapshot.writes_at(target);
            let Some(target) = row.target.as_ref().filter(writable) else { return };
            ExtensionOp::ClearBinding { target: target.clone(), hook: row.hook.clone(), key: row.key.clone() }
        };
        self.bindings.edit = None;
        self.queue(op, "Clearing binding");
    }

    /// Run the one offer of point `index`: Edit binding opens the form, every other offer goes to the bridge (consent).
    pub fn request_binding_offer(&mut self, index: usize) {
        let (offer, op) = {
            let Some(snapshot) = &self.bindings.snapshot else { return };
            let Some(offer) = snapshot.visible_offer(index) else { return };
            let row = &snapshot.points[index];
            let Some(BindingResolution::Unresolved { package_id, .. }) = &row.resolution else { return };
            let op = ExtensionOp::TakeBindingOffer {
                offer,
                verse_id: snapshot.verse_id.clone(),
                hook: row.hook.clone(),
                key: row.key.clone(),
                package_id: package_id.clone(),
            };
            (offer, op)
        };
        if offer == BindingOffer::EditBinding {
            self.bindings.start_edit(index);
            return;
        }
        self.queue(op, offer_working(offer));
    }
}

fn offer_working(offer: BindingOffer) -> &'static str {
    match offer {
        BindingOffer::Install => "Preparing install",
        BindingOffer::Update => "Preparing update",
        BindingOffer::Override => "Preparing override",
        BindingOffer::EditBinding => "Opening editor",
        BindingOffer::ReviewPermissions => "Preparing review",
        BindingOffer::AllowInVerse => "Allowing in this verse",
        BindingOffer::Enable => "Enabling",
    }
}

// ---------------------------------------------------------------------------
// Labels and names
// ---------------------------------------------------------------------------

/// Accessible name of a point's state row: `binding:<hook>` or `binding:<hook>:<key>`.
pub fn binding_name(hook: &str, key: &str) -> String {
    if key.is_empty() { format!("binding:{hook}") } else { format!("binding:{hook}:{key}") }
}

/// Accessible name of a control on a point: the state name plus `:<suffix>` (`action` for its one offer).
pub fn action_name(hook: &str, key: &str, suffix: &str) -> String {
    format!("{}:{suffix}", binding_name(hook, key))
}

fn offer_label(offer: BindingOffer) -> &'static str {
    match offer {
        BindingOffer::Install => "Install",
        BindingOffer::Update => "Update",
        BindingOffer::Override => "Override...",
        BindingOffer::EditBinding => "Edit binding",
        BindingOffer::ReviewPermissions => "Review permissions",
        BindingOffer::AllowInVerse => "Allow in this verse",
        BindingOffer::Enable => "Enable",
    }
}

fn reason_text(reason: UnresolvedReason) -> &'static str {
    match reason {
        UnresolvedReason::HostOff => "extensions are switched off on this device",
        UnresolvedReason::UnknownHook => "this build does not serve the hook",
        UnresolvedReason::NotInstalled => "not installed",
        UnresolvedReason::VersionUnsatisfied => "no installed version fits the binding",
        UnresolvedReason::PackageInvalid => "the package failed verification",
        UnresolvedReason::UntrustedSigner => "the publisher is not trusted",
        UnresolvedReason::EntitlementRequired => "an entitlement is required",
        UnresolvedReason::Revoked => "the build is on a kill list",
        UnresolvedReason::PolicyDenied => "policy does not allow it here",
        UnresolvedReason::NotProvided => "the installed version does not provide the hook",
        UnresolvedReason::InvalidConfig => "the binding settings are invalid for this version",
        UnresolvedReason::RoleDenied => "your role is too low to run it here",
        UnresolvedReason::ConsentMissing => "permissions were not reviewed",
        UnresolvedReason::NotAccepted => "not allowed in this verse yet",
        UnresolvedReason::Disabled => "switched off on this device",
        UnresolvedReason::Degraded => "it is failing and was stopped",
        UnresolvedReason::Capacity => "this device is at its extension limit",
    }
}

/// How a state row is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Muted,
}

/// A DID shortened for a status line (`did:key:…a1b2c3`).
fn short_did(did: &str) -> String {
    let chars: Vec<char> = did.chars().collect();
    if chars.len() <= 16 {
        return did.to_owned();
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

/// State text and tone for one point; the package and scope are on the detail line below it.
pub fn point_state(row: &BindingPointRow) -> (String, Tone) {
    match &row.resolution {
        None => ("Not reported".into(), Tone::Muted),
        Some(BindingResolution::Resolved { .. }) => ("Running".into(), Tone::Ok),
        Some(BindingResolution::Builtin { .. }) => ("Built-in".into(), Tone::Muted),
        Some(BindingResolution::Elsewhere { runs_on }) => (format!("Runs on {}, built-in here", short_did(runs_on)), Tone::Muted),
        Some(BindingResolution::Unresolved { reason, .. }) => (format!("Not running: {}", reason_text(*reason)), Tone::Warn),
    }
}

const FAMILY_ORDER: [HookFamily; 12] = [
    HookFamily::MapTerrain,
    HookFamily::Api,
    HookFamily::SceneOps,
    HookFamily::Ingest,
    HookFamily::Files,
    HookFamily::Egress,
    HookFamily::Storage,
    HookFamily::Network,
    HookFamily::Ui,
    HookFamily::PortalWeb,
    HookFamily::Renderer,
    HookFamily::Lifecycle,
];

fn family_title(family: Option<HookFamily>) -> &'static str {
    match family {
        Some(HookFamily::Lifecycle) => "Lifecycle",
        Some(HookFamily::MapTerrain) => "Map and terrain",
        Some(HookFamily::Api) => "API routes",
        Some(HookFamily::SceneOps) => "Scene",
        Some(HookFamily::Ingest) => "Device data",
        Some(HookFamily::Ui) => "Interface",
        Some(HookFamily::Egress) => "Export",
        Some(HookFamily::Files) => "Files",
        Some(HookFamily::Storage) => "Storage",
        Some(HookFamily::Network) => "Network",
        Some(HookFamily::PortalWeb) => "Web portal",
        Some(HookFamily::Renderer) => "Renderer",
        None => "Other",
    }
}

fn family_rank(family: Option<HookFamily>) -> usize {
    family.and_then(|f| FAMILY_ORDER.iter().position(|ordered| *ordered == f)).unwrap_or(FAMILY_ORDER.len())
}

/// Point indices grouped by family in display order; hooks this build does not know fall under "Other".
pub fn grouped_points(points: &[BindingPointRow]) -> Vec<(&'static str, Vec<usize>)> {
    let mut keyed: Vec<(usize, &str, &str, usize)> = points
        .iter()
        .enumerate()
        .map(|(i, row)| (family_rank(hook_spec(&row.hook).map(|s| s.family)), row.hook.as_str(), row.key.as_str(), i))
        .collect();
    keyed.sort();
    let mut groups: Vec<(usize, &'static str, Vec<usize>)> = Vec::new();
    for (rank, hook, _, index) in keyed {
        let title = family_title(hook_spec(hook).map(|s| s.family));
        match groups.last_mut() {
            Some((last, _, indices)) if *last == rank => indices.push(index),
            _ => groups.push((rank, title, vec![index])),
        }
    }
    groups.into_iter().map(|(_, title, indices)| (title, indices)).collect()
}

fn level_slug(level: BindingScopeLevel) -> &'static str {
    match level {
        BindingScopeLevel::Verse => "verse",
        BindingScopeLevel::Fractal => "fractal",
        BindingScopeLevel::Petal => "petal",
    }
}

fn point_title(hook: &str, key: &str) -> String {
    if key.is_empty() { hook.to_owned() } else { format!("{hook} \u{00B7} {key}") }
}

// ---------------------------------------------------------------------------
// Form generation from the args_schema subset
// ---------------------------------------------------------------------------

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn field_kind(node: &Value) -> FieldKind {
    if let Some(options) = node.get("enum").and_then(Value::as_array) {
        let texts: Vec<String> = options.iter().map(value_text).collect();
        return if texts.is_empty() { FieldKind::Kept } else { FieldKind::Choice(texts) };
    }
    match node.get("type").and_then(Value::as_str) {
        Some("string") => FieldKind::Text,
        Some("integer") => FieldKind::Integer,
        Some("number") => FieldKind::Number,
        Some("boolean") => FieldKind::Toggle,
        Some("array") if node.get("items").and_then(|i| i.get("type")).and_then(Value::as_str) == Some("string") => {
            FieldKind::List
        }
        _ => FieldKind::Kept,
    }
}

/// One field per schema property, seeded from `current`; no schema = no settings.
fn form_fields(schema: Option<&Value>, current: &Value) -> Vec<FormField> {
    let Some(schema) = schema else { return Vec::new() };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    properties
        .iter()
        .map(|(name, node)| {
            let seed = current.get(name);
            let mut field = FormField {
                name: name.clone(),
                description: node.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
                required: required.contains(&name.as_str()),
                kind: field_kind(node),
                text: String::new(),
                flag: false,
                min: node.get("minimum").and_then(Value::as_f64),
                max: node.get("maximum").and_then(Value::as_f64),
                min_len: node.get("minLength").and_then(Value::as_u64).map(|n| n as usize),
                max_len: node.get("maxLength").and_then(Value::as_u64).map(|n| n as usize),
                kept: None,
            };
            match seed {
                Some(other) if field.kind == FieldKind::Kept => field.kept = Some(other.clone()),
                Some(Value::Bool(flag)) => field.flag = *flag,
                Some(Value::Array(items)) if field.kind == FieldKind::List => {
                    field.text = items.iter().map(value_text).collect::<Vec<_>>().join(", ");
                }
                Some(other) => field.text = value_text(other),
                None => {}
            }
            field
        })
        .collect()
}

/// The config object the fields describe, or the first problem as a sentence.
fn config_from_fields(fields: &[FormField]) -> Result<Value, String> {
    let mut config = Map::new();
    for field in fields {
        let name = &field.name;
        match &field.kind {
            FieldKind::Kept => {
                if let Some(value) = &field.kept {
                    config.insert(name.clone(), value.clone());
                }
            }
            FieldKind::Toggle => {
                config.insert(name.clone(), Value::Bool(field.flag));
            }
            kind => {
                let text = field.text.trim();
                if text.is_empty() {
                    if field.required {
                        return Err(format!("{name} is required"));
                    }
                    continue;
                }
                let value = match kind {
                    FieldKind::Text => {
                        let length = text.chars().count();
                        if field.min_len.is_some_and(|min| length < min) || field.max_len.is_some_and(|max| length > max) {
                            return Err(format!("{name} has the wrong length"));
                        }
                        Value::String(text.to_owned())
                    }
                    FieldKind::Choice(options) => {
                        if !options.iter().any(|option| option == text) {
                            return Err(format!("{name} must be one of the listed values"));
                        }
                        Value::String(text.to_owned())
                    }
                    FieldKind::Integer => {
                        let number: i64 = text.parse().map_err(|_| format!("{name} must be a whole number"))?;
                        check_range(field, number as f64)?;
                        Value::Number(number.into())
                    }
                    FieldKind::Number => {
                        let number: f64 = text.parse().map_err(|_| format!("{name} must be a number"))?;
                        check_range(field, number)?;
                        Number::from_f64(number).map(Value::Number).ok_or_else(|| format!("{name} must be a number"))?
                    }
                    FieldKind::List => Value::Array(
                        text.split(',').map(str::trim).filter(|item| !item.is_empty()).map(|item| Value::String(item.to_owned())).collect(),
                    ),
                    FieldKind::Toggle | FieldKind::Kept => continue,
                };
                config.insert(name.clone(), value);
            }
        }
    }
    Ok(Value::Object(config))
}

fn check_range(field: &FormField, number: f64) -> Result<(), String> {
    if field.min.is_some_and(|min| number < min) || field.max.is_some_and(|max| number > max) {
        return Err(format!("{} is out of range", field.name));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

enum Act {
    Offer(usize),
    Edit(usize),
    Clear(usize),
    Save,
    Cancel,
    Provider(usize),
}

fn tone_color(tone: Tone) -> egui::Color32 {
    match tone {
        Tone::Ok => theme::TEXT_ROLE_EDITOR,
        Tone::Warn => theme::STATUS_OFFLINE,
        Tone::Muted => theme::TEXT_MUTED,
    }
}

/// Draw the Bindings view into `ui`.
pub fn draw(ui: &mut egui::Ui, state: &mut ExtensionsPanelState) {
    if !state.bindings.requested {
        state.request_bindings_refresh();
    }
    let busy = state.busy();
    let Some(snapshot) = state.bindings.snapshot.take() else {
        if state.working.is_some() {
            ui.label(egui::RichText::new("Loading bindings").color(theme::TEXT_MUTED));
            return;
        }
        // A failed load (its message is on the error line above) keeps a retry, never a dead end.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Bindings are not loaded").color(theme::TEXT_MUTED));
            if ui.add_enabled(!busy, egui::Button::new("Refresh")).button_named(ui, "Refresh bindings").clicked() {
                state.request_bindings_refresh();
            }
        });
        return;
    };
    let mut form = state.bindings.edit.take();
    let mut acts: Vec<Act> = Vec::new();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&snapshot.petal_label).strong());
        if ui.add_enabled(!busy, egui::Button::new("Refresh")).button_named(ui, "Refresh bindings").clicked() {
            state.request_bindings_refresh();
        }
    });
    ui.add_space(6.0);
    if let Some(form) = form.as_mut() {
        draw_form(ui, &snapshot, form, busy, &mut acts);
        ui.add_space(6.0);
    }
    if snapshot.points.is_empty() {
        ui.label(egui::RichText::new("No bindings at this petal").color(theme::TEXT_MUTED));
    }
    for (title, indices) in grouped_points(&snapshot.points) {
        ui.label(egui::RichText::new(title).small().color(theme::TEXT_SECTION));
        for index in indices {
            draw_row(ui, &snapshot, index, busy, &mut acts);
        }
        ui.add_space(6.0);
    }

    state.bindings.snapshot = Some(snapshot);
    state.bindings.edit = form;
    for act in acts {
        match act {
            Act::Offer(index) => state.request_binding_offer(index),
            Act::Edit(index) => state.bindings.start_edit(index),
            Act::Clear(index) => state.request_binding_clear(index),
            Act::Save => state.request_binding_save(),
            Act::Cancel => state.bindings.edit = None,
            Act::Provider(provider) => state.bindings.choose_provider(provider),
        }
    }
}

/// One point: title and state, the binding's scope and package, then at most the offer and (Manager+) Edit.
fn draw_row(ui: &mut egui::Ui, snapshot: &BindingsSnapshot, index: usize, busy: bool, acts: &mut Vec<Act>) {
    let row = &snapshot.points[index];
    let (state_text, tone) = point_state(row);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(point_title(&row.hook, &row.key)).strong());
        ui.label(egui::RichText::new(state_text).color(tone_color(tone))).named(ui, binding_name(&row.hook, &row.key));
    });
    if row.mode != BindingMode::Inherit && !row.scope_label.is_empty() {
        let mut detail = format!("Set at {}", row.scope_label);
        if let Some(package) = &row.package_id {
            detail.push_str(&format!(" \u{00B7} {}", snapshot.package_label(package)));
            if let Some(range) = &row.version_req {
                detail.push_str(&format!(" {range}"));
            }
        }
        ui.label(egui::RichText::new(detail).small().color(theme::TEXT_MUTED));
    }
    let offer = snapshot.visible_offer(index);
    let edit = snapshot.manages() && offer != Some(BindingOffer::EditBinding);
    if offer.is_some() || edit {
        ui.horizontal(|ui| {
            if let Some(offer) = offer {
                if ui
                    .add_enabled(!busy, egui::Button::new(offer_label(offer)))
                    .button_named(ui, action_name(&row.hook, &row.key, "action"))
                    .clicked()
                {
                    acts.push(Act::Offer(index));
                }
            }
            if edit
                && ui.add_enabled(!busy, egui::Button::new("Edit")).button_named(ui, action_name(&row.hook, &row.key, "edit")).clicked()
            {
                acts.push(Act::Edit(index));
            }
        });
    }
    ui.add_space(4.0);
}

fn draw_form(ui: &mut egui::Ui, snapshot: &BindingsSnapshot, form: &mut EditForm, busy: bool, acts: &mut Vec<Act>) {
    let base = binding_name(&form.hook, &form.key);
    // Remove is offered only where the winning row sits at a scope the local DID manages.
    let removable = snapshot.points.iter().position(|row| {
        row.hook == form.hook
            && row.key == form.key
            && row.mode != BindingMode::Inherit
            && row.target.as_ref().is_some_and(|target| snapshot.writes_at(target))
    });
    ui.group(|ui| {
        ui.label(egui::RichText::new(format!("Bind {}", point_title(&form.hook, &form.key))).strong());
        ui.horizontal(|ui| {
            ui.label("Apply at");
            for option in &snapshot.scopes {
                ui.selectable_value(&mut form.scope, Some(option.target.clone()), &option.label)
                    .button_named(ui, format!("{base}:scope:{}", level_slug(option.level)));
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.mode, BindingMode::Extension, "Extension").button_named(ui, format!("{base}:mode:extension"));
            ui.selectable_value(&mut form.mode, BindingMode::Builtin, "Built-in").button_named(ui, format!("{base}:mode:builtin"));
        });
        if form.mode == BindingMode::Extension {
            draw_provider_fields(ui, snapshot, form, &base, acts);
        }
        if let Some(message) = &form.error {
            ui.label(egui::RichText::new(message).color(theme::STATUS_OFFLINE));
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Save")).button_named(ui, format!("{base}:save")).clicked() {
                acts.push(Act::Save);
            }
            if ui.button("Cancel").button_named(ui, format!("{base}:cancel")).clicked() {
                acts.push(Act::Cancel);
            }
            if let Some(index) = removable {
                let label = format!("Remove binding at {}", snapshot.points[index].scope_label);
                if ui.add_enabled(!busy, egui::Button::new(label)).button_named(ui, format!("{base}:clear")).clicked() {
                    acts.push(Act::Clear(index));
                }
            }
        });
    });
}

/// Provider picker, version range and the generated settings; an empty catalogue says so instead of an empty list.
fn draw_provider_fields(ui: &mut egui::Ui, snapshot: &BindingsSnapshot, form: &mut EditForm, base: &str, acts: &mut Vec<Act>) {
    let offered: Vec<(usize, &ProviderOption)> =
        snapshot.providers.iter().enumerate().filter(|(_, option)| option.hook == form.hook).collect();
    if offered.is_empty() {
        ui.label(egui::RichText::new(format!("No installed extension provides {}", form.hook)).color(theme::TEXT_MUTED));
        return;
    }
    let selected = form
        .provider
        .and_then(|i| snapshot.providers.get(i))
        .map(|p| format!("{} {}", p.name, p.version))
        .unwrap_or_else(|| "Choose an extension".into());
    let current = form.provider;
    egui::ComboBox::from_id_salt(format!("{base}:provider"))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (i, option) in &offered {
                let label = format!("{} {}", option.name, option.version);
                if ui.selectable_label(current == Some(*i), label).clicked() {
                    acts.push(Act::Provider(*i));
                }
            }
        })
        .response
        .named(ui, format!("{base}:provider"));
    ui.add(egui::TextEdit::singleline(&mut form.version_req).hint_text("Version range (optional)"))
        .named(ui, format!("{base}:version"));
    for field in form.fields.iter_mut() {
        draw_field(ui, base, field);
    }
}

fn draw_field(ui: &mut egui::Ui, base: &str, field: &mut FormField) {
    let name = format!("{base}:field:{}", field.name);
    let label = if field.required { format!("{} *", field.name) } else { field.name.clone() };
    let hover = field.description.clone();
    ui.horizontal(|ui| {
        match &field.kind {
            FieldKind::Toggle => {
                ui.checkbox(&mut field.flag, label).named(ui, &name);
            }
            FieldKind::Choice(options) => {
                ui.label(label);
                let selected = if field.text.is_empty() { "Choose".to_owned() } else { field.text.clone() };
                egui::ComboBox::from_id_salt(&name)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for option in options {
                            ui.selectable_value(&mut field.text, option.clone(), option);
                        }
                    })
                    .response
                    .named(ui, &name);
            }
            FieldKind::Kept => {
                ui.label(egui::RichText::new(format!("{} is set through the API", field.name)).small().color(theme::TEXT_MUTED));
            }
            kind => {
                ui.label(label);
                let hint = if *kind == FieldKind::List { "comma separated" } else { "" };
                let response = ui.add(egui::TextEdit::singleline(&mut field.text).hint_text(hint)).named(ui, &name);
                if !hover.is_empty() {
                    response.on_hover_text(hover);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The generated form round-trips a config, refuses the first bad field, and keeps what it cannot render.
    #[test]
    fn config_form_roundtrip() {
        let schema = json!({
            "type": "object",
            "required": ["district"],
            "properties": {
                "district": {"type": "string", "enum": ["harbour", "old-town"]},
                "radius": {"type": "integer", "minimum": 1, "maximum": 50},
                "labels": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "style": {"type": "object", "properties": {}}
            }
        });
        let current = json!({"district": "harbour", "radius": 5, "tags": ["a", "b"], "style": {"x": 1}});
        let mut fields = form_fields(Some(&schema), &current);
        assert_eq!(config_from_fields(&fields).unwrap(), json!({"district": "harbour", "radius": 5, "labels": false, "tags": ["a", "b"], "style": {"x": 1}}));

        let radius = fields.iter_mut().find(|f| f.name == "radius").unwrap();
        for (text, ok) in [("12", true), ("0", false), ("51", false), ("2.5", false), ("", true)] {
            radius.text = text.into();
            assert_eq!(config_from_fields(std::slice::from_ref(radius)).is_ok(), ok, "radius {text:?}");
        }
        let district = fields.iter_mut().find(|f| f.name == "district").unwrap();
        district.text.clear();
        assert_eq!(config_from_fields(std::slice::from_ref(district)).unwrap_err(), "district is required");
        assert!(form_fields(None, &current).is_empty());
    }
}
