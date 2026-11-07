//! Input wire contract — synthetic-input injection request/reply types
//! shared by the API and Bevy threads. See src/AGENTS.md §input-bridge.

use serde::{Deserialize, Serialize};

use crate::bridge::{bounded_bridge, BridgeReceiver, BridgeReply, BridgeRequest, BridgeSender};

// ---------------------------------------------------------------------------
// Bridge capacity + channel
// ---------------------------------------------------------------------------

/// Bounded depth of the input request channel; a full channel is a busy
/// refusal, never a blocked async worker (mirrors `vision_bridge`).
pub const INPUT_BRIDGE_CAPACITY: usize = 4;

pub type InputRequestSender = BridgeSender<InputOp, InputOutcome>;
pub type InputRequestReceiver = BridgeReceiver<InputOp, InputOutcome>;

/// A fresh bounded request channel (API holds the sender, the Bevy host the receiver).
pub fn input_bridge() -> (InputRequestSender, InputRequestReceiver) {
    bounded_bridge(INPUT_BRIDGE_CAPACITY)
}

// ---------------------------------------------------------------------------
// Static validation constants (agent_input_surface_20261031 §4.2, §6 OD-3)
// ---------------------------------------------------------------------------

/// A documented cap on one request's batch size — bounds Bevy-side
/// per-request work (there is no spec-given limit; this is the input
/// surface's own structural guard, same spirit as vision's 4 KiB body cap).
pub const MAX_COMMANDS_PER_REQUEST: usize = 64;
/// `KeyCode` variant names (`"Enter"`, `"KeyA"`, `"ArrowLeft"`) never exceed this.
pub const MAX_KEY_LEN: usize = 32;
/// A generous single-paste bound; bigger than this is almost certainly a mistake.
pub const MAX_TEXT_LEN: usize = 4096;
/// OD-3: past this the reply is a `wait_frames_out_of_range` refusal, never a
/// held correlation slot — ~2s at 60 fps.
pub const WAIT_FRAMES_MAX: u32 = 120;
pub const DEFAULT_WAIT_FRAMES: u32 = 0;
pub const DEFAULT_CAPTURE_AFTER: bool = false;

// ---------------------------------------------------------------------------
// Wire types (the POST body / MCP `inject_input` arguments; AIS spec §4.2)
// ---------------------------------------------------------------------------

/// Primary-window pixel coordinates (AIS OD-4) — the same space a prior
/// screenshot's own PNG dimensions already give an agent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

/// One synthetic key, spelled exactly as the matching
/// `bevy::input::keyboard::KeyCode` variant name (e.g. `"Enter"`, `"KeyA"`,
/// `"ArrowLeft"`, `"ShiftLeft"`). A `String`, not `bevy::input::keyboard::KeyCode`
/// itself — this workspace does not enable bevy_input's `serialize` feature
/// (see src/AGENTS.md §input-bridge); the name is mapped to the real enum
/// Bevy-side, where that feature-gated `impl` is unnecessary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InputKey(pub String);

/// The v1 command vocabulary (AIS spec §4.2), each variant translated
/// Bevy-side into the same resources real OS input populates.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputCommand {
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerClick {
        x: f32,
        y: f32,
        button: PointerButton,
    },
    PointerDrag {
        from: PointerPoint,
        to: PointerPoint,
        button: PointerButton,
    },
    KeyPress {
        key: InputKey,
    },
    TextEntry {
        text: String,
    },
    Scroll {
        dx: f32,
        dy: f32,
    },
}

impl std::fmt::Debug for InputCommand {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PointerMove {x,y} => f.debug_struct("PointerMove").field("x",x).field("y",y).finish(),
            Self::PointerClick {x,y,button} => f.debug_struct("PointerClick").field("x",x).field("y",y).field("button",button).finish(),
            Self::PointerDrag {from,to,button} => f.debug_struct("PointerDrag").field("from",from).field("to",to).field("button",button).finish(),
            Self::KeyPress {key} => f.debug_struct("KeyPress").field("key",key).finish(),
            Self::TextEntry {..} => f.write_str("TextEntry([redacted])"),
            Self::Scroll {dx,dy} => f.debug_struct("Scroll").field("dx",dx).field("dy",dy).finish(),
        }
    }
}

/// `POST /api/v1/debug/input` request body (and the MCP `inject_input`
/// tool's arguments). `validate()` is the ONLY constructor of
/// `ValidInputSpec` — Bevy-side code never re-checks finiteness or bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    pub commands: Vec<InputCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_after: Option<bool>,
}

/// Stable code of the uniform scope denial: an out-of-scope target and a
/// nonexistent one answer identically (fe-api AGENTS.md §existence-oracle).
pub const DENIED_CODE: &str = "not_found_or_denied";
/// Message of the uniform scope denial — byte-identical to fe-api's
/// `rest::NOT_FOUND_OR_DENIED` (asserted there) and to `vision::DENIED_MESSAGE`.
pub const DENIED_MESSAGE: &str = "not found or not permitted";

/// Stable code of "a posed capture currently holds the camera"
/// (INPUT-ADDENDUM item 4) — mapped to 409 by fe-api, exactly like vision's
/// own `capture_in_progress`.
pub const CAMERA_LOCKED_BY_CAPTURE_CODE: &str = "camera_locked_by_capture";

/// Stable code of a command that would inject nothing (INPUT-ADDENDUM item 7):
/// a typed 400 from `validate()`, and the Bevy planner's own backstop refusal.
pub const NO_EFFECTIVE_INPUT_CODE: &str = "no_effective_input";

/// A typed 400: `code` is the stable machine string, `field` is the
/// batch-index-scoped wire path (`commands[2].text`) when the check is
/// field-scoped. Unlike `vision::VisionRefusal`, `field` is an owned
/// `String`: a bad command's path depends on its position in the batch, so
/// it cannot be a fixed `&'static str` known at compile time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRefusal {
    pub code: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
}

impl InputRefusal {
    fn new(code: &'static str, field: Option<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            field,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validated spec (the newtype `validate()` alone constructs)
// ---------------------------------------------------------------------------

/// An `InputSpec` that has passed every structural check below. No public
/// constructor besides [`InputSpec::validate`] — the "validate once" newtype
/// (mirrors `vision::ValidCaptureSpec`): a NaN, an unmapped key or an
/// unbounded batch can never cross the bridge because nothing else can build
/// one of these.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidInputSpec {
    commands: Vec<InputCommand>,
    wait_frames: u32,
    capture_after: bool,
    join_lease: Option<String>,
    progress: InputProgress,
}

/// Live progress of one batch: the Bevy host writes it as frames land, the API
/// reads it when its reply deadline fires — see src/AGENTS.md §input-bridge
/// "Truthful late replies". Clones share one record.
#[derive(Debug, Clone, Default)]
pub struct InputProgress(std::sync::Arc<ProgressCells>);

#[derive(Debug, Default)]
struct ProgressCells {
    admitted: std::sync::atomic::AtomicBool,
    frames_planned: std::sync::atomic::AtomicU32,
    frames_applied: std::sync::atomic::AtomicU32,
    frames_settled: std::sync::atomic::AtomicU32,
}

/// Progress is an observation channel, not part of a spec's identity.
impl PartialEq for InputProgress {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// How far a batch got, read once by the API after its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStage {
    /// The host never admitted it; once the caller hangs up it never will.
    NotStarted,
    /// Some planned input frames were written before the window stalled.
    PartiallyApplied { applied: u32, planned: u32 },
    /// Every planned frame was written; only settle/capture was outstanding.
    Applied { settled_frames: u32 },
}

impl InputProgress {
    /// Host, at admission: the batch plans `frames` input frames.
    pub fn admit(&self, frames: usize) {
        use std::sync::atomic::Ordering;
        let planned = u32::try_from(frames).unwrap_or(u32::MAX);
        self.0.frames_planned.store(planned, Ordering::Release);
        self.0.admitted.store(true, Ordering::Release);
    }
    /// Host: one planned input frame was written to the input messages.
    pub fn frame_applied(&self) {
        self.0.frames_applied.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    /// Host: one `wait_frames` settle frame elapsed.
    pub fn frame_settled(&self) {
        self.0.frames_settled.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    /// API: the stage this batch reached.
    pub fn stage(&self) -> InputStage {
        use std::sync::atomic::Ordering;
        if !self.0.admitted.load(Ordering::Acquire) {
            return InputStage::NotStarted;
        }
        let planned = self.0.frames_planned.load(Ordering::Acquire);
        let applied = self.0.frames_applied.load(Ordering::Acquire);
        if applied < planned {
            InputStage::PartiallyApplied { applied, planned }
        } else {
            InputStage::Applied {
                settled_frames: self.0.frames_settled.load(Ordering::Acquire),
            }
        }
    }
}

impl ValidInputSpec {
    pub fn progress(&self) -> &InputProgress {
        &self.progress
    }
    pub fn join_lease(&self) -> Option<&str> {
        self.join_lease.as_deref()
    }
    pub fn require_join_lease(&mut self, lease: &str) -> Result<(), InputRefusal> {
        if lease.len() > 64
            || !lease.starts_with("join-input-")
            || !lease.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(InputRefusal::new(
                "invalid_join_lease",
                None,
                "Invalid guarded Join lease",
            ));
        }
        self.join_lease = Some(lease.into());
        Ok(())
    }

    pub fn commands(&self) -> &[InputCommand] {
        &self.commands
    }
    pub fn wait_frames(&self) -> u32 {
        self.wait_frames
    }
    pub fn capture_after(&self) -> bool {
        self.capture_after
    }
}

impl InputSpec {
    /// The ONLY constructor of [`ValidInputSpec`]. Runs every check **in
    /// table order** — the first failure wins, and that order is part of
    /// the documented contract (mirrors `vision::CaptureSpec::validate`'s
    /// doctrine): `no_commands`/`too_many_commands` beat the per-command
    /// checks (in batch order) beat `wait_frames_out_of_range`.
    pub fn validate(self) -> Result<ValidInputSpec, InputRefusal> {
        if self.commands.is_empty() {
            return Err(InputRefusal::new(
                "no_commands",
                Some("commands".to_string()),
                "`commands` must contain at least one command",
            ));
        }
        if self.commands.len() > MAX_COMMANDS_PER_REQUEST {
            return Err(InputRefusal::new(
                "too_many_commands",
                Some("commands".to_string()),
                format!(
                    "`commands` must have at most {MAX_COMMANDS_PER_REQUEST} entries, got {}",
                    self.commands.len()
                ),
            ));
        }

        for (index, command) in self.commands.iter().enumerate() {
            check_command(index, command)?;
        }

        let wait_frames = match self.wait_frames {
            Some(w) if w > WAIT_FRAMES_MAX => {
                return Err(InputRefusal::new(
                    "wait_frames_out_of_range",
                    Some("wait_frames".to_string()),
                    format!("`wait_frames` must be <= {WAIT_FRAMES_MAX}, got {w}"),
                ))
            }
            Some(w) => w,
            None => DEFAULT_WAIT_FRAMES,
        };
        let capture_after = self.capture_after.unwrap_or(DEFAULT_CAPTURE_AFTER);

        Ok(ValidInputSpec {
            commands: self.commands,
            wait_frames,
            capture_after,
            join_lease: None,
            progress: InputProgress::default(),
        })
    }
}

fn field_path(index: usize, suffix: &str) -> String {
    format!("commands[{index}].{suffix}")
}

fn finite(v: f32, field: String) -> Result<(), InputRefusal> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(InputRefusal::new(
            "non_finite",
            Some(field.clone()),
            format!("`{field}` must be a finite number, got {v}"),
        ))
    }
}

fn check_key(key: &InputKey, field: String) -> Result<(), InputRefusal> {
    let name = &key.0;
    let well_formed = !name.is_empty()
        && name.len() <= MAX_KEY_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    if well_formed {
        Ok(())
    } else {
        Err(InputRefusal::new(
            "invalid_key",
            Some(field.clone()),
            format!(
                "`{field}` must be a non-empty ASCII alphanumeric KeyCode name of at most \
                 {MAX_KEY_LEN} characters, got {name:?}"
            ),
        ))
    }
}

/// Rows 3a-3f of `validate()`'s table: the per-command structural checks,
/// scoped to the command's index in the batch (plan I1 doctrine, AIS §4.2).
fn check_command(index: usize, command: &InputCommand) -> Result<(), InputRefusal> {
    match command {
        InputCommand::PointerMove { x, y } => {
            finite(*x, field_path(index, "x"))?;
            finite(*y, field_path(index, "y"))?;
        }
        InputCommand::PointerClick { x, y, .. } => {
            finite(*x, field_path(index, "x"))?;
            finite(*y, field_path(index, "y"))?;
        }
        InputCommand::PointerDrag { from, to, .. } => {
            finite(from.x, field_path(index, "from.x"))?;
            finite(from.y, field_path(index, "from.y"))?;
            finite(to.x, field_path(index, "to.x"))?;
            finite(to.y, field_path(index, "to.y"))?;
        }
        InputCommand::KeyPress { key } => {
            check_key(key, field_path(index, "key"))?;
        }
        InputCommand::TextEntry { text } => {
            // INPUT-ADDENDUM item 7: empty text plans to zero input frames.
            if text.is_empty() {
                return Err(InputRefusal::new(
                    NO_EFFECTIVE_INPUT_CODE,
                    Some(field_path(index, "text")),
                    "`text` must not be empty — an empty text_entry injects nothing",
                ));
            }
            let len = text.chars().count();
            if len > MAX_TEXT_LEN {
                return Err(InputRefusal::new(
                    "text_too_long",
                    Some(field_path(index, "text")),
                    format!("`text` must be at most {MAX_TEXT_LEN} characters, got {len}"),
                ));
            }
        }
        InputCommand::Scroll { dx, dy } => {
            finite(*dx, field_path(index, "dx"))?;
            finite(*dy, field_path(index, "dy"))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bridge request/reply (the shared §correlated-bridge envelope; AGENTS.md
// §input-bridge says why it is a separate CHANNEL from vision/screenshot)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum InputOp {
    Inject(ValidInputSpec),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFailureClass {
    /// A typed dynamic refusal (none expected past `ValidInputSpec` today,
    /// kept for parity with `vision::VisionFailureClass`) — maps to 422.
    Unresolvable,
    /// A posed capture already holds the camera (`CameraCaptureLock`) — maps to 409.
    Busy,
    /// No window to inject into (headless/relay) — maps to 503.
    NoWindow,
    /// Any other Bevy-side failure — maps to 500/504.
    Failed,
    /// The caller's scope does not cover the active verse at drain or
    /// injection time (check-at-use, INPUT-ADDENDUM item 2) — one uniform
    /// 404 ([`denied`]), identical to a nonexistent target.
    Denied,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputFailure {
    pub status_class: InputFailureClass,
    pub code: &'static str,
    pub message: String,
}

impl InputFailure {
    /// The one constructor every side (fe-api transport mapping, the Bevy
    /// orchestrator) builds a typed failure through.
    pub fn new(
        status_class: InputFailureClass,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status_class,
            code,
            message: message.into(),
        }
    }
}

/// The uniform scope/existence denial (no existence oracle) — mirrors
/// `vision::denied()`.
pub fn denied() -> InputFailure {
    InputFailure::new(InputFailureClass::Denied, DENIED_CODE, DENIED_MESSAGE)
}

/// "A posed capture currently holds the camera" (INPUT-ADDENDUM item 4) —
/// the Bevy side returns this while `CameraCaptureLock` exists; fe-api maps
/// it to 409, exactly like vision's own `capture_in_progress`.
pub fn camera_locked_by_capture() -> InputFailure {
    InputFailure::new(
        InputFailureClass::Busy,
        CAMERA_LOCKED_BY_CAPTURE_CODE,
        "a posed capture currently holds the camera — the input surface is refused until it \
         completes",
    )
}

/// What a successful injection produced: how many commands were applied,
/// how many frames were actually waited, and the optional post-action PNG
/// (`capture_after`) — captured through the same `capture_png` helper the
/// vision/screenshot bridges own (plan I4).
#[derive(Debug, Clone, PartialEq)]
pub struct InputOutput {
    pub commands_applied: u32,
    pub wait_frames_used: u32,
    pub png: Option<Vec<u8>>,
    /// False: every input frame was applied, but the settle (and any
    /// `capture_after`) did not finish in time — no PNG, re-read state.
    pub settled: bool,
}

/// Stable code of an input batch applied in full whose settle/capture did not
/// finish within the reply budget (fe-api answers 202, never 504).
pub const APPLIED_UNSETTLED_CODE: &str = "applied_unsettled";
/// Stable code of a timed-out batch the host never admitted: nothing was applied.
pub const INPUT_NOT_STARTED_CODE: &str = "input_not_started";
/// Stable code of a timed-out batch whose input landed only in part.
pub const INPUT_PARTIALLY_APPLIED_CODE: &str = "input_partially_applied";

/// What an input op produced, or the host's typed refusal/failure.
pub type InputOutcome = Result<InputOutput, InputFailure>;

/// One input request on the shared correlated-bridge envelope
/// (§correlated-bridge): correlation id, caller scope (the host refuses
/// unless it covers the active verse at drain time AND at injection time —
/// INPUT-ADDENDUM item 2), the operation, and its private reply slot.
pub type InputRequest = BridgeRequest<InputOp, InputOutcome>;

/// The input outcome, echoing the request's correlation id.
pub type InputReply = BridgeReply<InputOutcome>;

#[cfg(test)]
mod tests {
    use super::*;

    fn click(x: f32, y: f32) -> InputCommand {
        InputCommand::PointerClick {
            x,
            y,
            button: PointerButton::Left,
        }
    }

    /// One long journey over the whole input wire contract (TEST POLICY
    /// 2026-11-01: a brand-new region gets exactly one journey test, not a
    /// scatter of point tests): every structural refusal in `validate()`'s
    /// table, in table order, then the defaults a valid batch resolves to,
    /// then the two typed failure helpers, then the bridge's own bounded
    /// capacity. Each assertion would go red if its branch inverted.
    #[test]
    fn input_spec_validation_defaults_and_bridge_journey() {
        let mut tagged=InputSpec {commands:vec![click(10.0,10.0)],..Default::default()}.validate().unwrap();
        assert!(tagged.join_lease().is_none());
        assert!(tagged.require_join_lease("arbitrary/header").is_err());
        tagged.require_join_lease("join-input-opaque").unwrap();
        assert_eq!(tagged.join_lease(),Some("join-input-opaque"));
        let credential = "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJzZWNyZXQifQ.c2lnbmF0dXJl";
        let redacted =
            crate::qa_debug::redact(&format!("value=\"{credential}\" Bearer opaque-credential"));
        assert!(!redacted.contains(credential));
        assert!(!redacted.contains("opaque-credential"));
        assert_eq!(
            crate::qa_debug::redact("Bearer abc+private/rest=="),
            "Bearer [REDACTED]"
        );
        let ring = crate::qa_debug::QaLogRing::default();
        ring.set_verse(Some("A"));
        for _ in 0..2005 {
            ring.push(Some("A"), "warn", "test", credential);
        }
        let logs = ring.read(&crate::qa_debug::LogFilter::default());
        assert_eq!(logs["entries"].as_array().unwrap().len(), 2000);
        assert!(!logs.to_string().contains(credential));
        ring.push(Some("B"), "warn", "test", "foreign private node name");
        ring.push(None, "warn", "test", "unscoped private node name");
        let scoped = ring.read(&crate::qa_debug::LogFilter::default());
        assert!(!scoped.to_string().contains("private node name"));
        ring.set_verse(Some("B"));
        assert!(ring.read(&crate::qa_debug::LogFilter::default())["entries"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(crate::qa_debug::TreeFilter {
            name: Some("x".repeat(257)),
            ..Default::default()
        }
        .validate()
        .is_err());
        // Row 1: no_commands.
        let empty = InputSpec::default()
            .validate()
            .expect_err("empty batch is refused");
        assert_eq!(empty.code, "no_commands");
        assert_eq!(empty.field, Some("commands".to_string()));

        // Row 2: too_many_commands.
        let oversized = InputSpec {
            commands: (0..MAX_COMMANDS_PER_REQUEST + 1)
                .map(|i| click(i as f32, 0.0))
                .collect(),
            ..Default::default()
        }
        .validate()
        .expect_err("a batch past the cap is refused");
        assert_eq!(oversized.code, "too_many_commands");
        assert_eq!(oversized.field, Some("commands".to_string()));

        // Row 3a: non_finite on PointerMove, index-scoped.
        let bad_move = InputSpec {
            commands: vec![
                click(1.0, 1.0),
                InputCommand::PointerMove {
                    x: f32::NAN,
                    y: 0.0,
                },
            ],
            ..Default::default()
        }
        .validate()
        .expect_err("a NaN pointer coordinate is refused");
        assert_eq!(bad_move.code, "non_finite");
        assert_eq!(bad_move.field, Some("commands[1].x".to_string()));

        // Row 3a: non_finite on PointerClick.
        let bad_click = InputSpec {
            commands: vec![InputCommand::PointerClick {
                x: 0.0,
                y: f32::INFINITY,
                button: PointerButton::Right,
            }],
            ..Default::default()
        }
        .validate()
        .expect_err("an infinite pointer coordinate is refused");
        assert_eq!(bad_click.code, "non_finite");
        assert_eq!(bad_click.field, Some("commands[0].y".to_string()));

        // Row 3b: non_finite on PointerDrag, including the `to` point.
        let bad_drag = InputSpec {
            commands: vec![InputCommand::PointerDrag {
                from: PointerPoint { x: 0.0, y: 0.0 },
                to: PointerPoint {
                    x: f32::NEG_INFINITY,
                    y: 0.0,
                },
                button: PointerButton::Left,
            }],
            ..Default::default()
        }
        .validate()
        .expect_err("a non-finite drag endpoint is refused");
        assert_eq!(bad_drag.code, "non_finite");
        assert_eq!(bad_drag.field, Some("commands[0].to.x".to_string()));

        // Row 3c: invalid_key — empty, too long, and non-alphanumeric all refuse.
        for bad_name in ["", &"A".repeat(MAX_KEY_LEN + 1), "Key A"] {
            let refusal = InputSpec {
                commands: vec![InputCommand::KeyPress {
                    key: InputKey(bad_name.to_string()),
                }],
                ..Default::default()
            }
            .validate()
            .expect_err("a malformed key name is refused");
            assert_eq!(refusal.code, "invalid_key", "key {bad_name:?}");
            assert_eq!(refusal.field, Some("commands[0].key".to_string()));
        }

        // Row 3d: text_too_long.
        let bad_text = InputSpec {
            commands: vec![InputCommand::TextEntry {
                text: "x".repeat(MAX_TEXT_LEN + 1),
            }],
            ..Default::default()
        }
        .validate()
        .expect_err("oversized text entry is refused");
        assert_eq!(bad_text.code, "text_too_long");
        assert_eq!(bad_text.field, Some("commands[0].text".to_string()));

        // Row 3d': no_effective_input — empty text would plan zero input
        // frames (INPUT-ADDENDUM item 7, the panic=abort crash class).
        let empty_text = InputSpec {
            commands: vec![
                click(1.0, 1.0),
                InputCommand::TextEntry {
                    text: String::new(),
                },
            ],
            ..Default::default()
        }
        .validate()
        .expect_err("an empty text entry is refused");
        assert_eq!(empty_text.code, NO_EFFECTIVE_INPUT_CODE);
        assert_eq!(empty_text.field, Some("commands[1].text".to_string()));

        // Row 3e: non_finite on Scroll.
        let bad_scroll = InputSpec {
            commands: vec![InputCommand::Scroll {
                dx: f32::NAN,
                dy: 0.0,
            }],
            ..Default::default()
        }
        .validate()
        .expect_err("a non-finite scroll delta is refused");
        assert_eq!(bad_scroll.code, "non_finite");
        assert_eq!(bad_scroll.field, Some("commands[0].dx".to_string()));

        // Row 4: wait_frames_out_of_range.
        let bad_wait = InputSpec {
            commands: vec![click(1.0, 1.0)],
            wait_frames: Some(WAIT_FRAMES_MAX + 1),
            ..Default::default()
        }
        .validate()
        .expect_err("an over-cap wait_frames is refused");
        assert_eq!(bad_wait.code, "wait_frames_out_of_range");
        assert_eq!(bad_wait.field, Some("wait_frames".to_string()));

        // Defaults: a bare valid batch resolves wait_frames=0, capture_after=false.
        let bare = InputSpec {
            commands: vec![click(5.0, 5.0)],
            ..Default::default()
        }
        .validate()
        .expect("a well-formed single click is valid");
        assert_eq!(bare.wait_frames(), DEFAULT_WAIT_FRAMES);
        assert_eq!(bare.capture_after(), DEFAULT_CAPTURE_AFTER);
        assert_eq!(bare.commands().len(), 1);

        // A full, mixed-vocabulary batch with explicit wait_frames/capture_after
        // round-trips through JSON and resolves every field.
        let full: InputSpec = serde_json::from_value(serde_json::json!({
            "commands": [
                { "kind": "pointer_move", "x": 10.0, "y": 20.0 },
                { "kind": "pointer_click", "x": 10.0, "y": 20.0, "button": "left" },
                { "kind": "pointer_drag", "from": {"x": 0.0, "y": 0.0}, "to": {"x": 5.0, "y": 5.0}, "button": "right" },
                { "kind": "key_press", "key": "Enter" },
                { "kind": "text_entry", "text": "hello" },
                { "kind": "scroll", "dx": 0.0, "dy": -3.0 },
            ],
            "wait_frames": 10,
            "capture_after": true,
        }))
        .expect("parses structurally");
        let valid = full.validate().expect("a well-formed mixed batch is valid");
        assert_eq!(valid.wait_frames(), 10);
        assert!(valid.capture_after());
        assert_eq!(valid.commands().len(), 6);

        // Truthful late replies: a cloned handle (the API's) sees the host's
        // writes, walking NotStarted -> PartiallyApplied -> Applied.
        let watcher = valid.progress().clone();
        assert_eq!(watcher.stage(), InputStage::NotStarted);
        valid.progress().admit(3);
        valid.progress().frame_applied();
        assert_eq!(watcher.stage(), InputStage::PartiallyApplied { applied: 1, planned: 3 });
        valid.progress().frame_applied();
        valid.progress().frame_applied();
        valid.progress().frame_settled();
        assert_eq!(watcher.stage(), InputStage::Applied { settled_frames: 1 });

        // The two typed failure helpers carry their documented codes/classes.
        let scope_denied = denied();
        assert_eq!(scope_denied.code, DENIED_CODE);
        assert_eq!(scope_denied.status_class, InputFailureClass::Denied);
        let locked = camera_locked_by_capture();
        assert_eq!(locked.code, CAMERA_LOCKED_BY_CAPTURE_CODE);
        assert_eq!(locked.status_class, InputFailureClass::Busy);

        // The bridge itself is bounded at its capacity, never blocking.
        let (tx, _rx) = input_bridge();
        let mut parked = Vec::new();
        for i in 0..INPUT_BRIDGE_CAPACITY {
            let (request, reply_rx) =
                InputRequest::new(format!("c{i}"), "VERSE#v", InputOp::Inject(valid.clone()));
            tx.try_send(request).expect("within capacity");
            parked.push(reply_rx);
        }
        let (overflow, _overflow_rx) =
            InputRequest::new("overflow", "VERSE#v", InputOp::Inject(valid));
        assert!(
            tx.try_send(overflow).is_err(),
            "the bridge must refuse past its bounded capacity, never block"
        );
    }
}
