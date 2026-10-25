//! Scenario 3: Invite Flow — see fe-test-harness/src/AGENTS.md §scenarios.
//!
//! RP-8 (T10): the invitee owner leg. `join_verse_by_invite_handler` (RP-4)
//! pins `created_by` to the invite's signed `owner_did` — a real `did:key`,
//! never the inviter's node address hex-encoded. This scenario asserts that
//! directly, then proves what it buys: alice's later petal row (an owner
//! statement needs no role grant — "Owner via `created_by`") applies on bob
//! through the real inbound entry point, `InboundAdmissions::apply`
//! (fe-database/src/AGENTS.md §handlers "replicated_row" — the ONE entry
//! point production and the harness share).

use std::collections::HashMap;

use anyhow::{bail, Result};
use fe_runtime::messages::{DbCommand, DbResult, ReplicatedRowOutcome};

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;
    let alice_did = alice.keypair.to_did_key();

    // 1. Alice seeds (initializes DB schema and genesis data)
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 2. Alice creates a new verse
    alice.send(DbCommand::CreateVerse {
        name: "Test Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, name, .. } => {
            tracing::info!("Alice created verse: {name} ({id})");
            id.clone()
        }
        _ => unreachable!(),
    };

    // 3. Alice generates an invite
    alice.send(DbCommand::GenerateVerseInvite {
        verse_id: verse_id.clone(),
        include_write_cap: true,
        expiry_hours: 24,
    });
    let invite_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseInviteGenerated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let invite_string = match &invite_result {
        DbResult::VerseInviteGenerated {
            verse_id: vid,
            invite_string,
        } => {
            tracing::info!(
                "Alice generated invite for verse {vid} ({} chars)",
                invite_string.len()
            );
            invite_string.clone()
        }
        _ => unreachable!(),
    };

    // 4. Bob seeds his own DB
    bob.send(DbCommand::Seed);
    bob.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 5. Bob joins via invite
    bob.send(DbCommand::JoinVerseByInvite {
        invite_string: invite_string.clone(),
    });
    let join_result = bob.wait_for(
        |r| matches!(r, DbResult::VerseJoined { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let (joined_vid, joined_name) = match &join_result {
        DbResult::VerseJoined {
            verse_id,
            verse_name,
        } => {
            tracing::info!("Bob joined verse: {verse_name} ({verse_id})");
            (verse_id.clone(), verse_name.clone())
        }
        _ => unreachable!(),
    };

    // 6. Verify the verse ID matches
    if joined_vid != verse_id {
        return Ok(TestResult::fail(
            "invite_flow",
            &format!(
                "Joined verse ID mismatch: got '{}', expected '{}'",
                joined_vid, verse_id
            ),
        ));
    }

    if joined_name != "Test Verse" {
        return Ok(TestResult::fail(
            "invite_flow",
            &format!(
                "Joined verse name mismatch: got '{}', expected 'Test Verse'",
                joined_name
            ),
        ));
    }

    // 7. Verify Bob's hierarchy contains the verse
    bob.send(DbCommand::LoadHierarchy);
    let hierarchy = bob.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let found_verse = match &hierarchy {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .any(|v| v.id == verse_id && v.name == "Test Verse"),
        _ => false,
    };

    if !found_verse {
        return Ok(TestResult::fail(
            "invite_flow",
            "Bob's hierarchy does not contain the joined verse",
        ));
    }

    // 8. RP-8 (T10): bob's verse row carries `created_by == alice_did` — a
    //    `did:key`, never the inviter's node address hex-encoded (the bug
    //    `join_verse_by_invite_handler`'s owner-leg write fixed in RP-4).
    bob.send(DbCommand::RawQuery {
        correlation_id: None,
        sql: format!("SELECT created_by FROM verse WHERE verse_id = '{verse_id}'"),
        vars: HashMap::new(),
    });
    let created_by = match bob.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        std::time::Duration::from_secs(30),
    )? {
        DbResult::QueryResult { data, .. } => data
            .first()
            .and_then(|row| row["created_by"].as_str())
            .map(str::to_string),
        other => bail!("unexpected result reading bob's verse row: {other:?}"),
    };
    if created_by.as_deref() != Some(alice_did.as_str()) {
        return Ok(TestResult::fail(
            "invite_flow",
            &format!(
                "bob's verse row created_by is {created_by:?}, expected alice's did:key {alice_did}"
            ),
        ));
    }

    // 9. RP-8: alice's later petal row (via her fractal) applies on bob —
    //    an owner statement, resolved straight from `created_by`, needs no
    //    role grant. Submitted through `ApplyReplicatedRow`, the same entry
    //    point (`InboundAdmissions::apply`) the sync thread uses for an
    //    inbound row over the real transport.
    alice.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "Alice's Later Fractal".into(),
    });
    let fractal_id = match alice.wait_for(
        |r| matches!(r, DbResult::FractalCreated { .. }),
        std::time::Duration::from_secs(30),
    )? {
        DbResult::FractalCreated { id, .. } => id,
        other => bail!("unexpected result creating alice's fractal: {other:?}"),
    };
    alice.send(DbCommand::CreatePetal {
        fractal_id: fractal_id.clone(),
        name: "Alice's Later Petal".into(),
    });
    let petal_id = match alice.wait_for(
        |r| matches!(r, DbResult::PetalCreated { .. }),
        std::time::Duration::from_secs(30),
    )? {
        DbResult::PetalCreated { id, .. } => id,
        other => bail!("unexpected result creating alice's petal: {other:?}"),
    };

    let now = chrono::Utc::now().to_rfc3339();
    let fractal_row = serde_json::to_vec(&serde_json::json!({
        "fractal_id": fractal_id,
        "verse_id": verse_id,
        "owner_did": alice_did,
        "name": "Alice's Later Fractal",
        "created_at": now,
    }))?;
    bob.send(DbCommand::ApplyReplicatedRow {
        verse_id: verse_id.clone(),
        table: "fractal".to_string(),
        record_id: fractal_id.clone(),
        row_bytes: fractal_row,
        author_did: alice_did.clone(),
    });
    match bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { table, record_id, .. }
                    if table == "fractal" && record_id == &fractal_id
            )
        },
        std::time::Duration::from_secs(30),
    )? {
        DbResult::ReplicatedRowApplied {
            outcome: ReplicatedRowOutcome::Applied,
            ..
        } => {}
        other => {
            return Ok(TestResult::fail(
                "invite_flow",
                &format!("alice's fractal row did not apply on bob: {other:?}"),
            ))
        }
    }

    let petal_row = serde_json::to_vec(&serde_json::json!({
        "petal_id": petal_id,
        "fractal_id": fractal_id,
        "name": "Alice's Later Petal",
        "node_id": alice_did,
        "created_at": now,
    }))?;
    bob.send(DbCommand::ApplyReplicatedRow {
        verse_id: verse_id.clone(),
        table: "petal".to_string(),
        record_id: petal_id.clone(),
        row_bytes: petal_row,
        author_did: alice_did.clone(),
    });
    match bob.wait_for(
        |r| {
            matches!(
                r,
                DbResult::ReplicatedRowApplied { table, record_id, .. }
                    if table == "petal" && record_id == &petal_id
            )
        },
        std::time::Duration::from_secs(30),
    )? {
        DbResult::ReplicatedRowApplied {
            outcome: ReplicatedRowOutcome::Applied,
            ..
        } => {}
        other => {
            return Ok(TestResult::fail(
                "invite_flow",
                &format!(
                    "alice's petal row — Applied iff bob trusts her as the verse's real \
                     owner — did not apply on bob: {other:?}"
                ),
            ))
        }
    }

    drop(alice);
    drop(bob);
    Ok(TestResult::pass("invite_flow"))
}
