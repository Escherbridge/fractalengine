use super::shared;
use bevy_egui::egui;

#[derive(Clone, Copy)]
pub(crate) struct Confirmation {
    pub at: f64,
    lifetime: Option<u128>,
    pub confirm: Option<egui::Id>,
    pub cancel: Option<egui::Id>,
}

impl Confirmation {
    pub fn new(ctx: &egui::Context, at: f64) -> Self {
        Self {
            at,
            lifetime: shared(ctx)
                .and_then(|shared| shared.try_lock().ok().map(|pass| pass.lifetime)),
            confirm: None,
            cancel: None,
        }
    }
}

pub(crate) fn confirmation_state(ctx: &egui::Context, owner: egui::Id) -> Option<Confirmation> {
    let state = ctx.data(|data| data.get_temp::<Confirmation>(owner))?;
    if let Some(shared) = shared(ctx) {
        let pass = shared.try_lock().ok()?;
        if state.lifetime != Some(pass.lifetime)
            || pass.semantic_click.is_some_and(|(target, expected_owner)| {
                expected_owner != Some(owner)
                    || (Some(target.0) != state.confirm.map(|id| id.value())
                        && Some(target.0) != state.cancel.map(|id| id.value()))
            })
        {
            ctx.data_mut(|data| data.remove::<Confirmation>(owner));
            return None;
        }
    } else if state.lifetime.is_some() {
        ctx.data_mut(|data| data.remove::<Confirmation>(owner));
        return None;
    }
    Some(state)
}
