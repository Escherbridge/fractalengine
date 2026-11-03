//! Server-expanded Group captures and durable exact-delete proof; see AGENTS.md.

use super::capture::{self, Captured};
use crate::repo::Db;
use fe_runtime::editor_batch::*;
use serde_json::{json, Value};

pub(super) fn validate_target(target: &EditorBatchTarget) -> anyhow::Result<()> {
    match target {
        EditorBatchTarget::Nodes => Ok(()),
        EditorBatchTarget::Group { group_id } => {
            anyhow::ensure!(
                !group_id.trim().is_empty()
                    && group_id.chars().count()
                        <= crate::handlers::node_view::MAX_VIEW_GROUP_ID_LEN,
                "invalid captured Group identity"
            );
            Ok(())
        }
        EditorBatchTarget::DeletedGroup { delete_request_id } => {
            super::validate_request_id(delete_request_id)
        }
    }
}

const MEMBERS: &str = "SELECT node_id, tombstone FROM node WHERE petal_id=$group_petal AND properties['view.groups'] CONTAINS $group_id ORDER BY node_id LIMIT 257";

async fn member_ids(db: &Db, petal: &str, group: &str) -> anyhow::Result<Vec<String>> {
    let mut response = db
        .query(MEMBERS)
        .bind(("group_petal", petal.to_owned()))
        .bind(("group_id", group.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() <= MAX_EDITOR_BATCH_TARGETS,
        "Group has more than 256 stored members; no Group edit was prepared"
    );
    let protected = rows
        .iter()
        .filter(|row| row.get("tombstone").is_some_and(|value| !value.is_null()))
        .count();
    anyhow::ensure!(protected == 0, "Group has {protected} protected deleted-object references; no Group edit was prepared and no object was restored");
    rows.into_iter()
        .map(|row| {
            row["node_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("invalid stored Group member"))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_target(
    db: &Db,
    scope: &str,
    petal: &str,
    requested: &[String],
    target: &EditorBatchTarget,
    actor: &str,
    floor: fe_policy::RoleLevel,
    current_request: Option<&str>,
) -> anyhow::Result<Captured> {
    validate_target(target)?;
    if matches!(target, EditorBatchTarget::Nodes) || !requested.is_empty() {
        capture::validate_ids(requested)?;
    }
    if !matches!(target, EditorBatchTarget::Nodes) {
        let context = capture::prepare_nodes(db, scope, petal, &[], target, actor, floor).await?;
        if let EditorBatchTarget::Group { group_id } = target {
            anyhow::ensure!(
                context
                    .view
                    .groups
                    .iter()
                    .any(|group| group.id == *group_id),
                "Group is not registered in this Petal"
            );
        }
    }
    let proof = match target {
        EditorBatchTarget::DeletedGroup { delete_request_id } => {
            Some(load_delete(db, scope, petal, delete_request_id, actor).await?)
        }
        _ => None,
    };
    let ids = match target {
        EditorBatchTarget::Nodes => requested.to_vec(),
        EditorBatchTarget::Group { group_id } => member_ids(db, petal, group_id).await?,
        EditorBatchTarget::DeletedGroup { .. } => proof
            .as_ref()
            .unwrap()
            .before
            .members
            .iter()
            .map(|member| member.node_id.clone())
            .collect(),
    };
    if !requested.is_empty() {
        anyhow::ensure!(
            ids == requested,
            "the complete stored Group member set changed"
        );
    }
    let mut captured = capture::prepare_nodes(db, scope, petal, &ids, target, actor, floor).await?;
    match target {
        EditorBatchTarget::Group { group_id } => {
            for member in &captured.view.members {
                let memberships: Vec<String> = serde_json::from_value(member.state.properties.get("view.groups").cloned().unwrap_or(Value::Null))
                    .map_err(|_| anyhow::anyhow!("Group member {} has an unsupported membership representation; its exact inverse cannot be recorded", member.node_id))?;
                crate::handlers::node_view::validate_view_request(None, Some(&memberships))
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "Group member {} cannot be restored exactly: {error}",
                            member.node_id
                        )
                    })?;
            }
            anyhow::ensure!(
                captured
                    .view
                    .groups
                    .iter()
                    .any(|group| group.id == *group_id),
                "Group is not registered in this Petal"
            );
            captured.guards.push("IF (SELECT VALUE node_id FROM node WHERE petal_id=$group_petal AND properties['view.groups'] CONTAINS $group_id ORDER BY node_id LIMIT 257) != $group_members { THROW 'complete Group membership changed'; }".into());
            captured.bindings.extend([
                ("group_petal".into(), json!(petal)),
                ("group_id".into(), json!(group_id)),
                ("group_members".into(), json!(ids)),
            ]);
        }
        EditorBatchTarget::DeletedGroup { .. } => {
            let proof = proof.unwrap();
            anyhow::ensure!(
                captured.view.groups == proof.receipt.groups,
                "Group registry changed since the proven delete; restore was refused"
            );
            captured.view.deleted_group = Some(proof.context()?);
            proof.fence(
                &mut captured.guards,
                &mut captured.bindings,
                current_request,
            );
        }
        EditorBatchTarget::Nodes => {}
    }
    capture::bounded(&(
        &captured.view,
        &captured.guards,
        &captured.bindings,
        "BEGIN TRANSACTION; COMMIT TRANSACTION;",
    ))?;
    Ok(captured)
}

struct DeleteProof {
    before: EditorBatchCapture,
    receipt: EditorBatchReceipt,
    payload: Value,
    request_id: String,
    scope: String,
    actor: String,
}

const PROOF: &str = "SELECT VALUE payload FROM op_log WHERE op_type='EditorTransaction' AND payload.editor_batch_request_id=$deleted_request AND payload.scope=$deleted_scope AND payload.actor_did=$deleted_actor LIMIT 2";
const CONSUMED: &str = "SELECT VALUE id FROM op_log WHERE op_type='EditorTransaction' AND payload.group_restore_of=$deleted_request AND payload.editor_batch_request_id != $deleted_restoring_request LIMIT 1";

async fn load_delete(
    db: &Db,
    scope: &str,
    petal: &str,
    request_id: &str,
    actor: &str,
) -> anyhow::Result<DeleteProof> {
    super::validate_request_id(request_id)?;
    let mut query = db
        .query(PROOF)
        .bind(("deleted_request", request_id.to_owned()))
        .bind(("deleted_scope", scope.to_owned()))
        .bind(("deleted_actor", actor.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = query.take(0)?;
    anyhow::ensure!(
        rows.len() == 1,
        "exact original checked Group deletion receipt is unavailable"
    );
    let payload = rows.into_iter().next().unwrap();
    let deleted_before = payload
        .get("group_delete_before")
        .filter(|value| !value.is_null())
        .ok_or_else(|| {
            anyhow::anyhow!("manifest has no original checked Group deletion capture")
        })?;
    let before: EditorBatchCapture = serde_json::from_value(deleted_before.clone())?;
    let receipt: EditorBatchReceipt = serde_json::from_value(payload["receipt"].clone())?;
    let EditorBatchTarget::Group { group_id } = &before.target else {
        anyhow::bail!("manifest is not a checked Group deletion");
    };
    anyhow::ensure!(
        before.scope == scope
            && before.petal_id == petal
            && receipt.scope == scope
            && receipt.petal_id == petal
            && receipt.request_id == request_id
            && receipt.target == before.target
            && receipt.created_group_id.is_none(),
        "Group deletion proof scope or identity mismatch"
    );
    let mut expected_registry = before.groups.clone();
    anyhow::ensure!(
        expected_registry
            .iter()
            .filter(|group| group.id == *group_id)
            .count()
            == 1,
        "Group deletion proof lacks its exact registry row"
    );
    expected_registry.retain(|group| group.id != *group_id);
    anyhow::ensure!(
        expected_registry == receipt.groups
            && before.members.len() == receipt.members.len()
            && before.members.len() <= MAX_EDITOR_BATCH_TARGETS
            && before
                .members
                .iter()
                .zip(&receipt.members)
                .all(|(old, saved)| old.node_id == saved.node_id
                    && old.state.node_id == old.node_id
                    && saved.state.node_id == old.node_id
                    && old.state.petal_id == petal
                    && saved.state.petal_id == petal),
        "Group deletion proof target or registry mismatch"
    );
    let mut consumed = db
        .query(CONSUMED)
        .bind(("deleted_request", request_id.to_owned()))
        .bind(("deleted_restoring_request", String::new()))
        .await?
        .check()?;
    let consumed: Vec<Value> = consumed.take(0)?;
    anyhow::ensure!(
        consumed.is_empty(),
        "this checked Group deletion was already restored; use the latest deletion receipt"
    );
    capture::bounded(&payload)?;
    Ok(DeleteProof {
        before,
        receipt,
        payload,
        request_id: request_id.into(),
        scope: scope.into(),
        actor: actor.into(),
    })
}

impl DeleteProof {
    fn context(&self) -> anyhow::Result<EditorDeletedGroup> {
        let EditorBatchTarget::Group { group_id } = &self.before.target else {
            unreachable!()
        };
        let memberships = self
            .before
            .members
            .iter()
            .map(|member| match member.state.properties.get("view.groups") {
                None => Ok(EditorMembershipValue::Absent),
                Some(Value::Null) => Ok(EditorMembershipValue::Null),
                Some(value) => Ok(EditorMembershipValue::Groups(serde_json::from_value(
                    value.clone(),
                )?)),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(EditorDeletedGroup {
            group_id: group_id.clone(),
            registry_before: self.before.groups.clone(),
            memberships,
        })
    }

    fn fence(
        &self,
        guards: &mut Vec<String>,
        bindings: &mut Vec<(String, Value)>,
        current_request: Option<&str>,
    ) {
        guards.push(format!("IF ({PROOF}) != [$deleted_payload] {{ THROW 'checked Group deletion proof changed'; }}"));
        guards.push(format!("IF array::len(({CONSUMED})) != 0 {{ THROW 'checked Group deletion proof already consumed'; }}"));
        bindings.extend([
            ("deleted_request".into(), json!(self.request_id)),
            ("deleted_scope".into(), json!(self.scope)),
            ("deleted_actor".into(), json!(self.actor)),
            ("deleted_payload".into(), self.payload.clone()),
            (
                "deleted_restoring_request".into(),
                json!(current_request.unwrap_or("")),
            ),
        ]);
    }
}
