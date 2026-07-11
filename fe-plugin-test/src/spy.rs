//! Spy recorder — captures all host-API calls made by plugin scripts.
//!
//! [`SpyRecorder`] is embedded inside [`MockHostEnv`](crate::mock_host::MockHostEnv)
//! and records every call that passes through the mock host functions so tests
//! can assert on call counts, argument values, and ordering.

/// Records host-API calls for later assertion.
#[derive(Debug, Clone, Default)]
pub struct SpyRecorder {
    /// `get_node(node_id)` calls.
    pub get_node_calls: Vec<String>,
    /// `set_property(node_id, key, value)` calls.
    pub set_property_calls: Vec<(String, String, String)>,
    /// `create_node(petal_id, name)` calls.
    pub create_node_calls: Vec<(String, String)>,
    /// `query_nodes(petal_id, filter)` calls.
    pub query_calls: Vec<(String, String)>,
    /// `log_*(level, message)` calls.
    pub log_messages: Vec<(String, String)>,
    /// `node_get_properties(node_id)` calls.
    pub node_get_properties_calls: Vec<String>,
    /// `node_set_property(node_id, key, value_repr)` calls.
    pub node_set_property_calls: Vec<(String, String, String)>,
    /// `query_select(sql)` calls.
    pub query_select_calls: Vec<String>,
    /// `ext_storage_get(key)` calls.
    pub ext_storage_get_calls: Vec<String>,
    /// `ext_storage_set(key, value_repr)` calls.
    pub ext_storage_set_calls: Vec<(String, String)>,
}

impl SpyRecorder {
    /// Create a new empty spy recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// All `set_property` calls as `(node_id, key, value)` tuples.
    pub fn property_writes(&self) -> &[(String, String, String)] {
        &self.set_property_calls
    }

    /// All `get_node` calls as node-ID strings.
    pub fn node_reads(&self) -> &[String] {
        &self.get_node_calls
    }

    /// Whether a method with the given name was ever called.
    pub fn was_called(&self, method: &str) -> bool {
        self.call_count(method) > 0
    }

    /// Number of times a method with the given name was called.
    pub fn call_count(&self, method: &str) -> usize {
        match method {
            "get_node" => self.get_node_calls.len(),
            "set_property" => self.set_property_calls.len(),
            "create_node" => self.create_node_calls.len(),
            "query_nodes" => self.query_calls.len(),
            "node_get_properties" => self.node_get_properties_calls.len(),
            "node_set_property" => self.node_set_property_calls.len(),
            "query_select" => self.query_select_calls.len(),
            "ext_storage_get" => self.ext_storage_get_calls.len(),
            "ext_storage_set" => self.ext_storage_set_calls.len(),
            "log_debug" | "log_info" | "log_warn" | "log_error" => self
                .log_messages
                .iter()
                .filter(|(level, _)| level == method)
                .count(),
            "log" => self.log_messages.len(),
            _ => 0,
        }
    }

    /// Reset all recorded calls.
    pub fn reset(&mut self) {
        self.get_node_calls.clear();
        self.set_property_calls.clear();
        self.create_node_calls.clear();
        self.query_calls.clear();
        self.log_messages.clear();
        self.node_get_properties_calls.clear();
        self.node_set_property_calls.clear();
        self.query_select_calls.clear();
        self.ext_storage_get_calls.clear();
        self.ext_storage_set_calls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spy_recorder_tracks_calls() {
        let mut spy = SpyRecorder::new();

        spy.get_node_calls.push("node_01".into());
        spy.get_node_calls.push("node_02".into());
        spy.set_property_calls
            .push(("node_01".into(), "color".into(), "red".into()));
        spy.create_node_calls
            .push(("petal_01".into(), "Sensor".into()));
        spy.query_calls
            .push(("petal_01".into(), "type == sensor".into()));
        spy.log_messages
            .push(("log_info".into(), "hello world".into()));

        assert_eq!(spy.node_reads().len(), 2);
        assert_eq!(spy.property_writes().len(), 1);
        assert!(spy.was_called("get_node"));
        assert!(spy.was_called("set_property"));
        assert!(spy.was_called("create_node"));
        assert!(spy.was_called("query_nodes"));
        assert!(spy.was_called("log_info"));
        assert!(!spy.was_called("log_error"));
        assert!(!spy.was_called("nonexistent"));

        assert_eq!(spy.call_count("get_node"), 2);
        assert_eq!(spy.call_count("set_property"), 1);
        assert_eq!(spy.call_count("log"), 1);
    }

    #[test]
    fn spy_recorder_reset_clears_all() {
        let mut spy = SpyRecorder::new();
        spy.get_node_calls.push("node_01".into());
        spy.set_property_calls
            .push(("n".into(), "k".into(), "v".into()));
        spy.create_node_calls.push(("p".into(), "n".into()));
        spy.query_calls.push(("p".into(), "f".into()));
        spy.log_messages.push(("log_info".into(), "msg".into()));

        spy.reset();

        assert!(spy.get_node_calls.is_empty());
        assert!(spy.set_property_calls.is_empty());
        assert!(spy.create_node_calls.is_empty());
        assert!(spy.query_calls.is_empty());
        assert!(spy.log_messages.is_empty());
    }
}
