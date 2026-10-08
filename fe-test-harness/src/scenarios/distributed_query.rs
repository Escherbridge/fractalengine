//! Scenario: Distributed Query Fan-out (A15 merge + A16 offline honesty
//! through the real transport).
//!
//! Proves the M2/F7 `SubmitComputeTask` fan-out end-to-end on the real
//! loopback transport with THREE online peers, each running the real
//! in-process DB thread (so partials are executed by the actual
//! `execute_ts_partial` handler over a real store — not unit mocks).
//!
//! Setup: a `sharded` verse fabric (1 host per shard) with a 60s bucket
//! width. Each peer ingests its own anchor's readings through the real
//! `DbCommand::InsertIotReadings` leg (durable insert + one `ReplicationEvent`
//! per row), so the fleet's rows are spread across peers by shard placement —
//! the union lives distributed, not on one host.
//!
//! Legs:
//!
//! 1. **A15 aggregate across 3 peers**: a window aggregate (mean/min/max/
//!    count) fanned out from alice returns exactly the same result as the
//!    same query over the union of all rows (computed in-test).
//! 2. **A15 raw union dedupe**: a raw window query returns every row once,
//!    deduped by `reading_id` — an over-retained duplicate on a non-host is
//!    folded away.
//! 3. **A15 latest-per-anchor**: max `recorded_at_ms` per anchor/metric.
//! 4. **A16 honesty (all online)**: every target shard is covered, no host
//!    missing.
//! 5. **A16 R=1 offline host**: a shard only its single host holds is
//!    invisible once that host leaves the topic — it lands in
//!    `missing_shards` and the host in `missing_hosts`.
//! 6. **A16 R=2 surviving mirror**: after switching to `balanced` R=2, a
//!    shard whose two hosts include the departed peer stays covered — the
//!    surviving mirror serves it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use fe_runtime::distributed_query::{
    DistributedQueryCall, DistributedQueryOutcome, DistributedQueryRequest, TsQueryKind,
};
use fe_runtime::messages::{DbCommand, DbResult, IotReadingInput};
use fe_sync::messages::{SyncCommand, SyncEvent};

use crate::peer::TestPeer;
use crate::TestResult;

/// How long to wait for ledger/declaration convergence over the real transport.
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);
/// Deadline for an online fan-out (settles as soon as every expected host
/// answers, so this is a ceiling, not a wait).
const QUERY_TIMEOUT_MS: u64 = 4_000;
/// Deadline for the offline legs — the collector waits the full window for a
/// silent host, so keep it short.
const OFFLINE_TIMEOUT_MS: u64 = 2_500;
/// Grace window after a ledger demonstrably converged before asserting a
/// reading row's ABSENCE from a survivor's store (a retention-skipped row
/// never lands; a retained one lands well within this window — so "absent"
/// is settled, not merely in flight).
const ROUTE_GRACE: Duration = Duration::from_secs(3);

/// 60s buckets so consecutive minutes map to distinct shards.
const BUCKET_MS: i64 = 60_000;
/// Base bucket index (2026-07-15T10:00:00Z / 60000).
const B0: i64 = 29_209_680;
const METRIC: &str = "temperature_c";

fn bucket_ms(bucket: i64) -> i64 {
    bucket * BUCKET_MS
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("valid timestamp")
        .to_rfc3339()
}

/// The verse manifest row, carrying the `ts_*` settings the fabrics learn.
fn verse_manifest(
    verse_id: &str,
    name: &str,
    alice_did: &str,
    ts_mode: &str,
    ts_r: u32,
) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "verse_id": verse_id,
        "name": name,
        "created_by": alice_did,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "default_access": "viewer",
        "ts_mode": ts_mode,
        "ts_replication_factor": ts_r,
        "ts_bucket_width_ms": BUCKET_MS,
    }))?)
}

/// Publish one row the way the emission seam does: blob-store the row bytes,
/// then hand the content hash to the sync thread as a `WriteRowEntry`.
fn publish_row(
    peer: &TestPeer,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> Result<()> {
    let content_hash = peer.blob_store.add_blob(row_bytes)?;
    peer.sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.to_string(),
            table: table.into(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| anyhow!("failed to send WriteRowEntry for {record_id}: {e}"))?;
    Ok(())
}

/// Create an anchor node in `petal_id` on this peer and return its id.
fn create_anchor(peer: &TestPeer, petal_id: &str, name: &str) -> Result<String> {
    peer.send(DbCommand::CreateNode {
        petal_id: petal_id.to_string(),
        name: name.to_string(),
        position: [0.0, 0.0, 0.0],
        correlation_id: None,
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::NodeCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::NodeCreated { id, .. } => Ok(id),
        other => bail!("unexpected result creating anchor: {other:?}"),
    }
}

/// Ingest a batch of readings through the real DB leg (durable insert +
/// per-row replication emit), returning the number of rows written.
fn ingest(
    peer: &TestPeer,
    petal_id: &str,
    verse_id: &str,
    anchor: &str,
    points: &[(f64, i64)],
) -> Result<usize> {
    let readings: Vec<IotReadingInput> = points
        .iter()
        .map(|(value, bucket)| IotReadingInput {
            node_id: anchor.to_string(),
            metric: METRIC.to_string(),
            value: *value,
            units: "C".to_string(),
            recorded_at: Some(rfc3339(bucket_ms(*bucket))),
        })
        .collect();
    peer.send(DbCommand::InsertIotReadings {
        petal_id: petal_id.to_string(),
        verse_id: Some(verse_id.to_string()),
        source_did: peer.keypair.to_did_key(),
        readings,
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::IotReadingsInserted { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::IotReadingsInserted { written, .. } => Ok(written),
        other => bail!("unexpected result ingesting readings: {other:?}"),
    }
}

/// Run one distributed query through the real `SubmitComputeTask` seam and
/// wait for the merged outcome on the embedded reply channel.
fn run_query(
    peer: &TestPeer,
    verse_id: &str,
    spec: TsQueryKind,
    timeout_ms: u64,
) -> Result<DistributedQueryOutcome> {
    let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
    peer.sync_cmd_tx
        .send(SyncCommand::SubmitComputeTask {
            call: DistributedQueryCall {
                request: DistributedQueryRequest {
                    request_id: ulid::Ulid::new().to_string(),
                    verse_id: verse_id.to_string(),
                    spec,
                    timeout_ms,
                    row_cap: 0,
                },
                reply: reply_tx,
            },
        })
        .map_err(|e| anyhow!("failed to send SubmitComputeTask: {e}"))?;
    reply_rx
        .recv_timeout(Duration::from_millis(timeout_ms + 8_000))
        .map_err(|e| anyhow!("no distributed-query outcome within the deadline: {e}"))
}

/// Ask a peer's sync thread for its fabric dump (`GetShardLedger` →
/// `SyncEvent::ShardLedger`) and parse the JSON.
fn ledger_dump(peer: &TestPeer, verse_id: &str) -> Result<serde_json::Value> {
    peer.sync_cmd_tx
        .send(SyncCommand::GetShardLedger {
            verse_id: verse_id.to_string(),
        })
        .map_err(|e| anyhow!("failed to send GetShardLedger: {e}"))?;
    let evt = peer.wait_sync_event(
        |e| matches!(e, SyncEvent::ShardLedger { verse_id: v, .. } if v == verse_id),
        Duration::from_secs(30),
    )?;
    match evt {
        SyncEvent::ShardLedger { ledger_json, .. } => {
            serde_json::from_str(&ledger_json).map_err(|e| anyhow!("bad ledger JSON: {e}"))
        }
        _ => unreachable!(),
    }
}

/// Poll the fabric dump until `probe(dump)` passes (transport convergence).
fn poll_ledger<F: Fn(&serde_json::Value) -> bool>(
    peer: &TestPeer,
    verse_id: &str,
    probe: F,
) -> Result<serde_json::Value> {
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let dump = ledger_dump(peer, verse_id)?;
        if probe(&dump) {
            return Ok(dump);
        }
        if std::time::Instant::now() >= deadline {
            bail!("shard ledger never converged: {dump}");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// READ-BACK readings straight from a peer's durable store.
fn read_readings(peer: &TestPeer, where_clause: &str) -> Result<Vec<serde_json::Value>> {
    peer.send(DbCommand::RawQuery {
        sql: format!("SELECT * FROM iot_reading{where_clause}"),
        vars: HashMap::new(),
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::QueryResult { data } => Ok(data),
        other => bail!("unexpected read-back result: {other:?}"),
    }
}

/// Ingest single readings at successive buckets until `probe` accepts the
/// shard's ledger entry (or the probe budget runs out). Returns the accepted
/// shard key and the ledger dump that satisfied the probe.
///
/// The poll waits only for the entry to EXIST (the ingester plans the shard
/// in its own local ledger, so that converges immediately); the placement
/// combo `probe` decides on the settled entry — a wrong combo (e.g. an
/// R=2 shard that landed on the wrong host pair) moves to the next bucket
/// instead of burning the transport deadline.
fn ingest_until<F: Fn(&serde_json::Value) -> bool>(
    peer: &TestPeer,
    petal_id: &str,
    verse_id: &str,
    anchor: &str,
    value: f64,
    first_bucket: i64,
    probe: F,
) -> Result<(i64, String, serde_json::Value)> {
    for offset in 0..12 {
        let bucket = first_bucket + offset;
        ingest(peer, petal_id, verse_id, anchor, &[(value, bucket)])?;
        let shard = format!("{petal_id}/{anchor}/{bucket}");
        let shard_for_probe = shard.clone();
        let dump = poll_ledger(peer, verse_id, |d| {
            d["shards"][shard_for_probe.as_str()].is_object()
        })?;
        if probe(&dump["shards"][shard.as_str()]) {
            return Ok((bucket, shard, dump));
        }
        tracing::debug!("bucket {bucket} landed on the wrong hosts — trying the next");
    }
    bail!("no shard satisfied the probe within the bucket budget")
}

/// A shard ledger entry's host set is EXACTLY `want` (same length, every
/// wanted DID present).
fn hosts_are(entry: &serde_json::Value, want: &[&str]) -> bool {
    let Some(hosts) = entry["hosts"].as_array() else {
        return false;
    };
    let got: Vec<&str> = hosts.iter().filter_map(|v| v.as_str()).collect();
    got.len() == want.len() && want.iter().all(|w| got.contains(w))
}

/// Per-(petal, verse, anchor) probe context — the three ids every ingest
/// helper threads through; grouped to keep the probe signatures under
/// clippy's argument limit.
struct TsCtx<'a> {
    petal_id: &'a str,
    verse_id: &'a str,
    anchor: &'a str,
}

/// Ingest single readings on `author` at successive buckets until a shard is
/// EXCLUSIVELY held by `host_did`: every survivor fabric converged on that
/// single-host ledger entry AND (after a grace window) no survivor's durable
/// store holds the row — so once the host leaves the topic the shard is
/// genuinely invisible. A bucket where a survivor over-retained the row (the
/// ledger row and the reading raced through the doc) is retried at the next
/// bucket: over-retained data makes a shard honestly COVERED, not missing,
/// and this leg needs the missing direction.
fn ingest_exclusively_until(
    author: &TestPeer,
    survivors: &[&TestPeer],
    ctx: &TsCtx<'_>,
    value: f64,
    first_bucket: i64,
    host_did: &str,
) -> Result<(i64, String)> {
    for offset in 0..12 {
        let bucket = first_bucket + offset;
        ingest(
            author,
            ctx.petal_id,
            ctx.verse_id,
            ctx.anchor,
            &[(value, bucket)],
        )?;
        let shard = format!("{}/{}/{}", ctx.petal_id, ctx.anchor, bucket);
        // The author plans the shard in its own local ledger (instant): the
        // placement combo is decided there — a wrong combo (the shard landed
        // on someone else) moves to the next bucket instead of burning the
        // transport deadline below.
        let shard_for_exists = shard.clone();
        let author_dump = poll_ledger(author, ctx.verse_id, |d| {
            d["shards"][shard_for_exists.as_str()].is_object()
        })
        .map_err(|e| anyhow!("author's ledger never planned {shard}: {e}"))?;
        if !hosts_are(&author_dump["shards"][shard.as_str()], &[host_did]) {
            tracing::debug!("bucket {bucket} landed on the wrong host — trying the next");
            continue;
        }
        // The combo is fixed now, so polling the survivors for that exact
        // host set IS transport convergence — the ledger row crossing the
        // doc is the only thing left to wait for.
        for (i, survivor) in survivors.iter().enumerate() {
            let shard_for_probe = shard.clone();
            poll_ledger(survivor, ctx.verse_id, |d| {
                hosts_are(&d["shards"][shard_for_probe.as_str()], &[host_did])
            })
            .map_err(|e| anyhow!("survivor {i}'s ledger never converged on {shard}: {e}"))?;
        }
        std::thread::sleep(ROUTE_GRACE);
        let where_row = format!(
            " WHERE node_id = '{}' AND recorded_at_ms = {}",
            ctx.anchor,
            bucket_ms(bucket)
        );
        let mut over_retained = false;
        for survivor in survivors {
            if !read_readings(survivor, &where_row)?.is_empty() {
                over_retained = true;
                break;
            }
        }
        if !over_retained {
            return Ok((bucket, shard));
        }
        tracing::debug!("bucket {bucket} over-retained on a survivor — trying the next");
    }
    bail!("no exclusively-held shard within the bucket budget")
}

/// Expected per-(anchor, metric) aggregate over the union of all rows.
fn expected_aggregate(rows: &[(String, f64)]) -> BTreeMap<(String, String), (f64, u64, f64, f64)> {
    let mut acc: BTreeMap<(String, String), (f64, u64, f64, f64)> = BTreeMap::new();
    for (anchor, value) in rows {
        let e = acc.entry((anchor.clone(), METRIC.to_string())).or_insert((
            0.0,
            0,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ));
        e.0 += value;
        e.1 += 1;
        e.2 = e.2.min(*value);
        e.3 = e.3.max(*value);
    }
    acc
}

/// Compare a merged aggregate outcome to the expected monoid, returning a
/// human-readable mismatch reason (or `None` when it matches).
fn aggregate_mismatch(
    outcome: &DistributedQueryOutcome,
    expected: &BTreeMap<(String, String), (f64, u64, f64, f64)>,
) -> Option<String> {
    if outcome.rows.len() != expected.len() {
        return Some(format!(
            "expected {} merged rows, got {}: {:?}",
            expected.len(),
            outcome.rows.len(),
            outcome.rows
        ));
    }
    let mut seen = BTreeSet::new();
    for row in &outcome.rows {
        let key = (
            row["node_id"].as_str().unwrap_or_default().to_string(),
            row["metric"].as_str().unwrap_or_default().to_string(),
        );
        let Some(want) = expected.get(&key) else {
            return Some(format!("unexpected merged row {row}"));
        };
        let sum = row["avg_value"].as_f64().unwrap_or(f64::NAN);
        let count = row["sample_count"].as_u64().unwrap_or(0);
        let min = row["min_value"].as_f64().unwrap_or(f64::NAN);
        let max = row["max_value"].as_f64().unwrap_or(f64::NAN);
        let want_mean = want.0 / want.1 as f64;
        if count != want.1
            || (sum - want_mean).abs() > 1e-9
            || (min - want.2).abs() > 1e-9
            || (max - want.3).abs() > 1e-9
        {
            return Some(format!(
                "row {key:?} mismatch: got mean={sum} count={count} min={min} max={max}, \
                 want mean={want_mean} count={} min={} max={}",
                want.1, want.2, want.3
            ));
        }
        seen.insert(key);
    }
    None
}

pub fn run() -> Result<TestResult> {
    const NAME: &str = "distributed_query";
    let tmp = tempfile::tempdir()?;

    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;
    let carol = TestPeer::spawn("carol", tmp.path())?;

    let alice_addr = alice.sync_node_addr.clone().ok_or_else(|| {
        anyhow!("alice is offline — the real-transport scenario needs an endpoint")
    })?;
    bob.sync_node_addr
        .clone()
        .ok_or_else(|| anyhow!("bob is offline"))?;
    carol
        .sync_node_addr
        .clone()
        .ok_or_else(|| anyhow!("carol is offline"))?;

    let alice_did = alice.keypair.to_did_key();
    let bob_did = bob.keypair.to_did_key();
    let carol_did = carol.keypair.to_did_key();

    // 1. Alice creates the verse (durable row + namespace secret).
    alice.send(DbCommand::CreateVerse {
        name: "Distributed Query Verse".into(),
    });
    let verse_id = match alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::VerseCreated { id, .. } => id,
        other => bail!("unexpected result creating verse: {other:?}"),
    };
    let ns_secret_hex = alice
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow!("verse secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    // A petal id shared by every peer (each peer creates its own anchor node
    // under it — the harness DB loop is per-peer, and the reader path
    // validates anchors against the local store).
    let petal_id = ulid::Ulid::new().to_string();

    // 2. Alice opens her replica; bob and carol dial her (their bootstrap).
    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_id.clone(),
            namespace_id: ns_id_hex.clone(),
            namespace_secret: Some(ns_secret_hex.clone()),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow!("failed to open alice's replica: {e}"))?;
    // Give alice's open a beat before the joiners dial her: the gossip actor
    // DROPS a Join that arrives for a topic the host has not subscribed to yet
    // (one-shot, no retry — the joiner's compute path would be dead for the
    // whole session). The host's subscribe now runs first in her open
    // handler, and 500ms covers it against thread-scheduling noise.
    std::thread::sleep(Duration::from_millis(500));
    for (peer, who) in [(&bob, "bob"), (&carol, "carol")] {
        peer.sync_cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: verse_id.clone(),
                namespace_id: ns_id_hex.clone(),
                namespace_secret: Some(ns_secret_hex.clone()),
                bootstrap_peers: vec![alice_addr.clone()],
            })
            .map_err(|e| anyhow!("failed to open {who}'s replica: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(500));

    // 3. The verse manifest carries `sharded`/R=1 (1 host per shard) and the
    //    60s bucket width; all three fabrics must learn it.
    let manifest = verse_manifest(
        &verse_id,
        "Distributed Query Verse",
        &alice_did,
        "sharded",
        1,
    )?;
    publish_row(&alice, &verse_id, "verse", &verse_id, &manifest)?;
    for (peer, who) in [(&alice, "alice"), (&bob, "bob"), (&carol, "carol")] {
        poll_ledger(peer, &verse_id, |d| {
            d["settings"]["mode"].as_str() == Some("sharded")
        })
        .map_err(|e| anyhow!("{who}'s fabric never learned sharded mode: {e}"))?;
    }
    // All three declarations must converge on alice before she plans any
    // shard (placement sees the peers it knows about).
    poll_ledger(&alice, &verse_id, |d| {
        d["peers"][alice_did.as_str()].is_object()
            && d["peers"][bob_did.as_str()].is_object()
            && d["peers"][carol_did.as_str()].is_object()
    })
    .map_err(|e| anyhow!("the three peer declarations never converged: {e}"))?;

    // 4. Each peer ingests its own anchor's readings (3 buckets each) through
    //    the real ingestion leg — the rows live distributed by placement.
    let anchor_alice = create_anchor(&alice, &petal_id, "anchor-alice")?;
    let anchor_bob = create_anchor(&bob, &petal_id, "anchor-bob")?;
    let anchor_carol = create_anchor(&carol, &petal_id, "anchor-carol")?;

    let values_alice = [10.0, 20.0, 30.0];
    let values_bob = [1.0, 2.0, 6.0];
    let values_carol = [100.0, 50.0, 0.0];
    let mut union: Vec<(String, f64)> = Vec::new();
    for (peer, anchor, values) in [
        (&alice, &anchor_alice, &values_alice),
        (&bob, &anchor_bob, &values_bob),
        (&carol, &anchor_carol, &values_carol),
    ] {
        let points: Vec<(f64, i64)> = values
            .iter()
            .enumerate()
            .map(|(i, v)| (*v, B0 + i as i64))
            .collect();
        let written = ingest(peer, &petal_id, &verse_id, anchor, &points)?;
        if written != values.len() {
            return Ok(TestResult::fail(
                NAME,
                &format!("ingest wrote {written} rows, expected {}", values.len()),
            ));
        }
        for v in values {
            union.push((anchor.clone(), *v));
        }
    }

    // Every shard must be in alice's ledger before a fan-out can target it.
    let expected_shards: BTreeSet<String> = union
        .iter()
        .enumerate()
        .map(|(i, (anchor, _))| format!("{petal_id}/{anchor}/{}", B0 + (i % 3) as i64))
        .collect();
    poll_ledger(&alice, &verse_id, |d| {
        expected_shards
            .iter()
            .all(|s| d["shards"][s.as_str()].is_object())
    })
    .map_err(|e| {
        anyhow!(
            "alice's ledger never converged all {} shards: {e}",
            expected_shards.len()
        )
    })?;

    // --- Leg 1 (A15): aggregate across three peers == union ground truth. ---
    let expected = expected_aggregate(&union);
    let window = TsQueryKind::WindowAggregate {
        metric: METRIC.to_string(),
        start_ms: bucket_ms(B0),
        end_ms: bucket_ms(B0 + 3),
        petal_id: petal_id.clone(),
    };
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    let outcome = loop {
        let outcome = run_query(&alice, &verse_id, window.clone(), QUERY_TIMEOUT_MS)?;
        if let Some(reason) = outcome.error.clone() {
            bail!("leg 1: aggregate query errored: {reason}");
        }
        match aggregate_mismatch(&outcome, &expected) {
            None => break outcome,
            Some(reason) if std::time::Instant::now() >= deadline => {
                return Ok(TestResult::fail(
                    NAME,
                    &format!("leg 1: merged aggregate never matched the union: {reason}"),
                ));
            }
            Some(reason) => {
                tracing::debug!("leg 1: aggregate not yet converged: {reason}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    };
    if outcome.meta.truncated {
        return Ok(TestResult::fail(
            NAME,
            "leg 1: merged result reported truncation with only 9 rows in play",
        ));
    }

    // --- Leg 2 (A15): raw union dedupes by reading_id. ---------------------
    let raw = run_query(
        &alice,
        &verse_id,
        TsQueryKind::ReadingsInWindow {
            metric: METRIC.to_string(),
            start_ms: bucket_ms(B0),
            end_ms: bucket_ms(B0 + 3),
            petal_id: petal_id.clone(),
        },
        QUERY_TIMEOUT_MS,
    )?;
    if raw.error.is_some() {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 2: raw query errored: {:?}", raw.error),
        ));
    }
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for row in &raw.rows {
        let Some(id) = row["reading_id"].as_str() else {
            return Ok(TestResult::fail(
                NAME,
                "leg 2: a merged row has no reading_id",
            ));
        };
        if !ids.insert(id.to_string()) {
            return Ok(TestResult::fail(
                NAME,
                &format!("leg 2: duplicate reading_id {id} survived the union merge"),
            ));
        }
    }
    if ids.len() != union.len() {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 2: raw union returned {} rows, expected {} (the whole fleet)",
                ids.len(),
                union.len()
            ),
        ));
    }

    // --- Leg 3 (A15): latest per anchor == max recorded_at_ms. -------------
    let latest = run_query(
        &alice,
        &verse_id,
        TsQueryKind::LatestPerAnchor {
            petal_id: petal_id.clone(),
            metric: Some(METRIC.to_string()),
        },
        QUERY_TIMEOUT_MS,
    )?;
    let mut latest_by_anchor: BTreeMap<String, f64> = BTreeMap::new();
    for row in &latest.rows {
        let anchor = row["node_id"].as_str().unwrap_or_default().to_string();
        let value = row["value"].as_f64().unwrap_or(f64::NAN);
        if let Some(prev) = latest_by_anchor.insert(anchor.clone(), value) {
            return Ok(TestResult::fail(
                NAME,
                &format!("leg 3: two latest rows for anchor {anchor} ({prev}, {value})"),
            ));
        }
    }
    for (anchor, want) in [
        (&anchor_alice, values_alice[2]),
        (&anchor_bob, values_bob[2]),
        (&anchor_carol, values_carol[2]),
    ] {
        match latest_by_anchor.get(anchor) {
            Some(got) if (got - want).abs() < 1e-9 => {}
            other => {
                return Ok(TestResult::fail(
                    NAME,
                    &format!("leg 3: latest for {anchor} was {other:?}, expected {want}"),
                ));
            }
        }
    }

    // --- Leg 4 (A16): all online → everything covered, nothing missing. ----
    if !outcome.meta.missing_shards.is_empty() {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 4: all hosts online but shards reported missing: {:?}",
                outcome.meta.missing_shards
            ),
        ));
    }
    if !outcome.meta.missing_hosts.is_empty() {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 4: all hosts online but hosts reported missing: {:?}",
                outcome.meta.missing_hosts
            ),
        ));
    }
    for shard in &expected_shards {
        if !outcome.meta.covered_shards.contains(shard) {
            return Ok(TestResult::fail(
                NAME,
                &format!("leg 4: shard {shard} is not reported covered"),
            ));
        }
    }

    // --- Leg 5 (A16): departed-host honesty, BOTH directions. -------------
    // 5a: a shard whose ONLY formal host is carol but whose rows ALICE
    //     created (durable locally regardless of retention). Once carol
    //     leaves, the rows are still in the merged result, so the shard must
    //     be reported COVERED (via row attribution) — hiding it would be
    //     dishonest metadata.
    // 5b: a shard only CAROL holds anywhere (she created it, she hosts it,
    //     both survivors retention-skipped it). Once carol leaves it is
    //     genuinely invisible and must land in `missing_shards`, and carol in
    //     `missing_hosts` — never silently absent.
    let (_, creator_shard, _) = ingest_until(
        &alice,
        &petal_id,
        &verse_id,
        &anchor_alice,
        7.0,
        B0 + 100,
        |entry| hosts_are(entry, &[&carol_did]),
    )
    .map_err(|e| anyhow!("leg 5a: no carol-hosted shard for alice's anchor found: {e}"))?;
    let (_, invisible_shard) = ingest_exclusively_until(
        &carol,
        &[&alice, &bob],
        &TsCtx {
            petal_id: &petal_id,
            verse_id: &verse_id,
            anchor: &anchor_carol,
        },
        70.0,
        B0 + 120,
        &carol_did,
    )
    .map_err(|e| anyhow!("leg 5b: no carol-exclusive shard found: {e}"))?;

    // Carol leaves the topic entirely: her replica closes, so she neither
    // receives new rows nor answers compute requests (the fabric still lists
    // her — membership churn re-planning is deferred, which is what makes the
    // missing metadata honest).
    carol
        .sync_cmd_tx
        .send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        })
        .map_err(|e| anyhow!("failed to close carol's replica: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));

    let offline = run_query(
        &alice,
        &verse_id,
        TsQueryKind::AllReadings {
            petal_id: petal_id.clone(),
        },
        OFFLINE_TIMEOUT_MS,
    )?;
    if offline.error.is_some() {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 5: offline query errored: {:?}", offline.error),
        ));
    }
    if !offline.meta.missing_hosts.contains(&carol_did) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 5: carol left but is not in missing_hosts: {:?}",
                offline.meta.missing_hosts
            ),
        ));
    }
    // 5b: the exclusively-carol shard is reported missing, never silently
    // absent, and never both covered and missing.
    if !offline.meta.missing_shards.contains(&invisible_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 5b: shard {invisible_shard} held only by the departed carol is not \
                 reported missing: {:?}",
                offline.meta.missing_shards
            ),
        ));
    }
    if offline.meta.covered_shards.contains(&invisible_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 5b: shard {invisible_shard} is both covered and missing"),
        ));
    }
    // 5a: the carol-hosted shard whose creator copy still serves the data is
    // COVERED — the rows ARE in the merged result.
    if !offline.meta.covered_shards.contains(&creator_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 5a: shard {creator_shard} hosted by the departed carol lost its \
                 surviving creator copy — covered: {:?}, missing: {:?}",
                offline.meta.covered_shards, offline.meta.missing_shards
            ),
        ));
    }
    if offline.meta.missing_shards.contains(&creator_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 5a: shard {creator_shard} is both covered and missing"),
        ));
    }

    // --- Leg 6 (A16): R=2 surviving mirror keeps a departed-host shard. ----
    // Switch the fabric to balanced R=2 so new shards plan two hosts.
    let manifest_r2 = verse_manifest(
        &verse_id,
        "Distributed Query Verse",
        &alice_did,
        "balanced",
        2,
    )?;
    publish_row(&alice, &verse_id, "verse", &verse_id, &manifest_r2)?;
    for (peer, who) in [(&alice, "alice"), (&bob, "bob")] {
        poll_ledger(peer, &verse_id, |d| {
            d["settings"]["mode"].as_str() == Some("balanced")
        })
        .map_err(|e| anyhow!("leg 6: {who}'s fabric never switched to balanced: {e}"))?;
    }
    // A fresh shard whose two formal hosts are carol (departed) and alice
    // (online), ingested by alice, with bob holding NO local copy — so when
    // BOB runs the query, only alice's surviving mirror can serve it. A
    // bucket where bob over-retained the row is retried at the next.
    let mut mirror_base = B0 + 200;
    let (mirror_bucket, mirror_shard) = loop {
        let (bucket, shard, _) = ingest_until(
            &alice,
            &petal_id,
            &verse_id,
            &anchor_alice,
            8.0,
            mirror_base,
            |entry| hosts_are(entry, &[&carol_did, &alice_did]),
        )
        .map_err(|e| anyhow!("leg 6: no R=2 shard hosted by carol+alice found: {e}"))?;
        let shard_for_probe = shard.clone();
        poll_ledger(&bob, &verse_id, |d| {
            hosts_are(
                &d["shards"][shard_for_probe.as_str()],
                &[&carol_did, &alice_did],
            )
        })
        .map_err(|e| anyhow!("leg 6: bob's ledger never converged on {shard}: {e}"))?;
        std::thread::sleep(ROUTE_GRACE);
        let where_row = format!(
            " WHERE node_id = '{anchor_alice}' AND recorded_at_ms = {}",
            bucket_ms(bucket)
        );
        if read_readings(&bob, &where_row)?.is_empty() {
            break (bucket, shard);
        }
        tracing::debug!("leg 6: bucket {bucket} over-retained on bob — trying the next");
        mirror_base = bucket + 1;
    };

    // Bob fans out: he holds no copy and hosts nothing for this shard, so
    // its coverage can only come from alice's surviving mirror answer.
    let mirrored = run_query(
        &bob,
        &verse_id,
        TsQueryKind::AllReadings {
            petal_id: petal_id.clone(),
        },
        OFFLINE_TIMEOUT_MS,
    )?;
    if mirrored.error.is_some() {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 6: mirror query errored: {:?}", mirrored.error),
        ));
    }
    if !mirrored.meta.covered_shards.contains(&mirror_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 6: R=2 shard {mirror_shard} lost its surviving mirror — covered: {:?}, \
                 missing: {:?}",
                mirrored.meta.covered_shards, mirrored.meta.missing_shards
            ),
        ));
    }
    if mirrored.meta.missing_shards.contains(&mirror_shard) {
        return Ok(TestResult::fail(
            NAME,
            &format!("leg 6: R=2 shard {mirror_shard} is reported missing despite a live mirror"),
        ));
    }
    // The mirror shard's rows really are in the merged result (alice's answer
    // carried them) — coverage is honest, not just metadata.
    let mirror_ms = bucket_ms(mirror_bucket);
    let in_result = mirrored.rows.iter().any(|r| {
        r["node_id"].as_str() == Some(anchor_alice.as_str())
            && r["recorded_at_ms"].as_i64() == Some(mirror_ms)
    });
    if !in_result {
        return Ok(TestResult::fail(
            NAME,
            &format!(
                "leg 6: the mirror shard's row for anchor {anchor_alice} @ {mirror_ms} ms is \
                 not in the merged result ({} rows)",
                mirrored.rows.len()
            ),
        ));
    }

    // 7. Clean close.
    for peer in [&alice, &bob] {
        let _ = peer.sync_cmd_tx.send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        });
    }
    std::thread::sleep(Duration::from_millis(200));

    drop(carol);
    drop(bob);
    drop(alice);
    Ok(TestResult::pass(NAME))
}
