//! The API side of the one scene write door (R-X-13): every REST/MCP/WS scene write is admitted by
//! `fe_runtime::scene_write` with its caller kind; see fe-api/AGENTS.md §scene-gate-doors.

use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::DbCommand;
use fe_runtime::scene_hook_gate::SceneCallerKind;
use fe_runtime::scene_write::{
    self, SceneAugments, SceneRefusal, SceneWrite, SceneWriteError, SceneWriteOutcome,
};

/// The authenticated principal and the door it came through.
#[derive(Clone, Copy)]
pub(crate) struct Door<'a> {
    pub claims: &'a ApiClaims,
    pub kind: SceneCallerKind,
}

/// Why the door refused a write; the write is never sent.
pub(crate) enum SceneDenied {
    /// The bound extension refused (409) or the hook input was caller-sized over a cap (413).
    Hook(SceneRefusal),
    /// The command is not a scene write or names another petal (400).
    Door(SceneWriteError),
}

impl SceneDenied {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Hook(refusal) => refusal.message(),
            Self::Door(error) => error.to_string(),
        }
    }

    pub(crate) fn http_status(&self) -> u16 {
        match self {
            Self::Hook(refusal) => refusal.http_status(),
            Self::Door(_) => 400,
        }
    }
}

/// A core's failure: a door refusal (REST keeps its 409/413 form) or the long-standing message.
pub(crate) enum WriteFailure {
    Denied(SceneDenied),
    Failed(String),
}

impl WriteFailure {
    /// The client-facing line for doors that only carry a message.
    pub(crate) fn message(self) -> String {
        match self {
            Self::Denied(denied) => denied.message(),
            Self::Failed(message) => message,
        }
    }
}

impl From<String> for WriteFailure {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<SceneDenied> for WriteFailure {
    fn from(denied: SceneDenied) -> Self {
        Self::Denied(denied)
    }
}

impl From<WriteFailure> for String {
    fn from(failure: WriteFailure) -> Self {
        failure.message()
    }
}

/// Hook augments of an admitted write; stage them once it is durable, drop them if it failed.
#[derive(Default)]
#[must_use = "stage the admission once the write is durable"]
pub(crate) struct Admission(SceneAugments);

impl Admission {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stage each augment; `created_node_id` names the node a create produced.
    pub(crate) fn stage(self, created_node_id: Option<&str>) {
        for error in self.0.stage(created_node_id) {
            tracing::warn!(%error, "scene hook augment was not staged");
        }
    }
}

/// Admit `command` through the door for `petal_id`; `Ok` carries the command to send and its augments.
pub(crate) async fn admit(
    door: Door<'_>,
    petal_id: &str,
    command: DbCommand,
) -> Result<(DbCommand, Admission), SceneDenied> {
    let write = SceneWrite::new(petal_id, command)
        .map_err(SceneDenied::Door)?
        .by(door.claims.sub.clone());
    match scene_write::submit(door.kind, write).wait().await {
        SceneWriteOutcome::Admitted(admitted) => {
            let (command, augments) = admitted.into_parts();
            Ok((command, Admission(augments)))
        }
        SceneWriteOutcome::Refused(refusal) => Err(SceneDenied::Hook(refusal)),
    }
}

/// Admit a write whose send rides another rail (TransformPersist); only the verdict and augments matter.
pub(crate) async fn admit_only(
    door: Door<'_>,
    petal_id: &str,
    command: DbCommand,
) -> Result<Admission, SceneDenied> {
    admit(door, petal_id, command).await.map(|(_, admission)| admission)
}

/// The petal of a DB-resolved node scope (`VERSE#v-FRACTAL#f-PETAL#p`); `None` for a shallower scope.
pub(crate) fn scope_petal_id(scope: &str) -> Option<String> {
    fe_database::parse_scope(scope).ok()?.petal_id
}
