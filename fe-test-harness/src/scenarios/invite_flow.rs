//! Scenario 3: Invite Flow
//!
//! Verifies the full invite lifecycle:
//! 1. Alice creates a verse
//! 2. Alice generates an invite string
//! 3. Bob joins the verse via the invite string
//! 4. Bob's hierarchy contains the verse with the correct name

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;
    let bob = TestPeer::spawn("bob", tmp.path())?;

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
        DbResult::VerseCreated { id, name } => {
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
            tracing::info!(
                "Bob joined verse: {verse_name} ({verse_id})"
            );
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
        DbResult::HierarchyLoaded { verses } => {
            verses.iter().any(|v| v.id == verse_id && v.name == "Test Verse")
        }
        _ => false,
    };

    if !found_verse {
        return Ok(TestResult::fail(
            "invite_flow",
            "Bob's hierarchy does not contain the joined verse",
        ));
    }

    drop(alice);
    drop(bob);
    Ok(TestResult::pass("invite_flow"))
}
