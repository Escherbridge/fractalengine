//! Scoped numeric-setting drafts and interaction commits; see AGENTS.md.

use bevy_egui::egui;

#[derive(Clone)]
pub(super) struct SettingDraft<T> {
    scope: String,
    pub value: T,
    stored: T,
    pending: Option<(T, f64)>,
    ready: bool,
    editing: bool,
    pointer_edit: bool,
    error: Option<&'static str>,
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
                ready: false,
                editing: false,
                pointer_edit: false,
                error: None,
            });
        draft.reconcile(stored, ui.input(|input| input.time));
        draft.editing = false;
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
        } else if self.value == self.stored && !self.editing {
            self.value = stored;
        }
        self.stored = stored;
    }

    pub fn observe(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let pointer_down = ui.input(|input| input.pointer.any_down());
        if response.changed() {
            self.ready = false;
            self.error = None;
            self.pointer_edit |= pointer_down;
            if !pointer_down && !response.has_focus() {
                self.ready = true;
            }
        }
        let keyboard_release = response.has_focus()
            && ui.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::ArrowLeft
                                | egui::Key::ArrowRight
                                | egui::Key::ArrowUp
                                | egui::Key::ArrowDown
                                | egui::Key::Home
                                | egui::Key::End
                                | egui::Key::PageUp
                                | egui::Key::PageDown,
                            pressed: false,
                            ..
                        }
                    )
                })
            });
        if response.drag_stopped()
            || response.lost_focus()
            || keyboard_release
            || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            || (self.pointer_edit && !pointer_down)
        {
            self.ready = true;
            self.pointer_edit = false;
        }
        self.editing |= response.has_focus() || response.dragged();
    }

    pub fn finish(&mut self) {
        self.ready = true;
        self.error = None;
    }

    pub fn next_value(&self) -> Option<T> {
        (self.ready && self.pending.is_none() && self.value != self.stored).then_some(self.value)
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
