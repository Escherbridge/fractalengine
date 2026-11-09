//! `asset.import` hook seam (docs/extensions/bindings.md §6): a non-GLB upload whose MIME matches a bound
//! key is converted to GLB by the bound extension. Rationale: `fe-api/AGENTS.md` §import-hooks.
//!
//! fe-api never links the plugin host: the binary implements [`ImportHooks`] over the binding resolution
//! cache and the guest `on-hook` call. This module owns the caps, the deadline and the error mapping; the
//! caller (`upload.rs`) re-validates the output as a GLB through the normal store path.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use tokio::sync::oneshot;

/// The catalogue id this seam serves.
pub const IMPORT_HOOK: &str = "asset.import";
/// Largest input to, and output from, the hook.
pub const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;
/// Call deadline (bindings.md §6).
pub const IMPORT_DEADLINE: Duration = Duration::from_secs(30);
/// Longest accepted MIME key.
const MAX_MIME_LEN: usize = 127;

/// One conversion request: the hook input `{petal_id, file_name, mime}` plus the file bytes.
#[derive(Debug, Clone)]
pub struct ImportCall {
    pub petal_id: String,
    pub file_name: String,
    /// Normalised: lowercase, no parameters.
    pub mime: String,
    pub body: Vec<u8>,
    /// The token's `sub`; informational only (the upload path owns the write).
    pub caller_did: String,
}

/// What the guest returned: GLB bytes and an optional display name.
#[derive(Debug, Clone)]
pub struct ImportOutput {
    pub glb: Vec<u8>,
    pub name: Option<String>,
}

/// Why the hook did not produce a GLB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportRefusal {
    /// No effective binding for this MIME at the petal (or its package is not running).
    Unbound,
    /// The guest declined the file (`refused` / `invalid-input`); the text is shown to the uploader.
    Refused(String),
    /// Trap, fuel exhaustion, missed deadline or host fault (logged by the host; generic to the client).
    Failed,
}

/// The eventual answer of one conversion.
pub type ImportReply = oneshot::Receiver<Result<ImportOutput, ImportRefusal>>;

/// What the binary provides: resolution and the guest call.
pub trait ImportHooks: Send + Sync {
    /// Whether `mime` has an effective, resolved `asset.import` binding at the petal.
    fn is_bound(&self, petal_id: &str, mime: &str) -> bool;
    /// Start one conversion. `Err(Unbound)` if the binding vanished since [`ImportHooks::is_bound`].
    fn import(&self, call: ImportCall) -> Result<ImportReply, ImportRefusal>;
}

/// The host as an axum request extension (the binary layers it onto the upload route).
#[derive(Clone)]
pub struct ImportHooksHandle(pub Arc<dyn ImportHooks>);

/// A hook failure with its HTTP status and client text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    pub status: StatusCode,
    pub message: String,
}

impl ImportError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
}

/// Lowercase the MIME and drop parameters; `None` if empty, oversized or not `type/subtype`.
pub fn normalize_mime(raw: &str) -> Option<String> {
    let essence = raw.split(';').next()?.trim().to_ascii_lowercase();
    let valid = !essence.is_empty()
        && essence.len() <= MAX_MIME_LEN
        && essence.matches('/').count() == 1
        && essence.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-_".contains(c));
    valid.then_some(essence)
}

/// Run one bound conversion: size caps both ways and the 30 s deadline.
pub async fn run_import(host: &dyn ImportHooks, call: ImportCall) -> Result<ImportOutput, ImportError> {
    if call.body.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("a converted upload may be at most {MAX_IMPORT_BYTES} bytes"),
        ));
    }
    let reply = host.import(call).map_err(map_refusal)?;
    let answer = match tokio::time::timeout(IMPORT_DEADLINE, reply).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => return Err(map_refusal(ImportRefusal::Failed)),
        Err(_) => {
            tracing::warn!("asset.import hook missed its deadline");
            return Err(ImportError::new(StatusCode::GATEWAY_TIMEOUT, "the import extension timed out"));
        }
    };
    let output = answer.map_err(map_refusal)?;
    if output.glb.len() > MAX_IMPORT_BYTES {
        tracing::warn!(len = output.glb.len(), "asset.import hook output over cap");
        return Err(ImportError::new(StatusCode::BAD_GATEWAY, "the import extension returned an oversized result"));
    }
    Ok(output)
}

fn map_refusal(refusal: ImportRefusal) -> ImportError {
    match refusal {
        ImportRefusal::Unbound => ImportError::new(StatusCode::BAD_REQUEST, "no import extension is bound for this file type"),
        ImportRefusal::Refused(reason) => {
            let reason: String = reason.chars().filter(|c| !c.is_control()).take(256).collect();
            ImportError::new(StatusCode::UNPROCESSABLE_ENTITY, format!("the import extension refused the file: {reason}"))
        }
        ImportRefusal::Failed => ImportError::new(StatusCode::BAD_GATEWAY, "the import extension failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mime_table() {
        let cases = [
            ("Model/STL; charset=x", Some("model/stl")),
            ("application/vnd.ms-pki+x", Some("application/vnd.ms-pki+x")),
            ("", None),
            ("noslash", None),
            ("a/b/c", None),
            ("a/b c", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize_mime(raw).as_deref(), expected, "{raw}");
        }
    }
}
