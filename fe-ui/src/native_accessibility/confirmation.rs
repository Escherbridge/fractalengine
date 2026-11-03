use super::shared;
use bevy_egui::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Owner {
    pub id: egui::Id,
    pub generation: u128,
}

#[derive(Clone, Copy)]
pub(crate) struct Confirmation {
    pub at: f64,
    lifetime: Option<u128>,
    owner: Owner,
    seen: u64,
    controls: Option<(egui::Id, egui::Id)>,
}

impl Confirmation {
    pub fn new(ctx: &egui::Context, id: egui::Id, at: f64) -> Self {
        Self {
            at,
            lifetime: shared(ctx)
                .and_then(|shared| shared.try_lock().ok().map(|pass| pass.lifetime)),
            owner: Owner {
                id,
                generation: ulid::Ulid::new().into(),
            },
            seen: ctx.cumulative_pass_nr(),
            controls: None,
        }
    }

    pub fn owner(&self) -> Owner {
        self.owner
    }

    pub fn reconcile(
        &mut self,
        ctx: &egui::Context,
        confirm: egui::Id,
        cancel: egui::Id,
    ) -> Option<bool> {
        let current = (confirm, cancel);
        if self.controls.is_some_and(|previous| previous != current) {
            ctx.data_mut(|data| data.remove::<Confirmation>(self.owner.id));
            return None;
        }
        let established = self.controls.is_some();
        self.controls.get_or_insert(current);
        self.seen = ctx.cumulative_pass_nr();
        Some(established)
    }
}

pub(crate) fn confirmation_state(ctx: &egui::Context, owner: egui::Id) -> Option<Confirmation> {
    let state = ctx.data(|data| data.get_temp::<Confirmation>(owner))?;
    let mut valid = state.seen.checked_add(1) == Some(ctx.cumulative_pass_nr());
    if let Some(shared) = shared(ctx) {
        let pass = shared.try_lock().ok()?;
        valid &= state.lifetime == Some(pass.lifetime)
            && !pass.semantic_click.is_some_and(|(target, expected_owner)| {
                expected_owner != Some(state.owner)
                    || !state.controls.is_some_and(|(confirm, cancel)| {
                        target.0 == confirm.value() || target.0 == cancel.value()
                    })
            });
    } else {
        valid &= state.lifetime.is_none();
    }
    if !valid {
        ctx.data_mut(|data| data.remove::<Confirmation>(owner));
        return None;
    }
    Some(state)
}
