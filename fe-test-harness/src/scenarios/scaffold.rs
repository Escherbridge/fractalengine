//! Hierarchy scaffold for scenarios that hand-publish replicated rows (role propagation RP-4):
//! the inbound gate admits nothing but the verse row before the manifest, and confines every
//! content row to parents the receiving peer already knows — a row naming an unknown parent is
//! Deferred until it lands. A scenario that publishes content rows by hand therefore publishes
//! the manifest and the fractal → petal (→ anchor node) rows they name first, and waits until
//! every receiver applied each one. See fe-test-harness/src/AGENTS.md §scaffold.

use std::time::Duration;

use anyhow::{bail, Result};
use fe_runtime::messages::{DbResult, ReplicatedRowOutcome};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;

/// How long one scaffold row may take to cross the real transport.
const SCAFFOLD_TIMEOUT: Duration = Duration::from_secs(45);

/// The fractal and petal one scenario's content rows live in.
pub struct Scaffold<'a> {
    pub verse_id: &'a str,
    pub owner_did: &'a str,
    pub fractal_id: &'a str,
    pub petal_id: &'a str,
}

/// Publish one row from `author` (blob-store, then `WriteRowEntry`) and wait until every
/// receiver reports it `Applied`.
pub fn publish_and_await(
    author: &TestPeer,
    receivers: &[&TestPeer],
    verse_id: &str,
    table: &str,
    record_id: &str,
    row: &serde_json::Value,
) -> Result<()> {
    let content_hash = author.blob_store.add_blob(&serde_json::to_vec(row)?)?;
    author
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.to_string(),
            table: table.to_string(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| anyhow::anyhow!("failed to publish {table}/{record_id}: {e}"))?;
    for receiver in receivers {
        // A row that arrives before its own parent is first echoed `Deferred`, then echoed
        // again when a retry settles it — wait for the settled echo.
        let result = receiver.wait_for(
            |r| {
                matches!(
                    r,
                    DbResult::ReplicatedRowApplied { table: t, record_id: id, outcome, .. }
                        if t == table
                            && id == record_id
                            && !matches!(outcome, ReplicatedRowOutcome::Deferred(_))
                )
            },
            SCAFFOLD_TIMEOUT,
        )?;
        match result {
            DbResult::ReplicatedRowApplied {
                outcome: ReplicatedRowOutcome::Applied,
                ..
            } => {}
            DbResult::ReplicatedRowApplied { outcome, .. } => {
                bail!("scaffold {table}/{record_id} did not apply on a receiver: {outcome:?}")
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

/// The owner's verse manifest (the receivers' first manifest — trust on first use).
pub fn publish_manifest(
    author: &TestPeer,
    receivers: &[&TestPeer],
    verse_id: &str,
    owner_did: &str,
    name: &str,
) -> Result<()> {
    let manifest = serde_json::json!({
        "verse_id": verse_id,
        "name": name,
        "created_by": owner_did,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "default_access": "viewer",
    });
    publish_and_await(author, receivers, verse_id, "verse", verse_id, &manifest)
}

/// The fractal, the petal, then each anchor node in `node_ids`, in parent order.
pub fn publish_scaffold(
    author: &TestPeer,
    receivers: &[&TestPeer],
    scaffold: &Scaffold<'_>,
    node_ids: &[&str],
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let fractal = serde_json::json!({
        "fractal_id": scaffold.fractal_id,
        "verse_id": scaffold.verse_id,
        "owner_did": scaffold.owner_did,
        "name": "Scaffold Fractal",
        "created_at": now,
    });
    publish_and_await(
        author,
        receivers,
        scaffold.verse_id,
        "fractal",
        scaffold.fractal_id,
        &fractal,
    )?;
    let petal = serde_json::json!({
        "petal_id": scaffold.petal_id,
        "fractal_id": scaffold.fractal_id,
        "name": "Scaffold Petal",
        "node_id": scaffold.owner_did,
        "created_at": now,
    });
    publish_and_await(
        author,
        receivers,
        scaffold.verse_id,
        "petal",
        scaffold.petal_id,
        &petal,
    )?;
    for node_id in node_ids {
        let node = serde_json::json!({
            "node_id": node_id,
            "petal_id": scaffold.petal_id,
            "display_name": "Scaffold Anchor",
            "position": { "type": "Point", "coordinates": [0.0, 0.0] },
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "created_at": now,
        });
        publish_and_await(author, receivers, scaffold.verse_id, "node", node_id, &node)?;
    }
    Ok(())
}
