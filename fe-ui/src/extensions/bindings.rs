//! Bindings view of the Extensions panel (bindings.md section 9, BND-9): the hook points at the active petal grouped by
//! family, each with its winning scope, this host's state and one offer, plus the Manager+ edit form generated from the
//! provider's `config_schema` (the `args_schema` subset). Pure state and egui; the bridge fills the snapshot and executes
//! the queued ops. See `docs/extensions/bindings.md` section 9 and `docs/extensions/extensions-ui.md`.

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
    /// Where the winning row sits (clear writes `inherit` here); `None` when nothing is bound.
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

/// Everything the view shows (a snapshot; fe-ui never reaches the host).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BindingsSnapshot {
    pub verse_id: String,
    /// Display name of the active petal.
    pub petal_label: String,
    /// The local DID holds Manager+ at the petal: Edit and Clear show.
    pub can_manage: bool,
    pub scopes: Vec<ScopeOption>,
    pub points: Vec<BindingPointRow>,
    pub providers: Vec<ProviderOption>,
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
    scope_index: usize,
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
    /// Replace the snapshot; an open form whose point vanished is closed.
    pub fn apply(&mut self, snapshot: BindingsSnapshot) {
        if let Some(form) = &self.edit {
            if !snapshot.points.iter().any(|row| row.hook == form.hook && row.key == form.key) {
                self.edit = None;
            }
        }
        self.snapshot = Some(snapshot);
    }

    /// Open the form for point `index`, seeded from its current binding; defaults to writing at the nearest scope.
    pub fn start_edit(&mut self, index: usize) {
        let Some(snapshot) = &self.snapshot else { return };
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
            scope_index: 0,
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
        let form = self.edit.as_mut()?;
        let Some(scope) = snapshot.scopes.get(form.scope_index) else {
            form.error = Some("Choose where to apply the binding".into());
            return None;
        };
        let target = scope.target.clone();
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

    /// Clear point `index` at the scope its winning row sits at.
    pub fn request_binding_clear(&mut self, index: usize) {
        let Some(row) = self.bindings.snapshot.as_ref().and_then(|s| s.points.get(index)) else { return };
        let Some(target) = row.target.clone() else { return };
        let op = ExtensionOp::ClearBinding { target, hook: row.hook.clone(), key: row.key.clone() };
        self.queue(op, "Clearing binding");
    }

    /// Run the one offer of point `index`: edit opens the form, every other offer goes to the bridge (which shows consent).
    pub fn request_binding_offer(&mut self, index: usize) {
        let Some(snapshot) = &self.bindings.snapshot else { return };
        let Some(row) = snapshot.points.get(index) else { return };
        let Some(BindingResolution::Unresolved { package_id, offer: Some(offer), .. }) = &row.resolution else { return };
        let (offer, can_manage) = (*offer, snapshot.can_manage);
        let op = ExtensionOp::TakeBindingOffer {
            offer,
            verse_id: snapshot.verse_id.clone(),
            hook: row.hook.clone(),
            key: row.key.clone(),
            package_id: package_id.clone(),
        };
        if offer == BindingOffer::EditBinding {
            if can_manage {
                self.bindings.start_edit(index);
            }
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

/// Accessible name of a point's one action: the state name plus `:action`.
pub fn action_name(hook: &str, key: &str, action: &str) -> String {
    format!("{}:{action}", binding_name(hook, key))
}

/// Short action slug used in accessible names.
pub fn offer_slug(offer: BindingOffer) -> &'static str {
    match offer {
        BindingOffer::Install => "install",
        BindingOffer::Update => "update",
        BindingOffer::Override => "override",
        BindingOffer::EditBinding => "edit",
        BindingOffer::ReviewPermissions => "review",
        BindingOffer::AllowInVerse => "allow",
        BindingOffer::Enable => "enable",
    }
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

/// State text and tone for one point.
pub fn point_state(row: &BindingPointRow) -> (String, Tone) {
    match &row.resolution {
        None => ("Not reported".into(), Tone::Muted),
        Some(BindingResolution::Resolved { package_id, .. }) => (format!("Served by {package_id}"), Tone::Ok),
        Some(BindingResolution::Builtin { .. }) => {
            if row.mode == BindingMode::Builtin && !row.scope_label.is_empty() {
                (format!("Built-in, set at {}", row.scope_label), Tone::Muted)
            } else {
                ("Built-in".into(), Tone::Muted)
            }
        }
        Some(BindingResolution::Elsewhere { .. }) => ("Runs on another peer; built-in here".into(), Tone::Muted),
        Some(BindingResolution::Unresolved { package_id, reason, .. }) => {
            (format!("{package_id} is not running: {}. Built-in in use", reason_text(*reason)), Tone::Warn)
        }
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
        let text = if state.working.is_some() { "Loading bindings" } else { "No active petal" };
        ui.label(egui::RichText::new(text).color(theme::TEXT_MUTED));
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
        ui.label(egui::RichText::new("No extension hooks at this petal").color(theme::TEXT_MUTED));
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

fn draw_row(ui: &mut egui::Ui, snapshot: &BindingsSnapshot, index: usize, busy: bool, acts: &mut Vec<Act>) {
    let row = &snapshot.points[index];
    let name = binding_name(&row.hook, &row.key);
    let (state_text, tone) = point_state(row);
    let title = if row.key.is_empty() { row.hook.clone() } else { format!("{} · {}", row.hook, row.key) };
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.label(egui::RichText::new(state_text).color(tone_color(tone))).named(ui, &name);
    });
    if row.mode != BindingMode::Inherit && !row.scope_label.is_empty() {
        let mut detail = format!("Set at {}", row.scope_label);
        if let Some(package) = &row.package_id {
            detail.push_str(&format!(" · {package}"));
            if let Some(range) = &row.version_req {
                detail.push_str(&format!(" {range}"));
            }
        }
        ui.label(egui::RichText::new(detail).small().color(theme::TEXT_MUTED));
    }
    ui.horizontal(|ui| {
        if let Some(BindingResolution::Unresolved { offer: Some(offer), .. }) = &row.resolution {
            let allowed = *offer != BindingOffer::EditBinding || snapshot.can_manage;
            if allowed
                && ui
                    .add_enabled(!busy, egui::Button::new(offer_label(*offer)))
                    .button_named(ui, action_name(&row.hook, &row.key, offer_slug(*offer)))
                    .clicked()
            {
                acts.push(Act::Offer(index));
            }
        }
        if snapshot.can_manage {
            if ui.add_enabled(!busy, egui::Button::new("Edit")).button_named(ui, action_name(&row.hook, &row.key, "edit-form")).clicked() {
                acts.push(Act::Edit(index));
            }
            if row.mode != BindingMode::Inherit
                && row.target.is_some()
                && ui.add_enabled(!busy, egui::Button::new("Clear")).button_named(ui, action_name(&row.hook, &row.key, "clear")).clicked()
            {
                acts.push(Act::Clear(index));
            }
        }
    });
    ui.add_space(4.0);
}

fn draw_form(ui: &mut egui::Ui, snapshot: &BindingsSnapshot, form: &mut EditForm, busy: bool, acts: &mut Vec<Act>) {
    let base = binding_name(&form.hook, &form.key);
    ui.group(|ui| {
        ui.label(egui::RichText::new(format!("Bind {}", if form.key.is_empty() { form.hook.clone() } else { format!("{} · {}", form.hook, form.key) })).strong());
        ui.horizontal(|ui| {
            ui.label("Apply at");
            for (i, option) in snapshot.scopes.iter().enumerate() {
                ui.selectable_value(&mut form.scope_index, i, &option.label)
                    .button_named(ui, format!("{base}:scope:{}", level_slug(option.level)));
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.mode, BindingMode::Extension, "Extension").button_named(ui, format!("{base}:mode:extension"));
            ui.selectable_value(&mut form.mode, BindingMode::Builtin, "Built-in").button_named(ui, format!("{base}:mode:builtin"));
        });
        if form.mode == BindingMode::Extension {
            let selected = form
                .provider
                .and_then(|i| snapshot.providers.get(i))
                .map(|p| format!("{} {}", p.name, p.version))
                .unwrap_or_else(|| "Choose an extension".into());
            let current = form.provider;
            egui::ComboBox::from_id_salt(format!("{base}:provider"))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (i, option) in snapshot.providers.iter().enumerate().filter(|(_, p)| p.hook == form.hook) {
                        let label = format!("{} {}", option.name, option.version);
                        if ui.selectable_label(current == Some(i), label).clicked() {
                            acts.push(Act::Provider(i));
                        }
                    }
                })
                .response
                .named(ui, format!("{base}:provider"));
            ui.add(egui::TextEdit::singleline(&mut form.version_req).hint_text("Version range (optional)"))
                .named(ui, format!("{base}:version"));
            for field in form.fields.iter_mut() {
                draw_field(ui, &base, field);
            }
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
        });
    });
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
