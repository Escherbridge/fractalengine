use super::Handler;
use bevy::prelude::*;
use bevy_egui::egui::{self, accesskit as ak};
use fe_ui::native_accessibility::{self as ui, Guard, Snapshot};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

const CAPACITY: usize = 32;
const MAX_AGE: Duration = Duration::from_millis(250);

struct Owner {
    window: Entity,
    context: Entity,
    egui: egui::Context,
    handler: Handler,
}

struct Pending {
    request: ak::ActionRequest,
    guard: Guard,
    lifetime: u128,
    queued_at: Instant,
}

#[derive(Resource, Default)]
pub(super) struct Adapter {
    owner: Option<Owner>,
    snapshot: Option<Snapshot>,
    pending: VecDeque<Pending>,
    delivered_pass: Option<u64>,
    contention: u8,
    discard_ingress: bool,
}

impl Adapter {
    pub fn owns_context(&self, entity: Entity) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|owner| owner.context == entity)
    }
    pub fn owns_window(&self, entity: Entity) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|owner| owner.window == entity)
    }

    pub fn clear(&mut self) {
        if let Some(owner) = self.owner.take() {
            ui::retire(&owner.egui);
        }
        self.snapshot = None;
        self.pending.clear();
        self.delivered_pass = None;
        self.contention = 0;
        self.discard_ingress = true;
    }

    pub fn discard_pending(&mut self) {
        self.pending.clear();
    }

    pub fn bind(
        &mut self,
        window: Entity,
        context: Entity,
        egui: &egui::Context,
        handler: &Handler,
    ) {
        if self.owner.as_ref().is_some_and(|owner| {
            owner.window == window
                && owner.context == context
                && owner.egui == *egui
                && Arc::ptr_eq(&owner.handler, handler)
        }) {
            return;
        }
        self.clear();
        ui::activate(egui);
        self.owner = Some(Owner {
            window,
            context,
            egui: egui.clone(),
            handler: handler.clone(),
        });
    }

    pub fn admit(&mut self, snapshot: Option<Snapshot>, handler: &Handler, now: Instant) {
        let mut handler = match handler.try_lock() {
            Ok(handler) => {
                self.contention = 0;
                handler
            }
            Err(_) => {
                self.contention = self.contention.saturating_add(1);
                self.discard_pending();
                self.snapshot = None;
                self.discard_ingress = true;
                if self.contention >= 2 {
                    tracing::warn!("native accessibility queue unavailable; actions refused");
                }
                return;
            }
        };
        let Some(snapshot) = snapshot else {
            self.snapshot = None;
            self.pending.clear();
            handler.clear();
            self.discard_ingress = true;
            return;
        };
        let changed = self.snapshot.as_ref().is_some_and(|old| {
            old.guard() != snapshot.guard() || old.lifetime() != snapshot.lifetime()
        });
        if self.discard_ingress || changed {
            handler.clear();
            self.pending.clear();
            self.snapshot = Some(snapshot);
            self.discard_ingress = false;
            tracing::debug!(
                "native accessibility fresh output barrier discarded uncertain ingress"
            );
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.pass() >= snapshot.pass())
        {
            self.pending.clear();
            handler.clear();
            self.snapshot = None;
            self.discard_ingress = true;
            return;
        }
        if handler.len() > CAPACITY || self.pending.len().saturating_add(handler.len()) > CAPACITY {
            self.pending.clear();
            handler.clear();
            self.snapshot = None;
            self.discard_ingress = true;
            tracing::warn!("native accessibility queue overflow; batch refused");
            return;
        }
        let before = self.pending.len();
        self.pending.retain(|pending| {
            pending.guard == snapshot.guard()
                && pending.lifetime == snapshot.lifetime()
                && now.saturating_duration_since(pending.queued_at) < MAX_AGE
                && snapshot.allows(&pending.request)
        });
        let mut refused = before - self.pending.len();
        while let Some(request) = handler.pop_front() {
            if snapshot.allows(&request) {
                self.pending.push_back(Pending {
                    request,
                    guard: snapshot.guard(),
                    lifetime: snapshot.lifetime(),
                    queued_at: now,
                });
            } else {
                refused += 1;
            }
        }
        if refused > 0 {
            tracing::debug!(
                refused,
                "native accessibility stale or unsupported actions refused"
            );
        }
        self.snapshot = Some(snapshot);
    }

    pub fn next(
        &mut self,
        ctx: &egui::Context,
        guard: Guard,
        now: Instant,
    ) -> Option<ak::ActionRequest> {
        let snapshot = self.snapshot.as_ref()?;
        if snapshot.guard() != guard {
            self.pending.clear();
            return None;
        }
        if snapshot.pass() != ctx.cumulative_pass_nr()
            || self.delivered_pass == Some(snapshot.pass())
        {
            return None;
        }
        while let Some(pending) = self.pending.pop_front() {
            if pending.guard != guard
                || pending.lifetime != snapshot.lifetime()
                || now.saturating_duration_since(pending.queued_at) >= MAX_AGE
                || !snapshot.allows(&pending.request)
            {
                continue;
            }
            if !ui::stage(ctx, snapshot, &pending.request) {
                self.pending.clear();
                return None;
            }
            self.delivered_pass = Some(snapshot.pass());
            return Some(pending.request);
        }
        None
    }
}
