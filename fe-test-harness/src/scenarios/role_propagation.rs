//! Scenario: Role Propagation end-to-end (RP-8) — the real `AssignRole`/
//! `RevokeRole` handlers (RP-2 §2.3), six peers, real loopback transport.
//!
//! Proves SYNC-ROLE-PROPAGATION and P2P-REF-CONFINE through the production
//! seam, not a unit fixture: every grant, revoke and content row crosses the
//! real sync thread, and every admission decision is the receiving peer's
//! OWN materialization of the statements it received — never locally
//! provisioned. See plan `role_propagation_20261013/plan.md` §RP-8 and
//! fe-database/src/AGENTS.md §role-propagation.
//!
//! Cast: alice (owner of verse A, and of verse B for the confinement leg),
//! bob (granted Editor, then Manager, then revoked), carol (a FRESH peer —
//! never granted anything directly; every admission on her is pure
//! propagation), dave (granted Editor by bob, then cascades when bob is
//! revoked — T4), erin (granted Manager scoped to ONE petal — R7 subtree),
//! frank (granted Editor by erin, inside that one petal only).
//!
//! Legs:
//! 1. Editor grant (alice→bob): carol admits bob's node row AND his reading
//!    with no local provisioning (SYNC-ROLE-PROPAGATION acceptance).
//! 2. Manager grant + re-grant (alice→bob→dave): carol admits dave's row
//!    through a grantor whose own authority came from the owner.
//! 3. Petal-scoped Manager (alice→erin→frank, R7 subtree): carol admits
//!    frank's row in the Manager's OWN petal, and Defers his row in the
//!    sibling petal the Manager has no authority over.
//! 4. Revocation cascade (T4): alice revokes bob, and dave's next row
//!    (admitted only through bob's now-void authority) Defers on carol.
//! 5. P2P-REF-CONFINE (two-peer): a node bob plants in verse A, naming a
//!    petal that genuinely belongs to verse B, is Denied on carol — never
//!    merely deferred, because carol already knows that petal as foreign.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use fe_runtime::messages::{
    DbCommand, DbResult, DeferralReason, IotReadingInput, ReplicatedRowOutcome, SourceClass,
};
use fe_sync::messages::SyncCommand;

use crate::peer::TestPeer;
use crate::scenarios::scaffold::{self, Scaffold};
use crate::TestResult;

/// How long a grant, revoke or content row may take to converge over the
/// real transport.
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a Manager-issued grant may retry while the GRANTOR's own
/// authority is still converging onto their own replica (§2.3: a local
/// `AssignRole` checks the caller's LOCAL materialization, which needs the
/// owner's statement to have arrived first).
const AUTH_CONVERGE_TIMEOUT: Duration = Duration::from_secs(20);

/// Publish one row from `author` (blob-store, then `WriteRowEntry`) — the
/// same shape as `scaffold::publish_and_await`, kept local because this
/// scenario sometimes wants the FIRST echo (deferred or denied), not the
/// settled one `publish_and_await` waits for.
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
            table: table.to_string(),
            record_id: record_id.to_string(),
            content_hash,
        })
        .map_err(|e| anyhow!("failed to publish {table}/{record_id}: {e}"))?;
    Ok(())
}

/// The standard node row shape (see `scaffold.rs` and
/// `fe-database/src/handlers/replicated_row.rs::node_in`).
fn node_row(node_id: &str, petal_id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "node_id": node_id,
        "petal_id": petal_id,
        "display_name": name,
        "position": { "type": "Point", "coordinates": [0.0, 0.0] },
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
        "created_at": chrono::Utc::now().to_rfc3339(),
    })
}

/// The FIRST echo for one `(table, record_id)` — used where the scenario
/// wants to see the immediate verdict itself (Denied, or a Defer that will
/// never resolve), not the eventually-settled outcome.
fn wait_outcome(
    receiver: &TestPeer,
    table: &str,
    record_id: &str,
    timeout: Duration,
) -> Result<ReplicatedRowOutcome> {
    match receiver.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { table: t, record_id: id, .. }
                    if t == table && id == record_id
            )
        },
        timeout,
    )? {
        DbResult::ReplicatedRowApplied { outcome, .. } => Ok(outcome),
        other => bail!("unexpected result waiting for {table}/{record_id}: {other:?}"),
    }
}

/// The SETTLED outcome for the next row of `table` in `verse_id` — skips any
/// transient `Deferred` echo (a row that arrived before the grantor's role
/// did) and returns once it resolves. Used for the reading leg, whose
/// `reading_id` the caller cannot predict (so it cannot match on a specific
/// `record_id` the way `wait_outcome` does).
fn wait_table_settled(
    receiver: &TestPeer,
    verse_id: &str,
    table: &str,
    timeout: Duration,
) -> Result<ReplicatedRowOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                anyhow!(
                    "timed out waiting for a settled {table} outcome on {}",
                    receiver.name
                )
            })?;
        match receiver.wait_for(
            |r| matches!(r, DbResult::ReplicatedRowApplied { verse_id: v, table: t, .. } if v == verse_id && t == table),
            remaining,
        )? {
            DbResult::ReplicatedRowApplied { outcome: ReplicatedRowOutcome::Deferred(_), .. } => {
                continue
            }
            DbResult::ReplicatedRowApplied { outcome, .. } => return Ok(outcome),
            other => bail!("unexpected result waiting for a {table} outcome: {other:?}"),
        }
    }
}

/// Assign a role as the OWNER (never contended — a single attempt suffices).
fn assign_role(granter: &TestPeer, peer_did: &str, scope: &str, role: &str) -> Result<()> {
    granter.send(DbCommand::AssignRole {
        peer_did: peer_did.to_string(),
        scope: scope.to_string(),
        role: role.to_string(),
    });
    match granter.wait_for(|r| matches!(r, DbResult::RoleAssigned { .. }), SYNC_TIMEOUT)? {
        DbResult::RoleAssigned { .. } => Ok(()),
        other => bail!("unexpected result assigning role: {other:?}"),
    }
}

/// Assign a role as a MANAGER whose own authority was itself just granted:
/// the local `AssignRole` check reads the grantor's OWN materialization
/// (§2.3), which needs the owner's statement to have reached them first.
/// Retries (not a single wait) because that convergence is a real network
/// race, never a bug to fail fast on.
fn assign_role_retrying(
    granter: &TestPeer,
    peer_did: &str,
    scope: &str,
    role: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        granter.send(DbCommand::AssignRole {
            peer_did: peer_did.to_string(),
            scope: scope.to_string(),
            role: role.to_string(),
        });
        match granter.wait_for(
            |r| matches!(r, DbResult::RoleAssigned { .. }),
            Duration::from_millis(600),
        ) {
            Ok(_) => return Ok(()),
            Err(e) => {
                if Instant::now() >= deadline {
                    bail!(
                        "{}'s own authority at '{scope}' never converged in time: {e}",
                        granter.name
                    );
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Revoke as the OWNER (pins the cell — never contended).
fn revoke_role(owner: &TestPeer, peer_did: &str, scope: &str) -> Result<()> {
    owner.send(DbCommand::RevokeRole {
        peer_did: peer_did.to_string(),
        scope: scope.to_string(),
    });
    match owner.wait_for(|r| matches!(r, DbResult::RoleRevoked { .. }), SYNC_TIMEOUT)? {
        DbResult::RoleRevoked { .. } => Ok(()),
        other => bail!("unexpected result revoking role: {other:?}"),
    }
}

/// T4: after a revoke whose cascade has not necessarily reached `receiver`
/// yet, publish fresh rows from `author` until one DEFERS on `author`'s own
/// now-void authority — the same "poll until the placement/convergence
/// combo is right" idiom `distributed_query.rs::ingest_until` uses for shard
/// placement races, applied here to a role-cascade race.
fn publish_until_deferred(
    author: &TestPeer,
    receiver: &TestPeer,
    verse_id: &str,
    petal_id: &str,
    name_prefix: &str,
    expect_did: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let node_id = format!("{name_prefix}-{}", ulid::Ulid::new());
        publish_row(
            author,
            verse_id,
            "node",
            &node_id,
            &serde_json::to_vec(&node_row(&node_id, petal_id, name_prefix))?,
        )?;
        let outcome = wait_outcome(receiver, "node", &node_id, Duration::from_secs(10))?;
        match &outcome {
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole { did })
                if did == expect_did =>
            {
                return Ok(node_id)
            }
            _ if Instant::now() >= deadline => bail!(
                "{}'s row never deferred on the revoked cascade (last outcome: {outcome:?})",
                author.name
            ),
            _ => {
                tracing::debug!("cascade not yet converged on {outcome:?} — retrying");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Create a node LOCALLY (no publish) — the author's own durable copy,
/// needed before that author can locally ingest a reading against it.
fn create_local_node(peer: &TestPeer, petal_id: &str, name: &str) -> Result<String> {
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
        other => bail!("unexpected result creating a local node: {other:?}"),
    }
}

pub fn run() -> Result<TestResult> {
    const NAME: &str = "role_propagation";
    let tmp = tempfile::tempdir()?;

    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;
    let carol = TestPeer::spawn("carol", tmp.path())?;
    let dave = TestPeer::spawn("dave", tmp.path())?;
    let erin = TestPeer::spawn("erin", tmp.path())?;
    let frank = TestPeer::spawn("frank", tmp.path())?;

    let alice_addr = alice
        .sync_node_addr
        .clone()
        .ok_or_else(|| anyhow!("alice is offline — this scenario needs real transport"))?;
    for (peer, who) in [
        (&bob, "bob"),
        (&carol, "carol"),
        (&dave, "dave"),
        (&erin, "erin"),
        (&frank, "frank"),
    ] {
        peer.sync_node_addr
            .clone()
            .ok_or_else(|| anyhow!("{who} is offline"))?;
    }

    let alice_did = alice.keypair.to_did_key();
    let bob_did = bob.keypair.to_did_key();
    let dave_did = dave.keypair.to_did_key();
    let erin_did = erin.keypair.to_did_key();
    let frank_did = frank.keypair.to_did_key();

    let non_owner: [&TestPeer; 5] = [&bob, &carol, &dave, &erin, &frank];

    // --- Setup: verse A, owned by alice, with a fractal and THREE petals
    //     (P0 shared content, P1 the petal-scoped Manager's own petal, P2
    //     its untouched sibling). ------------------------------------------
    alice.send(DbCommand::CreateVerse {
        name: "Role Propagation Verse".into(),
    });
    let verse_a_id = match alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::VerseCreated { id, .. } => id,
        other => bail!("unexpected result creating verse A: {other:?}"),
    };
    let ns_secret_a = alice
        .namespace_secret(&verse_a_id)
        .ok_or_else(|| anyhow!("verse A secret missing from the test secret map"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_a)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_a = hex::encode(fe_database::derive_namespace_id(&secret_bytes));

    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_a_id.clone(),
            namespace_id: ns_id_a.clone(),
            namespace_secret: Some(ns_secret_a.clone()),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow!("failed to open alice's verse-A replica: {e}"))?;
    // Beat for alice's gossip subscribe to land before joiners dial her
    // (distributed_query.rs's same caveat: a Join for a not-yet-subscribed
    // topic is dropped, one-shot, no retry).
    std::thread::sleep(Duration::from_millis(500));
    for (peer, who) in [
        (&bob, "bob"),
        (&carol, "carol"),
        (&dave, "dave"),
        (&erin, "erin"),
        (&frank, "frank"),
    ] {
        peer.sync_cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: verse_a_id.clone(),
                namespace_id: ns_id_a.clone(),
                namespace_secret: Some(ns_secret_a.clone()),
                bootstrap_peers: vec![alice_addr.clone()],
            })
            .map_err(|e| anyhow!("failed to open {who}'s verse-A replica: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(500));

    scaffold::publish_manifest(
        &alice,
        &non_owner,
        &verse_a_id,
        &alice_did,
        "Role Propagation Verse",
    )?;

    // The hierarchy is created on alice's own DB first (production shape): an
    // author's published rows are never echoed back to its DB, and a grant at a
    // fractal/petal scope (leg 3) requires that scope to exist on the granter
    // (`AssignRole` precondition).
    let fractal_id =
        scaffold::create_local_fractal(&alice, &verse_a_id, "Role Propagation Fractal")?;
    let petal_p0 = scaffold::create_local_petal(&alice, &fractal_id, "P0")?;
    let petal_p1 = scaffold::create_local_petal(&alice, &fractal_id, "P1")?;
    let petal_p2 = scaffold::create_local_petal(&alice, &fractal_id, "P2")?;
    let now = chrono::Utc::now().to_rfc3339();
    let fractal = serde_json::json!({
        "fractal_id": fractal_id,
        "verse_id": verse_a_id,
        "owner_did": alice_did,
        "name": "Role Propagation Fractal",
        "created_at": now,
    });
    scaffold::publish_and_await(
        &alice,
        &non_owner,
        &verse_a_id,
        "fractal",
        &fractal_id,
        &fractal,
    )?;
    for (petal_id, label) in [(&petal_p0, "P0"), (&petal_p1, "P1"), (&petal_p2, "P2")] {
        let petal = serde_json::json!({
            "petal_id": petal_id,
            "fractal_id": fractal_id,
            "name": label,
            "node_id": alice_did,
            "created_at": now,
        });
        scaffold::publish_and_await(&alice, &non_owner, &verse_a_id, "petal", petal_id, &petal)?;
    }

    let verse_a_scope = fe_database::build_scope(&verse_a_id, None, None);
    let petal_p1_scope = fe_database::build_scope(&verse_a_id, Some(&fractal_id), Some(&petal_p1));

    // --- Leg 1 (SYNC-ROLE-PROPAGATION acceptance): alice grants bob Editor;
    //     carol — a fresh peer, never granted anything — admits bob's node
    //     row AND his reading with no local provisioning. ------------------
    assign_role(&alice, &bob_did, &verse_a_scope, "editor")?;

    let node_bob = create_local_node(&bob, &petal_p0, "Bob's Anchor")?;
    // Receivers exclude bob himself: the sync thread never echoes an
    // own-author row to its DB (`handle_inbound_row_change` loop prevention),
    // so waiting on bob's own `ReplicatedRowApplied` could only time out.
    scaffold::publish_and_await(
        &bob,
        &[&carol, &dave, &erin, &frank],
        &verse_a_id,
        "node",
        &node_bob,
        &node_row(&node_bob, &petal_p0, "Bob's Anchor"),
    )?;

    bob.send(DbCommand::InsertIotReadings {
        petal_id: petal_p0.clone(),
        verse_id: Some(verse_a_id.clone()),
        source_did: bob_did.clone(),
        readings: vec![IotReadingInput {
            node_id: node_bob.clone(),
            metric: "temperature_c".to_string(),
            value: 21.5,
            units: "C".to_string(),
            recorded_at: None,
            payload_id: None,
        }],
        source_class: SourceClass::Device,
        correlation_id: None,
    });
    match bob.wait_for(
        |r| matches!(r, DbResult::IotReadingsInserted { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::IotReadingsInserted { written: 1, .. } => {}
        other => bail!("unexpected result ingesting bob's reading: {other:?}"),
    }
    match wait_table_settled(&carol, &verse_a_id, "iot_reading", SYNC_TIMEOUT)? {
        ReplicatedRowOutcome::Applied => {}
        other => {
            return Ok(TestResult::fail(
                NAME,
                &format!(
                "leg 1: carol did not admit bob's reading with no local provisioning: {other:?}"
            ),
            ))
        }
    }

    // --- Leg 2: alice upgrades bob to Manager; bob grants dave Editor; carol
    //     admits dave's row through a grantor whose own authority traces
    //     back to the owner. ------------------------------------------------
    assign_role(&alice, &bob_did, &verse_a_scope, "manager")?;
    assign_role_retrying(
        &bob,
        &dave_did,
        &verse_a_scope,
        "editor",
        AUTH_CONVERGE_TIMEOUT,
    )?;

    let node_dave = format!("dave-{}", ulid::Ulid::new());
    scaffold::publish_and_await(
        &dave,
        &[&carol],
        &verse_a_id,
        "node",
        &node_dave,
        &node_row(&node_dave, &petal_p0, "Dave's Node"),
    )?;

    // --- Leg 3 (R7 subtree, end to end): alice makes erin Manager of petal
    //     P1 ONLY; erin grants frank Editor in P1; carol admits frank's P1
    //     row and Defers his row in the SIBLING petal P2, where erin has no
    //     authority at all. ------------------------------------------------
    assign_role(&alice, &erin_did, &petal_p1_scope, "manager")?;
    assign_role_retrying(
        &erin,
        &frank_did,
        &petal_p1_scope,
        "editor",
        AUTH_CONVERGE_TIMEOUT,
    )?;

    let node_frank_p1 = format!("frank-p1-{}", ulid::Ulid::new());
    scaffold::publish_and_await(
        &frank,
        &[&carol],
        &verse_a_id,
        "node",
        &node_frank_p1,
        &node_row(&node_frank_p1, &petal_p1, "Frank's P1 Node"),
    )?;

    let node_frank_p2 = format!("frank-p2-{}", ulid::Ulid::new());
    publish_row(
        &frank,
        &verse_a_id,
        "node",
        &node_frank_p2,
        &serde_json::to_vec(&node_row(&node_frank_p2, &petal_p2, "Frank's P2 Node"))?,
    )?;
    match wait_outcome(&carol, "node", &node_frank_p2, SYNC_TIMEOUT)? {
        ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole { did }) if did == frank_did => {}
        other => {
            return Ok(TestResult::fail(
                NAME,
                &format!(
                    "leg 3: frank's row in the sibling petal P2 should defer on his \
                     missing authority there (R7 subtree confinement), got {other:?}"
                ),
            ))
        }
    }

    // --- Leg 4 (T4): alice revokes bob. dave's grant (admitted only through
    //     bob's authority) cascades to pending, so dave's next row Defers
    //     on carol. --------------------------------------------------------
    revoke_role(&alice, &bob_did, &verse_a_scope)?;
    publish_until_deferred(
        &dave,
        &carol,
        &verse_a_id,
        &petal_p0,
        "dave-cascade",
        &dave_did,
        SYNC_TIMEOUT,
    )?;

    // --- Leg 5 (P2P-REF-CONFINE, two-peer): a SECOND verse B, with its own
    //     petal that carol genuinely knows (she joined it). A node bob
    //     plants in verse A naming verse B's petal is Denied on carol —
    //     known-foreign, never merely deferred. --------------------------
    alice.send(DbCommand::CreateVerse {
        name: "Confinement Verse B".into(),
    });
    let verse_b_id = match alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        Duration::from_secs(30),
    )? {
        DbResult::VerseCreated { id, .. } => id,
        other => bail!("unexpected result creating verse B: {other:?}"),
    };
    let ns_secret_b = alice
        .namespace_secret(&verse_b_id)
        .ok_or_else(|| anyhow!("verse B secret missing from the test secret map"))?;
    let secret_bytes_b: [u8; 32] = hex::decode(&ns_secret_b)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_b = hex::encode(fe_database::derive_namespace_id(&secret_bytes_b));

    alice
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_b_id.clone(),
            namespace_id: ns_id_b.clone(),
            namespace_secret: Some(ns_secret_b.clone()),
            bootstrap_peers: Vec::new(),
        })
        .map_err(|e| anyhow!("failed to open alice's verse-B replica: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));
    carol
        .sync_cmd_tx
        .send(SyncCommand::OpenVerseReplica {
            verse_id: verse_b_id.clone(),
            namespace_id: ns_id_b.clone(),
            namespace_secret: Some(ns_secret_b.clone()),
            bootstrap_peers: vec![alice_addr.clone()],
        })
        .map_err(|e| anyhow!("failed to open carol's verse-B replica: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));

    scaffold::publish_manifest(
        &alice,
        &[&carol],
        &verse_b_id,
        &alice_did,
        "Confinement Verse B",
    )?;
    let fractal_b_id = ulid::Ulid::new().to_string();
    let petal_b_id = ulid::Ulid::new().to_string();
    scaffold::publish_scaffold(
        &alice,
        &[&carol],
        &Scaffold {
            verse_id: &verse_b_id,
            owner_did: &alice_did,
            fractal_id: &fractal_b_id,
            petal_id: &petal_b_id,
        },
        &[],
    )?;

    // bob (an Editor admitted in verse A, before his revoke above — still
    // structurally plants the row through verse A's replica either way,
    // since this leg is about referential confinement, not his role) plants
    // a node THROUGH VERSE A's REPLICA naming verse B's petal.
    let forged_node = format!("bob-forged-{}", ulid::Ulid::new());
    publish_row(
        &bob,
        &verse_a_id,
        "node",
        &forged_node,
        &serde_json::to_vec(&node_row(
            &forged_node,
            &petal_b_id,
            "Forged Cross-Verse Node",
        ))?,
    )?;
    match wait_outcome(&carol, "node", &forged_node, SYNC_TIMEOUT)? {
        ReplicatedRowOutcome::Denied => {}
        other => {
            return Ok(TestResult::fail(
                NAME,
                &format!(
                    "leg 5: a node naming verse B's petal through verse A's replica should be \
                     Denied on carol (she already knows that petal as foreign), got {other:?}"
                ),
            ))
        }
    }

    // Clean close.
    for (peer, verses) in [
        (&alice, vec![verse_a_id.clone(), verse_b_id.clone()]),
        (&bob, vec![verse_a_id.clone()]),
        (&carol, vec![verse_a_id.clone(), verse_b_id.clone()]),
        (&dave, vec![verse_a_id.clone()]),
        (&erin, vec![verse_a_id.clone()]),
        (&frank, vec![verse_a_id.clone()]),
    ] {
        for verse_id in verses {
            let _ = peer
                .sync_cmd_tx
                .send(SyncCommand::CloseVerseReplica { verse_id });
        }
    }
    std::thread::sleep(Duration::from_millis(200));

    drop(frank);
    drop(erin);
    drop(dave);
    drop(carol);
    drop(bob);
    drop(alice);
    Ok(TestResult::pass(NAME))
}
