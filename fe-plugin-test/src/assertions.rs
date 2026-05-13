//! Assertion functions for plugin test verification.
//!
//! These are plain functions (not macros) that return `Result<(), String>` so
//! callers can use `?` or `.unwrap()` depending on their preference. The error
//! messages include enough context to diagnose failures without a debugger.

use crate::mock_host::MockHostEnv;

/// Assert that a specific property was set on a node during script execution.
pub fn assert_property_set(
    host: &MockHostEnv,
    node_id: &str,
    key: &str,
    expected: &str,
) -> Result<(), String> {
    let writes = host.spy().property_writes();
    for (nid, k, v) in writes {
        if nid == node_id && k == key && v == expected {
            return Ok(());
        }
    }

    // Build a helpful error message showing what *was* written
    let relevant: Vec<_> = writes
        .iter()
        .filter(|(nid, k, _)| nid == node_id && k == key)
        .collect();

    if relevant.is_empty() {
        Err(format!(
            "Expected set_property({node_id}, {key}, {expected}) but no writes to {node_id}.{key} were recorded.\n\
             All writes: {writes:?}"
        ))
    } else {
        let actual_values: Vec<_> = relevant.iter().map(|(_, _, v)| v.as_str()).collect();
        Err(format!(
            "Expected set_property({node_id}, {key}, {expected}) but actual values were: {actual_values:?}"
        ))
    }
}

/// Assert that exactly `count` nodes were created during script execution.
pub fn assert_nodes_created(host: &MockHostEnv, count: usize) -> Result<(), String> {
    let actual = host.spy().create_node_calls.len();
    if actual == count {
        Ok(())
    } else {
        Err(format!(
            "Expected {count} node(s) created but {actual} were created.\n\
             Creates: {:?}",
            host.spy().create_node_calls
        ))
    }
}

/// Assert that no property writes occurred during script execution.
pub fn assert_no_writes(host: &MockHostEnv) -> Result<(), String> {
    let writes = host.spy().property_writes();
    if writes.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Expected no property writes but {} occurred: {:?}",
            writes.len(),
            writes
        ))
    }
}

/// Assert that a log message at the given level containing the substring was recorded.
pub fn assert_logged(
    host: &MockHostEnv,
    level: &str,
    contains: &str,
) -> Result<(), String> {
    let logs = &host.spy().log_messages;
    for (lvl, msg) in logs {
        if lvl == level && msg.contains(contains) {
            return Ok(());
        }
    }

    let relevant: Vec<_> = logs.iter().filter(|(lvl, _)| lvl == level).collect();
    if relevant.is_empty() {
        Err(format!(
            "Expected log at level '{level}' containing '{contains}' but no {level} messages were recorded.\n\
             All logs: {logs:?}"
        ))
    } else {
        let messages: Vec<_> = relevant.iter().map(|(_, m)| m.as_str()).collect();
        Err(format!(
            "Expected log at level '{level}' containing '{contains}' but messages were: {messages:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_property_set_passes_when_present() {
        let mut host = MockHostEnv::new();
        host.spy
            .set_property_calls
            .push(("n1".into(), "color".into(), "red".into()));

        assert!(assert_property_set(&host, "n1", "color", "red").is_ok());
    }

    #[test]
    fn assert_property_set_fails_when_missing() {
        let host = MockHostEnv::new();
        let result = assert_property_set(&host, "n1", "color", "red");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no writes"));
    }

    #[test]
    fn assert_property_set_fails_when_wrong_value() {
        let mut host = MockHostEnv::new();
        host.spy
            .set_property_calls
            .push(("n1".into(), "color".into(), "blue".into()));

        let result = assert_property_set(&host, "n1", "color", "red");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blue"));
    }

    #[test]
    fn assert_nodes_created_passes() {
        let mut host = MockHostEnv::new();
        host.spy
            .create_node_calls
            .push(("p1".into(), "Node A".into()));
        host.spy
            .create_node_calls
            .push(("p1".into(), "Node B".into()));

        assert!(assert_nodes_created(&host, 2).is_ok());
        assert!(assert_nodes_created(&host, 3).is_err());
    }

    #[test]
    fn assert_no_writes_passes_when_empty() {
        let host = MockHostEnv::new();
        assert!(assert_no_writes(&host).is_ok());
    }

    #[test]
    fn assert_no_writes_fails_when_writes_exist() {
        let mut host = MockHostEnv::new();
        host.spy
            .set_property_calls
            .push(("n1".into(), "k".into(), "v".into()));
        assert!(assert_no_writes(&host).is_err());
    }

    #[test]
    fn assert_logged_passes_when_found() {
        let mut host = MockHostEnv::new();
        host.spy
            .log_messages
            .push(("log_info".into(), "Processing sensor data".into()));

        assert!(assert_logged(&host, "log_info", "sensor").is_ok());
    }

    #[test]
    fn assert_logged_fails_when_not_found() {
        let host = MockHostEnv::new();
        let result = assert_logged(&host, "log_info", "something");
        assert!(result.is_err());
    }
}
