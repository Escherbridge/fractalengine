//! Committed scene publication and bounded recovery; see AGENTS.md.

mod current;
mod inbound;
mod publisher;
pub(crate) mod wire;

pub(crate) use current::{echo_applied, read_current_and_need};
#[cfg(test)]
pub(crate) use current::{read_current, current_read_transaction_error};
pub(crate) use inbound::apply_complete;
#[cfg(test)]
pub(crate) use inbound::capture_ordering_fact;
#[cfg(test)]
pub(crate) use publisher::prepare;
pub(crate) use publisher::{publish_after_commit, publish_tombstones_after_commit, recover};

use crate::repo::Db;
use serde_json::Value;

pub(crate) async fn read_clock(db: &Db, id: &str) -> anyhow::Result<Vec<Value>> {
    let mut response = db.query("SELECT node_id, hlc, publisher, row_fingerprint FROM node_scene_clock WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned())).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(rows.len() <= 1, "ambiguous scene clock");
    Ok(rows)
}

pub(crate) fn clock_of(rows: &[Value]) -> anyhow::Result<Option<wire::SceneClock>> {
    rows.first()
        .map(|row| {
            wire::SceneClock::from_fields(
                &serde_json::json!(row["hlc"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("invalid stored scene HLC"))?
                    .to_string()),
                &row["publisher"],
            )
        })
        .transpose()
}

pub(crate) const CLOCK_FENCE: &str = "IF (SELECT node_id, hlc, publisher, row_fingerprint FROM node_scene_clock WHERE node_id=$scene_id) != $scene_clock_before { THROW 'scene version changed'; }";
pub(crate) const CLOCK_WRITE: &str = "UPSERT node_scene_clock SET node_id=$scene_id, hlc=$scene_hlc, publisher=$scene_publisher, row_fingerprint=array::first((SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id)) WHERE node_id=$scene_id";

/// Restore the original accepted scene watermark before issuing another local commit.
pub(crate) async fn persisted_hlc(db: &Db) -> anyhow::Result<u64> {
    let mut response = db
        .query("SELECT math::max(hlc) AS max_clock FROM node_scene_clock GROUP ALL TIMEOUT 5s")
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    let max = match rows.first().and_then(|row| row.get("max_clock")) {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .filter(|clock| *clock > 0)
            .ok_or_else(|| anyhow::anyhow!("stored scene HLC is invalid"))?,
    };
    if max > 0 {
        crate::op_log::admit_scene_hlc(max)?;
    }
    Ok(max)
}
