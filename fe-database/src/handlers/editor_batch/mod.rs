//! One checked editor transaction; see AGENTS.md for the commit/read contract.

mod capture;
mod geometry;
mod prepare;
mod stamp;

use crate::{
    repo::Db,
    types::{NodeId, OpLogEntry, OpType},
};
use fe_runtime::editor_batch::*;
use serde_json::{Value, json};

pub(crate) async fn read(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    actor: &str,
) -> anyhow::Result<EditorBatchCapture> {
    let captured =
        capture::prepare(db, scope, petal, ids, actor, fe_policy::RoleLevel::Viewer).await?;
    capture::verify(db, &captured).await?;
    Ok(captured.view)
}

#[derive(Debug)]
pub(crate) struct PreparedBatch {
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
    facts: Vec<(String, String, i64, Value)>,
    checks: Vec<String>,
    receipt: EditorBatchReceipt,
    actor: String,
}

pub(crate) use prepare::prepare;

/// Persist the manifest, every materialization and every fact in the same transaction.
pub(crate) async fn commit(db: &Db, prepared: PreparedBatch) -> anyhow::Result<EditorBatchReceipt> {
    let PreparedBatch {
        mut guards,
        mut bindings,
        mut facts,
        checks,
        receipt,
        actor,
    } = prepared;
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(receipt.petal_id.clone()),
        op_type: OpType::EditorTransaction,
        payload: json!({"editor_batch_request_id":receipt.request_id,"scope":receipt.scope,"actor_did":actor,"receipt":receipt}),
        sig: "00".repeat(64),
    };
    guards.insert(0, "IF (SELECT VALUE payload FROM op_log WHERE payload.editor_batch_request_id=$batch_request) != [$batch_manifest_payload] { THROW 'batch manifest differs from the exact current request'; }".into());
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
        bindings.push((name,json!({"log_id":ulid::Ulid::new().to_string(),"node_id":id,"hlc_timestamp":i64::MIN,"op":op,"source_did":actor,"payload":payload,"row_version":version,"created_at":chrono::Utc::now().to_rfc3339()})));
    }
    guards.extend(checks);
    let mut maximum_entry = entry.clone();
    maximum_entry.lamport_clock = u64::MAX;
    maximum_entry.hlc_timestamp = "18446744073709551615:FFFFFFFFFFFFFFFF".into();
    capture::bounded(&(
        maximum_entry,
        &guards,
        &bindings,
        "BEGIN TRANSACTION;\nCREATE op_log CONTENT $__op_entry;\nCOMMIT TRANSACTION;",
    ))?;
    crate::op_log::commit_operation_atomic_with(db, entry, |hlc| {
        for slot in envelope_slots {
            bindings[slot].1["hlc_timestamp"] = json!(hlc as i64);
        }
        (guards, bindings)
    })
    .await?;
    Ok(receipt)
}

pub(crate) async fn resolve(
    db: &Db,
    scope: &str,
    petal: &str,
    ids: &[String],
    request_id: &str,
    actor: &str,
) -> anyhow::Result<EditorBatchResolution> {
    validate_request_id(request_id)?;
    let mut captured =
        capture::prepare(db, scope, petal, ids, actor, fe_policy::RoleLevel::Viewer).await?;
    let select = "SELECT payload.receipt AS receipt FROM op_log WHERE op_type='EditorTransaction' AND payload.editor_batch_request_id=$batch_request AND payload.scope=$batch_scope AND payload.actor_did=$batch_actor LIMIT 2";
    captured.bindings.extend([
        ("batch_request".into(), json!(request_id)),
        ("batch_scope".into(), json!(scope)),
        ("batch_actor".into(), json!(actor)),
    ]);
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
                && receipt.scope == scope
                && receipt.petal_id == petal
                && receipt
                    .members
                    .iter()
                    .map(|m| m.node_id.as_str())
                    .eq(ids.iter().map(String::as_str)),
            "batch receipt target mismatch"
        );
        capture::bounded(receipt)?;
        anyhow::ensure!(
            receipt
                .members
                .iter()
                .all(|m| m.state.node_id == m.node_id && m.state.petal_id == petal),
            "invalid batch receipt members"
        );
    }
    Ok(EditorBatchResolution {
        request_id: request_id.into(),
        receipt,
        current: captured.view,
    })
}

fn validate_request_id(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 128,
        "invalid editor batch request identity"
    );
    Ok(())
}
