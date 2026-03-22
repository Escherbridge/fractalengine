use fe_auth::cache::{SessionCache, SESSION_TTL_SECS};
use fe_database::RoleId;

#[test]
fn test_session_cache_insert_and_get() {
    let mut cache = SessionCache::new();
    let key = [1u8; 32];
    cache.insert(key, RoleId("admin".to_string()));
    assert!(cache.get(&key).is_some());
    assert_eq!(cache.get(&key).unwrap().0, "admin");
}

#[test]
fn test_session_cache_revoke() {
    let mut cache = SessionCache::new();
    let key = [2u8; 32];
    cache.insert(key, RoleId("public".to_string()));
    cache.revoke(&key);
    assert!(cache.get(&key).is_none());
}

#[test]
fn test_session_cache_different_keys() {
    let mut cache = SessionCache::new();
    let key1 = [1u8; 32];
    let key2 = [2u8; 32];
    cache.insert(key1, RoleId("admin".to_string()));
    cache.insert(key2, RoleId("public".to_string()));
    assert!(cache.get(&key1).is_some());
    assert!(cache.get(&key2).is_some());
    cache.revoke(&key1);
    assert!(cache.get(&key1).is_none());
    assert!(cache.get(&key2).is_some());
}

// test_session_cache_ttl: requires mock Instant — DEFERRED TO WAVE 6 VALIDATION
// test_connect_public_fallback: requires running DB — DEFERRED TO WAVE 6 VALIDATION
// test_revocation_order: requires running DB — DEFERRED TO WAVE 6 VALIDATION

const _: u64 = SESSION_TTL_SECS;
