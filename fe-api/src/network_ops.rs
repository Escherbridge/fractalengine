//! Host-services config for network operation: usage sink, metrics bind, advertisement signer (see `AGENTS.md` §usage-metering).

use std::sync::Arc;

use fe_runtime::usage::{JsonLinesUsageSink, NoopUsageSink, UsageSink};

/// Operator-facing network services a host hands to the API; every field defaults to off.
#[derive(Clone)]
pub struct NetworkOps {
    /// Receives one event per API request; `NoopUsageSink` when metering is off.
    pub usage_sink: Arc<dyn UsageSink>,
    /// `host:port` for the opt-in metrics listener; `None` binds nothing.
    pub metrics_bind: Option<String>,
    /// Key that signs relay advertisements; `None` disables signed advertisement.
    pub advertisement_signer: Option<Arc<fe_identity::NodeKeypair>>,
}

impl Default for NetworkOps {
    fn default() -> Self {
        Self {
            usage_sink: Arc::new(NoopUsageSink),
            metrics_bind: None,
            advertisement_signer: None,
        }
    }
}

impl NetworkOps {
    /// Reads `FE_API_USAGE_LOG` and `FE_API_METRICS_BIND` once at startup.
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("FE_API_USAGE_LOG").ok(),
            std::env::var("FE_API_METRICS_BIND").ok(),
        )
    }

    /// Pure core of `from_env`: empty strings count as unset, an unopenable log path warns and falls back to no-op.
    fn from_values(usage_log: Option<String>, metrics_bind: Option<String>) -> Self {
        let mut ops = Self::default();
        if let Some(path) = usage_log.filter(|value| !value.trim().is_empty()) {
            match JsonLinesUsageSink::open(&path) {
                Ok(sink) => ops.usage_sink = Arc::new(sink),
                Err(error) => {
                    tracing::warn!(%error, path = %path, "FE_API_USAGE_LOG not writable; usage metering off")
                }
            }
        }
        ops.metrics_bind = metrics_bind.filter(|value| !value.trim().is_empty());
        ops
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::usage::UsageEvent;

    fn sample_event() -> UsageEvent {
        UsageEvent {
            caller_did: None,
            scope: None,
            route_class: "/api/v1/health".to_string(),
            method: "GET".to_string(),
            status: 200,
            bytes_in: 0,
            bytes_out: 0,
            duration_ms: 0,
            at_unix_ms: 0,
        }
    }

    #[tokio::test]
    async fn network_ops_from_env_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_file = dir.path().join("usage.jsonl");
        let unopenable = dir.path().to_string_lossy().to_string(); // a directory cannot be opened for append
        // (usage_log, metrics_bind, expect_log_line, expect_metrics_bind)
        let rows: Vec<(Option<String>, Option<String>, bool, Option<&str>)> = vec![
            // usage_sink_defaults_to_noop_when_fe_api_usage_log_unset
            (None, None, false, None),
            (Some(String::new()), Some("  ".into()), false, None),
            (Some(unopenable), None, false, None),
            (
                Some(log_file.to_string_lossy().to_string()),
                Some("127.0.0.1:9100".into()),
                true,
                Some("127.0.0.1:9100"),
            ),
        ];
        for (usage_log, metrics_bind, expect_line, expect_bind) in rows {
            let ops = NetworkOps::from_values(usage_log.clone(), metrics_bind);
            ops.usage_sink.record(sample_event()).await;
            assert_eq!(ops.metrics_bind.as_deref(), expect_bind, "bind for {usage_log:?}");
            assert!(ops.advertisement_signer.is_none());
            let lines = std::fs::read_to_string(&log_file).map(|text| text.lines().count()).unwrap_or(0);
            assert_eq!(lines, usize::from(expect_line), "log lines for {usage_log:?}");
        }
        let entries = std::fs::read_dir(dir.path()).expect("read dir").count();
        assert_eq!(entries, 1, "only the explicit log path may be created");
    }
}
