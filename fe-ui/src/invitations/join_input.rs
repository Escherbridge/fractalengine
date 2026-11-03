//! Guarded real Join widget replay; see AGENTS.md section guarded Join input.

use bevy_egui::egui;
use fe_runtime::{
    input::{InputCommand, PointerButton},
    qa_invitation::{JoinInputLease, JoinInputWindow},
};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Controls {
    input: egui::Id,
    submit: egui::Id,
    input_rect: egui::Rect,
    submit_rect: egui::Rect,
    input_clip: egui::Rect,
    submit_clip: egui::Rect,
    input_layer: egui::LayerId,
    submit_layer: egui::LayerId,
    input_transform: egui::emath::TSTransform,
    submit_transform: egui::emath::TSTransform,
    ppp: f32,
    input_enabled: bool,
    submit_enabled: bool,
    submit_pointer_down: bool,
    layers_allowed: bool,
    focused: Option<egui::Id>,
    viewport: egui::ViewportId,
    frame: u64,
    pass: u64,
}
impl Controls {
    fn matches(&self, other: &Self) -> bool {
        self.input == other.input
            && self.submit == other.submit
            && self.input_rect == other.input_rect
            && self.submit_rect == other.submit_rect
            && self.input_clip == other.input_clip
            && self.submit_clip == other.submit_clip
            && self.input_layer == other.input_layer
            && self.submit_layer == other.submit_layer
            && self.input_transform == other.input_transform
            && self.submit_transform == other.submit_transform
            && self.viewport == other.viewport
            && self.ppp.to_bits() == other.ppp.to_bits()
    }
    fn physical(rect: egui::Rect, clip: egui::Rect, ppp: f32) -> Option<[f32; 4]> {
        let rect = rect.intersect(clip);
        (rect.is_finite() && rect.is_positive() && ppp.is_finite() && ppp > 0.0).then_some([
            rect.min.x * ppp,
            rect.min.y * ppp,
            rect.width() * ppp,
            rect.height() * ppp,
        ])
    }
    fn completed(&self, ctx: &egui::Context) -> bool {
        self.viewport == ctx.viewport_id()
            && self.frame.checked_add(1) == Some(ctx.cumulative_frame_nr())
            && self.pass.checked_add(1) == Some(ctx.cumulative_pass_nr())
            && self.focused == ctx.memory(|memory| memory.focused())
    }
    fn live(&self, ctx: &egui::Context) -> bool {
        self.ppp.to_bits() == ctx.pixels_per_point().to_bits()
            && self.input_enabled
            && self.layers_allowed
            && self.input_transform
                == ctx
                    .layer_transform_to_global(self.input_layer)
                    .unwrap_or_default()
            && self.submit_transform
                == ctx
                    .layer_transform_to_global(self.submit_layer)
                    .unwrap_or_default()
            && [self.input_layer, self.submit_layer]
                .into_iter()
                .all(|layer| ctx.memory(|memory| memory.allows_interaction(layer)))
    }
    fn dto(&self, id: String) -> Option<JoinInputLease> {
        Some(JoinInputLease {
            lease_id: id,
            input_widget: self.input.value().to_string(),
            join_widget: self.submit.value().to_string(),
            input_bounds: Self::physical(self.input_rect, self.input_clip, self.ppp)?,
            join_bounds: Self::physical(self.submit_rect, self.submit_clip, self.ppp)?,
        })
    }
}
struct Lease {
    id: String,
    recipient: String,
    nav: u64,
    dialog: u64,
    revision: u64,
    window: JoinInputWindow,
    controls: Controls,
    deadline: Instant,
    started: bool,
    failed: bool,
    chunks: Vec<String>,
    prefix: String,
    next_frame: usize,
    current_frame: Option<usize>,
    prepared_frame: Option<u64>,
    prepared_pass: Option<u64>,
    pending: Option<String>,
    inserted: bool,
    submitted: bool,
}
#[derive(Default)]
pub(crate) struct JoinInputState {
    controls: Option<Controls>,
    lease: Option<Lease>,
}

impl JoinInputState {
    pub fn invalidate(&mut self) {
        self.lease = None;
        self.controls = None;
    }
    pub fn record(
        &mut self,
        input: &egui::Response,
        submit: &egui::Response,
        input_clip: egui::Rect,
        submit_clip: egui::Rect,
    ) {
        let ctx = &input.ctx;
        let global = |layer| ctx.layer_transform_to_global(layer).unwrap_or_default();
        let controls = Controls {
            input: input.id,
            submit: submit.id,
            input_rect: global(input.layer_id) * input.rect,
            submit_rect: global(submit.layer_id) * submit.rect,
            input_clip: global(input.layer_id) * input_clip,
            submit_clip: global(submit.layer_id) * submit_clip,
            input_layer: input.layer_id,
            submit_layer: submit.layer_id,
            input_transform: global(input.layer_id),
            submit_transform: global(submit.layer_id),
            ppp: ctx.pixels_per_point(),
            input_enabled: input.enabled(),
            submit_enabled: submit.enabled(),
            submit_pointer_down: submit.is_pointer_button_down_on(),
            layers_allowed: ctx.memory(|memory| {
                memory.allows_interaction(input.layer_id)
                    && memory.allows_interaction(submit.layer_id)
            }),
            focused: ctx.memory(|memory| memory.focused()),
            viewport: ctx.viewport_id(),
            frame: ctx.cumulative_frame_nr(),
            pass: ctx.cumulative_pass_nr(),
        };
        if let Some(lease) = self.lease.as_mut() {
            if !lease.controls.matches(&controls) {
                lease.failed = true;
                lease.pending = None;
            }
        }
        self.controls = Some(controls);
    }
    pub fn validate_domain(&mut self, recipient: Option<&str>, nav: u64, dialog: u64) {
        if let Some(lease) = self.lease.as_mut() {
            if recipient != Some(lease.recipient.as_str())
                || nav != lease.nav
                || dialog != lease.dialog
            {
                lease.failed = true;
                lease.pending = None;
            }
        }
    }
    pub fn mint(
        &mut self,
        recipient: &str,
        nav: u64,
        dialog: u64,
        revision: u64,
        window: JoinInputWindow,
    ) -> Option<JoinInputLease> {
        if !window.focused {
            return None;
        }
        let controls = self.controls.as_ref()?;
        if !controls.input_enabled
            || !controls.layers_allowed
            || window.width == 0
            || window.height == 0
        {
            return None;
        }
        if let Some(lease) = self.lease.as_ref() {
            if lease.deadline > Instant::now()
                && !lease.failed
                && lease.recipient == recipient
                && lease.nav == nav
                && lease.dialog == dialog
                && lease.revision == revision
                && lease.window == window
                && lease.controls.matches(controls)
            {
                return lease.controls.dto(lease.id.clone());
            }
        }
        let id = format!("join-input-{}", ulid::Ulid::new());
        let dto = controls.dto(id.clone())?;
        self.lease = Some(Lease {
            id,
            recipient: recipient.into(),
            nav,
            dialog,
            revision,
            window,
            controls: controls.clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            started: false,
            failed: false,
            chunks: Vec::new(),
            prefix: String::new(),
            next_frame: 0,
            current_frame: None,
            prepared_frame: None,
            prepared_pass: None,
            pending: None,
            inserted: false,
            submitted: false,
        });
        Some(dto)
    }
    pub fn admit(
        &mut self,
        id: &str,
        recipient: &str,
        nav: u64,
        dialog: u64,
        revision: u64,
        buffer: &str,
        window: &JoinInputWindow,
        commands: &[InputCommand],
        wait: u32,
        capture: bool,
    ) -> bool {
        let Some(lease) = self.lease.as_mut() else {
            return false;
        };
        if lease.id != id
            || lease.started
            || lease.failed
            || lease.deadline <= Instant::now()
            || lease.recipient != recipient
            || lease.nav != nav
            || lease.dialog != dialog
            || lease.revision != revision
            || lease.window != *window
            || !buffer.is_empty()
            || capture
            || wait != 0
            || commands.len() < 3
        {
            return false;
        }
        let Some(dto) = lease.controls.dto(lease.id.clone()) else {
            return false;
        };
        let inside = |command: &InputCommand, bounds: [f32; 4]| matches!(command,InputCommand::PointerClick {x,y,button:PointerButton::Left} if *x>=bounds[0] && *x<bounds[0]+bounds[2] && *y>=bounds[1] && *y<bounds[1]+bounds[3]);
        if !inside(&commands[0], dto.input_bounds)
            || !inside(&commands[commands.len() - 1], dto.join_bounds)
        {
            return false;
        }
        let mut size = 0usize;
        let mut chunks = Vec::new();
        for command in &commands[1..commands.len() - 1] {
            let InputCommand::TextEntry { text } = command else {
                return false;
            };
            size = size.saturating_add(text.len());
            if text.is_empty() || !text.is_ascii() || size > 64 * 1024 {
                return false;
            }
            chunks.push(text.clone());
        }
        if !chunks
            .first()
            .is_some_and(|text| text.starts_with("fractalengine://join/"))
        {
            return false;
        }
        lease.started = true;
        lease.chunks = chunks;
        true
    }
    pub fn prepare(
        &mut self,
        id: &str,
        index: usize,
        recipient: &str,
        nav: u64,
        dialog: u64,
        revision: u64,
        buffer: &str,
        window: &JoinInputWindow,
        ctx: &egui::Context,
    ) -> bool {
        let Some(lease) = self.lease.as_mut() else {
            return false;
        };
        if lease.id != id
            || !lease.started
            || lease.failed
            || lease.deadline <= Instant::now()
            || lease.recipient != recipient
            || lease.nav != nav
            || lease.dialog != dialog
            || lease.revision != revision
            || lease.prefix != buffer
            || lease.window != *window
            || !window.focused
            || lease.next_frame != index
            || lease.pending.is_some()
        {
            return false;
        }
        let Some(controls) = self.controls.as_ref() else {
            return false;
        };
        if !lease.controls.matches(controls)
            || !controls.completed(ctx)
            || !controls.live(ctx)
            || lease
                .prepared_frame
                .is_some_and(|frame| controls.frame != frame)
        {
            return false;
        }
        let focus = ctx.memory(|memory| memory.focused());
        let submit_press = lease.chunks.len() + 2;
        if index >= submit_press && !controls.submit_enabled {
            return false;
        }
        if (1..=submit_press).contains(&index) && focus != Some(lease.controls.input) {
            return false;
        }
        if index == submit_press + 1
            && focus != Some(lease.controls.input)
            && focus != Some(lease.controls.submit)
            && !(focus.is_none() && controls.submit_pointer_down)
        {
            return false;
        }
        if index > submit_press + 1 {
            return false;
        }
        lease.current_frame = Some(index);
        lease.prepared_frame = Some(ctx.cumulative_frame_nr());
        lease.prepared_pass = Some(ctx.cumulative_pass_nr());
        lease.next_frame = index + 1;
        lease.inserted = false;
        if (2..submit_press).contains(&index) {
            lease.pending = lease.chunks.get(index - 2).cloned();
        }
        true
    }
    pub fn begin_event(&self, ctx: &egui::Context) -> Option<String> {
        let lease = self.lease.as_ref()?;
        if lease.failed
            || lease.deadline <= Instant::now()
            || lease.prepared_frame != Some(ctx.cumulative_frame_nr())
            || lease.prepared_pass != Some(ctx.cumulative_pass_nr())
        {
            return None;
        }
        let text = lease.pending.clone()?;
        ctx.input_mut(|input| input.events.push(egui::Event::Text(text.clone())));
        Some(text)
    }
    pub fn insert_allowed(
        &mut self,
        ctx: &egui::Context,
        text: &str,
        index: usize,
        buffer: &str,
        clip: egui::Rect,
    ) -> bool {
        let Some(lease) = self.lease.as_mut() else {
            return true;
        };
        if !lease.started {
            return true;
        }
        let valid = !lease.failed
            && lease.prepared_frame == Some(ctx.cumulative_frame_nr())
            && lease.prepared_pass == Some(ctx.cumulative_pass_nr())
            && ctx.input(|input| input.focused)
            && ctx
                .layer_transform_to_global(lease.controls.input_layer)
                .unwrap_or_default()
                * clip
                == lease.controls.input_clip
            && !lease.inserted
            && lease.deadline > Instant::now()
            && lease.pending.as_deref() == Some(text)
            && lease.prefix == buffer
            && index == buffer.chars().count()
            && ctx.memory(|memory| memory.focused()) == Some(lease.controls.input)
            && ctx
                .read_response(lease.controls.input)
                .is_some_and(|response| {
                    response.enabled()
                        && response.layer_id == lease.controls.input_layer
                        && ctx
                            .layer_transform_to_global(response.layer_id)
                            .unwrap_or_default()
                            * response.rect
                            == lease.controls.input_rect
                        && ctx.pixels_per_point().to_bits() == lease.controls.ppp.to_bits()
                        && ctx.memory(|memory| memory.allows_interaction(response.layer_id))
                });
        if !valid {
            lease.failed = true;
            return false;
        }
        lease.prefix.push_str(text);
        lease.inserted = true;
        true
    }
    pub fn delete_allowed(&mut self, range: &std::ops::Range<usize>) -> bool {
        if let Some(lease) = self.lease.as_mut() {
            if lease.started && !range.is_empty() {
                lease.failed = true;
                return false;
            }
        }
        true
    }
    pub fn end_event(&mut self, ctx: &egui::Context, event: Option<String>, buffer: &str) {
        if let Some(text) = event {
            ctx.input_mut(|input| {
                input
                    .events
                    .retain(|event| !matches!(event,egui::Event::Text(value) if value==&text))
            });
            if let Some(lease) = self.lease.as_mut() {
                if !lease.inserted || lease.prefix != buffer {
                    lease.failed = true;
                }
                lease.pending = None;
            }
        }
    }
    pub fn revision_changed(&mut self, revision: u64) {
        if let Some(lease) = self.lease.as_mut() {
            if lease.started && lease.inserted && revision == lease.revision.wrapping_add(1) {
                lease.revision = revision;
            } else {
                lease.failed = true;
                lease.pending = None;
            }
        }
    }
    pub fn submit_allowed(
        &mut self,
        response: &egui::Response,
        buffer: &str,
        clip: egui::Rect,
    ) -> bool {
        let Some(lease) = self.lease.as_mut() else {
            return true;
        };
        if !lease.started {
            return true;
        }
        let valid = !lease.failed
            && lease.prepared_frame == Some(response.ctx.cumulative_frame_nr())
            && lease.prepared_pass == Some(response.ctx.cumulative_pass_nr())
            && lease.deadline > Instant::now()
            && response.ctx.input(|input| input.focused)
            && response.ctx.memory(|memory| memory.focused()).map_or_else(
                || response.clicked_by(egui::PointerButton::Primary),
                |id| id == lease.controls.input || id == lease.controls.submit,
            )
            && response
                .ctx
                .layer_transform_to_global(response.layer_id)
                .unwrap_or_default()
                * clip
                == lease.controls.submit_clip
            && lease.current_frame == Some(lease.chunks.len() + 3)
            && !lease.submitted
            && lease.prefix == buffer
            && response.id == lease.controls.submit
            && response.layer_id == lease.controls.submit_layer
            && response.enabled()
            && response
                .ctx
                .layer_transform_to_global(response.layer_id)
                .unwrap_or_default()
                * response.rect
                == lease.controls.submit_rect
            && response
                .ctx
                .memory(|memory| memory.allows_interaction(response.layer_id));
        if valid {
            lease.submitted = true;
        } else {
            lease.failed = true;
        }
        valid
    }
    pub fn completed(&self, id: &str) -> bool {
        self.lease.as_ref().is_some_and(|lease| {
            lease.id == id
                && !lease.failed
                && lease.submitted
                && lease.pending.is_none()
                && lease.deadline > Instant::now()
        })
    }
    pub fn finish(&mut self, id: &str) {
        if self.lease.as_ref().is_some_and(|lease| lease.id == id) {
            self.lease = None;
        }
    }
}
