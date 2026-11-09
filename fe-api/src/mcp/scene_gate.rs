//! `scene.*` hook gate for the MCP and `/ws` node-write doors: the same `rest::scene_gate_verdict` REST asks,
//! with the door's own caller kind (fe-api/AGENTS.md §scene-gate-doors).

use fe_identity::api_token::ApiClaims;
use fe_runtime::scene_hook_gate::{SceneAugment, SceneCallerKind, SceneHookInput, SceneVerdict};

/// Ask the gate before a node write; `Err` is the client-facing refusal line ("Refused by <package>: <reason>").
pub(crate) async fn gate_scene_write(
    claims: &ApiClaims,
    kind: SceneCallerKind,
    input: impl FnOnce() -> SceneHookInput,
) -> Result<Option<SceneAugment>, String> {
    match crate::rest::scene_gate_verdict(claims, kind, input).await {
        SceneVerdict::Proceed { augment } => Ok(augment),
        // Any verdict that is not Proceed keeps the write unsent (fail closed on a new refusal kind).
        refused => Err(refused
            .refusal_message()
            .unwrap_or_else(|| "refused by the bound extension".to_owned())),
    }
}

/// Stage an allowed augment once its write was sent; `created_node_id` names a `scene.create` result.
pub(crate) fn stage_scene_augment(augment: Option<SceneAugment>, created_node_id: Option<&str>) {
    let Some(augment) = augment else { return };
    let staged = match created_node_id {
        Some(node_id) => augment.stage_created(node_id),
        None => augment.stage(),
    };
    if let Err(error) = staged {
        tracing::warn!(%error, "scene hook augment was not staged");
    }
}

/// The petal of a DB-resolved node scope (`VERSE#v-FRACTAL#f-PETAL#p`); `None` for a shallower scope.
pub(crate) fn scope_petal_id(scope: &str) -> Option<String> {
    fe_database::parse_scope(scope).ok()?.petal_id
}
