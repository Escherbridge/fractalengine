//! Scenario 6: Two-Peer Verse Join
//!
//! Full two-peer verse lifecycle:
//! 1. Alice creates a verse, a fractal, and a petal
//! 2. Alice generates an invite (include_write_cap=true)
//! 3. Bob joins via the invite string
//! 4. Bob's hierarchy contains the verse (with matching verse_id and name)
//! 5. Bob creates a fractal in the joined verse (using the same verse_id)
//! 6. Both Alice and Bob have their own fractals
//!
//! This proves invite-based collaboration works with independent DB state.

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

    // 1. Alice seeds her DB
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 2. Alice creates a verse
    alice.send(DbCommand::CreateVerse {
        name: "Collab Verse".into(),
    });
    let verse_result = alice.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let verse_id = match &verse_result {
        DbResult::VerseCreated { id, name } => {
            tracing::info!("Alice created verse: {name} ({id})");
            id.clone()
        }
        _ => unreachable!(),
    };

    // 3. Alice creates a fractal in the verse
    alice.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "Alice's Fractal".into(),
    });
    let alice_fractal_result = alice.wait_for(
        |r| matches!(r, DbResult::FractalCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let alice_fractal_id = match &alice_fractal_result {
        DbResult::FractalCreated { id, .. } => {
            tracing::info!("Alice created fractal: {id}");
            id.clone()
        }
        _ => unreachable!(),
    };

    // 4. Alice creates a petal in her fractal
    alice.send(DbCommand::CreatePetal {
        fractal_id: alice_fractal_id.clone(),
        name: "Alice's Petal".into(),
    });
    alice.wait_for(
        |r| matches!(r, DbResult::PetalCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 5. Alice generates an invite with write capability
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
        DbResult::VerseInviteGenerated { invite_string, .. } => {
            tracing::info!(
                "Alice generated invite ({} chars)",
                invite_string.len()
            );
            invite_string.clone()
        }
        _ => unreachable!(),
    };

    // 6. Bob seeds his own DB
    bob.send(DbCommand::Seed);
    bob.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 7. Bob joins via invite
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

    // 8. Verify verse_id and name match
    if joined_vid != verse_id {
        return Ok(TestResult::fail(
            "two_peer_verse_join",
            &format!(
                "Verse ID mismatch: Alice='{}', Bob='{}'",
                verse_id, joined_vid
            ),
        ));
    }

    if joined_name != "Collab Verse" {
        return Ok(TestResult::fail(
            "two_peer_verse_join",
            &format!(
                "Verse name mismatch: expected 'Collab Verse', got '{}'",
                joined_name
            ),
        ));
    }

    // 9. Verify Bob's hierarchy contains the verse
    bob.send(DbCommand::LoadHierarchy);
    let bob_hierarchy = bob.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let bob_has_verse = match &bob_hierarchy {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .any(|v| v.id == verse_id && v.name == "Collab Verse"),
        _ => false,
    };

    if !bob_has_verse {
        return Ok(TestResult::fail(
            "two_peer_verse_join",
            "Bob's hierarchy does not contain the joined verse",
        ));
    }

    // 10. Bob creates his own fractal in the joined verse
    bob.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "Bob's Fractal".into(),
    });
    let bob_fractal_result = bob.wait_for(
        |r| matches!(r, DbResult::FractalCreated { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let bob_fractal_id = match &bob_fractal_result {
        DbResult::FractalCreated { id, .. } => {
            tracing::info!("Bob created fractal: {id}");
            id.clone()
        }
        _ => unreachable!(),
    };

    // 11. Verify Alice's hierarchy has her fractal
    alice.send(DbCommand::LoadHierarchy);
    let alice_hierarchy = alice.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let alice_has_fractal = match &alice_hierarchy {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .find(|v| v.id == verse_id)
            .map(|v| {
                v.fractals
                    .iter()
                    .any(|f| f.id == alice_fractal_id && f.name == "Alice's Fractal")
            })
            .unwrap_or(false),
        _ => false,
    };

    if !alice_has_fractal {
        return Ok(TestResult::fail(
            "two_peer_verse_join",
            "Alice's hierarchy does not contain her fractal",
        ));
    }

    // 12. Verify Bob's hierarchy has his fractal
    bob.send(DbCommand::LoadHierarchy);
    let bob_hierarchy2 = bob.wait_for(
        |r| matches!(r, DbResult::HierarchyLoaded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let bob_has_fractal = match &bob_hierarchy2 {
        DbResult::HierarchyLoaded { verses } => verses
            .iter()
            .find(|v| v.id == verse_id)
            .map(|v| {
                v.fractals
                    .iter()
                    .any(|f| f.id == bob_fractal_id && f.name == "Bob's Fractal")
            })
            .unwrap_or(false),
        _ => false,
    };

    if !bob_has_fractal {
        return Ok(TestResult::fail(
            "two_peer_verse_join",
            "Bob's hierarchy does not contain his fractal",
        ));
    }

    tracing::info!(
        "Two-peer verse join verified: verse_id={}, alice_fractal={}, bob_fractal={}",
        verse_id,
        alice_fractal_id,
        bob_fractal_id
    );

    drop(alice);
    drop(bob);
    Ok(TestResult::pass("two_peer_verse_join"))
}
