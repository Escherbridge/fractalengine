//! Peer compute mesh -- distributed query execution with result verification.
//!
//! Phase 6.2: local execution + result hashing. Peer offloading is stubbed
//! for Phase 8 (fe-hexon P2P distribution).

use serde::{Deserialize, Serialize};

/// Advertised compute capability of a peer node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerCapability {
    pub compute_budget: ComputeBudget,
    pub storage_gb: u32,
    pub bandwidth_mbps: u32,
}

/// Self-reported compute budget tier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ComputeBudget {
    Low,
    Medium,
    High,
}

/// A compute task submitted for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeTask {
    pub task_id: String,
    pub query: String,
    pub petal_scope: Option<String>,
    pub requester_did: String,
}

/// Result of a completed compute task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeResult {
    pub task_id: String,
    pub rows: Vec<serde_json::Value>,
    pub result_hash: String,
}

impl ComputeResult {
    /// Verify the result integrity by recomputing the blake3 hash.
    pub fn verify(&self) -> bool {
        let expected = hash_rows(&self.rows);
        self.result_hash == expected
    }
}

/// Compute the blake3 hash of a result set for verification.
pub fn hash_rows(rows: &[serde_json::Value]) -> String {
    let mut hasher = blake3::Hasher::new();
    for row in rows {
        // Canonical JSON serialization for deterministic hashing
        let bytes = serde_json::to_vec(row).unwrap_or_default();
        hasher.update(&bytes);
    }
    hasher.finalize().to_hex().to_string()
}

/// Read the local node's compute capability from environment variables.
///
/// - `FE_COMPUTE_BUDGET`: "low", "medium", or "high" (default: "medium")
/// - `FE_STORAGE_GB`: available storage in GB (default: 10)
/// - `FE_BANDWIDTH_MBPS`: available bandwidth in Mbps (default: 100)
pub fn local_capability() -> PeerCapability {
    let budget = match std::env::var("FE_COMPUTE_BUDGET")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "low" => ComputeBudget::Low,
        "high" => ComputeBudget::High,
        _ => ComputeBudget::Medium,
    };

    let storage_gb = std::env::var("FE_STORAGE_GB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let bandwidth_mbps = std::env::var("FE_BANDWIDTH_MBPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    PeerCapability {
        compute_budget: budget,
        storage_gb,
        bandwidth_mbps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_rows_deterministic() {
        let rows = vec![json!({"id": 1, "name": "a"}), json!({"id": 2, "name": "b"})];
        let h1 = hash_rows(&rows);
        let h2 = hash_rows(&rows);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());

        // Different order produces a different hash
        let rows_rev = vec![json!({"id": 2, "name": "b"}), json!({"id": 1, "name": "a"})];
        let h3 = hash_rows(&rows_rev);
        assert_ne!(h1, h3);
    }

    #[test]
    fn verify_result_valid() {
        let rows = vec![json!({"x": 10}), json!({"x": 20})];
        let result = ComputeResult {
            task_id: "t1".into(),
            rows: rows.clone(),
            result_hash: hash_rows(&rows),
        };
        assert!(result.verify());
    }

    #[test]
    fn verify_result_tampered() {
        let rows = vec![json!({"x": 10}), json!({"x": 20})];
        let result = ComputeResult {
            task_id: "t1".into(),
            rows,
            result_hash: "definitely_wrong_hash".into(),
        };
        assert!(!result.verify());
    }

    #[test]
    fn local_capability_defaults() {
        // Remove env vars to test defaults (they may not be set)
        std::env::remove_var("FE_COMPUTE_BUDGET");
        std::env::remove_var("FE_STORAGE_GB");
        std::env::remove_var("FE_BANDWIDTH_MBPS");

        let cap = local_capability();
        assert_eq!(cap.compute_budget, ComputeBudget::Medium);
        assert_eq!(cap.storage_gb, 10);
        assert_eq!(cap.bandwidth_mbps, 100);
    }

    #[test]
    fn compute_budget_serde() {
        let low = ComputeBudget::Low;
        let json = serde_json::to_string(&low).unwrap();
        assert_eq!(json, "\"low\"");

        let med: ComputeBudget = serde_json::from_str("\"medium\"").unwrap();
        assert_eq!(med, ComputeBudget::Medium);

        let high: ComputeBudget = serde_json::from_str("\"high\"").unwrap();
        assert_eq!(high, ComputeBudget::High);
    }

    #[test]
    fn peer_capability_serde_roundtrip() {
        let cap = PeerCapability {
            compute_budget: ComputeBudget::High,
            storage_gb: 256,
            bandwidth_mbps: 1000,
        };
        let json = serde_json::to_string(&cap).unwrap();
        let decoded: PeerCapability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, decoded);
    }
}
