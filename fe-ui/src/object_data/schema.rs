//! Versioned enforced descriptor editing with retained drafts and committed confirmation.

use bevy_egui::egui;
use fe_runtime::messages::{DbCommand, DbResult, FieldDefInfo};
use fe_runtime::node_history::NodeAccessGrant;
use serde_json::{json, Value};
use std::time::Instant;

use super::{button, error, json_view, request_error, send, Request, Target};
use crate::accessibility::ResponseExt;

struct SchemaRequest {
    request: Request,
    field_id: String,
    scope: String,
    expected: Option<Value>,
}

struct SchemaAttempt {
    descriptor: Value,
    submitted_text: String,
}

pub(crate) struct SchemaState {
    scope_index: usize,
    listed_scope: Option<String>,
    list_request: Option<Request>,
    definitions: Vec<FieldDefInfo>,
    selected: String,
    request: Option<SchemaRequest>,
    stored: Option<Value>,
    stored_json: String,
    loaded: bool,
    draft: String,
    dirty: bool,
    conflict: bool,
    needs_refresh: bool,
    attempt: Option<SchemaAttempt>,
    error: Option<String>,
    status: Option<String>,
}

impl Default for SchemaState {
    fn default() -> Self {
        Self {
            scope_index: 2,
            listed_scope: None,
            list_request: None,
            definitions: Vec::new(),
            selected: String::new(),
            request: None,
            stored: None,
            stored_json: String::new(),
            loaded: false,
            draft: String::new(),
            dirty: false,
            conflict: false,
            needs_refresh: false,
            attempt: None,
            error: None,
            status: None,
        }
    }
}

impl SchemaState {
    pub(crate) fn expire_requests(&mut self, now: Instant) {
        if self
            .list_request
            .as_ref()
            .is_some_and(|request| request.expired_at(now))
        {
            self.list_request = None;
            self.error = Some("Definition list timed out. Refresh to try again.".into());
        }
        if self
            .request
            .as_ref()
            .is_some_and(|pending| pending.request.expired_at(now))
        {
            self.request = None;
            self.needs_refresh = true;
            self.error = Some("Schema request timed out. Your draft is retained; refresh the stored descriptor before saving.".into());
        }
    }

    pub(crate) fn edit_definition(
        &mut self,
        field_id: &str,
        scope: &str,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) {
        self.selected = field_id.to_owned();
        self.listed_scope = Some(scope.to_owned());
        self.request = None;
        self.stored = None;
        self.loaded = false;
        self.dirty = false;
        self.conflict = false;
        self.needs_refresh = false;
        self.attempt = None;
        self.draft.clear();
        self.status = None;
        self.read(sender);
    }

    pub(crate) fn draft(&self) -> &str {
        &self.draft
    }

    pub(crate) fn replace_draft(&mut self, text: String) {
        self.draft = text;
        self.dirty = true;
        self.status = None;
    }

    fn list(&mut self, scope: String, sender: &crossbeam::channel::Sender<DbCommand>) {
        self.listed_scope = Some(scope.clone());
        self.error = None;
        match send(sender, DbCommand::ListFieldDefs { scope }) {
            Ok(()) => self.list_request = Some(Request::new("schema-list")),
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn note_definitions(&mut self, result: &DbResult) {
        if let DbResult::FieldDefsListed { scope, field_defs } = result {
            if self.list_request.is_some() && self.listed_scope.as_deref() == Some(scope) {
                self.list_request = None;
                self.definitions = field_defs
                    .iter()
                    .filter(|field| field.scope == *scope && field.entity_type == "node")
                    .take(256)
                    .cloned()
                    .collect();
            }
        }
    }

    pub(crate) fn read(&mut self, sender: &crossbeam::channel::Sender<DbCommand>) {
        let Some(scope) = self.listed_scope.clone() else {
            return;
        };
        let request = Request::new("schema-read");
        match send(
            sender,
            DbCommand::GetFieldSchema {
                field_def_id: self.selected.clone(),
                grant: NodeAccessGrant::Local,
                correlation_id: Some(request.id.clone()),
            },
        ) {
            Ok(()) => {
                self.request = Some(SchemaRequest {
                    request,
                    field_id: self.selected.clone(),
                    scope,
                    expected: None,
                });
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn reset_draft(&mut self) {
        let mut next = self.stored.clone().unwrap_or_else(|| json!({"version":0,"schema":{"type":"object","properties":{},"additionalProperties":true}}));
        if let Some(version) = next["version"]
            .as_u64()
            .and_then(|version| version.checked_add(1))
        {
            next["version"] = json!(version);
        }
        self.draft = serde_json::to_string_pretty(&next).unwrap_or_default();
        self.dirty = false;
        self.conflict = false;
    }

    pub(crate) fn save(&mut self, sender: &crossbeam::channel::Sender<DbCommand>) {
        if !self.loaded || self.request.is_some() || self.needs_refresh || self.conflict {
            self.error =
                Some("Read the current stored descriptor before saving this draft.".into());
            return;
        }
        let result = (|| {
            if self.draft.len() > fe_sdk::property_schema::MAX_PROPERTY_BYTES {
                return Err("Schema exceeds the 64 KiB limit.".to_owned());
            }
            let value: Value = serde_json::from_str(&self.draft)
                .map_err(|error| format!("Invalid descriptor JSON: {error}"))?;
            fe_sdk::property_schema::validate_persistable_numbers(&value)?;
            let descriptor = fe_sdk::property_schema::EnforcedPropertySchema::parse(&value)?;
            let expected = self
                .stored
                .as_ref()
                .and_then(|value| value["version"].as_u64())
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Schema revision is exhausted.")?;
            if u64::from(descriptor.version) != expected {
                return Err(format!("The next descriptor version must be {expected}."));
            }
            Ok(value)
        })();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let Some(scope) = self.listed_scope.clone() else {
            return;
        };
        let request = Request::new("schema-save");
        match send(
            sender,
            DbCommand::SetFieldSchema {
                field_def_id: self.selected.clone(),
                descriptor: value.clone(),
                grant: NodeAccessGrant::Local,
                correlation_id: Some(request.id.clone()),
            },
        ) {
            Ok(()) => {
                self.attempt = Some(SchemaAttempt {
                    descriptor: value.clone(),
                    submitted_text: self.draft.clone(),
                });
                self.request = Some(SchemaRequest {
                    request,
                    field_id: self.selected.clone(),
                    scope,
                    expected: Some(value),
                });
                self.error = None;
                self.status = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn route(&mut self, target: &Target, result: &DbResult) -> bool {
        if let DbResult::QueryResult {
            data,
            correlation_id: Some(id),
        } = result
        {
            if !self
                .request
                .as_ref()
                .is_some_and(|pending| pending.request.matches(id))
            {
                return false;
            }
            let pending = self.request.take().expect("matched request");
            let Some(row) = data.first().filter(|row| {
                data.len() == 1
                    && row["field_def_id"].as_str() == Some(pending.field_id.as_str())
                    && row["scope"].as_str() == Some(pending.scope.as_str())
                    && fe_database::scope_contains(&pending.scope, &target.scope)
            }) else {
                self.error =
                    Some("Schema response did not match the selected definition and scope.".into());
                self.needs_refresh = true;
                return true;
            };
            let stored = row
                .get("enforced_schema")
                .filter(|value| !value.is_null())
                .cloned();
            if pending
                .expected
                .as_ref()
                .is_some_and(|expected| stored.as_ref() != Some(expected))
            {
                self.error = Some(
                    "Schema write returned a different descriptor. Refresh before editing again."
                        .into(),
                );
                self.needs_refresh = true;
                return true;
            }
            let confirmed = self
                .attempt
                .as_ref()
                .is_some_and(|attempt| stored.as_ref() == Some(&attempt.descriptor));
            let draft_unchanged = self
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.submitted_text == self.draft);
            let conflict = self.loaded && self.dirty && self.stored != stored && !confirmed;
            self.stored_json = stored
                .as_ref()
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                .unwrap_or_default();
            self.stored = stored;
            self.loaded = true;
            self.needs_refresh = false;
            if confirmed {
                let revision = self
                    .stored
                    .as_ref()
                    .and_then(|value| value["version"].as_u64())
                    .unwrap_or(0);
                self.attempt = None;
                if draft_unchanged {
                    self.reset_draft();
                    self.status = Some(format!("Saved descriptor revision {revision}"));
                } else {
                    self.dirty = true;
                    self.conflict = false;
                    let next = revision
                        .checked_add(1)
                        .map_or_else(|| "unavailable".to_owned(), |version| version.to_string());
                    self.status = Some(format!("Saved descriptor revision {revision}. Your newer draft is retained; its next version must be {next}."));
                }
            } else if conflict {
                self.conflict = true;
                self.error = Some("Stored schema changed. Your draft is retained; load the stored rules before saving.".into());
            } else if !self.dirty {
                self.reset_draft();
            }
            return true;
        }
        let Some((id, message)) = request_error(result) else {
            return false;
        };
        if !self
            .request
            .as_ref()
            .is_some_and(|pending| pending.request.matches(id))
        {
            return false;
        }
        let pending = self.request.take().expect("matched request");
        self.needs_refresh |= pending.expected.is_some();
        self.error = Some(message.to_owned());
        true
    }

    pub(super) fn render(
        &mut self,
        ui: &mut egui::Ui,
        target: &Target,
        role: &crate::plugin::LocalUserRole,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) {
        self.expire_requests(Instant::now());
        ui.label("Enforced rules validate future property writes and reject incompatible schema revisions after checking current values.");
        ui.small("Legacy type/required/default metadata does not enforce a descriptor. Rules do not backfill values.");
        let previous_scope = self.scope_index;
        egui::ComboBox::from_id_salt("schema-definition-scope")
            .width(ui.available_width())
            .selected_text(
                [
                    "Verse definitions",
                    "Fractal definitions",
                    "Petal definitions",
                ][self.scope_index],
            )
            .show_ui(ui, |ui| {
                for (index, label) in [
                    "Verse definitions",
                    "Fractal definitions",
                    "Petal definitions",
                ]
                .iter()
                .enumerate()
                {
                    ui.selectable_value(&mut self.scope_index, index, *label)
                        .observed(ui);
                }
            })
            .response
            .named(ui, "Enforced schema definition scope");
        if previous_scope != self.scope_index {
            let scope_index = self.scope_index;
            *self = Self {
                scope_index,
                ..Default::default()
            };
        }
        let scope = &target.scopes[self.scope_index];
        if self.listed_scope.as_ref() != Some(scope) {
            self.list(scope.clone(), sender);
        }
        if button(
            ui,
            "Refresh definitions",
            "Refresh available schema definitions",
            self.list_request.is_none(),
        ) {
            self.list(scope.clone(), sender);
        }
        if self.list_request.is_some() {
            ui.label("Loading definitions...");
        }
        error(ui, &self.error);
        if self.definitions.is_empty() {
            ui.label("No object field definitions at this scope. For a new Petal definition, use Properties → Property Schema, then refresh Petal definitions here.");
            return;
        }
        let before = self.selected.clone();
        egui::ComboBox::from_id_salt("enforced-schema-definition")
            .width(ui.available_width())
            .selected_text(
                self.definitions
                    .iter()
                    .find(|field| field.field_def_id == self.selected)
                    .map(|field| field.key.as_str())
                    .unwrap_or("Choose a property definition"),
            )
            .show_ui(ui, |ui| {
                for field in &self.definitions {
                    ui.selectable_value(&mut self.selected, field.field_def_id.clone(), &field.key)
                        .named(ui, format_args!("Enforced schema for {}", field.key));
                }
            })
            .response
            .named(ui, "Object enforced schema definition");
        if before != self.selected {
            let selected = self.selected.clone();
            self.edit_definition(&selected, &target.scopes[self.scope_index], sender);
        }
        if self.selected.is_empty() {
            return;
        }
        if button(
            ui,
            "Refresh stored descriptor",
            "Refresh stored enforced schema",
            self.request.is_none(),
        ) {
            self.read(sender);
        }
        if self.request.is_some() {
            ui.label("Waiting for authoritative schema response...");
        }
        if !self.loaded {
            return;
        }
        match &self.stored {
            Some(stored) => {
                ui.label(format!("Current enforced revision: {}", stored["version"]));
            }
            None => {
                ui.label("Legacy definition: no enforced rules. Saving installs revision 1.");
            }
        }
        ui.small("Supported rules: type, nullable, enum, properties, required, additionalProperties, items, min/maxItems, min/maxLength, minimum, maximum. Limits: 64 KiB, depth 16, 256 rules. Unsupported keywords are rejected.");
        ui.small(format!(
            "Draft size: {} / {} bytes",
            self.draft().len(),
            fe_sdk::property_schema::MAX_PROPERTY_BYTES
        ));
        let can_edit = role.can_manage();
        ui.add_enabled_ui(can_edit && self.request.is_none(), |ui| {
            if ui
                .add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .code_editor()
                        .desired_rows(12)
                        .desired_width(ui.available_width())
                        .char_limit(65_536),
                )
                .named(ui, "Versioned enforced schema JSON")
                .changed()
            {
                self.replace_draft(self.draft.clone());
            }
        });
        if !can_edit {
            ui.label("Manager role is required at the definition's scope to save rules.");
        }
        if self.needs_refresh {
            ui.label("The previous outcome needs a stored readback before another save.");
        }
        if self.conflict
            && button(
                ui,
                "Load stored rules",
                "Discard schema draft and load stored rules",
                self.request.is_none(),
            )
        {
            self.reset_draft();
            self.error = None;
        }
        if button(
            ui,
            "Save next revision",
            "Save enforced schema next revision",
            can_edit
                && self.loaded
                && (self.dirty || self.stored.is_none())
                && self.request.is_none()
                && !self.conflict
                && !self.needs_refresh,
        ) {
            self.save(sender);
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
        if self.stored.is_some() {
            egui::CollapsingHeader::new("Stored descriptor")
                .show(ui, |ui| {
                    json_view(ui, "stored-schema-json", &self.stored_json)
                })
                .header_response
                .observed(ui);
        }
    }
}
