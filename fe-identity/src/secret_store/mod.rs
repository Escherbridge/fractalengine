//! Pluggable secret storage abstraction.
//!
//! Provides [`SecretStore`] — a trait for get/set/delete of string secrets
//! keyed by `(service, account)` pairs — plus concrete backends:
//!
//! - [`InMemoryBackend`] — always compiled, used in tests.
//! - [`OsKeystoreBackend`] — wraps `keyring::Entry`, compiled only with the
//!   `keyring` feature.
//! - [`EnvBackend`] — reads from environment variables, always compiled.

use std::collections::HashMap;
use std::sync::RwLock;

/// Errors that can occur during secret store operations.
#[derive(Debug, thiserror::Error)]
pub enum SecretStoreError {
    /// The underlying platform keystore reported an error.
    #[error("backend error: {0}")]
    Backend(String),
}

/// A pluggable store for string secrets keyed by `(service, account)`.
///
/// All implementations must be `Send + Sync + 'static` so they can be shared
/// across threads via `Arc<dyn SecretStore>`.
pub trait SecretStore: Send + Sync + 'static {
    /// Retrieve a secret. Returns `Ok(None)` when no entry exists.
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;

    /// Store (or overwrite) a secret.
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError>;

    /// Delete a secret. Returns `Ok(())` even if the entry did not exist.
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}

/// Pure in-memory backend. Useful for tests — no OS keychain interaction.
pub struct InMemoryBackend {
    map: RwLock<HashMap<(String, String), String>>,
}

impl InMemoryBackend {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for InMemoryBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        let map = self.map.read().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        Ok(map.get(&(service.to_string(), account.to_string())).cloned())
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError> {
        let mut map = self.map.write().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        map.insert((service.to_string(), account.to_string()), value.to_string());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        let mut map = self.map.write().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        map.remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OsKeystoreBackend — compiled only when the `keyring` feature is active
// ---------------------------------------------------------------------------

#[cfg(feature = "keyring")]
mod os_keystore;
#[cfg(feature = "keyring")]
pub use os_keystore::OsKeystoreBackend;

// ---------------------------------------------------------------------------
// EnvBackend
// ---------------------------------------------------------------------------

/// Backend that maps `(service, account)` to environment variable
/// `FE_SECRET_{SERVICE}_{ACCOUNT}` (uppercased, special chars replaced with `_`).
///
/// Reads check the real environment first; if absent, falls back to a
/// runtime-written in-memory map. `set` writes only to the in-memory map
/// (it does *not* mutate the process environment).
pub struct EnvBackend {
    overrides: RwLock<HashMap<(String, String), String>>,
}

impl EnvBackend {
    /// Create a new environment-backed store.
    pub fn new() -> Self {
        Self {
            overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Normalize a key component: uppercase, replace non-alphanumeric with `_`.
    fn normalize(s: &str) -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
            .collect()
    }

    /// Build the env var name for a `(service, account)` pair.
    fn env_var_name(service: &str, account: &str) -> String {
        format!("FE_SECRET_{}_{}", Self::normalize(service), Self::normalize(account))
    }
}

impl Default for EnvBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for EnvBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        let var_name = Self::env_var_name(service, account);
        if let Ok(val) = std::env::var(&var_name) {
            return Ok(Some(val));
        }
        let map = self.overrides.read().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        Ok(map.get(&(service.to_string(), account.to_string())).cloned())
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError> {
        let mut map = self.overrides.write().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        map.insert((service.to_string(), account.to_string()), value.to_string());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        let mut map = self.overrides.write().map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        map.remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- InMemoryBackend tests --

    #[test]
    fn in_memory_get_missing_returns_none() {
        let store = InMemoryBackend::new();
        assert_eq!(store.get("svc", "acct").unwrap(), None);
    }

    #[test]
    fn in_memory_set_then_get() {
        let store = InMemoryBackend::new();
        store.set("svc", "acct", "secret123").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), Some("secret123".to_string()));
    }

    #[test]
    fn in_memory_set_overwrites() {
        let store = InMemoryBackend::new();
        store.set("svc", "acct", "old").unwrap();
        store.set("svc", "acct", "new").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), Some("new".to_string()));
    }

    #[test]
    fn in_memory_delete_removes_entry() {
        let store = InMemoryBackend::new();
        store.set("svc", "acct", "val").unwrap();
        store.delete("svc", "acct").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), None);
    }

    #[test]
    fn in_memory_delete_missing_is_ok() {
        let store = InMemoryBackend::new();
        assert!(store.delete("svc", "acct").is_ok());
    }

    // -- EnvBackend tests --

    #[test]
    fn env_backend_set_then_get() {
        let store = EnvBackend::new();
        store.set("svc", "acct", "val42").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), Some("val42".to_string()));
    }

    #[test]
    fn env_backend_reads_env_var() {
        // Use a unique var name to avoid test interference.
        let unique = format!("TEST_{}", std::process::id());
        let var_name = EnvBackend::env_var_name(&unique, "acct");
        std::env::set_var(&var_name, "from_env");
        let store = EnvBackend::new();
        assert_eq!(store.get(&unique, "acct").unwrap(), Some("from_env".to_string()));
        std::env::remove_var(&var_name);
    }

    #[test]
    fn env_backend_env_var_takes_precedence() {
        let unique = format!("PREC_{}", std::process::id());
        let var_name = EnvBackend::env_var_name(&unique, "acct");
        std::env::set_var(&var_name, "env_wins");
        let store = EnvBackend::new();
        store.set(&unique, "acct", "runtime_value").unwrap();
        assert_eq!(store.get(&unique, "acct").unwrap(), Some("env_wins".to_string()));
        std::env::remove_var(&var_name);
    }

    #[test]
    fn env_backend_key_normalization() {
        assert_eq!(
            EnvBackend::env_var_name("my-service:v1", "user.name"),
            "FE_SECRET_MY_SERVICE_V1_USER_NAME"
        );
    }

    #[test]
    fn env_backend_delete_removes_runtime_value() {
        let store = EnvBackend::new();
        store.set("svc", "acct", "val").unwrap();
        store.delete("svc", "acct").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), None);
    }

    // -- Trait object tests --

    #[test]
    fn trait_object_works() {
        let store: std::sync::Arc<dyn SecretStore> = std::sync::Arc::new(InMemoryBackend::new());
        store.set("svc", "acct", "via_trait").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), Some("via_trait".to_string()));
    }
}
