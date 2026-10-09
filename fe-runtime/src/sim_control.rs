//! Sim control-surface vocabulary (F9/A20): the API→sim-bridge call seam
//! behind `POST /api/v1/sim/*` and the MCP `sim_*` tools — see
//! `fe-runtime/src/AGENTS.md` §sim-control.
//!
//! Payloads stay `serde_json::Value` so fe-runtime (and fe-api) never depend
//! on fe-sim: the bridge parses `ScenarioScript` / `ScriptedEvent` on its
//! side of the channel.

/// Call-channel depth (the bridge services calls serially; a deeper queue
/// would only hide a wedged session behind stale requests).
pub const SIM_CONTROL_QUEUE: usize = 8;

/// Upper bound on one `Step` call's action count.
pub const MAX_SIM_STEP: u32 = 10_000;

/// One control verb.
#[derive(Debug, Clone, PartialEq)]
pub enum SimControlCommand {
    /// Start a session: a scenario JSON object, JSON text, or a built-in
    /// script name (`default` | `sharded_query` | `offline_degraded`).
    Start { script: serde_json::Value },
    /// Settle, fingerprint, and tear down the live session.
    Stop,
    /// Snapshot the live session (or report that none is running).
    Status,
    /// Execute the next `n` merged actions (`1..=MAX_SIM_STEP`).
    Step { n: u32 },
    /// Apply one `ScriptedEvent` fault now (its `at_ms` is ignored).
    InjectFault { event: serde_json::Value },
}

impl SimControlCommand {
    /// The verb name (logs, MCP tool suffix).
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::Stop => "stop",
            Self::Status => "status",
            Self::Step { .. } => "step",
            Self::InjectFault { .. } => "inject_fault",
        }
    }
}

/// Why a control call failed — surfaces map it to their status shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimControlErrorKind {
    /// Malformed script/event/step count — nothing changed.
    BadRequest,
    /// A session is already running (start rejected before the run lock).
    Conflict,
    /// The verb needs a live session and none is running.
    NotRunning,
    /// The driver failed; the session was torn down.
    Failed,
}

/// A typed control failure (kind + human-readable reason).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SimControlError {
    pub kind: SimControlErrorKind,
    pub message: String,
}

impl SimControlError {
    pub fn new(kind: SimControlErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SimControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SimControlError {}

/// A verb's reply: a JSON payload, or a typed failure.
pub type SimControlResult = Result<serde_json::Value, SimControlError>;

/// A command plus its reply seam (the `DistributedQueryCall` shape: a
/// crossbeam sender, so the bridge thread needs no async runtime).
#[derive(Debug, Clone)]
pub struct SimControlCall {
    pub cmd: SimControlCommand,
    pub reply_tx: crossbeam::channel::Sender<SimControlResult>,
}

/// Channel halves for the seam.
pub type SimControlCallSender = crossbeam::channel::Sender<SimControlCall>;
pub type SimControlCallReceiver = crossbeam::channel::Receiver<SimControlCall>;

/// A fresh bounded seam (`SIM_CONTROL_QUEUE`).
pub fn sim_control_channel() -> (SimControlCallSender, SimControlCallReceiver) {
    crossbeam::channel::bounded(SIM_CONTROL_QUEUE)
}
