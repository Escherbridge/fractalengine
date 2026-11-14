//! Log output format selection (`FE_LOG_FORMAT`); see `fe-runtime/src/AGENTS.md` §log-format.

/// Environment variable naming the log output format.
pub const LOG_FORMAT_ENV: &str = "FE_LOG_FORMAT";

/// Log output format for the tracing subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable default output.
    #[default]
    Text,
    /// One JSON object per line.
    Json,
}

impl LogFormat {
    /// Only a trimmed, case-insensitive `json` selects Json; everything else is Text.
    pub fn from_env_value(value: Option<&str>) -> LogFormat {
        match value {
            Some(raw) if raw.trim().eq_ignore_ascii_case("json") => LogFormat::Json,
            _ => LogFormat::Text,
        }
    }

    /// Reads `FE_LOG_FORMAT` from the process environment.
    pub fn from_env() -> LogFormat {
        let value = std::env::var(LOG_FORMAT_ENV).ok();
        LogFormat::from_env_value(value.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_format_env_selects_json_vs_text_formatter() {
        let cases: [(Option<&str>, LogFormat); 6] = [
            (None, LogFormat::Text),
            (Some(""), LogFormat::Text),
            (Some("json"), LogFormat::Json),
            (Some(" JSON "), LogFormat::Json),
            (Some("text"), LogFormat::Text),
            (Some("pretty"), LogFormat::Text),
        ];
        for (value, expected) in cases {
            assert_eq!(LogFormat::from_env_value(value), expected, "value {value:?}");
        }
    }
}
