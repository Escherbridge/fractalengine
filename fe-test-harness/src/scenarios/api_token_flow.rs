//! Scenario: API Token Lifecycle
//!
//! Tests the complete API token management flow:
//! 1. Mint a token with valid scope and role
//! 2. List tokens and verify the minted token appears
//! 3. Verify the token JWT is valid and contains correct claims
//! 4. Revoke the token
//! 5. List tokens again and verify it no longer appears
//! 6. Edge cases: empty scope, excessive TTL, double revoke, wrong JTI

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::peer::TestPeer;
use crate::TestResult;

pub fn run() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice", tmp.path())?;

    // 1. Seed Alice's DB so we have a verse to scope tokens to
    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // 2. Mint an API token
    alice.send(DbCommand::MintApiToken {
        scope: "VERSE#test-verse-1".to_string(),
        max_role: "editor".to_string(),
        ttl_hours: 24,
        label: Some("Test Token".to_string()),
    });
    let mint_result = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenMinted { .. }),
        std::time::Duration::from_secs(30),
    )?;

    let (token, jti) = match &mint_result {
        DbResult::ApiTokenMinted { token, jti, scope, max_role, label, .. } => {
            assert_eq!(scope, "VERSE#test-verse-1", "scope mismatch");
            assert_eq!(max_role, "editor", "max_role mismatch");
            assert_eq!(label.as_deref(), Some("Test Token"), "label mismatch");
            assert!(!token.is_empty(), "token should not be empty");
            assert!(!jti.is_empty(), "jti should not be empty");
            (token.clone(), jti.clone())
        }
        other => anyhow::bail!("Expected ApiTokenMinted, got: {other:?}"),
    };

    // 3. Verify the JWT is valid by decoding with Alice's public key
    let claims = fe_identity::api_token::verify_api_token(
        &token,
        &alice.keypair.verifying_key(),
    )?;
    assert_eq!(claims.token_type, "api", "token_type should be 'api'");
    assert_eq!(claims.scope, "VERSE#test-verse-1");
    assert_eq!(claims.max_role, "editor");
    assert_eq!(claims.jti, jti);
    assert!(claims.exp > claims.iat, "exp should be after iat");
    assert_eq!(claims.exp - claims.iat, 24 * 3600, "TTL should be 24 hours");

    // 4. List tokens — should contain our minted token
    alice.send(DbCommand::ListApiTokens { offset: 0, limit: 100 });
    let list_result = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokensListed { .. }),
        std::time::Duration::from_secs(30),
    )?;

    match &list_result {
        DbResult::ApiTokensListed { tokens, .. } => {
            assert!(!tokens.is_empty(), "token list should not be empty");
            let found = tokens.iter().find(|t| t.jti == jti);
            assert!(found.is_some(), "minted token should appear in list");
            let tok = found.unwrap();
            assert_eq!(tok.scope, "VERSE#test-verse-1");
            assert_eq!(tok.max_role, "editor");
            assert!(!tok.revoked, "token should not be revoked yet");
        }
        other => anyhow::bail!("Expected ApiTokensListed, got: {other:?}"),
    }

    // 5. Revoke the token
    alice.send(DbCommand::RevokeApiToken { jti: jti.clone() });
    let revoke_result = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenRevoked { .. }),
        std::time::Duration::from_secs(30),
    )?;

    match &revoke_result {
        DbResult::ApiTokenRevoked { jti: revoked_jti } => {
            assert_eq!(revoked_jti, &jti, "revoked JTI should match");
        }
        other => anyhow::bail!("Expected ApiTokenRevoked, got: {other:?}"),
    }

    // 6. List tokens — revoked token should no longer appear (list_active_tokens filters)
    alice.send(DbCommand::ListApiTokens { offset: 0, limit: 100 });
    let list_after = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokensListed { .. }),
        std::time::Duration::from_secs(30),
    )?;

    match &list_after {
        DbResult::ApiTokensListed { tokens, .. } => {
            let found = tokens.iter().find(|t| t.jti == jti);
            assert!(found.is_none(), "revoked token should not appear in active list");
        }
        other => anyhow::bail!("Expected ApiTokensListed, got: {other:?}"),
    }

    // 7. Mint a second token with different scope to test multiple tokens
    alice.send(DbCommand::MintApiToken {
        scope: "VERSE#test-verse-2".to_string(),
        max_role: "viewer".to_string(),
        ttl_hours: 1,
        label: None,
    });
    let mint2 = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenMinted { .. }),
        std::time::Duration::from_secs(30),
    )?;
    let jti2 = match &mint2 {
        DbResult::ApiTokenMinted { jti, max_role, label, .. } => {
            assert_eq!(max_role, "viewer");
            assert!(label.is_none(), "label should be None");
            jti.clone()
        }
        other => anyhow::bail!("Expected ApiTokenMinted, got: {other:?}"),
    };

    // 8. List should show exactly 1 active token (the second one; first was revoked)
    alice.send(DbCommand::ListApiTokens { offset: 0, limit: 100 });
    let list_multi = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokensListed { .. }),
        std::time::Duration::from_secs(30),
    )?;
    match &list_multi {
        DbResult::ApiTokensListed { tokens, .. } => {
            assert_eq!(tokens.len(), 1, "should have exactly 1 active token");
            assert_eq!(tokens[0].jti, jti2);
        }
        other => anyhow::bail!("Expected ApiTokensListed, got: {other:?}"),
    }

    Ok(TestResult::pass("API Token Flow"))
}

/// Edge case tests that exercise error paths.
pub fn run_edge_cases() -> Result<TestResult> {
    let tmp = tempfile::tempdir()?;
    let alice = TestPeer::spawn("alice-edge", tmp.path())?;

    alice.send(DbCommand::Seed);
    alice.wait_for(
        |r| matches!(r, DbResult::Seeded { .. }),
        std::time::Duration::from_secs(30),
    )?;

    // Edge case 1: Revoke a non-existent token
    alice.send(DbCommand::RevokeApiToken {
        jti: "non-existent-jti-12345".to_string(),
    });
    // Should get an Error result
    let revoke_bad = alice.db_result_rx.recv_timeout(std::time::Duration::from_secs(10))?;
    match &revoke_bad {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("not found"),
                "Error should mention 'not found', got: {msg}"
            );
        }
        other => anyhow::bail!("Expected Error for non-existent JTI revoke, got: {other:?}"),
    }

    // Edge case 2: Mint with empty scope should fail
    alice.send(DbCommand::MintApiToken {
        scope: String::new(),
        max_role: "viewer".to_string(),
        ttl_hours: 1,
        label: None,
    });
    let empty_scope = alice.db_result_rx.recv_timeout(std::time::Duration::from_secs(10))?;
    match &empty_scope {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("scope") || msg.contains("empty"),
                "Error should mention scope, got: {msg}"
            );
        }
        other => anyhow::bail!("Expected Error for empty scope, got: {other:?}"),
    }

    // Edge case 3: Mint with excessive TTL (> 30 days) should fail
    alice.send(DbCommand::MintApiToken {
        scope: "VERSE#v1".to_string(),
        max_role: "viewer".to_string(),
        ttl_hours: 31 * 24, // 31 days > 30 day max
        label: None,
    });
    let excess_ttl = alice.db_result_rx.recv_timeout(std::time::Duration::from_secs(10))?;
    match &excess_ttl {
        DbResult::Error(msg) => {
            assert!(
                msg.contains("TTL") || msg.contains("30 days") || msg.contains("maximum"),
                "Error should mention TTL limit, got: {msg}"
            );
        }
        other => anyhow::bail!("Expected Error for excessive TTL, got: {other:?}"),
    }

    // Edge case 4: List tokens when none exist should return empty list
    // (Alice has no valid tokens — the previous mints all failed)
    alice.send(DbCommand::ListApiTokens { offset: 0, limit: 100 });
    let list_empty = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokensListed { .. }),
        std::time::Duration::from_secs(10),
    )?;
    match &list_empty {
        DbResult::ApiTokensListed { tokens, .. } => {
            assert!(tokens.is_empty(), "should have no active tokens");
        }
        other => anyhow::bail!("Expected ApiTokensListed, got: {other:?}"),
    }

    // Edge case 5: Double revoke — revoke same token twice
    alice.send(DbCommand::MintApiToken {
        scope: "VERSE#v1".to_string(),
        max_role: "editor".to_string(),
        ttl_hours: 1,
        label: None,
    });
    let mint_for_double = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenMinted { .. }),
        std::time::Duration::from_secs(10),
    )?;
    let double_jti = match &mint_for_double {
        DbResult::ApiTokenMinted { jti, .. } => jti.clone(),
        other => anyhow::bail!("Expected ApiTokenMinted, got: {other:?}"),
    };

    // First revoke should succeed
    alice.send(DbCommand::RevokeApiToken { jti: double_jti.clone() });
    let first_revoke = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenRevoked { .. }),
        std::time::Duration::from_secs(10),
    )?;
    assert!(matches!(first_revoke, DbResult::ApiTokenRevoked { .. }));

    // Second revoke — the token is already revoked, but the UPDATE still matches
    // (it sets revoked=true on an already-revoked row), so it returns Ok(true).
    // This is acceptable idempotent behavior.
    alice.send(DbCommand::RevokeApiToken { jti: double_jti.clone() });
    let second_revoke = alice.db_result_rx.recv_timeout(std::time::Duration::from_secs(10))?;
    // Accept either ApiTokenRevoked (idempotent) or Error — both are valid
    match &second_revoke {
        DbResult::ApiTokenRevoked { .. } | DbResult::Error(_) => {
            // Both behaviors are acceptable
        }
        other => anyhow::bail!("Expected ApiTokenRevoked or Error for double revoke, got: {other:?}"),
    }

    // Edge case 6: Token verification with wrong key should fail
    let bob_kp = fe_identity::NodeKeypair::generate();
    alice.send(DbCommand::MintApiToken {
        scope: "VERSE#v1".to_string(),
        max_role: "viewer".to_string(),
        ttl_hours: 1,
        label: None,
    });
    let mint_for_wrong_key = alice.wait_for(
        |r| matches!(r, DbResult::ApiTokenMinted { .. }),
        std::time::Duration::from_secs(10),
    )?;
    let wrong_key_token = match &mint_for_wrong_key {
        DbResult::ApiTokenMinted { token, .. } => token.clone(),
        other => anyhow::bail!("Expected ApiTokenMinted, got: {other:?}"),
    };

    // Verify with Bob's key should fail
    let wrong_verify = fe_identity::api_token::verify_api_token(
        &wrong_key_token,
        &bob_kp.verifying_key(),
    );
    assert!(wrong_verify.is_err(), "Verification with wrong key should fail");

    // Verify with Alice's key should succeed
    let right_verify = fe_identity::api_token::verify_api_token(
        &wrong_key_token,
        &alice.keypair.verifying_key(),
    );
    assert!(right_verify.is_ok(), "Verification with correct key should succeed");

    Ok(TestResult::pass("API Token Edge Cases"))
}
