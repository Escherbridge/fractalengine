//! Loaded-token guard: no outgoing argument may carry a token the CLI holds (AGENTS.md §secrets).

use std::path::Path;

use serde_json::Value;

use crate::envelope::{CliError, ExitCode};
use crate::instance::TokenFile;

/// Shorter strings are never treated as tokens (a JWT is far longer).
const MIN_SECRET_CHARS: usize = 16;

/// Every bearer the token file holds: verse tokens and the operator token.
pub fn loaded(tokens: &TokenFile) -> Vec<String> {
    tokens
        .tokens
        .values()
        .chain(tokens.operator_token.iter())
        .map(|token| token.trim().to_string())
        .filter(|token| token.chars().count() >= MIN_SECRET_CHARS)
        .collect()
}

/// Whether `text` contains any loaded secret.
pub fn text_has_secret(text: &str, secrets: &[String]) -> bool {
    secrets.iter().any(|secret| text.contains(secret.as_str()))
}

/// Whether any string (key or value) anywhere in `value` contains a loaded secret.
pub fn value_has_secret(value: &Value, secrets: &[String]) -> bool {
    match value {
        Value::String(text) => text_has_secret(text, secrets),
        Value::Array(items) => items.iter().any(|item| value_has_secret(item, secrets)),
        Value::Object(map) => map.iter().any(|(key, inner)| text_has_secret(key, secrets) || value_has_secret(inner, secrets)),
        _ => false,
    }
}

/// The exit-6 refusal; never echoes the value.
pub fn token_in_args() -> CliError {
    CliError::new(ExitCode::Usage, "token-in-args", "the arguments carry an API token this CLI holds; refusing to send it")
}

/// Refuses `--args @<path>` / `--args=@<path>` naming the instance's token file (compared canonically).
pub fn refuse_token_file_args(words: &[String], token_file: &Path) -> Result<(), CliError> {
    let Ok(token_file) = std::fs::canonicalize(token_file) else { return Ok(()) };
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let target = match word.as_str() {
            "--args" => words.next().and_then(|next| next.strip_prefix('@')),
            other => other.strip_prefix("--args=@"),
        };
        if target.and_then(|path| std::fs::canonicalize(path).ok()).is_some_and(|path| path == token_file) {
            return Err(token_in_args());
        }
    }
    Ok(())
}
