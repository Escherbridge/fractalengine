//! Durable creation lifecycle proofs; see AGENTS.md.

use super::capture;
use crate::repo::Db;
use fe_runtime::{editor_batch::*, messages::VisibilityGroupDto};
use serde_json::{json, Value};

struct Manifest {
    id: String,
    scope: String,
    actor: String,
    payload: Value,
    receipt: EditorBatchReceipt,
}

pub(super) fn proof_guard(prefix: &str) -> String {
    format!("IF (SELECT VALUE payload FROM op_log WHERE op_type='EditorTransaction' AND payload.editor_batch_request_id=${prefix}_request AND payload.scope=${prefix}_scope AND payload.actor_did=${prefix}_actor LIMIT 2) != [${prefix}_payload] {{ THROW 'Group lifecycle proof changed'; }}")
}

pub(super) fn consumption_select(marker: &str, prefix: &str) -> String {
    format!("SELECT VALUE id FROM op_log WHERE op_type='EditorTransaction' AND payload.{marker}=${prefix}_request AND payload.editor_batch_request_id != ${prefix}_current LIMIT 1")
}

impl Manifest {
    async fn load(
        db: &Db,
        id: &str,
        current: &EditorBatchCapture,
        actor: &str,
    ) -> anyhow::Result<Self> {
        super::validate_request_id(id)?;
        let mut query = db.query("SELECT VALUE payload FROM op_log WHERE op_type='EditorTransaction' AND payload.editor_batch_request_id=$creation_request AND payload.scope=$creation_scope AND payload.actor_did=$creation_actor LIMIT 2")
            .bind(("creation_request", id.to_owned())).bind(("creation_scope", current.scope.clone())).bind(("creation_actor", actor.to_owned())).await?.check()?;
        let mut rows: Vec<Value> = query.take(0)?;
        anyhow::ensure!(
            rows.len() == 1,
            "exact same-actor Group lifecycle proof is unavailable"
        );
        let payload = rows.pop().unwrap();
        capture::bounded(&payload)?;
        let receipt: EditorBatchReceipt = serde_json::from_value(payload["receipt"].clone())?;
        anyhow::ensure!(
            receipt.request_id == id
                && receipt.scope == current.scope
                && receipt.petal_id == current.petal_id
                && receipt.target == EditorBatchTarget::Nodes,
            "Group lifecycle proof scope or target mismatch"
        );
        Ok(Self {
            id: id.into(),
            scope: current.scope.clone(),
            actor: actor.into(),
            payload,
            receipt,
        })
    }

    fn before(&self, key: &str) -> anyhow::Result<EditorBatchCapture> {
        let before: EditorBatchCapture = serde_json::from_value(self.payload[key].clone())?;
        anyhow::ensure!(
            before.target == EditorBatchTarget::Nodes
                && before.scope == self.receipt.scope
                && before.petal_id == self.receipt.petal_id
                && !before.members.is_empty()
                && before.members.len() <= MAX_EDITOR_BATCH_TARGETS
                && before.members.iter().map(|m| &m.node_id).eq(self
                    .receipt
                    .members
                    .iter()
                    .map(|m| &m.node_id))
                && before
                    .members
                    .iter()
                    .zip(&self.receipt.members)
                    .all(|(a, b)| a.state.node_id == a.node_id
                        && b.state.node_id == a.node_id
                        && a.state.petal_id == before.petal_id
                        && b.state.petal_id == before.petal_id),
            "Group lifecycle proof has inconsistent original targets"
        );
        Ok(before)
    }

    fn fence(&self, prefix: &str, guards: &mut Vec<String>, bindings: &mut Vec<(String, Value)>) {
        guards.push(proof_guard(prefix));
        bindings.extend([
            (format!("{prefix}_request"), json!(self.id)),
            (format!("{prefix}_scope"), json!(self.scope)),
            (format!("{prefix}_actor"), json!(self.actor)),
            (format!("{prefix}_payload"), self.payload.clone()),
        ]);
    }

    async fn unconsumed(
        &self,
        db: &Db,
        marker: &str,
        prefix: &str,
        current_request: &str,
        guards: &mut Vec<String>,
        bindings: &mut Vec<(String, Value)>,
    ) -> anyhow::Result<()> {
        let select = consumption_select(marker, prefix);
        let mut query = db
            .query(select.clone())
            .bind((format!("{prefix}_request"), self.id.clone()))
            .bind((format!("{prefix}_current"), String::new()))
            .await?
            .check()?;
        let used: Vec<Value> = query.take(0)?;
        anyhow::ensure!(
            used.is_empty(),
            "Group lifecycle proof was already consumed; use its latest owned receipt"
        );
        guards.push(format!(
            "IF array::len(({select})) != 0 {{ THROW 'Group lifecycle proof already consumed'; }}"
        ));
        bindings.push((format!("{prefix}_current"), json!(current_request)));
        Ok(())
    }
}

struct Creation {
    manifest: Manifest,
    before: EditorBatchCapture,
    group_id: String,
}

impl Creation {
    async fn load(
        db: &Db,
        id: &str,
        current: &EditorBatchCapture,
        actor: &str,
    ) -> anyhow::Result<Self> {
        let manifest = Manifest::load(db, id, current, actor).await?;
        let before = manifest.before("group_create_before")?;
        let group_id = manifest
            .receipt
            .created_group_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("proof is not a checked Group creation"))?;
        let mut without = manifest.receipt.groups.clone();
        anyhow::ensure!(
            ulid::Ulid::from_string(&group_id).is_ok()
                && !before.groups.iter().any(|g| g.id == group_id)
                && without.iter().filter(|g| g.id == group_id).count() == 1,
            "invalid original created Group identity"
        );
        without.retain(|g| g.id != group_id);
        anyhow::ensure!(
            without == before.groups,
            "original Group creation registry context is inconsistent"
        );
        crate::handlers::visibility_groups::validate_groups(&manifest.receipt.groups)
            .map_err(anyhow::Error::msg)?;
        for (old, saved) in before.members.iter().zip(&manifest.receipt.members) {
            let mut memberships = match old.state.properties.get("view.groups") {
                None | Some(Value::Null) => vec![],
                Some(value) => serde_json::from_value::<Vec<String>>(value.clone())?,
            };
            anyhow::ensure!(
                !memberships.contains(&group_id),
                "original creation retained a resurrected Group reference"
            );
            memberships.push(group_id.clone());
            anyhow::ensure!(
                saved.state.properties.get("view.groups") == Some(&json!(memberships)),
                "original creation membership context is inconsistent"
            );
        }
        Ok(Self {
            manifest,
            before,
            group_id,
        })
    }
}

fn memberships(capture: &EditorBatchCapture) -> anyhow::Result<Vec<EditorMembershipValue>> {
    capture
        .members
        .iter()
        .map(|m| match m.state.properties.get("view.groups") {
            None => Ok(EditorMembershipValue::Absent),
            Some(Value::Null) => Ok(EditorMembershipValue::Null),
            Some(value) => Ok(EditorMembershipValue::Groups(serde_json::from_value(
                value.clone(),
            )?)),
        })
        .collect()
}

fn same_memberships(
    current: &EditorBatchCapture,
    expected: &[fe_runtime::editor_batch::EditorBatchMember],
) -> bool {
    current
        .members
        .iter()
        .map(|m| &m.node_id)
        .eq(expected.iter().map(|m| &m.node_id))
        && current.members.iter().zip(expected).all(|(a, b)| {
            a.state.properties.get("view.groups") == b.state.properties.get("view.groups")
        })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prove_undo(
    db: &Db,
    current: &EditorBatchCapture,
    create_request: &str,
    group_id: &str,
    restored: &[EditorMembershipValue],
    actor: &str,
    current_request: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<()> {
    let creation = Creation::load(db, create_request, current, actor).await?;
    anyhow::ensure!(
        creation.group_id == group_id
            && current.groups == creation.manifest.receipt.groups
            && same_memberships(current, &creation.manifest.receipt.members)
            && serde_json::to_value(restored)?
                == serde_json::to_value(memberships(&creation.before)?)?,
        "creation Undo differs from its exact original Group context"
    );
    creation.manifest.fence("created_origin", guards, bindings);
    creation
        .manifest
        .unconsumed(
            db,
            "group_undo_of",
            "created_origin",
            current_request,
            guards,
            bindings,
        )
        .await?;
    Ok(())
}

pub(super) struct Redo {
    pub group_id: String,
    pub groups: Vec<VisibilityGroupDto>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prove_redo(
    db: &Db,
    current: &EditorBatchCapture,
    undo_request: &str,
    actor: &str,
    current_request: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<Redo> {
    let undo = Manifest::load(db, undo_request, current, actor).await?;
    let undo_before = undo.before("group_undo_before")?;
    let create_id = undo.payload["group_undo_of"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("proof is not a checked creation Undo"))?;
    let creation = Creation::load(db, create_id, current, actor).await?;
    anyhow::ensure!(
        undo.receipt.created_group_id.is_none()
            && undo_before.groups == creation.manifest.receipt.groups
            && same_memberships(&undo_before, &creation.manifest.receipt.members)
            && undo.receipt.groups == creation.before.groups
            && same_memberships(&creation.before, &undo.receipt.members)
            && current.groups == undo.receipt.groups
            && same_memberships(current, &undo.receipt.members),
        "creation Redo differs from its proven undone lifecycle"
    );
    creation.manifest.fence("created_origin", guards, bindings);
    undo.fence("created_undo", guards, bindings);
    undo.unconsumed(
        db,
        "group_redo_of",
        "created_undo",
        current_request,
        guards,
        bindings,
    )
    .await?;
    Ok(Redo {
        group_id: creation.group_id,
        groups: creation.manifest.receipt.groups,
    })
}
