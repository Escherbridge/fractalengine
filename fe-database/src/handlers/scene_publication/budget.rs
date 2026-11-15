//! Per-author byte budgets for node data: the local write door and inbound admission; see handlers/AGENTS.md §write-budget.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::publisher::{publish_after_commit, publish_deferred, Published};
use crate::{repo::Db, BlobStoreHandle, ReplicationSender};

/// Stable prefix of a node write refused by its author's byte budget; the text goes on `; retry after <ms> ms`.
pub const NODE_WRITE_BUDGET_EXCEEDED: &str = "node write budget exceeded";

/// The retry-after hint (milliseconds) of a budget refusal found anywhere in `error`.
pub fn node_write_retry_after_ms(error: &str) -> Option<u64> {
    let start = error.find(NODE_WRITE_BUDGET_EXCEEDED)? + NODE_WRITE_BUDGET_EXCEEDED.len();
    let rest = error[start..].strip_prefix("; retry after ")?;
    rest[..rest.find(" ms")?].parse().ok()
}

/// Bytes a local author may commit at once before the sustained rate applies.
pub(crate) const LOCAL_BURST_BYTES: u64 = 8 * 1024 * 1024;
/// Sustained local node bytes per second per author per verse.
pub(crate) const LOCAL_RATE_BYTES_PER_SECOND: u64 = 64 * 1024;
/// Applied inbound node bytes a remote author may land at once (a replica-open replay included).
pub(crate) const INBOUND_BURST_BYTES: u64 = 16 * 1024 * 1024;
/// Sustained applied inbound node bytes per second per remote author per verse.
pub(crate) const INBOUND_RATE_BYTES_PER_SECOND: u64 = 256 * 1024;
/// Budget buckets kept before full (idle) ones are forgotten.
const TRACKED_BUCKETS: usize = 4096;
/// Node and petal ids whose verse, or last charged journal version, is remembered.
const TRACKED_IDS: usize = 65_536;
/// Journal facts read back to measure one commit (a clear writes one fact per key, at most 200).
const COMMIT_FACTS: usize = 256;

/// A token bucket in bytes: `burst_bytes` at once, refilled at `bytes_per_second`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ByteRate {
    pub burst_bytes: u64,
    pub bytes_per_second: u64,
}

/// The local node-write budget of one verse: the single per-verse setting hook (today the defaults).
pub(crate) fn node_write_budget_for_verse(_verse_id: &str) -> ByteRate {
    ByteRate { burst_bytes: LOCAL_BURST_BYTES, bytes_per_second: LOCAL_RATE_BYTES_PER_SECOND }
}

/// The inbound applied-node budget of one verse (same hook shape as the local one).
pub(crate) fn inbound_node_budget_for_verse(_verse_id: &str) -> ByteRate {
    ByteRate { burst_bytes: INBOUND_BURST_BYTES, bytes_per_second: INBOUND_RATE_BYTES_PER_SECOND }
}

/// Balance in bytes; it may go negative, because a write is admitted while positive and charged after commit.
struct Bucket {
    balance: i64,
    at: Instant,
}

impl Bucket {
    fn refill(&mut self, rate: ByteRate, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_nanos();
        let earned = elapsed.saturating_mul(u128::from(rate.bytes_per_second)) / 1_000_000_000;
        let burst = i128::from(rate.burst_bytes.min(i64::MAX as u64));
        self.balance = (i128::from(self.balance) + earned.min(burst as u128) as i128).min(burst) as i64;
        self.at = self.at.max(now);
    }
}

/// Token buckets per (verse, author DID), held in memory on the DB thread.
pub(crate) struct ByteBudgets {
    rate_of: fn(&str) -> ByteRate,
    buckets: HashMap<(String, String), Bucket>,
}

impl ByteBudgets {
    pub(crate) fn new(rate_of: fn(&str) -> ByteRate) -> Self {
        Self { rate_of, buckets: HashMap::new() }
    }

    fn bucket(&mut self, verse: &str, author: &str, now: Instant) -> (&mut Bucket, ByteRate) {
        let rate = (self.rate_of)(verse);
        let bucket = self
            .buckets
            .entry((verse.to_owned(), author.to_owned()))
            .or_insert(Bucket { balance: rate.burst_bytes.min(i64::MAX as u64) as i64, at: now });
        bucket.refill(rate, now);
        (bucket, rate)
    }

    /// `None` when the author may write now, else how long until its balance is positive again.
    pub(crate) fn wait(&mut self, verse: &str, author: &str, now: Instant) -> Option<Duration> {
        let (bucket, rate) = self.bucket(verse, author, now);
        if bucket.balance > 0 {
            return None;
        }
        let owed = u128::from(bucket.balance.unsigned_abs()) + 1;
        let nanos = (owed * 1_000_000_000).div_ceil(u128::from(rate.bytes_per_second.max(1)));
        Some(Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64))
    }

    /// Debit `bytes` the author has just stored or published.
    pub(crate) fn charge(&mut self, verse: &str, author: &str, bytes: usize, now: Instant) {
        let (bucket, _) = self.bucket(verse, author, now);
        bucket.balance = bucket.balance.saturating_sub(i64::try_from(bytes).unwrap_or(i64::MAX));
        if self.buckets.len() > TRACKED_BUCKETS {
            let rate_of = self.rate_of;
            // A full bucket is the same as an absent one.
            self.buckets.retain(|(verse, _), bucket| {
                let rate = rate_of(verse);
                bucket.refill(rate, now);
                bucket.balance < rate.burst_bytes.min(i64::MAX as u64) as i64
            });
        }
    }
}

/// The refusal text: the stable prefix plus a whole-millisecond retry-after hint.
pub(crate) fn refusal(wait: Duration) -> anyhow::Error {
    let millis = wait.as_nanos().div_ceil(1_000_000).max(1);
    anyhow::anyhow!("{NODE_WRITE_BUDGET_EXCEEDED}; retry after {millis} ms")
}

/// The local node write door: admits a user write while its author's budget is positive and
/// charges what the commit stored (journal) and published (wire row) afterwards.
pub(crate) struct NodeWriteDoor {
    budgets: ByteBudgets,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    verse_of: HashMap<String, String>,
    charged: HashMap<String, i64>,
    /// Nodes whose deferred transform publication is charged when it is sent.
    deferred_charged: HashSet<String>,
}

impl Default for NodeWriteDoor {
    fn default() -> Self {
        Self::with(node_write_budget_for_verse, Box::new(Instant::now))
    }
}

impl NodeWriteDoor {
    /// A door with an explicit budget source and clock (tests inject both).
    pub(crate) fn with(rate_of: fn(&str) -> ByteRate, clock: Box<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self {
            budgets: ByteBudgets::new(rate_of),
            clock,
            verse_of: HashMap::new(),
            charged: HashMap::new(),
            deferred_charged: HashSet::new(),
        }
    }

    /// The author's current balance in bytes; negative while in debt (tests and the import seam read it).
    pub(crate) fn balance(&mut self, verse: &str, author: &str) -> i64 {
        let now = (self.clock)();
        self.budgets.bucket(verse, author, now).0.balance
    }

    fn admit_verse(&mut self, verse: &str, actor: &str) -> anyhow::Result<()> {
        let now = (self.clock)();
        match self.budgets.wait(verse, actor, now) {
            None => Ok(()),
            Some(wait) => Err(refusal(wait)),
        }
    }

    /// Admit a write whose scope the caller already resolved.
    pub(crate) fn admit_scope(&mut self, scope: &str, actor: &str) -> anyhow::Result<()> {
        let verse = crate::parse_scope(scope)?.verse_id;
        self.admit_verse(&verse, actor)
    }

    /// Admit a write to an existing node; an unknown node is left to the handler to refuse.
    pub(crate) async fn admit_node(&mut self, db: &Db, node_id: &str, actor: &str) -> anyhow::Result<()> {
        match self.verse_of_node(db, node_id).await? {
            Some(verse) => self.admit_verse(&verse, actor),
            None => Ok(()),
        }
    }

    /// Admit a creation in a petal; an unknown petal is left to the handler to refuse.
    pub(crate) async fn admit_petal(&mut self, db: &Db, petal_id: &str, actor: &str) -> anyhow::Result<()> {
        let key = format!("petal:{petal_id}");
        let verse = match self.verse_of.get(&key) {
            Some(verse) => Some(verse.clone()),
            None => {
                let verse = match super::super::crud::resolve_petal_scope_handler(db, petal_id).await? {
                    Some(scope) => Some(crate::parse_scope(&scope)?.verse_id),
                    None => None,
                };
                if let Some(verse) = &verse {
                    remember(&mut self.verse_of, key, verse.clone());
                }
                verse
            }
        };
        match verse {
            Some(verse) => self.admit_verse(&verse, actor),
            None => Ok(()),
        }
    }

    async fn verse_of_node(&mut self, db: &Db, node_id: &str) -> anyhow::Result<Option<String>> {
        if let Some(verse) = self.verse_of.get(node_id) {
            return Ok(Some(verse.clone()));
        }
        let Some(scope) = super::super::crud::resolve_node_scope_handler(db, node_id).await? else {
            return Ok(None);
        };
        let verse = crate::parse_scope(&scope)?.verse_id;
        remember(&mut self.verse_of, node_id.to_owned(), verse.clone());
        Ok(Some(verse))
    }

    /// Publish a committed node and charge the commit's journal facts and its wire row.
    pub(crate) async fn publish(
        &mut self,
        db: &Db,
        blobs: &BlobStoreHandle,
        sender: Option<&ReplicationSender>,
        local_did: &str,
        node_id: &str,
    ) {
        let published = publish_after_commit(db, blobs, sender, local_did, node_id).await;
        self.charge_journal(db, node_id).await;
        if let Some(published) = published {
            self.charge_published(&published);
        }
    }

    /// Charge a committed write whose publication the lane defers; its row is charged when sent.
    pub(crate) async fn charge_deferred(&mut self, db: &Db, node_id: &str) {
        self.charge_journal(db, node_id).await;
        if self.deferred_charged.len() >= TRACKED_IDS {
            self.deferred_charged.clear();
        }
        self.deferred_charged.insert(node_id.to_owned());
    }

    /// Send the lane's deferred publications; rows of door-admitted writes are charged to their cause.
    pub(crate) async fn publish_deferred(
        &mut self,
        db: &Db,
        blobs: &BlobStoreHandle,
        sender: Option<&ReplicationSender>,
        local_did: &str,
        ids: Vec<String>,
    ) {
        for (id, published) in publish_deferred(db, blobs, sender, local_did, ids).await {
            if self.deferred_charged.remove(&id) {
                self.charge_published(&published);
            }
        }
    }

    /// Charge a committed write whose row is not published now (no sender, full queue): its journal facts only.
    pub(crate) async fn charge_committed(&mut self, db: &Db, node_id: &str) {
        self.charge_journal(db, node_id).await;
    }

    /// Debit measured bytes the author caused outside one node's latest commit (an import's restored history).
    pub(crate) fn charge_bytes(&mut self, verse: &str, author: &str, bytes: usize) {
        let now = (self.clock)();
        self.budgets.charge(verse, author, bytes, now);
    }

    fn charge_published(&mut self, published: &Published) {
        let now = (self.clock)();
        self.budgets.charge(&published.verse_id, &published.cause, published.bytes, now);
    }

    /// Charge the journal facts of the node's latest commit (one HLC) to their `source_did`.
    async fn charge_journal(&mut self, db: &Db, node_id: &str) {
        let measured = async {
            let verse = self.verse_of_node(db, node_id).await?;
            let mut response = db
                .query(format!("SELECT row_version, source_did, hlc_timestamp, bytes::len(encoding::cbor::encode($this)) AS encoded FROM node_log WHERE node_id=$budget_node ORDER BY row_version DESC LIMIT {COMMIT_FACTS} TIMEOUT 5s"))
                .bind(("budget_node", node_id.to_owned()))
                .await?
                .check()?;
            let rows: Vec<Value> = response.take(0)?;
            Ok::<_, anyhow::Error>((verse, rows))
        }
        .await;
        let (verse, rows) = match measured {
            Ok((Some(verse), rows)) => (verse, rows),
            Ok((None, _)) => return,
            Err(error) => {
                tracing::debug!(node_id, "node write budget could not measure the commit: {error:#}");
                return;
            }
        };
        let Some(latest) = rows.first() else { return };
        let (latest_hlc, latest_version) = (latest["hlc_timestamp"].clone(), latest["row_version"].as_i64());
        let already = self.charged.get(node_id).copied();
        let now = (self.clock)();
        for row in rows.iter().take_while(|row| row["hlc_timestamp"] == latest_hlc) {
            if already.is_some_and(|charged| row["row_version"].as_i64().is_some_and(|version| version <= charged)) {
                continue;
            }
            // CBOR bytes, as the store encodes the fact (the plugin host measures rows the same way).
            let bytes = row["encoded"].as_u64().unwrap_or(0) as usize;
            let author = row["source_did"].as_str().unwrap_or_default();
            self.budgets.charge(&verse, author, bytes, now);
        }
        if let Some(version) = latest_version {
            if self.charged.len() >= TRACKED_IDS && !self.charged.contains_key(node_id) {
                self.charged.clear();
            }
            self.charged.insert(node_id.to_owned(), version);
        }
    }
}

/// Insert into a bounded id map, restarting it when full (a forgotten id is only looked up again).
fn remember(map: &mut HashMap<String, String>, key: String, value: String) {
    if map.len() >= TRACKED_IDS && !map.contains_key(&key) {
        map.clear();
    }
    map.insert(key, value);
}
