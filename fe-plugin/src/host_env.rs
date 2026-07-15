//! Host environment — routes extension storage/query calls through injected
//! `fe-sdk` trait objects. The engine binary injects the implementations; the
//! plugin engine itself never depends on `fe-database`. Capability gating and
//! host-boundary input validation live here so both the Rhai and WASM paths
//! share one fail-closed policy. See `fe-plugin/src/AGENTS.md` §host-env.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use fe_policy::{Action, CapabilityPolicy, Decision, PolicyEngine, Scope};

use fe_sdk::property::PropertyValue;
use fe_sdk::query::{self, ExtensionQueryApi, QueryError, MAX_RESULT_ROWS};
use fe_sdk::storage::{self, ExtensionStorageApi, StorageError};
use fe_sdk::{CAP_QUERY_SELECT, CAP_STORAGE_READ, CAP_STORAGE_WRITE};

use crate::capability::CapabilityToken;

/// Injected host services available to an extension at runtime.
///
/// Cheap to clone (holds `Arc` trait objects). An empty `HostEnv` means the
/// binary injected nothing — every routed call then fails closed with
/// [`HostApiError::NotAvailable`].
#[derive(Clone, Default)]
pub struct HostEnv {
    storage: Option<Arc<dyn ExtensionStorageApi>>,
    query: Option<Arc<dyn ExtensionQueryApi>>,
}

impl HostEnv {
    /// An empty host environment (no services injected).
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a storage implementation (node properties + per-extension KV).
    pub fn with_storage(mut self, api: Arc<dyn ExtensionStorageApi>) -> Self {
        self.storage = Some(api);
        self
    }

    /// Attach a SELECT-only query implementation.
    pub fn with_query(mut self, api: Arc<dyn ExtensionQueryApi>) -> Self {
        self.query = Some(api);
        self
    }

    /// Whether a storage implementation is present.
    pub fn has_storage(&self) -> bool {
        self.storage.is_some()
    }

    /// Whether a query implementation is present.
    pub fn has_query(&self) -> bool {
        self.query.is_some()
    }

    // -- routed operations (capability-checked + validated + delegated) -----

    /// Read all custom properties of a node. Requires `storage.read`.
    pub fn node_get_properties(
        &self,
        token: &CapabilityToken,
        node_id: &str,
    ) -> Result<HashMap<String, PropertyValue>, HostApiError> {
        require(token, CAP_STORAGE_READ)?;
        storage::validate_node_id(node_id)?;
        let api = self
            .storage
            .as_ref()
            .ok_or(HostApiError::NotAvailable("storage"))?;
        Ok(api.node_get_properties(node_id)?)
    }

    /// Set a single custom property on a node. Requires `storage.write`.
    pub fn node_set_property(
        &self,
        token: &CapabilityToken,
        node_id: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), HostApiError> {
        require(token, CAP_STORAGE_WRITE)?;
        storage::validate_node_id(node_id)?;
        storage::validate_key(key)?;
        let api = self
            .storage
            .as_ref()
            .ok_or(HostApiError::NotAvailable("storage"))?;
        Ok(api.node_set_property(node_id, key, value)?)
    }

    /// Read a per-extension KV value, namespaced by the extension's own id.
    /// Requires `storage.read`.
    pub fn storage_get(
        &self,
        token: &CapabilityToken,
        namespace: &str,
        key: &str,
    ) -> Result<Option<PropertyValue>, HostApiError> {
        require(token, CAP_STORAGE_READ)?;
        storage::validate_key(key)?;
        let api = self
            .storage
            .as_ref()
            .ok_or(HostApiError::NotAvailable("storage"))?;
        Ok(api.storage_get(namespace, key)?)
    }

    /// Write a per-extension KV value, namespaced by the extension's own id.
    /// Requires `storage.write`.
    pub fn storage_set(
        &self,
        token: &CapabilityToken,
        namespace: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), HostApiError> {
        require(token, CAP_STORAGE_WRITE)?;
        storage::validate_key(key)?;
        let api = self
            .storage
            .as_ref()
            .ok_or(HostApiError::NotAvailable("storage"))?;
        Ok(api.storage_set(namespace, key, value)?)
    }

    /// Run a SELECT-only query with bound params. Requires `query.select`.
    ///
    /// Enforces the [`query::is_select_only`] keyword guard and caps the result
    /// at [`MAX_RESULT_ROWS`] before returning.
    pub fn query_select(
        &self,
        token: &CapabilityToken,
        sql: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, HostApiError> {
        require(token, CAP_QUERY_SELECT)?;
        query::validate_query_len(sql)?;
        if !query::is_select_only(sql) {
            return Err(HostApiError::NotSelectOnly);
        }
        let api = self
            .query
            .as_ref()
            .ok_or(HostApiError::NotAvailable("query"))?;
        let mut rows = api.query_select(sql, params)?;
        rows.truncate(MAX_RESULT_ROWS);
        Ok(rows)
    }
}

impl std::fmt::Debug for HostEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostEnv")
            .field("has_storage", &self.storage.is_some())
            .field("has_query", &self.query.is_some())
            .finish()
    }
}

/// Shared capability engine (fe-policy) — see AGENTS.md §capability-policy.
fn capability_engine() -> &'static PolicyEngine {
    static ENGINE: OnceLock<PolicyEngine> = OnceLock::new();
    ENGINE.get_or_init(|| PolicyEngine::new().with_policy(Arc::new(CapabilityPolicy)))
}

/// Fail-closed capability check used by every routed operation (policy-engine decision).
fn require(token: &CapabilityToken, capability: &str) -> Result<(), HostApiError> {
    let subject = token.to_auth_context();
    let action = Action::Custom(capability.to_string());
    match capability_engine().evaluate(&subject, &action, &Scope::global()) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(HostApiError::CapabilityDenied(capability.to_string())),
    }
}

/// Errors surfaced by [`HostEnv`] operations. Never swallowed — the Rhai/WASM
/// bindings convert these into typed script errors and emit a tracing event.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HostApiError {
    /// The extension lacks the required capability grant.
    #[error("capability '{0}' not granted")]
    CapabilityDenied(String),
    /// No implementation for this API was injected by the host binary.
    #[error("host API '{0}' not available")]
    NotAvailable(&'static str),
    /// The query was not a single safe SELECT statement.
    #[error("only single SELECT statements are allowed")]
    NotSelectOnly,
    /// An argument failed host-boundary validation.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The underlying storage/query backend failed.
    #[error("backend error: {0}")]
    Backend(String),
}

impl From<StorageError> for HostApiError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::CapabilityDenied(c) => HostApiError::CapabilityDenied(c),
            StorageError::InvalidInput(m) => HostApiError::InvalidInput(m),
            StorageError::NotAvailable => HostApiError::NotAvailable("storage"),
            StorageError::Backend(m) => HostApiError::Backend(m),
        }
    }
}

impl From<QueryError> for HostApiError {
    fn from(e: QueryError) -> Self {
        match e {
            QueryError::CapabilityDenied(c) => HostApiError::CapabilityDenied(c),
            QueryError::InvalidInput(m) => HostApiError::InvalidInput(m),
            QueryError::NotSelectOnly => HostApiError::NotSelectOnly,
            QueryError::NotAvailable => HostApiError::NotAvailable("query"),
            QueryError::Backend(m) => HostApiError::Backend(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use fe_sdk::query::QueryError;

    use super::*;
    use crate::capability::{CapabilityManifest, CapabilityToken};

    #[derive(Default)]
    struct RecordingStore {
        props: Mutex<HashMap<(String, String), PropertyValue>>,
        kv: Mutex<HashMap<(String, String), PropertyValue>>,
    }

    impl ExtensionStorageApi for RecordingStore {
        fn node_get_properties(
            &self,
            node_id: &str,
        ) -> Result<HashMap<String, PropertyValue>, StorageError> {
            let props = self.props.lock().unwrap();
            Ok(props
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
            self.props
                .lock()
                .unwrap()
                .insert((node_id.to_string(), key.to_string()), value);
            Ok(())
        }
        fn storage_get(
            &self,
            namespace: &str,
            key: &str,
        ) -> Result<Option<PropertyValue>, StorageError> {
            Ok(self
                .kv
                .lock()
                .unwrap()
                .get(&(namespace.to_string(), key.to_string()))
                .cloned())
        }
        fn storage_set(
            &self,
            namespace: &str,
            key: &str,
            value: PropertyValue,
        ) -> Result<(), StorageError> {
            self.kv
                .lock()
                .unwrap()
                .insert((namespace.to_string(), key.to_string()), value);
            Ok(())
        }
    }

    struct RejectingQuery;
    impl ExtensionQueryApi for RejectingQuery {
        fn query_select(
            &self,
            _sql: &str,
            _params: HashMap<String, serde_json::Value>,
        ) -> Result<Vec<serde_json::Value>, QueryError> {
            Ok(vec![serde_json::json!({"ok": true})])
        }
    }

    fn token_with(caps: &str) -> CapabilityToken {
        CapabilityToken::mint(
            "ext-a",
            &CapabilityManifest::from_json(&format!(r#"{{"capabilities": {caps}}}"#)).unwrap(),
        )
    }

    #[test]
    fn denied_without_capability() {
        let env = HostEnv::new().with_storage(Arc::new(RecordingStore::default()));
        let token = token_with("[]");
        let err = env.node_get_properties(&token, "n1").unwrap_err();
        assert!(matches!(err, HostApiError::CapabilityDenied(_)));
    }

    #[test]
    fn not_available_without_injection() {
        let env = HostEnv::new();
        let token = token_with(r#"["storage.read"]"#);
        let err = env.node_get_properties(&token, "n1").unwrap_err();
        assert!(matches!(err, HostApiError::NotAvailable("storage")));
    }

    #[test]
    fn invalid_input_fails_closed() {
        let env = HostEnv::new().with_storage(Arc::new(RecordingStore::default()));
        let token = token_with(r#"["storage.read"]"#);
        assert!(matches!(
            env.node_get_properties(&token, "").unwrap_err(),
            HostApiError::InvalidInput(_)
        ));
    }

    #[test]
    fn kv_roundtrip_is_namespaced() {
        let store = Arc::new(RecordingStore::default());
        let env = HostEnv::new().with_storage(store);
        let token = token_with(r#"["storage.read", "storage.write"]"#);
        env.storage_set(&token, "ext-a", "k", PropertyValue::Number(1.0))
            .unwrap();
        // Same key under a different namespace is invisible.
        assert!(env.storage_get(&token, "ext-b", "k").unwrap().is_none());
        assert_eq!(
            env.storage_get(&token, "ext-a", "k").unwrap(),
            Some(PropertyValue::Number(1.0))
        );
    }

    #[test]
    fn query_rejects_non_select() {
        let env = HostEnv::new().with_query(Arc::new(RejectingQuery));
        let token = token_with(r#"["query.select"]"#);
        assert!(matches!(
            env.query_select(&token, "DELETE node", HashMap::new())
                .unwrap_err(),
            HostApiError::NotSelectOnly
        ));
        assert!(env
            .query_select(&token, "SELECT * FROM node", HashMap::new())
            .is_ok());
    }
}
