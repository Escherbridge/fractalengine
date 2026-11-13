//! The one scene write door (R-X-13): every scene-writing `DbCommand` passes the `scene.*` hook gate
//! here, whatever door issued it; rationale and the door census in `fe-runtime/src/AGENTS.md` §scene-write.

use bevy::math::{EulerRot, Quat};
use bevy::prelude::Resource;
use crossbeam::channel::{Sender, TrySendError};
use serde_json::Value;

use crate::editor_batch::{
    EditorBatchEdit, EditorPropertyEdit, EditorTargetMutation, MAX_EDITOR_BATCH_TARGETS,
};
use crate::messages::{DbCommand, NodeMutation};
use crate::scene_hook_gate::{
    SceneAugment, SceneCaller, SceneCallerKind, SceneGateTicket, SceneHookGate, SceneHookInput,
    SceneVerdict,
};

/// Most hook calls one write may dispatch; a bound write over it is refused `input-too-large`.
pub const MAX_HOOK_CALLS_PER_WRITE: usize = MAX_EDITOR_BATCH_TARGETS;

/// How the door treats each scene-writing `DbCommand` (`hooks` empty = plain send, no hook covers it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CensusRow {
    pub variant: &'static str,
    pub hooks: &'static str,
    pub note: &'static str,
}

/// The door census; every other `DbCommand` is refused by [`SceneWrite::new`] (kept in step with `census`).
pub const SCENE_WRITE_CENSUS: &[CensusRow] = &[
    CensusRow { variant: "CreateNode", hooks: "scene.create", note: "name, position" },
    CensusRow { variant: "CreateNodeWithAsset", hooks: "scene.create", note: "name, position" },
    CensusRow { variant: "ImportGltf", hooks: "scene.create", note: "name, position" },
    CensusRow { variant: "CreateObject", hooks: "scene.create + scene.property", note: "one property call per initial key, node null" },
    CensusRow { variant: "DuplicateNode", hooks: "scene.create", note: "proposed.source = the copied node" },
    CensusRow { variant: "PromoteInstance", hooks: "scene.create", note: "proposed.source = {path}#inst-{i}" },
    CensusRow { variant: "PromoteCheckedStamp", hooks: "scene.create", note: "proposed.source = {path}#inst-{i}" },
    CensusRow { variant: "UpdateNodeTransform", hooks: "scene.transform", note: "" },
    CensusRow { variant: "SetNodeProperty", hooks: "scene.property", note: "" },
    CensusRow { variant: "DeleteNodeProperty", hooks: "scene.property", note: "value null" },
    CensusRow { variant: "ClearNodeProperties", hooks: "scene.property", note: "one call per key, value null" },
    CensusRow { variant: "CommitNodeMutation", hooks: "scene.property | scene.transform", note: "by mutation; delete = value null" },
    CensusRow { variant: "CommitCheckedNodeMutation", hooks: "scene.property | scene.transform", note: "by mutation; delete = value null" },
    CensusRow { variant: "CommitCheckedEditorBatch", hooks: "scene.transform + scene.property", note: "per target; path, stamp and group edits plain" },
    CensusRow { variant: "CommitStampOverrides", hooks: "", note: "stamp projection; no scene.* hook covers it" },
    CensusRow { variant: "RenameNode", hooks: "", note: "display name; no scene.* hook covers it" },
    CensusRow { variant: "SetNodeView", hooks: "", note: "view state; no scene.* hook covers it" },
    CensusRow { variant: "UpdateNodeUrl", hooks: "", note: "portal url; no scene.* hook covers it" },
    CensusRow { variant: "DeleteNode", hooks: "", note: "delete; no scene.delete hook in v1" },
    CensusRow { variant: "TombstoneNode", hooks: "", note: "delete; no scene.delete hook in v1" },
    CensusRow { variant: "CascadeTombstoneNode", hooks: "", note: "delete; no scene.delete hook in v1" },
];

/// Why a command cannot pass the door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneWriteError {
    /// Not in [`SCENE_WRITE_CENSUS`]; send it on its own path.
    NotASceneWrite,
    /// The door needs the authorized petal to resolve bindings.
    MissingPetal,
    /// The command names another petal than the authorized one.
    PetalMismatch { given: String, carried: String },
}

impl std::fmt::Display for SceneWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotASceneWrite => f.write_str("not a scene write"),
            Self::MissingPetal => f.write_str("a scene write needs its petal"),
            Self::PetalMismatch { given, carried } => {
                write!(f, "scene write for petal {given} carries petal {carried}")
            }
        }
    }
}

impl std::error::Error for SceneWriteError {}

/// One scene-writing command with the authorized petal and the hook inputs it triggers.
#[derive(Debug, Clone)]
pub struct SceneWrite {
    petal_id: String,
    author_did: Option<String>,
    command: DbCommand,
    inputs: Vec<SceneHookInput>,
}

impl SceneWrite {
    /// Classify `command`; `petal_id` must be the petal the door authorized (the node's, for node writes).
    pub fn new(petal_id: impl Into<String>, command: DbCommand) -> Result<Self, SceneWriteError> {
        let petal_id = petal_id.into();
        if petal_id.is_empty() {
            return Err(SceneWriteError::MissingPetal);
        }
        let Census { carried, inputs } =
            census(&petal_id, &command).ok_or(SceneWriteError::NotASceneWrite)?;
        if let Some(other) = carried.into_iter().find(|carried| *carried != petal_id) {
            return Err(SceneWriteError::PetalMismatch {
                carried: other.to_owned(),
                given: petal_id,
            });
        }
        Ok(Self {
            petal_id,
            author_did: None,
            command,
            inputs,
        })
    }

    /// The authenticated API principal (token `sub`); ignored for the GUI.
    pub fn by(mut self, did: impl Into<String>) -> Self {
        self.author_did = Some(did.into());
        self
    }

    pub fn petal_id(&self) -> &str {
        &self.petal_id
    }

    pub fn command(&self) -> &DbCommand {
        &self.command
    }

    /// The hook inputs the gate checks, in command order.
    pub fn hook_inputs(&self) -> &[SceneHookInput] {
        &self.inputs
    }
}

/// A gate refusal; the write is never sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneRefusal {
    /// The bound extension refused.
    Refused { package_id: String, reason: String },
    /// One hook input the caller sized is over the gate's byte cap.
    InputTooLarge { package_id: String, limit_bytes: usize },
    /// The write would dispatch more than [`MAX_HOOK_CALLS_PER_WRITE`] calls.
    TooManyHookCalls { package_id: String, limit: usize },
}

impl SceneRefusal {
    /// The client-facing code: `refused` or `input-too-large`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused { .. } => "refused",
            Self::InputTooLarge { .. } | Self::TooManyHookCalls { .. } => "input-too-large",
        }
    }

    /// REST status: 409 for a refusal, 413 for caller-sized input.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Refused { .. } => 409,
            Self::InputTooLarge { .. } | Self::TooManyHookCalls { .. } => 413,
        }
    }

    pub fn package_id(&self) -> &str {
        match self {
            Self::Refused { package_id, .. }
            | Self::InputTooLarge { package_id, .. }
            | Self::TooManyHookCalls { package_id, .. } => package_id,
        }
    }

    /// One line for an advisory or an error body.
    pub fn message(&self) -> String {
        match self {
            Self::Refused { package_id, reason } => format!("Refused by {package_id}: {reason}"),
            Self::InputTooLarge {
                package_id,
                limit_bytes,
            } => format!(
                "Refused by {package_id}: input-too-large (the hook input exceeds {limit_bytes} bytes)"
            ),
            Self::TooManyHookCalls { package_id, limit } => format!(
                "Refused by {package_id}: input-too-large (the write needs more than {limit} hook calls)"
            ),
        }
    }

    fn from_verdict(verdict: SceneVerdict) -> Result<Option<SceneAugment>, Self> {
        match verdict {
            SceneVerdict::Proceed { augment } => Ok(augment),
            SceneVerdict::Refused { package_id, reason } => {
                Err(Self::Refused { package_id, reason })
            }
            SceneVerdict::InputTooLarge {
                package_id,
                limit_bytes,
            } => Err(Self::InputTooLarge {
                package_id,
                limit_bytes,
            }),
        }
    }
}

/// Augments from allowing answers; stage them once the write is durable, drop them if it failed.
#[derive(Debug, Default)]
#[must_use = "stage the augments once the write is durable"]
pub struct SceneAugments(Vec<SceneAugment>);

impl SceneAugments {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stage each augment on its node; `created_node_id` names the node a create produced. Returns errors.
    pub fn stage(self, created_node_id: Option<&str>) -> Vec<String> {
        self.0
            .into_iter()
            .filter_map(|augment| {
                let staged = match (augment.subject().is_some(), created_node_id) {
                    (true, _) => augment.stage(),
                    (false, Some(node_id)) => augment.stage_created(node_id),
                    (false, None) => Err("a create augment needs the created node id".to_owned()),
                };
                staged.err()
            })
            .collect()
    }
}

/// An admitted write: send `command` (plain send), then stage the augments.
#[derive(Debug)]
#[must_use = "send the command, then stage its augments"]
pub struct AdmittedWrite {
    command: DbCommand,
    augments: SceneAugments,
}

impl AdmittedWrite {
    pub fn command(&self) -> &DbCommand {
        &self.command
    }

    /// For doors with their own round trip (the API's `DbRequest`).
    pub fn into_parts(self) -> (DbCommand, SceneAugments) {
        (self.command, self.augments)
    }
}

/// The door's answer for one write.
#[derive(Debug)]
pub enum SceneWriteOutcome {
    Admitted(AdmittedWrite),
    Refused(SceneRefusal),
}

/// A pending write; dropping it abandons the outstanding hook calls and never sends.
#[must_use = "poll or wait the ticket, then send the admitted write"]
pub struct SceneWriteTicket(TicketState);

enum TicketState {
    Settled(SceneWriteOutcome),
    Pending(Box<Pending>),
}

struct Pending {
    command: DbCommand,
    checks: Vec<SceneGateTicket>,
    augments: Vec<SceneAugment>,
}

impl Pending {
    fn admit(self) -> SceneWriteOutcome {
        SceneWriteOutcome::Admitted(AdmittedWrite {
            command: self.command,
            augments: SceneAugments(self.augments),
        })
    }
}

impl SceneWriteTicket {
    fn settled(outcome: SceneWriteOutcome) -> Self {
        Self(TicketState::Settled(outcome))
    }

    /// True while a hook answer is outstanding.
    pub fn is_waiting(&self) -> bool {
        match &self.0 {
            TicketState::Settled(_) => false,
            TicketState::Pending(pending) => pending.checks.iter().any(SceneGateTicket::is_waiting),
        }
    }

    /// GUI (non-blocking): the outcome once every check settled or one refused, else the ticket back.
    pub fn poll(self, now: std::time::Instant) -> Result<SceneWriteOutcome, Self> {
        let mut pending = match self.0 {
            TicketState::Settled(outcome) => return Ok(outcome),
            TicketState::Pending(pending) => pending,
        };
        let mut open = Vec::new();
        for mut check in std::mem::take(&mut pending.checks) {
            match check.poll(now) {
                None => open.push(check),
                Some(verdict) => match SceneRefusal::from_verdict(verdict) {
                    Ok(augment) => pending.augments.extend(augment),
                    // The first refusal settles the write; the other calls are abandoned with it.
                    Err(refusal) => return Ok(SceneWriteOutcome::Refused(refusal)),
                },
            }
        }
        if open.is_empty() {
            return Ok((*pending).admit());
        }
        pending.checks = open;
        Err(Self(TicketState::Pending(pending)))
    }

    /// API (async): the outcome, each check waiting at most its hook deadline (needs a tokio time driver).
    pub async fn wait(self) -> SceneWriteOutcome {
        let mut pending = match self.0 {
            TicketState::Settled(outcome) => return outcome,
            TicketState::Pending(pending) => pending,
        };
        for check in std::mem::take(&mut pending.checks) {
            match SceneRefusal::from_verdict(check.wait().await) {
                Ok(augment) => pending.augments.extend(augment),
                Err(refusal) => return SceneWriteOutcome::Refused(refusal),
            }
        }
        (*pending).admit()
    }
}

/// Why an admitted write was not sent; `command` comes back for a rollback.
#[derive(Debug)]
pub struct SceneSendError {
    pub command: Box<DbCommand>,
    pub reason: SceneSendFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneSendFailure {
    /// An API door: send through the round trip with [`AdmittedWrite::into_parts`].
    NoSender,
    Full,
    Disconnected,
}

/// The scene write door over the gate and (for the GUI) the DB command sender; `Default` gates nothing.
#[derive(Resource, Clone, Default)]
pub struct SceneWriteDoor {
    gate: SceneHookGate,
    sender: Option<Sender<DbCommand>>,
}

impl SceneWriteDoor {
    /// The GUI door: `gate` plus the DB command sender.
    pub fn new(gate: SceneHookGate, sender: Sender<DbCommand>) -> Self {
        Self {
            gate,
            sender: Some(sender),
        }
    }

    /// The API door over the installed process gate (disabled when none is installed).
    pub fn installed() -> Self {
        Self {
            gate: SceneHookGate::installed().unwrap_or_default(),
            sender: None,
        }
    }

    pub fn gate(&self) -> &SceneHookGate {
        &self.gate
    }

    /// Start every hook check of `write` without blocking; unbound writes settle at once.
    pub fn submit(&self, caller_kind: SceneCallerKind, write: SceneWrite) -> SceneWriteTicket {
        let SceneWrite {
            author_did,
            command,
            inputs,
            ..
        } = write;
        if let Some(refusal) = self.over_call_cap(&inputs) {
            return SceneWriteTicket::settled(SceneWriteOutcome::Refused(refusal));
        }
        let caller = caller_for(caller_kind, author_did);
        let checks = inputs
            .into_iter()
            .map(|input| self.gate.begin(input, caller.clone()))
            .collect();
        SceneWriteTicket(TicketState::Pending(Box::new(Pending {
            command,
            checks,
            augments: Vec::new(),
        })))
    }

    /// Send an admitted write, blocking like a plain `Sender::send`; returns its augments.
    pub fn send(&self, admitted: AdmittedWrite) -> Result<SceneAugments, SceneSendError> {
        let AdmittedWrite { command, augments } = admitted;
        let Some(sender) = &self.sender else {
            return Err(no_sender(command));
        };
        sender.send(command).map(|()| augments).map_err(|error| SceneSendError {
            command: Box::new(error.into_inner()),
            reason: SceneSendFailure::Disconnected,
        })
    }

    /// Send an admitted write without blocking (frame systems); returns its augments.
    pub fn try_send(&self, admitted: AdmittedWrite) -> Result<SceneAugments, SceneSendError> {
        let AdmittedWrite { command, augments } = admitted;
        let Some(sender) = &self.sender else {
            return Err(no_sender(command));
        };
        sender.try_send(command).map(|()| augments).map_err(|error| {
            let reason = match &error {
                TrySendError::Full(_) => SceneSendFailure::Full,
                TrySendError::Disconnected(_) => SceneSendFailure::Disconnected,
            };
            SceneSendError {
                command: Box::new(error.into_inner()),
                reason,
            }
        })
    }

    /// A bound write that would dispatch more calls than the cap is refused, never partly checked.
    fn over_call_cap(&self, inputs: &[SceneHookInput]) -> Option<SceneRefusal> {
        let mut targets = inputs
            .iter()
            .filter_map(|input| Some((self.gate.dispatch_target(input)?, input.hook())));
        let (package_id, hook) = targets.next()?;
        if 1 + targets.count() <= MAX_HOOK_CALLS_PER_WRITE {
            return None;
        }
        self.gate.count_too_large(&package_id, hook);
        Some(SceneRefusal::TooManyHookCalls {
            package_id,
            limit: MAX_HOOK_CALLS_PER_WRITE,
        })
    }
}

/// [`SceneWriteDoor::submit`] through the installed process gate (the API doors).
pub fn submit(caller_kind: SceneCallerKind, write: SceneWrite) -> SceneWriteTicket {
    SceneWriteDoor::installed().submit(caller_kind, write)
}

fn no_sender(command: DbCommand) -> SceneSendError {
    SceneSendError {
        command: Box::new(command),
        reason: SceneSendFailure::NoSender,
    }
}

/// The GUI is the local DID; an API caller without a principal never reads as local.
fn caller_for(kind: SceneCallerKind, author_did: Option<String>) -> SceneCaller {
    match kind {
        SceneCallerKind::Ui => SceneCaller::ui(),
        _ => SceneCaller {
            kind,
            did: Some(author_did.unwrap_or_default()),
        },
    }
}

/// A classified command: the petals it names itself and the hook inputs it triggers.
struct Census<'a> {
    carried: Vec<&'a str>,
    inputs: Vec<SceneHookInput>,
}

/// The door census (see [`SCENE_WRITE_CENSUS`]); exhaustive, so a new `DbCommand` must be classified here.
fn census<'a>(petal: &str, command: &'a DbCommand) -> Option<Census<'a>> {
    let found = |carried: Vec<&'a str>, inputs: Vec<SceneHookInput>| Some(Census { carried, inputs });
    match command {
        DbCommand::CreateNode {
            petal_id,
            name,
            position,
            ..
        }
        | DbCommand::CreateNodeWithAsset {
            petal_id,
            name,
            position,
            ..
        } => found(vec![petal_id.as_str()], vec![create(petal, name, *position)]),
        DbCommand::ImportGltf {
            request,
            petal_id,
            name,
            position,
            ..
        } => {
            let mut carried = vec![petal_id.as_str()];
            carried.extend(request.as_ref().map(|request| request.petal_id.as_str()));
            found(carried, vec![create(petal, name, *position)])
        }
        DbCommand::CreateObject {
            petal_id, input, ..
        } => {
            let mut inputs = vec![create(petal, &input.name, input.position)];
            inputs.extend(input.properties.iter().map(|(key, value)| {
                SceneHookInput::InitialProperty {
                    petal_id: petal.to_owned(),
                    key: key.clone(),
                    value: value.clone(),
                }
            }));
            found(vec![petal_id.as_str()], inputs)
        }
        DbCommand::DuplicateNode { node_id, .. } => {
            found(Vec::new(), vec![create_from(petal, node_id.clone())])
        }
        DbCommand::PromoteInstance {
            petal_id,
            path_id,
            instance_index,
            ..
        } => found(
            vec![petal_id.as_str()],
            vec![create_from(petal, format!("{path_id}#inst-{instance_index}"))],
        ),
        DbCommand::PromoteCheckedStamp { capture, .. } => found(
            vec![capture.petal_id.as_str()],
            vec![create_from(
                petal,
                format!("{}#inst-{}", capture.path_id, capture.instance_index),
            )],
        ),
        DbCommand::UpdateNodeTransform {
            node_id,
            position,
            rotation,
            scale,
        } => found(
            Vec::new(),
            vec![transform(petal, node_id, *position, *rotation, *scale)],
        ),
        DbCommand::SetNodeProperty {
            node_id,
            key,
            value,
            ..
        } => found(Vec::new(), vec![property(petal, node_id, key, value.clone())]),
        DbCommand::DeleteNodeProperty { node_id, key, .. } => {
            found(Vec::new(), vec![property(petal, node_id, key, Value::Null)])
        }
        DbCommand::ClearNodeProperties { node_id, keys } => found(
            Vec::new(),
            keys.iter()
                .map(|key| property(petal, node_id, key, Value::Null))
                .collect(),
        ),
        DbCommand::CommitNodeMutation {
            node_id, mutation, ..
        }
        | DbCommand::CommitCheckedNodeMutation {
            node_id, mutation, ..
        } => found(Vec::new(), vec![mutation_input(petal, node_id, mutation)]),
        DbCommand::CommitCheckedEditorBatch { request, .. } => found(
            vec![request.capture.petal_id.as_str()],
            editor_batch_inputs(petal, &request.edit),
        ),
        // Scene writes no scene.* hook covers: they still pass the door, as a plain send.
        DbCommand::CommitStampOverrides { .. }
        | DbCommand::RenameNode { .. }
        | DbCommand::SetNodeView { .. }
        | DbCommand::UpdateNodeUrl { .. }
        | DbCommand::DeleteNode { .. }
        | DbCommand::TombstoneNode { .. }
        | DbCommand::CascadeTombstoneNode { .. } => found(Vec::new(), Vec::new()),
        // Not scene writes; replicated rows and extension commits are judged where they were written.
        DbCommand::ReadSceneAssetPublication { .. }
        | DbCommand::ReadSceneAssetAdoption { .. }
        | DbCommand::AdoptSceneAsset { .. }
        | DbCommand::InspectSceneAssetTransfer { .. }
        | DbCommand::ReadReplicatedNodeCurrent { .. }
        | DbCommand::ReadEditorBatch { .. }
        | DbCommand::ResolveCheckedEditorBatch { .. }
        | DbCommand::ResolveCheckedNodeMutation { .. }
        | DbCommand::PluginRead { .. }
        | DbCommand::PluginCommit { .. }
        | DbCommand::SetExtensionPolicy { .. }
        | DbCommand::GetExtensionPolicy { .. }
        | DbCommand::AppendExtensionAudit { .. }
        | DbCommand::ListExtensionAudit { .. }
        | DbCommand::RecordKillListOverride { .. }
        | DbCommand::LoadExtensionGovernance { .. }
        | DbCommand::PutBinding { .. }
        | DbCommand::ListBindings { .. }
        | DbCommand::LoadBindingSnapshot { .. }
        | DbCommand::Ping
        | DbCommand::Seed
        | DbCommand::Shutdown
        | DbCommand::CreateVerse { .. }
        | DbCommand::CreateFractal { .. }
        | DbCommand::CreatePetal { .. }
        | DbCommand::CreateAsset { .. }
        | DbCommand::GcUnreferencedAssets { .. }
        | DbCommand::LoadHierarchy
        | DbCommand::RejudgeReplicatedNodes { .. }
        | DbCommand::ReadHierarchy
        | DbCommand::GenerateVerseInvite { .. }
        | DbCommand::JoinVerseByInvite { .. }
        | DbCommand::ResetDatabase
        | DbCommand::ResetDevelopmentDatabase { .. }
        | DbCommand::TerminalDevelopmentReset { .. }
        | DbCommand::ReadNodeUrl { .. }
        | DbCommand::ReadNativePetalView { .. }
        | DbCommand::GetEntityDetails { .. }
        | DbCommand::RenameEntity { .. }
        | DbCommand::SetVerseDefaultAccess { .. }
        | DbCommand::SetVerseTimeseriesSettings { .. }
        | DbCommand::UpdateFractalDescription { .. }
        | DbCommand::DeleteEntity { .. }
        | DbCommand::EntityAccess { .. }
        | DbCommand::PrepareJoinInvite { .. }
        | DbCommand::IssueJoinInvite { .. }
        | DbCommand::AdmitJoinInvite { .. }
        | DbCommand::CheckJoinReadiness { .. }
        | DbCommand::FinalizeJoinProof { .. }
        | DbCommand::ResolveRolesForPeers { .. }
        | DbCommand::AssignRole { .. }
        | DbCommand::RevokeRole { .. }
        | DbCommand::GenerateScopedInvite { .. }
        | DbCommand::ResolveLocalRole { .. }
        | DbCommand::MintApiToken { .. }
        | DbCommand::RevokeApiToken { .. }
        | DbCommand::ListApiTokens { .. }
        | DbCommand::ListApiTokensByScope { .. }
        | DbCommand::ResolvePetalScope { .. }
        | DbCommand::ResolveNodeScope { .. }
        | DbCommand::ResolveFractalScope { .. }
        | DbCommand::ResolveVerseScope { .. }
        | DbCommand::LoadNodesByPetal { .. }
        | DbCommand::GetNodeTransform { .. }
        | DbCommand::GetNodeProperties { .. }
        | DbCommand::CountNodeDescendants { .. }
        | DbCommand::LoadPetalViewState { .. }
        | DbCommand::SetPetalVisibilityGroups { .. }
        | DbCommand::ComparePetalVisibilityGroups { .. }
        | DbCommand::GetPetalVisibilityGroups { .. }
        | DbCommand::SetScopeHidden { .. }
        | DbCommand::CreateFieldDef { .. }
        | DbCommand::ListFieldDefs { .. }
        | DbCommand::UpdateFieldDef { .. }
        | DbCommand::SetFieldSchema { .. }
        | DbCommand::GetFieldSchema { .. }
        | DbCommand::ReadNodeHistory { .. }
        | DbCommand::ReadNodeMeasurements { .. }
        | DbCommand::ReadSceneMaterials { .. }
        | DbCommand::DeleteFieldDef { .. }
        | DbCommand::ScopedQuery { .. }
        | DbCommand::RawQuery { .. }
        | DbCommand::ElevatedQuery { .. }
        | DbCommand::ExecuteTsPartial { .. }
        | DbCommand::InsertIotReadings { .. }
        | DbCommand::RegisterDeviceFormat { .. }
        | DbCommand::ListDeviceFormats { .. }
        | DbCommand::InsertDevicePayloads { .. }
        | DbCommand::ListDevicePayloads { .. }
        | DbCommand::InstallCrate { .. }
        | DbCommand::InstallCrateEntry { .. }
        | DbCommand::UninstallCrate { .. }
        | DbCommand::GetPetalRequirements { .. }
        | DbCommand::SetPetalRequirements { .. }
        | DbCommand::SetPetalTerrain { .. }
        | DbCommand::GetPetalTerrain { .. }
        | DbCommand::ReadTerrainApiCredential { .. }
        | DbCommand::SetPetalRenderDistance { .. }
        | DbCommand::GetPetalRenderDistance { .. }
        | DbCommand::ClaimOrphanedVerse { .. }
        | DbCommand::EnrolLocalDevice { .. }
        | DbCommand::RevokeLocalDevice { .. }
        | DbCommand::ApplyReplicatedRow { .. }
        | DbCommand::ReemitLocalVerseCatalog { .. }
        | DbCommand::ReemitLocalRoleStatements { .. } => None,
    }
}

fn create(petal: &str, name: &str, position: [f32; 3]) -> SceneHookInput {
    SceneHookInput::Create {
        petal_id: petal.to_owned(),
        name: name.to_owned(),
        position,
    }
}

fn create_from(petal: &str, source_node_id: String) -> SceneHookInput {
    SceneHookInput::CreateFrom {
        petal_id: petal.to_owned(),
        source_node_id,
    }
}

fn transform(
    petal: &str,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> SceneHookInput {
    SceneHookInput::Transform {
        petal_id: petal.to_owned(),
        node_id: node_id.to_owned(),
        position,
        rotation,
        scale,
    }
}

fn property(petal: &str, node_id: &str, key: &str, value: Value) -> SceneHookInput {
    SceneHookInput::Property {
        petal_id: petal.to_owned(),
        node_id: node_id.to_owned(),
        key: key.to_owned(),
        value,
    }
}

fn mutation_input(petal: &str, node_id: &str, mutation: &NodeMutation) -> SceneHookInput {
    match mutation {
        NodeMutation::PropertySet { key, value } => property(petal, node_id, key, value.clone()),
        NodeMutation::PropertyDeleted { key } => property(petal, node_id, key, Value::Null),
        NodeMutation::FullTransform {
            position,
            rotation,
            scale,
        } => transform(petal, node_id, *position, *rotation, *scale),
    }
}

/// Ordinary transform and property targets are gated; path, stamp and group edits pass plain.
fn editor_batch_inputs(petal: &str, edit: &EditorBatchEdit) -> Vec<SceneHookInput> {
    let EditorBatchEdit::Targets(targets) = edit else {
        return Vec::new();
    };
    let mut inputs = Vec::new();
    for target in targets {
        match &target.mutation {
            EditorTargetMutation::Transform(value) => {
                let (x, y, z) = Quat::from_array(value.rotation).to_euler(EulerRot::XYZ);
                inputs.push(transform(
                    petal,
                    &target.node_id,
                    value.position.map(|v| v as f32),
                    [x, y, z],
                    value.scale.map(|v| v as f32),
                ));
            }
            EditorTargetMutation::Properties(edits) => {
                inputs.extend(edits.iter().map(|edit| match edit {
                    EditorPropertyEdit::Set { key, value } => {
                        property(petal, &target.node_id, key, value.clone())
                    }
                    EditorPropertyEdit::Delete { key } => {
                        property(petal, &target.node_id, key, Value::Null)
                    }
                }));
            }
            EditorTargetMutation::PathAffine(_)
            | EditorTargetMutation::PathRestore { .. }
            | EditorTargetMutation::StampOverrides(_)
            | EditorTargetMutation::StampRestore { .. } => {}
        }
    }
    inputs
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;
    use crate::messages::CreateObjectInput;
    use crate::node_history::NodeAccessGrant;
    use crate::scene_hook_gate::tests::gate_with;
    use crate::scene_hook_gate::{SceneHook, SceneHookReply, MAX_HOOK_INPUT_BYTES};

    fn set_property(node: &str, key: &str, value: Value) -> DbCommand {
        DbCommand::SetNodeProperty {
            node_id: node.into(),
            key: key.into(),
            value,
            correlation_id: None,
        }
    }

    fn create_node(petal: &str) -> DbCommand {
        DbCommand::CreateNode {
            petal_id: petal.into(),
            name: "kiosk".into(),
            position: [1.0, 0.0, 2.0],
            correlation_id: None,
        }
    }

    fn create_object(petal: &str, keys: impl IntoIterator<Item = String>) -> DbCommand {
        DbCommand::CreateObject {
            petal_id: petal.into(),
            input: CreateObjectInput {
                name: "plot".into(),
                asset_id: None,
                position: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
                properties: keys.into_iter().map(|key| (key, json!("A"))).collect(),
            },
            actor_did: "did:key:remote".into(),
            grant: NodeAccessGrant::Local,
            correlation_id: None,
        }
    }

    fn settle(door: &SceneWriteDoor, kind: SceneCallerKind, write: SceneWrite) -> SceneWriteOutcome {
        door.submit(kind, write)
            .poll(Instant::now())
            .unwrap_or_else(|_| panic!("an answered write settles at once"))
    }

    /// Every door verdict: unbound and uncovered writes are plain sends; a refusal is 409, caller-sized
    /// input 413; a write that would dispatch too many calls is refused before any call.
    #[test]
    fn scene_write_door_verdicts() {
        let refuse = SceneHookReply::Output(json!({"verdict": "refuse", "reason": "zoned"}));
        let allow = SceneHookReply::Output(json!({"verdict": "allow"}));
        let oversized = json!("x".repeat(MAX_HOOK_INPUT_BYTES));
        let many = (0..=MAX_HOOK_CALLS_PER_WRITE).map(|i| format!("zone{i}"));
        let cases: Vec<(&str, SceneHookReply, &str, DbCommand, Option<(&str, u16)>, usize)> = vec![
            ("unbound petal", refuse.clone(), "p2", create_node("p2"), None, 0),
            ("no hook covers a delete", refuse.clone(), "p1", DbCommand::DeleteNode { node_id: "n1".into() }, None, 0),
            ("out-of-scope key", refuse.clone(), "p1", set_property("n1", "owner", json!(1)), None, 0),
            ("allowed", allow.clone(), "p1", set_property("n1", "zone", json!(1)), None, 1),
            ("refused create", refuse.clone(), "p1", create_node("p1"), Some(("refused", 409)), 1),
            ("refused delete", refuse.clone(), "p1", DbCommand::DeleteNodeProperty { node_id: "n1".into(), key: "zone".into(), correlation_id: None }, Some(("refused", 409)), 1),
            ("caller-sized value", allow.clone(), "p1", set_property("n1", "zone", oversized), Some(("input-too-large", 413)), 0),
            ("too many calls", allow, "p1", create_object("p1", many), Some(("input-too-large", 413)), 0),
        ];
        for (label, answer, petal, command, refusal, calls) in cases {
            let (gate, backend) = gate_with(Some(answer), true);
            let door = SceneWriteDoor::new(gate, crossbeam::channel::unbounded().0);
            let write = SceneWrite::new(petal, command).unwrap().by("did:key:remote");
            match (settle(&door, SceneCallerKind::Rest, write), refusal) {
                (SceneWriteOutcome::Admitted(_), None) => {}
                (SceneWriteOutcome::Refused(refused), Some((code, status))) => {
                    assert_eq!((refused.code(), refused.http_status()), (code, status), "{label}");
                    assert_eq!(refused.package_id(), "org.example.zoning", "{label}");
                }
                (outcome, _) => panic!("{label}: unexpected {outcome:?}"),
            }
            assert_eq!(backend.calls.lock().unwrap().len(), calls, "{label}");
        }
    }

    /// The commit flow through one door: classification, petal checks, a create's augments staged on
    /// the created node after the send, a silent guest failing open on the GUI path, and the API wait.
    #[test]
    fn scene_write_door_journey() {
        // Only census commands pass, for the authorized petal only.
        assert_eq!(SceneWrite::new("p1", DbCommand::Ping).unwrap_err(), SceneWriteError::NotASceneWrite);
        assert_eq!(SceneWrite::new("", create_node("p1")).unwrap_err(), SceneWriteError::MissingPetal);
        assert!(matches!(
            SceneWrite::new("p1", create_node("p2")),
            Err(SceneWriteError::PetalMismatch { .. })
        ));
        let census: Vec<_> = SCENE_WRITE_CENSUS.iter().map(|row| row.variant).collect();
        let mut unique = census.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), census.len());

        // An object create asks scene.create plus one scene.property per in-scope initial key, then
        // stages every augment on the created node once the command was sent.
        let augment = json!({"verdict": "allow", "augment": [{"key": "zone.tag", "value": "B"}]});
        let (gate, backend) = gate_with(Some(SceneHookReply::Output(augment)), true);
        let (tx, rx) = crossbeam::channel::unbounded();
        let door = SceneWriteDoor::new(gate.clone(), tx);
        let write = SceneWrite::new("p1", create_object("p1", ["zone".into(), "owner".into()])).unwrap();
        assert_eq!(write.hook_inputs().len(), 3);
        let SceneWriteOutcome::Admitted(admitted) = settle(&door, SceneCallerKind::Ui, write) else {
            panic!("allowed")
        };
        let calls = backend.calls.lock().unwrap().clone();
        assert_eq!(
            calls.iter().map(|call| call.hook).collect::<Vec<_>>(),
            [SceneHook::Create, SceneHook::Property]
        );
        assert_eq!(calls[1].input["node"], Value::Null);
        assert_eq!(calls[0].input["caller"]["local"], json!(true));
        let augments = door.try_send(admitted).unwrap();
        assert!(matches!(rx.try_recv(), Ok(DbCommand::CreateObject { .. })));
        assert!(augments.stage(Some("n9")).is_empty());
        let staged = backend.staged.lock().unwrap().clone();
        assert_eq!(staged.iter().map(|(node, _)| node.as_str()).collect::<Vec<_>>(), ["n9", "n9"]);

        // An editor batch gates each ordinary transform target with Euler XYZ rotation.
        let batch = crate::editor_batch::EditorBatchEdit::Targets(vec![
            crate::editor_batch::EditorTargetEdit {
                node_id: "n1".into(),
                mutation: EditorTargetMutation::Transform(crate::node_history::NodeTransformValue {
                    position: [1.0, 2.0, 3.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                }),
            },
        ]);
        assert_eq!(
            editor_batch_inputs("p1", &batch),
            [transform("p1", "n1", [1.0, 2.0, 3.0], [0.0; 3], [1.0; 3])]
        );

        // GUI: a silent guest keeps the ticket pending, then the write proceeds past the deadline.
        let (gate, backend) = gate_with(None, true);
        let door = SceneWriteDoor::new(gate, crossbeam::channel::unbounded().0);
        let write = SceneWrite::new("p1", set_property("n1", "zone", json!(2))).unwrap();
        let ticket = door.submit(SceneCallerKind::Ui, write);
        assert!(ticket.is_waiting());
        let ticket = ticket.poll(Instant::now()).err().expect("still waiting");
        let late = Instant::now() + SceneHook::Property.deadline() + Duration::from_millis(1);
        assert!(matches!(ticket.poll(late), Ok(SceneWriteOutcome::Admitted(_))));
        assert!(backend.held.lock().unwrap()[0].is_closed());

        // API: a door without a sender hands the command back; the async wait settles a refusal, and an
        // API caller never reads as local.
        let api_door = SceneWriteDoor::default();
        let SceneWriteOutcome::Admitted(admitted) = settle(&api_door, SceneCallerKind::Mcp, SceneWrite::new("p1", create_node("p1")).unwrap()) else {
            panic!("a disabled gate admits")
        };
        assert_eq!(api_door.send(admitted).unwrap_err().reason, SceneSendFailure::NoSender);
        let (gate, backend) = gate_with(Some(SceneHookReply::Refused("closed".into())), true);
        let door = SceneWriteDoor::new(gate, crossbeam::channel::unbounded().0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let mutation = DbCommand::CommitNodeMutation {
            node_id: "n1".into(),
            mutation: NodeMutation::FullTransform {
                position: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
            actor_did: "did:key:remote".into(),
            grant: NodeAccessGrant::Local,
            correlation_id: None,
        };
        let outcome = runtime.block_on(door.submit(SceneCallerKind::Mcp, SceneWrite::new("p1", mutation).unwrap()).wait());
        let SceneWriteOutcome::Refused(refusal) = outcome else {
            panic!("refused")
        };
        assert_eq!(refusal.message(), "Refused by org.example.zoning: closed");
        let caller = backend.calls.lock().unwrap()[0].input["caller"].clone();
        assert_eq!(caller, json!({"did": "", "kind": "mcp", "local": false}));
    }
}
