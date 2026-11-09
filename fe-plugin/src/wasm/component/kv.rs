//! Per-instance key/value state behind the `kv` import; see src/wasm/AGENTS.md §kv.

use std::collections::BTreeMap;

/// Longest kv key, in bytes.
pub const MAX_KV_KEY_BYTES: usize = 256;
/// Longest kv value (JSON text), in bytes.
pub const MAX_KV_VALUE_BYTES: usize = fe_sdk::plugin_host::MAX_HOST_BYTES;
/// Host ceiling on manifest `kv.max_bytes`.
pub const MAX_KV_BUDGET_BYTES: u64 = 1 << 20;
/// Most keys one `list-keys` call returns.
pub const MAX_KV_LIST: u32 = 1_000;

/// Host-local storage for one instance: never replicated, never on the DB thread.
pub trait KvBackend: Send {
    /// The stored JSON text for `key`.
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    /// Replace `key`.
    fn set(&mut self, key: &str, value: String) -> Result<(), String>;
    /// Remove `key`; absent is not an error.
    fn delete(&mut self, key: &str) -> Result<(), String>;
    /// Keys starting with `prefix`, sorted, at most `limit`.
    fn keys(&self, prefix: &str, limit: usize) -> Result<Vec<String>, String>;
    /// Bytes held (sum of key and value lengths), checked against the manifest budget.
    fn used_bytes(&self) -> u64;
}

/// In-memory backend; state lasts for the life of the host process.
#[derive(Debug, Default, Clone)]
pub struct MemoryKv {
    entries: BTreeMap<String, String>,
    used: u64,
}

impl MemoryKv {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

fn entry_bytes(key: &str, value: &str) -> u64 {
    (key.len() + value.len()) as u64
}

impl KvBackend for MemoryKv {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.entries.get(key).cloned())
    }

    fn set(&mut self, key: &str, value: String) -> Result<(), String> {
        let added = entry_bytes(key, &value);
        if let Some(previous) = self.entries.insert(key.to_string(), value) {
            self.used -= entry_bytes(key, &previous);
        }
        self.used += added;
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), String> {
        if let Some(previous) = self.entries.remove(key) {
            self.used -= entry_bytes(key, &previous);
        }
        Ok(())
    }

    fn keys(&self, prefix: &str, limit: usize) -> Result<Vec<String>, String> {
        Ok(self
            .entries
            .range(prefix.to_string()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .take(limit)
            .map(|(key, _)| key.clone())
            .collect())
    }

    fn used_bytes(&self) -> u64 {
        self.used
    }
}

/// The kv grant of one instance: a backend and the manifest byte budget.
pub struct KvSlot {
    backend: Box<dyn KvBackend>,
    max_bytes: u64,
}

impl KvSlot {
    /// `max_bytes` is clamped to [`MAX_KV_BUDGET_BYTES`].
    pub fn new(backend: Box<dyn KvBackend>, max_bytes: u64) -> Self {
        Self {
            backend,
            max_bytes: max_bytes.min(MAX_KV_BUDGET_BYTES),
        }
    }

    /// Whether replacing `key` with `value` stays inside the budget.
    pub(crate) fn fits(&self, key: &str, value: &str) -> Result<bool, String> {
        let current = self
            .backend
            .get(key)?
            .map_or(0, |previous| entry_bytes(key, &previous));
        let after = self.backend.used_bytes().saturating_sub(current) + entry_bytes(key, value);
        Ok(after <= self.max_bytes)
    }

    pub(crate) fn backend(&self) -> &dyn KvBackend {
        self.backend.as_ref()
    }

    pub(crate) fn backend_mut(&mut self) -> &mut dyn KvBackend {
        self.backend.as_mut()
    }
}

/// Key shape shared by every kv call: 1..=256 bytes, no control characters.
pub(crate) fn validate_kv_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KV_KEY_BYTES {
        return Err(format!("kv key must be 1..={MAX_KV_KEY_BYTES} bytes"));
    }
    if key.chars().any(char::is_control) {
        return Err("kv key contains a control character".into());
    }
    Ok(())
}
