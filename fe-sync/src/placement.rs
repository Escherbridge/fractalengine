//! Capacity-aware shard placement (M2/F6 — A13/A14) — the pure planner the
//! sync thread's fabric consults when a shard is first seen. See
//! `fe-sync/src/AGENTS.md` §sharding.
//!
//! Every function here is **pure** (deterministic on its inputs, no I/O, no
//! clocks): placement is the coordination-free decision of a no-coordinator
//! fabric — the ingesting peer plans when it first sees a shard and publishes
//! the host set as the shard's ledger row, and every peer replays the same
//! decision from the same inputs if it ever re-plans. D2's ratified
//! properties are pinned by the tests below: mode honored (mirror = all,
//! sharded = one host, balanced = R hosts), capacity respected, the smallest
//! peer never caps total hosted data, overflow spills to fallback/relay
//! seeders, and low-utilization hosts are preferred.

use std::collections::BTreeMap;

use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings};

use crate::sharding::PeerDeclaration;

/// How many deterministic probes the "power of choices" pick evaluates. A
/// pure "least-utilized" pick funnels every shard of an unloaded fleet onto
/// one peer; a pure hash ignores load. Probing W hash-derived candidates and
/// keeping the least-utilized of them blends both: load spreads across the
/// fleet while pressure steers placement to the underloaded. 4 is the classic
/// power-of-two-choices neighborhood, widened for determinism headroom.
const PROBES: usize = 4;

/// The plan for one shard: who hosts it, and whether capacity was exceeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementPlan {
    /// Hosting peers' DIDs, distinct, in pick order.
    pub hosts: Vec<String>,
    /// True when the last-resort branch placed a host past every eligible
    /// peer's declared capacity — a shard must live somewhere, so the
    /// planner refuses to leave it homeless; the caller logs loudly.
    pub overflowed: bool,
}

/// Where a row transfer must reach, per mode (D2 #1): `mirror` fans to every
/// peer; `sharded`/`balanced` to the shard's host set (seeders included for
/// overflowed shards, since a seeder hosting a shard IS a host).
///
/// The route is the record the ledger row publishes and the seam F7's
/// targeted transport consumes; on the doc transport every subscriber
/// physically receives the entry, and each peer's retention decision
/// (below) enforces the route's host set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferRoute {
    /// All online peers must hold the row (mirror).
    Broadcast,
    /// Only these peers must hold the row (sharded/balanced).
    Targeted(Vec<String>),
}

/// Whether the local peer retains a reading for a shard (transfer routing,
/// receive side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Keep the row: the local peer hosts this shard (or the mode says all
    /// peers do, or the shard's ledger has not converged yet — the safe
    /// default, because dropping a hosted row loses data while
    /// over-retaining is harmless under union semantics).
    Retain,
    /// Do not apply: another peer hosts this shard.
    Skip,
}

/// Plan the host set for one shard.
///
/// - `shard_key` — the doc key identity (`{petal}/{anchor}/{bucket}`); it
///   drives the deterministic pick, so the same shard always lands on the
///   same hosts for identical inputs.
/// - `size_bytes` — the planner's estimate of the shard's size at first
///   sight (the first row's byte length; capacity is a planning hint, not
///   an enforcement wall).
/// - `peers` — every declared peer including the local one.
/// - `assigned` — each peer's currently-planned total (the fabric's fold
///   over its shard ledger).
pub fn plan_shard_hosts(
    shard_key: &str,
    size_bytes: u64,
    peers: &BTreeMap<String, PeerDeclaration>,
    assigned: &BTreeMap<String, u64>,
    settings: &VerseTimeseriesSettings,
) -> PlacementPlan {
    // Mirror mode: full replication by contract — capacity hints deliberately
    // do not gate a mode whose point is "everyone holds everything".
    if settings.mode == TimeseriesMode::Mirror {
        return PlacementPlan {
            hosts: peers.keys().cloned().collect(),
            overflowed: false,
        };
    }

    let wanted = match settings.mode {
        TimeseriesMode::Sharded => 1,
        // R is a request (the slider), never a hard requirement: it clamps
        // to the peers a shard can actually reach (A13's 1..N).
        TimeseriesMode::Mirror | TimeseriesMode::Balanced => {
            settings.replication_factor.min(peers.len().max(1) as u32) as usize
        }
    };

    let mut hosts: Vec<String> = Vec::with_capacity(wanted);
    let mut overflowed = false;
    if peers.is_empty() {
        return PlacementPlan { hosts, overflowed };
    }

    // Running assigned totals so the R replicas of one shard see each
    // other's load (each replica costs its host the shard's size).
    let mut running: BTreeMap<String, u64> = assigned.clone();
    // Utilization denominator for unlimited-capacity peers: the fleet's
    // largest declared capacity keeps ratio and absolute load comparable.
    let fleet_scale = peers
        .values()
        .filter_map(|p| p.capacity_bytes)
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    for round in 0..wanted {
        match pick_host(
            shard_key,
            round,
            size_bytes,
            peers,
            &running,
            fleet_scale,
            &hosts,
            // Declared capacity is absolute whenever ANY host fits: extra
            // replica slots are dropped (fewer than R hosts, honestly) rather
            // than breaking a capacity declaration. Only a shard that would
            // otherwise be HOMELESS may be placed over capacity.
            hosts.is_empty(),
        ) {
            Some((did, past_capacity)) => {
                overflowed |= past_capacity;
                *running.entry(did.clone()).or_insert(0) += size_bytes;
                hosts.push(did);
            }
            None => break,
        }
    }
    PlacementPlan { hosts, overflowed }
}

/// Pick one host for replica slot `round`, preferring non-seeder peers with
/// remaining capacity and low utilization, then seeders, then (only when
/// `allow_last_resort` and loudly) any least-utilized peer. Returns the
/// chosen DID and whether the pick went past every eligible peer's declared
/// capacity.
#[allow(clippy::too_many_arguments)]
fn pick_host(
    shard_key: &str,
    round: usize,
    size_bytes: u64,
    peers: &BTreeMap<String, PeerDeclaration>,
    running: &BTreeMap<String, u64>,
    fleet_scale: f64,
    already: &[String],
    allow_last_resort: bool,
) -> Option<(String, bool)> {
    let has_room = |did: &str, decl: &PeerDeclaration| -> bool {
        match decl.capacity_bytes {
            None => true,
            Some(cap) => running.get(did).copied().unwrap_or(0) + size_bytes <= cap,
        }
    };
    // Regular pool: non-seeder peers with remaining capacity. Seeders are
    // the overflow tier (D2 #3/#4), deliberately excluded here so a seeder
    // with room is never consumed by regular placement and stays available
    // for the spillover branch below.
    let eligible: Vec<&String> = peers
        .keys()
        .filter(|did| !already.contains(did))
        .filter(|did| !peers[*did].seeder)
        .filter(|did| has_room(did, &peers[*did]))
        .collect();
    if !eligible.is_empty() {
        return Some((
            choose_by_hash(shard_key, round, &eligible, peers, running, fleet_scale),
            false,
        ));
    }
    // Regular capacity is exhausted for this slot: overflow to seeders —
    // the fallback/relay nodes whose whole role is absorbing shards the
    // regular peers cannot (D2 #3, A14). A seeding pick within the seeder's
    // own capacity is spillover, not over-capacity.
    let seeders: Vec<&String> = peers
        .keys()
        .filter(|did| !already.contains(did))
        .filter(|did| peers[*did].seeder && has_room(did, &peers[*did]))
        .collect();
    if !seeders.is_empty() {
        return Some((
            choose_by_hash(shard_key, round, &seeders, peers, running, fleet_scale),
            false,
        ));
    }
    // Nothing fits anywhere. A shard must live somewhere: place on the
    // least-utilized peer regardless of its declared capacity (the caller
    // reports this loudly — an over-capacity host beats invisible data).
    // Only reachable for a homeless shard (see `allow_last_resort`).
    if !allow_last_resort {
        return None;
    }
    let rest: Vec<&String> = peers.keys().filter(|did| !already.contains(did)).collect();
    if rest.is_empty() {
        return None;
    }
    Some((
        choose_by_hash(shard_key, round, &rest, peers, running, fleet_scale),
        true,
    ))
}

/// Deterministic "power of choices" pick over `candidates`: probe
/// [`PROBES`] hash-derived indices and keep the least-utilized of the
/// probed peers (ties break by DID, so the result is a pure function of the
/// inputs). A candidate set no larger than [`PROBES`] is probed in full —
/// small fleets always cover their underloaded peers, while large fleets
/// get the hash spread that gives `sharded`/`balanced` their distribution.
fn choose_by_hash(
    shard_key: &str,
    round: usize,
    candidates: &[&String],
    peers: &BTreeMap<String, PeerDeclaration>,
    running: &BTreeMap<String, u64>,
    fleet_scale: f64,
) -> String {
    let utilization = |did: &String| -> f64 {
        let assigned = running.get(did.as_str()).copied().unwrap_or(0) as f64;
        let denom = peers
            .get(did.as_str())
            .and_then(|p| p.capacity_bytes)
            .map(|c| c as f64)
            .unwrap_or(fleet_scale)
            .max(1.0);
        assigned / denom
    };
    let len = candidates.len();
    let mut probed: Vec<usize> = if len <= PROBES {
        (0..len).collect()
    } else {
        (0..PROBES)
            .map(|probe| {
                let hash = blake3::hash(format!("{shard_key}:{round}:{probe}").as_bytes());
                let word = u64::from_le_bytes(hash.as_bytes()[0..8].try_into().unwrap_or([0; 8]));
                (word % len as u64) as usize
            })
            .collect()
    };
    probed.sort_unstable();
    probed.dedup();
    let mut best: Option<&String> = None;
    for idx in probed {
        let candidate = candidates[idx];
        best = match best {
            None => Some(candidate),
            Some(current)
                if (utilization(candidate), candidate) < (utilization(current), current) =>
            {
                Some(candidate)
            }
            Some(current) => Some(current),
        };
    }
    best.expect("candidates is non-empty by every caller")
        .clone()
}

/// The transfer route a published row takes, per mode.
pub fn transfer_route(settings: &VerseTimeseriesSettings, hosts: &[String]) -> TransferRoute {
    match settings.mode {
        TimeseriesMode::Mirror => TransferRoute::Broadcast,
        TimeseriesMode::Sharded | TimeseriesMode::Balanced => {
            TransferRoute::Targeted(hosts.to_vec())
        }
    }
}

/// Whether the local peer retains a reading for the shard whose ledger hosts
/// are `hosts` (`None` = the shard's ledger has not converged locally yet).
pub fn retention_decision(
    settings: &VerseTimeseriesSettings,
    hosts: Option<&[String]>,
    local_did: &str,
) -> Retention {
    match settings.mode {
        // Mirror: every peer holds every shard — the pre-F6 behavior.
        TimeseriesMode::Mirror => Retention::Retain,
        TimeseriesMode::Sharded | TimeseriesMode::Balanced => match hosts {
            // Unknown shard: retain. The ledger row and the first reading
            // race through the doc, and the safe direction is to keep —
            // union semantics make over-retention idempotent, while a
            // wrongly-dropped hosted row would be data loss.
            None => Retention::Retain,
            Some(hosts) if hosts.iter().any(|h| h == local_did) => Retention::Retain,
            Some(_) => Retention::Skip,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS;

    fn decl(capacity: Option<u64>, seeder: bool) -> PeerDeclaration {
        PeerDeclaration {
            capacity_bytes: capacity,
            seeder,
        }
    }

    fn peers(spec: &[(&str, Option<u64>, bool)]) -> BTreeMap<String, PeerDeclaration> {
        spec.iter()
            .map(|(did, cap, seeder)| (did.to_string(), decl(*cap, *seeder)))
            .collect()
    }

    fn settings(mode: TimeseriesMode, r: u32) -> VerseTimeseriesSettings {
        VerseTimeseriesSettings {
            mode,
            replication_factor: r,
            bucket_width_ms: DEFAULT_BUCKET_WIDTH_MS,
        }
    }

    fn plan(
        shard: &str,
        size: u64,
        peers: &BTreeMap<String, PeerDeclaration>,
        settings: &VerseTimeseriesSettings,
    ) -> PlacementPlan {
        plan_shard_hosts(shard, size, peers, &BTreeMap::new(), settings)
    }

    // ------------------------------------------------------------------
    // A13: mode honored by placement
    // ------------------------------------------------------------------

    #[test]
    fn mirror_places_every_declared_peer() {
        let peers = peers(&[("a", None, false), ("b", None, false), ("c", None, false)]);
        let p = plan("p/x/0", 1_000, &peers, &settings(TimeseriesMode::Mirror, 1));
        assert_eq!(p.hosts, vec!["a", "b", "c"]);
        assert!(!p.overflowed);
    }

    #[test]
    fn sharded_places_exactly_one_host() {
        let peers = peers(&[("a", None, false), ("b", None, false), ("c", None, false)]);
        for shard in ["p/x/0", "p/y/1", "p/z/2"] {
            let p = plan(shard, 1_000, &peers, &settings(TimeseriesMode::Sharded, 1));
            assert_eq!(p.hosts.len(), 1, "{shard} must have exactly one host");
            assert!(!p.overflowed);
        }
    }

    #[test]
    fn balanced_respects_the_replication_factor_slider() {
        let peers = peers(&[
            ("a", None, false),
            ("b", None, false),
            ("c", None, false),
            ("d", None, false),
            ("e", None, false),
        ]);
        for r in 1..=5u32 {
            let p = plan(
                "p/x/0",
                1_000,
                &peers,
                &settings(TimeseriesMode::Balanced, r),
            );
            assert_eq!(p.hosts.len(), r as usize, "R={r} must yield R hosts");
            // Distinct hosts — a shard never replicates onto one peer twice.
            let mut sorted = p.hosts.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), p.hosts.len());
        }
    }

    #[test]
    fn balanced_r_clamps_to_the_peer_count() {
        let peers = peers(&[("a", None, false), ("b", None, false)]);
        let p = plan(
            "p/x/0",
            1_000,
            &peers,
            &settings(TimeseriesMode::Balanced, 9),
        );
        assert_eq!(
            p.hosts.len(),
            2,
            "R> N clamps to N (the slider requests, never requires)"
        );
    }

    #[test]
    fn placement_is_deterministic_for_identical_inputs() {
        let peers = peers(&[("a", None, false), ("b", None, false), ("c", None, false)]);
        let s = settings(TimeseriesMode::Balanced, 2);
        let first = plan("p/x/0", 1_000, &peers, &s);
        for _ in 0..8 {
            assert_eq!(plan("p/x/0", 1_000, &peers, &s), first);
        }
    }

    // ------------------------------------------------------------------
    // A14: capacity respected, smallest never caps, overflow to seeders
    // ------------------------------------------------------------------

    #[test]
    fn capacity_is_respected_no_host_exceeds_its_declaration() {
        // a: 4 shards of room, b: 4, c (seeder): unlimited but reserved for
        // overflow. Six 5KB shards at R=2: the four full pairs saturate the
        // two regulars exactly (seeders never absorb regular slots); the last
        // two shards then take a single seeder host each — an honest
        // fewer-than-R placement, never a broken capacity declaration.
        let peers = peers(&[
            ("a", Some(20_000), false),
            ("b", Some(20_000), false),
            ("c", None, true),
        ]);
        let mut assigned: BTreeMap<String, u64> = BTreeMap::new();
        let s = settings(TimeseriesMode::Balanced, 2);
        let mut full_replica_pairs = 0usize;
        for i in 0..6 {
            let p = plan_shard_hosts(&format!("p/x/{i}"), 5_000, &peers, &assigned, &s);
            assert!(
                !p.hosts.is_empty(),
                "shard {i} placed nowhere — data would be invisible"
            );
            assert!(
                !p.overflowed,
                "a seeder was available — over-capacity is never reached"
            );
            if p.hosts.len() == 2 {
                full_replica_pairs += 1;
            }
            for h in &p.hosts {
                *assigned.entry(h.clone()).or_insert(0) += 5_000;
                let cap = peers[h].capacity_bytes.unwrap_or(u64::MAX);
                assert!(
                    assigned[h] <= cap,
                    "host {h} exceeded its declared capacity ({} > {cap})",
                    assigned[h]
                );
            }
        }
        assert_eq!(
            full_replica_pairs, 4,
            "the regulars' combined room is exactly four R=2 pairs"
        );
    }

    #[test]
    fn seeders_are_reserved_for_overflow_never_regular_placement() {
        // A seeder with room is never consumed by regular placement while
        // non-seeders can host the shard — the overflow tier stays in
        // reserve (D2 #3/#4). (A seeder still serves as the *second* replica
        // of an R=2 shard when it is the only remaining host — that is
        // spillover, and `overflow_spills_to_fallback_and_relay_seeders`
        // pins that branch.)
        let peers = peers(&[
            ("r1", Some(1_000_000), false),
            ("r2", Some(1_000_000), false),
            ("seeder", None, true),
        ]);
        let assigned: BTreeMap<String, u64> = BTreeMap::new();
        let s = settings(TimeseriesMode::Balanced, 2);
        for i in 0..8 {
            let p = plan_shard_hosts(&format!("p/x/{i}"), 1_000, &peers, &assigned, &s);
            assert_eq!(
                p.hosts.len(),
                2,
                "shard {i}: R=2 over two eligible regulars yields 2 hosts"
            );
            assert!(
                !p.hosts.contains(&"seeder".to_string()),
                "shard {i}: the seeder must stay reserved while regulars have room"
            );
        }
    }

    #[test]
    fn smallest_peer_never_caps_total_hosted_data() {
        // The smallest peer can hold 1 shard. Six shards (2 hosts each) are
        // placed anyway: total assigned across the fleet is 6*5_000*2/…
        // far beyond the smallest capacity — the fleet total is bounded by
        // the SUM of capacities plus seeders, never the MINIMUM (D2 #3).
        let peers = peers(&[
            ("tiny", Some(5_000), false),
            ("big", Some(1_000_000), false),
            ("relay", None, true),
        ]);
        let mut assigned: BTreeMap<String, u64> = BTreeMap::new();
        let s = settings(TimeseriesMode::Balanced, 2);
        let mut placed = 0usize;
        for i in 0..10 {
            let p = plan_shard_hosts(&format!("p/x/{i}"), 5_000, &peers, &assigned, &s);
            assert_eq!(p.hosts.len(), 2, "shard {i} placed with 2 hosts");
            for h in &p.hosts {
                *assigned.entry(h.clone()).or_insert(0) += 5_000;
            }
            placed += 1;
        }
        assert_eq!(placed, 10, "placement never stops at the smallest peer");
        // `tiny` never exceeded its 5_000-byte declaration.
        assert!(assigned["tiny"] <= 5_000);
    }

    #[test]
    fn overflow_spills_to_fallback_and_relay_seeders() {
        // Two full regulars; the only room left is the seeder.
        let peers = peers(&[
            ("a", Some(4_000), false),
            ("b", Some(4_000), false),
            ("s", None, true),
        ]);
        let mut assigned: BTreeMap<String, u64> = BTreeMap::new();
        assigned.insert("a".to_string(), 4_000);
        assigned.insert("b".to_string(), 4_000);
        let s = settings(TimeseriesMode::Sharded, 1);
        let p = plan_shard_hosts("p/x/0", 4_000, &peers, &assigned, &s);
        assert_eq!(p.hosts, vec!["s"], "a full fleet overflows to the seeder");
        assert!(
            !p.overflowed,
            "the seeder had room — this is spillover, not over-capacity"
        );
    }

    #[test]
    fn no_eligible_peer_anywhere_is_a_loud_last_resort_not_a_drop() {
        // Everyone is full and there are no seeders: the shard still gets a
        // home (the least-utilized probed peer), flagged so the caller can warn.
        let peers = peers(&[("a", Some(10_000), false), ("b", Some(40_000), false)]);
        let mut assigned: BTreeMap<String, u64> = BTreeMap::new();
        assigned.insert("a".to_string(), 20_000); // over-full, ratio 2.0
        assigned.insert("b".to_string(), 40_000); // full, ratio 1.0 — least utilized
        let s = settings(TimeseriesMode::Sharded, 1);
        let p = plan_shard_hosts("p/x/0", 50_000, &peers, &assigned, &s);
        assert_eq!(
            p.hosts,
            vec!["b"],
            "least-utilized peer wins the last resort"
        );
        assert!(p.overflowed, "an over-capacity pick must be flagged");
    }

    #[test]
    fn placement_prefers_low_utilization_peers() {
        // `idle` hosts nothing; `busy` already hosts 300k of 1M. Every probe
        // pair contains both, so utilization must decide every time.
        let peers = peers(&[
            ("busy", Some(1_000_000), false),
            ("idle", Some(1_000_000), false),
        ]);
        let mut assigned: BTreeMap<String, u64> = BTreeMap::new();
        assigned.insert("busy".to_string(), 300_000);
        let s = settings(TimeseriesMode::Sharded, 1);
        let mut idle_picks = 0;
        for i in 0..16 {
            let p = plan_shard_hosts(&format!("p/x/{i}"), 10_000, &peers, &assigned, &s);
            if p.hosts == ["idle"] {
                idle_picks += 1;
            }
        }
        assert_eq!(
            idle_picks, 16,
            "power-of-probes must prefer the underloaded peer every time"
        );
    }

    #[test]
    fn single_peer_fleet_places_on_itself() {
        let peers = peers(&[("a", None, false)]);
        let p = plan(
            "p/x/0",
            1_000,
            &peers,
            &settings(TimeseriesMode::Sharded, 1),
        );
        assert_eq!(p.hosts, vec!["a"]);
        let p = plan(
            "p/x/0",
            1_000,
            &peers,
            &settings(TimeseriesMode::Balanced, 3),
        );
        assert_eq!(p.hosts, vec!["a"]);
    }

    // ------------------------------------------------------------------
    // Transfer routing + retention (A13: honored by transfer)
    // ------------------------------------------------------------------

    #[test]
    fn transfer_route_mirrors_broadcast_and_shards_target() {
        let mirror = settings(TimeseriesMode::Mirror, 1);
        let hosts = vec!["a".to_string(), "b".to_string()];
        assert_eq!(transfer_route(&mirror, &hosts), TransferRoute::Broadcast);
        for mode in [TimeseriesMode::Sharded, TimeseriesMode::Balanced] {
            assert_eq!(
                transfer_route(&settings(mode, 1), &hosts),
                TransferRoute::Targeted(hosts.clone())
            );
        }
    }

    #[test]
    fn retention_mirrors_always_and_shards_follow_the_host_set() {
        let local = "me";
        // Mirror: everyone retains, regardless of any ledger.
        assert_eq!(
            retention_decision(
                &settings(TimeseriesMode::Mirror, 1),
                Some(&["other".to_string()]),
                local
            ),
            Retention::Retain
        );
        let sharded = settings(TimeseriesMode::Sharded, 1);
        assert_eq!(
            retention_decision(&sharded, Some(&["me".to_string()]), local),
            Retention::Retain
        );
        assert_eq!(
            retention_decision(&sharded, Some(&["other".to_string()]), local),
            Retention::Skip
        );
        // Unknown shard (ledger not converged): the safe default keeps the row.
        assert_eq!(retention_decision(&sharded, None, local), Retention::Retain);
    }

    #[test]
    fn balanced_r1_and_sharded_agree_on_host_count() {
        // balanced with R=1 IS the sharded contract (one host per shard).
        let peers = peers(&[("a", None, false), ("b", None, false)]);
        let assigned: BTreeMap<String, u64> = BTreeMap::new();
        let sharded = plan_shard_hosts(
            "p/x/0",
            1_000,
            &peers,
            &assigned,
            &settings(TimeseriesMode::Sharded, 1),
        );
        let balanced = plan_shard_hosts(
            "p/x/0",
            1_000,
            &peers,
            &assigned,
            &settings(TimeseriesMode::Balanced, 1),
        );
        assert_eq!(sharded.hosts.len(), 1);
        assert_eq!(balanced.hosts.len(), 1);
    }
}
