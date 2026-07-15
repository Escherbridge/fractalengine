//! In-memory implementations of the `fe-sdk` storage + query traits.
//!
//! [`MockStorage`] lets plugin/extension tests exercise `node_get_properties`,
//! `node_set_property`, `query_select`, and the per-extension KV without a real
//! database. It applies the same host-boundary validation and SELECT-only guard
//! as the engine so tests observe realistic fail-closed behavior. It is cheap to
//! clone (shared `Arc<Mutex<..>>`) so the same store can be seeded, injected,
//! and inspected. See `fe-plugin-test`'s crate docs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fe_sdk::property::PropertyValue;
use fe_sdk::query::{self, ExtensionQueryApi, QueryError, MAX_RESULT_ROWS};
use fe_sdk::storage::{self, ExtensionStorageApi, StorageError};

/// Recorded / seeded state shared by all clones of a [`MockStorage`].
#[derive(Debug, Default)]
struct MockStorageState {
    /// Node properties keyed by `(node_id, key)`.
    node_props: HashMap<(String, String), PropertyValue>,
    /// Per-extension KV keyed by `(namespace, key)`.
    kv: HashMap<(String, String), PropertyValue>,
    /// Canned rows returned by every `query_select`.
    query_rows: Vec<serde_json::Value>,
    /// Recorded `query_select` calls as `(sql, params)`.
    query_log: Vec<(String, HashMap<String, serde_json::Value>)>,
    /// Recorded `node_set_property` calls.
    property_sets: Vec<(String, String, PropertyValue)>,
    /// Recorded `storage_set` (KV) calls as `(namespace, key, value)`.
    kv_sets: Vec<(String, String, PropertyValue)>,
}

/// In-memory store implementing [`ExtensionStorageApi`] + [`ExtensionQueryApi`].
#[derive(Debug, Clone, Default)]
pub struct MockStorage {
    state: Arc<Mutex<MockStorageState>>,
}

impl MockStorage {
    /// Create an empty mock store.
    pub fn new() -> Self {
        Self::default()
    }

    // -- seeding -----------------------------------------------------------

    /// Seed a node property that reads will return.
    pub fn seed_node_property(
        &self,
        node_id: impl Into<String>,
        key: impl Into<String>,
        value: PropertyValue,
    ) {
        self.state
            .lock()
            .unwrap()
            .node_props
            .insert((node_id.into(), key.into()), value);
    }

    /// Seed a per-extension KV value that reads will return.
    pub fn seed_kv(
        &self,
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: PropertyValue,
    ) {
        self.state
            .lock()
            .unwrap()
            .kv
            .insert((namespace.into(), key.into()), value);
    }

    /// Seed the canned rows returned by `query_select`.
    pub fn seed_query_rows(&self, rows: Vec<serde_json::Value>) {
        self.state.lock().unwrap().query_rows = rows;
    }

    // -- inspection --------------------------------------------------------

    /// Current value of a node property, if any.
    pub fn node_property(&self, node_id: &str, key: &str) -> Option<PropertyValue> {
        self.state
            .lock()
            .unwrap()
            .node_props
            .get(&(node_id.to_string(), key.to_string()))
            .cloned()
    }

    /// Current value of a namespaced KV entry, if any.
    pub fn kv_value(&self, namespace: &str, key: &str) -> Option<PropertyValue> {
        self.state
            .lock()
            .unwrap()
            .kv
            .get(&(namespace.to_string(), key.to_string()))
            .cloned()
    }

    /// All recorded `node_set_property` calls.
    pub fn property_sets(&self) -> Vec<(String, String, PropertyValue)> {
        self.state.lock().unwrap().property_sets.clone()
    }

    /// All recorded `storage_set` (KV) calls.
    pub fn kv_sets(&self) -> Vec<(String, String, PropertyValue)> {
        self.state.lock().unwrap().kv_sets.clone()
    }

    /// All recorded `query_select` SQL strings.
    pub fn queries(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .query_log
            .iter()
            .map(|(sql, _)| sql.clone())
            .collect()
    }
}

impl ExtensionStorageApi for MockStorage {
    fn node_get_properties(
        &self,
        node_id: &str,
    ) -> Result<HashMap<String, PropertyValue>, StorageError> {
        storage::validate_node_id(node_id)?;
        let state = self.state.lock().unwrap();
        Ok(state
            .node_props
            .iter()
            .filter(|((n, _), _)| n == node_id)
            .map(|((_, k), v)| (k.clone(), v.clone()))
            .collect())
    }

    fn node_set_property(
        &self,
        node_id: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), StorageError> {
        storage::validate_node_id(node_id)?;
        storage::validate_key(key)?;
        let mut state = self.state.lock().unwrap();
        state
            .property_sets
            .push((node_id.to_string(), key.to_string(), value.clone()));
        state
            .node_props
            .insert((node_id.to_string(), key.to_string()), value);
        Ok(())
    }

    fn storage_get(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<PropertyValue>, StorageError> {
        storage::validate_key(key)?;
        let state = self.state.lock().unwrap();
        Ok(state
            .kv
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }

    fn storage_set(
        &self,
        namespace: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), StorageError> {
        storage::validate_key(key)?;
        let mut state = self.state.lock().unwrap();
        state
            .kv_sets
            .push((namespace.to_string(), key.to_string(), value.clone()));
        state
            .kv
            .insert((namespace.to_string(), key.to_string()), value);
        Ok(())
    }
}

impl ExtensionQueryApi for MockStorage {
    fn query_select(
        &self,
        sql: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, QueryError> {
        query::validate_query_len(sql)?;
        if !query::is_select_only(sql) {
            return Err(QueryError::NotSelectOnly);
        }
        let mut state = self.state.lock().unwrap();
        state.query_log.push((sql.to_string(), params));
        let mut rows = state.query_rows.clone();
        rows.truncate(MAX_RESULT_ROWS);
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_property_roundtrip_and_record() {
        let store = MockStorage::new();
        store
            .node_set_property("n1", "color", PropertyValue::String("red".into()))
            .unwrap();
        assert_eq!(
            store.node_property("n1", "color"),
            Some(PropertyValue::String("red".into()))
        );
        assert_eq!(store.property_sets().len(), 1);
        let props = store.node_get_properties("n1").unwrap();
        assert_eq!(
            props.get("color"),
            Some(&PropertyValue::String("red".into()))
        );
    }

    #[test]
    fn kv_is_namespaced() {
        let store = MockStorage::new();
        store
            .storage_set("ext-a", "token", PropertyValue::String("secret".into()))
            .unwrap();
        assert!(store.storage_get("ext-b", "token").unwrap().is_none());
        assert_eq!(
            store.storage_get("ext-a", "token").unwrap(),
            Some(PropertyValue::String("secret".into()))
        );
    }

    #[test]
    fn query_enforces_select_only() {
        let store = MockStorage::new();
        store.seed_query_rows(vec![serde_json::json!({"id": 1})]);
        assert!(store
            .query_select("SELECT * FROM node", HashMap::new())
            .is_ok());
        assert!(matches!(
            store
                .query_select("DELETE node", HashMap::new())
                .unwrap_err(),
            QueryError::NotSelectOnly
        ));
    }

    #[test]
    fn invalid_inputs_fail_closed() {
        let store = MockStorage::new();
        assert!(store.node_get_properties("").is_err());
        assert!(store
            .node_set_property("n1", "", PropertyValue::Bool(true))
            .is_err());
    }
}
