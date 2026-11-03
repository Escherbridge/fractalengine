//! One checked editor transaction; see AGENTS.md for the commit/read contract.

mod budget;
mod capture;
mod creation;
mod geometry;
mod groups;
mod prepare;
mod stamp;

use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::editor_batch::*;
use serde_json::{json, Value};

#[cfg(test)]
pub(crate) async fn read(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    actor: &str,
) -> anyhow::Result<EditorBatchCapture> {
    read_target(db, scope, petal, ids, &EditorBatchTarget::Nodes, actor).await
}

pub(crate) async fn read_target(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    target: &EditorBatchTarget,
    actor: &str,
) -> anyhow::Result<EditorBatchCapture> {
    let captured = groups::prepare_target(
        db,
        scope,
        petal,
        ids,
        target,
        actor,
        fe_policy::RoleLevel::Viewer,
        None,
    )
    .await?;
    capture::verify(db, &captured).await?;
    Ok(captured.view)
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedBatch {
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
    facts: Vec<(String, String, i64, Value)>,
    checks: Vec<String>,
    receipt: EditorBatchReceipt,
    actor: String,
    provenance: Provenance,
    petal_budget_base_bytes: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct Provenance {
    group_delete_before: Option<EditorBatchCapture>,
    group_restore_of: Option<String>,
    group_create_before: Option<EditorBatchCapture>,
    group_undo_before: Option<EditorBatchCapture>,
    group_undo_of: Option<String>,
    group_redo_of: Option<String>,
}

fn manifest_payload(receipt: &EditorBatchReceipt, actor: &str, provenance: &Provenance) -> Value {
    let mut payload = json!({"editor_batch_request_id":receipt.request_id,"scope":receipt.scope,"actor_did":actor,"receipt":receipt});
    payload.as_object_mut().unwrap().extend(
        serde_json::to_value(provenance)
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
    );
    payload
}

struct FactIdentity {
    log_id: String,
    created_at: String,
    hlc_timestamp: i64,
}

fn fact_envelope(
    id: &str,
    op: &str,
    version: i64,
    payload: &Value,
    actor: &str,
    identity: FactIdentity,
) -> Value {
    json!({"log_id":identity.log_id,"node_id":id,"hlc_timestamp":identity.hlc_timestamp,"op":op,"source_did":actor,"payload":payload,"row_version":version,"created_at":identity.created_at})
}

pub(crate) use prepare::prepare;

struct CompiledBatch {
    entry: OpLogEntry,
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    envelope_slots: Vec<usize>,
    receipt: EditorBatchReceipt,
    #[cfg(test)]
    reserved_bytes: Option<usize>,
}

#[cfg(test)]
impl PreparedBatch {
    pub(crate) fn check_admission(&self) -> anyhow::Result<usize> {
        compile(self.clone()).map(|compiled| compiled.reserved_bytes.unwrap_or(0))
    }
}

pub(super) const MANIFEST_GUARD: &str = "IF (SELECT VALUE payload FROM op_log WHERE payload.editor_batch_request_id=$batch_request) != [$batch_manifest_payload] { THROW 'batch manifest differs from the exact current request'; }";

fn compile(prepared: PreparedBatch) -> anyhow::Result<CompiledBatch> {
    let PreparedBatch {
        mut guards,
        mut bindings,
        mut facts,
        checks,
        receipt,
        actor,
        provenance,
        petal_budget_base_bytes,
    } = prepared;
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(receipt.petal_id.clone()),
        op_type: OpType::EditorTransaction,
        payload: manifest_payload(&receipt, &actor, &provenance),
        sig: "00".repeat(64),
    };
    guards.insert(0, MANIFEST_GUARD.into());
    bindings.push(("batch_manifest_payload".into(), entry.payload.clone()));
    let mut last = std::collections::BTreeMap::new();
    for (index, (id, op, _, _)) in facts.iter().enumerate() {
        if op != "baseline" {
            last.insert(id.clone(), index);
        }
    }
    let mut envelope_slots = Vec::with_capacity(facts.len());
    for (index, (id, op, version, payload)) in facts.iter_mut().enumerate() {
        if op != "baseline" {
            payload["atomic_save_id"] = json!(receipt.request_id);
        }
        if last.get(id) == Some(&index) {
            payload["atomic_save_request_id"] = json!(receipt.request_id);
            payload["editor_request_id"] = json!(receipt.request_id);
        }
        let name = format!("batch_fact_{index}");
        guards.push(format!("CREATE node_log CONTENT ${name}"));
        envelope_slots.push(bindings.len());
        bindings.push((
            name,
            fact_envelope(
                id,
                op,
                *version,
                payload,
                &actor,
                FactIdentity {
                    log_id: ulid::Ulid::new().to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    hlc_timestamp: i64::MIN,
                },
            ),
        ));
    }
    guards.extend(checks);
    let mut maximum_entry = entry.clone();
    maximum_entry.lamport_clock = u64::MAX;
    maximum_entry.hlc_timestamp = "18446744073709551615:FFFFFFFFFFFFFFFF".into();
    capture::bounded(&(
        &maximum_entry,
        &guards,
        &bindings,
        "BEGIN TRANSACTION;\nCREATE op_log CONTENT $__op_entry;\nCOMMIT TRANSACTION;",
    ))?;
    let initial = provenance
        .group_create_before
        .as_ref()
        .filter(|_| provenance.group_redo_of.is_none())
        .or(provenance.group_delete_before.as_ref());
    let _reserved_bytes = initial
        .map(|before| {
            budget::reserve(
                before,
                &receipt,
                &maximum_entry,
                &guards,
                &bindings,
                &actor,
                petal_budget_base_bytes,
            )
        })
        .transpose()?;
    Ok(CompiledBatch {
        entry,
        statements: guards,
        bindings,
        envelope_slots,
        receipt,
        #[cfg(test)]
        reserved_bytes: _reserved_bytes,
    })
}

/// Persist the complete admitted envelope in one existing atomic transaction.
pub(crate) async fn commit(db: &Db, prepared: PreparedBatch) -> anyhow::Result<EditorBatchReceipt> {
    let CompiledBatch {
        entry,
        statements,
        mut bindings,
        envelope_slots,
        receipt,
        ..
    } = compile(prepared)?;
    crate::op_log::commit_operation_atomic_with(db, entry, |hlc| {
        for slot in envelope_slots {
            bindings[slot].1["hlc_timestamp"] = json!(hlc as i64);
        }
        (statements, bindings)
    })
    .await?;
    Ok(receipt)
}

#[cfg(test)]
pub(crate) async fn resolve(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    request_id: &str,
    actor: &str,
) -> anyhow::Result<EditorBatchResolution> {
    resolve_target(
        db,
        scope,
        petal,
        ids,
        &EditorBatchTarget::Nodes,
        request_id,
        actor,
    )
    .await
}

pub(crate) async fn resolve_target(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    target: &EditorBatchTarget,
    request_id: &str,
    actor: &str,
) -> anyhow::Result<EditorBatchResolution> {
    validate_request_id(request_id)?;
    let mut captured = capture::prepare_nodes(
        db,
        scope,
        petal,
        ids,
        target,
        actor,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let select = "SELECT payload.receipt AS receipt FROM op_log WHERE op_type='EditorTransaction' AND payload.editor_batch_request_id=$batch_request AND payload.scope=$batch_scope AND payload.actor_did=$batch_actor LIMIT 2";
    captured.bindings.extend([
        ("batch_request".into(), json!(request_id)),
        ("batch_scope".into(), json!(scope)),
        ("batch_actor".into(), json!(actor)),
    ]);
    let expanded_read_bytes = serde_json::to_vec(&(
        &captured.view,
        &captured.guards,
        &captured.bindings,
        select,
        "BEGIN TRANSACTION; COMMIT TRANSACTION;",
    ))?
    .len();
    anyhow::ensure!(
        expanded_read_bytes <= MAX_EDITOR_BATCH_BYTES,
        "expanded editor resolution exceeds 4 MiB"
    );
    let slot = captured.guards.len() + 1;
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; {select}; COMMIT TRANSACTION;",
        captured.guards.join("; ")
    ));
    for binding in captured.bindings {
        query = query.bind(binding);
    }
    let mut result = query.await?.check()?;
    let rows: Vec<Value> = result.take(slot)?;
    anyhow::ensure!(rows.len() <= 1, "ambiguous editor batch receipt");
    let receipt = rows
        .first()
        .map(|row| serde_json::from_value::<EditorBatchReceipt>(row["receipt"].clone()))
        .transpose()?;
    if let Some(receipt) = &receipt {
        anyhow::ensure!(
            receipt.request_id == request_id
                && receipt.target == *target
                && receipt.scope == scope
                && receipt.petal_id == petal
                && receipt
                    .members
                    .iter()
                    .map(|m| m.node_id.as_str())
                    .eq(ids.iter().map(String::as_str)),
            "batch receipt target mismatch"
        );
        anyhow::ensure!(
            expanded_read_bytes.saturating_add(serde_json::to_vec(receipt)?.len())
                <= MAX_EDITOR_BATCH_BYTES,
            "expanded editor resolution and receipt exceed 4 MiB"
        );
        anyhow::ensure!(
            receipt
                .members
                .iter()
                .all(|m| m.state.node_id == m.node_id && m.state.petal_id == petal),
            "invalid batch receipt members"
        );
    }
    let resolution = EditorBatchResolution {
        request_id: request_id.into(),
        receipt,
        current: captured.view,
    };
    capture::bounded(&resolution)?;
    Ok(resolution)
}

fn validate_request_id(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 128,
        "invalid editor batch request identity"
    );
    Ok(())
}
