//! Per-agent kubectl-style context file (S2).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::envelope::{CliError, ExitCode};
use crate::Env;

/// An id plus its display name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NamedRef {
    pub id: String,
    pub name: String,
}

/// Context file shape, version 1.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Context {
    pub v: u32,
    pub instance: Option<String>,
    pub verse: Option<NamedRef>,
    pub fractal: Option<NamedRef>,
    pub petal: Option<NamedRef>,
    /// RFC3339 time of the last hierarchy read (2 s throttle).
    pub hierarchy_read_at: Option<String>,
}

/// CLI state directory: `FE_STATE_DIR`, else `%LOCALAPPDATA%` / `$XDG_STATE_HOME` / `~/.local/state` + `fractalengine/cli`.
pub fn state_dir(env: &Env) -> PathBuf {
    let non_empty = |key: &str| env.get(key).map(String::as_str).filter(|value| !value.trim().is_empty());
    if let Some(explicit) = non_empty("FE_STATE_DIR") {
        return PathBuf::from(explicit);
    }
    let root = if let Some(local) = non_empty("LOCALAPPDATA") {
        PathBuf::from(local)
    } else if let Some(xdg) = non_empty("XDG_STATE_HOME") {
        PathBuf::from(xdg)
    } else if let Some(home) = non_empty("HOME").or_else(|| non_empty("USERPROFILE")) {
        PathBuf::from(home).join(".local").join("state")
    } else if let Some(temp) = non_empty("TEMP").or_else(|| non_empty("TMPDIR")) {
        PathBuf::from(temp)
    } else {
        PathBuf::from(".")
    };
    root.join("fractalengine").join("cli")
}

/// Context path from `FE_CONTEXT`, else the per-instance state-dir default.
pub fn context_path(env: &Env, instance: &str) -> PathBuf {
    if let Some(explicit) = env.get("FE_CONTEXT").map(|value| value.trim()).filter(|value| !value.is_empty()) {
        return PathBuf::from(explicit);
    }
    let safe: String = instance
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') { character } else { '_' })
        .collect();
    state_dir(env).join(format!("ctx-{safe}.json"))
}

/// Loads the context; a missing file yields the default.
pub fn load(path: &Path) -> Result<Context, CliError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Context { v: 1, ..Context::default() }),
        Err(error) => {
            return Err(CliError::new(
                ExitCode::Internal,
                "context-unreadable",
                format!("cannot read context {}: {}", path.display(), error.kind()),
            ))
        }
    };
    serde_json::from_str::<Context>(text.trim_start_matches('\u{feff}')).map_err(|error| {
        CliError::new(
            ExitCode::Internal,
            "context-corrupt",
            format!("context {} is not valid ({error}); run `fe ctx clear`", path.display()),
        )
    })
}

/// Saves atomically (temp file + rename).
pub fn save(path: &Path, context: &Context) -> Result<(), CliError> {
    let failure = |error: std::io::Error| {
        CliError::new(
            ExitCode::Internal,
            "context-write-failed",
            format!("cannot write context {}: {}", path.display(), error.kind()),
        )
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(failure)?;
        }
    }
    let mut document = context.clone();
    document.v = 1;
    let bytes = serde_json::to_vec_pretty(&document).expect("context serializes");
    let mut temporary_name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    std::fs::write(&temporary, bytes).map_err(failure)?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        failure(error)
    })
}
