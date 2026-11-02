//! Fenced declaration persistence; see handlers/AGENTS.md (petal-requirements).

use fe_runtime::petal_requirements::{PetalRequirements, PetalRequirementsSnapshot};
use serde_json::{json, Value};

use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

fn decode(
    row: &Value,
    scope: String,
    can_manage: bool,
) -> anyhow::Result<PetalRequirementsSnapshot> {
    let document = row
        .get("hexon_manifest")
        .filter(|value| !value.is_null())
        .cloned();
    Ok(PetalRequirementsSnapshot {
        petal_id: row["petal_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid Petal identity"))?
            .into(),
        scope,
        requirements: PetalRequirements::from_document(document.as_ref())
            .map_err(anyhow::Error::msg)?,
        document,
        can_manage,
    })
}

/// Read one exact Petal after fencing Viewer authority and stored ancestry.
pub(crate) async fn get(
    db: &Db,
    actor: &str,
    petal: &str,
) -> anyhow::Result<PetalRequirementsSnapshot> {
    let scope = super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Petal not found or not permitted"))?;
    let access =
        super::entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Viewer)
            .await?;
    let can_manage = access.can_manage();
    let rows = access
        .read_rows(
            db,
            "SELECT petal_id, hexon_manifest FROM petal WHERE petal_id=$requirements_petal",
            vec![("requirements_petal".into(), json!(petal))],
        )
        .await?;
    anyhow::ensure!(rows.len() == 1, "Petal not found or not permitted");
    decode(&rows[0], scope, can_manage)
}

/// Commit declaration and operation atomically, returning the actual stored document.
pub(crate) async fn set(
    db: &Db,
    actor: &str,
    expected: &PetalRequirementsSnapshot,
    requirements: &PetalRequirements,
) -> anyhow::Result<PetalRequirementsSnapshot> {
    let document = requirements
        .update_document(expected.document.as_ref())
        .map_err(anyhow::Error::msg)?;
    let scope = super::crud::resolve_petal_scope_handler(db, &expected.petal_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Petal not found or not permitted"))?;
    anyhow::ensure!(
        scope == expected.scope,
        "Petal ancestry changed; reload requirements"
    );
    let access =
        super::entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Manager)
            .await?;
    let (mut statements, mut bindings) = access.into_parts();
    let changed = if expected.document.is_some() {
        "(SELECT VALUE hexon_manifest FROM petal WHERE petal_id=$requirements_petal)[0] != $requirements_expected"
    } else {
        "array::len((SELECT petal_id FROM petal WHERE petal_id=$requirements_petal AND hexon_manifest != NONE AND hexon_manifest != NULL)) != 0"
    };
    statements.push(format!(
        "IF {changed} {{ THROW 'Requirements changed; reload before saving'; }}"
    ));
    statements.push("UPDATE petal SET hexon_manifest=$requirements_document WHERE petal_id=$requirements_petal RETURN AFTER".into());
    statements.push("IF (SELECT VALUE hexon_manifest FROM petal WHERE petal_id=$requirements_petal)[0] != $requirements_document { THROW 'Requirements readback mismatch'; }".into());
    statements.push(
        "SELECT petal_id, hexon_manifest FROM petal WHERE petal_id=$requirements_petal".into(),
    );
    bindings.extend([
        ("requirements_petal".into(), json!(expected.petal_id)),
        (
            "requirements_expected".into(),
            expected.document.clone().unwrap_or(Value::Null),
        ),
        ("requirements_document".into(), document),
    ]);
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(expected.petal_id.clone()),
        op_type: OpType::SetPetalRequirements,
        payload: json!({"petal_id":expected.petal_id,"requirements":requirements}),
        sig: "00".repeat(64),
    };
    let (_, mut rows) =
        crate::op_log::commit_operation_atomic(db, entry, &statements, bindings).await?;
    let stored = rows.pop().unwrap_or_default();
    anyhow::ensure!(stored.len() == 1, "Requirements readback unavailable");
    decode(&stored[0], scope, true)
}
