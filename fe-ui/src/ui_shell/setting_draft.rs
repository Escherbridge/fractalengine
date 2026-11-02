//! Scoped numeric-setting drafts and interaction commits; see AGENTS.md.

use bevy_egui::egui;

const ADJUSTMENT_KEYS: [egui::Key; 8] = [
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::Home,
    egui::Key::End,
    egui::Key::PageUp,
    egui::Key::PageDown,
];

#[derive(Clone)]
pub(crate) struct SettingDraft<T> {
    scope: String,
    pub value: T,
    stored: T,
    pending: Option<(T, f64)>,
    dirty: bool,
    ready: bool,
    editing: bool,
    active: bool,
    interactions: Vec<NumericInteraction>,
    error: Option<&'static str>,
}

#[derive(Clone)]
struct NumericInteraction {
    field: &'static str,
    changed: bool,
    pointer: bool,
    text: bool,
}

impl<T: Copy + PartialEq + Send + Sync + 'static> SettingDraft<T> {
    pub fn load(ui: &egui::Ui, id: egui::Id, scope: &str, stored: T) -> Self {
        let mut draft = ui
            .data(|data| data.get_temp::<Self>(id))
            .filter(|draft| draft.scope == scope)
            .unwrap_or_else(|| Self {
                scope: scope.to_owned(),
                value: stored,
                stored,
                pending: None,
                dirty: false,
                ready: false,
                editing: false,
                active: false,
                interactions: Vec::new(),
                error: None,
            });
        draft.reconcile(stored, ui.input(|input| input.time));
        draft.editing = false;
        draft.active = false;
        draft
    }

    fn reconcile(&mut self, stored: T, now: f64) {
        if let Some((submitted, at)) = self.pending {
            if stored == submitted {
                self.pending = None;
                self.error = None;
            } else if now - at >= 30.0 {
                self.pending = None;
                self.ready = false;
                self.error = Some("Save is unconfirmed. Your draft is kept; edit again to retry.");
            }
        } else if !self.dirty && !self.editing {
            self.value = stored;
        }
        self.stored = stored;
        if self.pending.is_none() && self.value == stored {
            self.dirty = false;
        }
    }

    pub fn observe(&mut self, ui: &egui::Ui, field: &'static str, response: &egui::Response) {
        let pointer_down = ui.input(|input| input.pointer.any_down());
        let index = self
            .interactions
            .iter()
            .position(|entry| entry.field == field)
            .unwrap_or_else(|| {
                self.interactions.push(NumericInteraction {
                    field,
                    changed: false,
                    pointer: false,
                    text: false,
                });
                self.interactions.len() - 1
            });
        let interaction = &mut self.interactions[index];
        if response.changed() {
            self.dirty = true;
            self.ready = false;
            self.error = None;
            interaction.changed = true;
            interaction.pointer |= pointer_down;
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if response.has_focus()
            && ui.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Key {
                                key: egui::Key::Backspace | egui::Key::Delete,
                                pressed: true,
                                ..
                            }
                    )
                })
            })
        {
            interaction.text = true;
        }
        if response.lost_focus() || (response.has_focus() && enter) {
            interaction.text = false;
        }
        let keyboard_release = response.has_focus()
            && ui.input(|input| {
                input.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, pressed: false, .. } if ADJUSTMENT_KEYS.contains(key))
                })
            });
        let keyboard_down = response.has_focus()
            && ui.input(|input| ADJUSTMENT_KEYS.iter().any(|key| input.key_down(*key)));
        let active = response.dragged()
            || response.is_pointer_button_down_on()
            || keyboard_down
            || interaction.text;
        self.active |= active;
        let finished = response.drag_stopped()
            || response.lost_focus()
            || keyboard_release
            || (response.has_focus() && enter)
            || (interaction.pointer && !pointer_down)
            || (response.changed() && !pointer_down && !response.has_focus());
        if interaction.changed && finished && !active {
            self.ready = true;
            interaction.changed = false;
            interaction.pointer = false;
        }
        self.editing |= response.has_focus() || response.dragged();
    }

    pub fn finish(&mut self) {
        self.dirty = true;
        self.ready = true;
        self.error = None;
    }

    pub fn next_value(&self) -> Option<T> {
        (self.ready
            && self.dirty
            && !self.active
            && self
                .interactions
                .iter()
                .all(|entry| !entry.changed && !entry.text)
            && self.pending.is_none()
            && self.value != self.stored)
            .then_some(self.value)
    }

    pub fn sent(&mut self, now: f64) {
        self.pending = Some((self.value, now));
        self.ready = false;
    }

    pub fn failed(&mut self) {
        self.error = Some("Could not send this change. Your draft is kept; edit again to retry.");
        self.ready = false;
    }

    pub fn store(self, ui: &mut egui::Ui, id: egui::Id) {
        if let Some(error) = self.error {
            ui.colored_label(crate::theme::STATUS_OFFLINE, error);
        } else if self.pending.is_some() {
            ui.label("Waiting for stored confirmation…");
        }
        ui.data_mut(|data| data.insert_temp(id, self));
    }
}
