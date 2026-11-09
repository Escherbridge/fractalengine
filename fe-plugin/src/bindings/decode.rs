//! `ingest.decode` after-commit call (bindings.md §6, BND-10b); rationale in `fe-plugin/src/bindings/AGENTS.md` §decode.

use std::sync::Mutex;

use fe_runtime::extension_binding::BindingPoint;
use fe_sdk::source_class::SourceClass;
use serde_json::{json, Value};

use super::activation::{BindingActivator, BindingLauncher, CallTarget};
use crate::lifecycle::PluginManager;

/// Catalogue id of the decoder hook.
pub const DECODE_HOOK: &str = "ingest.decode";
/// Largest body handed to the guest (the hook's 1 MiB cap).
pub const MAX_DECODE_BODY_BYTES: usize = 1024 * 1024;

/// One file of an accepted payload, as the guest sees it in `input.files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeFile {
    pub asset_id: String,
    pub content_type: String,
    pub size: u64,
}

/// An accepted payload on the accepting peer; built by the binary from the DB-thread accepted-payload event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeJob {
    pub payload_id: String,
    pub format_id: String,
    pub mime: String,
    pub node_id: String,
    pub petal_id: String,
    pub recorded_at_ms: i64,
    pub asset_ids: Vec<String>,
}

/// Host-set provenance for readings staged during one decode call; the guest can set neither field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeStamp {
    pub payload_id: String,
    pub source_class: SourceClass,
}

impl DecodeStamp {
    /// The only stamp a decode call ever carries.
    pub fn for_payload(payload_id: &str) -> Self {
        Self { payload_id: payload_id.to_string(), source_class: SourceClass::Decoder }
    }
}

/// The hook call as the host delivers it: `HookRequest { hook: "ingest.decode", key: format_id, .. }`.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeCall {
    pub key: String,
    pub binding_scope: String,
    pub config: Value,
    pub input: Value,
    pub body: Vec<u8>,
    pub stamp: DecodeStamp,
}

/// What a finished call returns: the hook's output JSON (`{staged: n}`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeReply {
    pub output: Value,
}

/// Why a call produced no reply; none of them touches the stored payload (fail open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeCallError {
    /// The guest declined or rejected the input.
    Declined(String),
    /// A trap, fuel exhaustion, timeout or bad output; the host degrades the instance.
    Fault(String),
}

/// What only the binary can do for a decode: read stored bytes and call the running instance.
pub trait DecodeHost {
    /// Size and bytes of each asset in `asset_ids` order; `Err` when a blob is not locally readable.
    fn payload_files(&mut self, asset_ids: &[String]) -> Result<Vec<(DecodeFile, Vec<u8>)>, String>;
    /// Call the instance named by `target`, with `call.stamp` applied to every reading it stages until the call returns.
    fn call_decode(&mut self, target: &CallTarget, call: &DecodeCall) -> Result<DecodeReply, DecodeCallError>;
}

/// Why a payload stayed opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpaqueReason {
    /// Nothing is bound, the binding is `builtin`/`elsewhere`, or the point is unresolved here (OQ-B10: never decoded later).
    Unresolved,
    /// The bytes are over the 1 MiB hook cap.
    TooLarge,
    /// A blob is not readable on this peer.
    Unreadable(String),
}

/// The result of one decode attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeOutcome {
    /// The guest ran; `staged` readings wait for the instance's next tick flush.
    Decoded { staged: u64 },
    /// No call was made; the payload stays as stored.
    Opaque(OpaqueReason),
    /// The call failed; the payload stays as stored and the host counts the fault (fail open).
    Failed(DecodeCallError),
}

/// Decode one locally accepted payload: lazy-start the bound decoder, hand it the bytes, and leave its readings staged.
pub fn decode_payload(
    activator: &mut BindingActivator,
    manager: &Mutex<PluginManager>,
    launcher: &mut dyn BindingLauncher,
    host: &mut dyn DecodeHost,
    job: &DecodeJob,
) -> DecodeOutcome {
    let point = BindingPoint::new(DECODE_HOOK, job.format_id.clone());
    // Resolve before reading any blob: an unbound format costs nothing.
    let Ok(target) = activator.ensure_call_target(&job.petal_id, &point, manager, launcher) else {
        return DecodeOutcome::Opaque(OpaqueReason::Unresolved);
    };
    let files = match host.payload_files(&job.asset_ids) {
        Ok(files) => files,
        Err(reason) => return DecodeOutcome::Opaque(OpaqueReason::Unreadable(reason)),
    };
    let total: usize = files.iter().map(|(_, bytes)| bytes.len()).sum();
    if total > MAX_DECODE_BODY_BYTES {
        return DecodeOutcome::Opaque(OpaqueReason::TooLarge);
    }
    let call = build_call(&target, job, &files);
    match host.call_decode(&target, &call) {
        Ok(reply) => DecodeOutcome::Decoded { staged: staged_count(&reply.output) },
        Err(error) => DecodeOutcome::Failed(error),
    }
}

/// The call for `job`: bindings.md §6 input plus the concatenated bytes, stamped by the host.
pub fn build_call(target: &CallTarget, job: &DecodeJob, files: &[(DecodeFile, Vec<u8>)]) -> DecodeCall {
    let listed: Vec<Value> = files
        .iter()
        .map(|(file, _)| json!({ "asset_id": file.asset_id, "content_type": file.content_type, "size": file.size }))
        .collect();
    let mut body = Vec::with_capacity(files.iter().map(|(_, bytes)| bytes.len()).sum());
    for (_, bytes) in files {
        body.extend_from_slice(bytes);
    }
    DecodeCall {
        key: job.format_id.clone(),
        binding_scope: target.binding_scope.clone(),
        config: target.config.clone(),
        input: json!({
            "payload_id": job.payload_id,
            "format_id": job.format_id,
            "mime": job.mime,
            "node_id": job.node_id,
            "recorded_at_ms": job.recorded_at_ms,
            "files": listed,
        }),
        body,
        stamp: DecodeStamp::for_payload(&job.payload_id),
    }
}

/// `{staged: n}` from the guest; anything else counts zero (the window, not the reply, is authoritative).
fn staged_count(output: &Value) -> u64 {
    output.get("staged").and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> CallTarget {
        CallTarget {
            plugin_id: "p".into(),
            installation_id: "i".into(),
            key: crate::bus::InstanceKey::new("petal".to_string(), "pkg".to_string()),
            binding_scope: "FRACTAL#f".into(),
            config: json!({}),
        }
    }

    #[test]
    fn call_is_host_stamped_and_concatenates_files() {
        let job = DecodeJob {
            payload_id: "01P".into(),
            format_id: "acme.thermo/frame@1".into(),
            mime: "application/octet-stream".into(),
            node_id: "n1".into(),
            petal_id: "petal".into(),
            recorded_at_ms: 5,
            asset_ids: vec!["a".into(), "b".into()],
        };
        let file = |id: &str| DecodeFile { asset_id: id.into(), content_type: job.mime.clone(), size: 2 };
        let call = build_call(&target(), &job, &[(file("a"), vec![1, 2]), (file("b"), vec![3, 4])]);
        assert_eq!(call.body, vec![1, 2, 3, 4]);
        assert_eq!(call.stamp, DecodeStamp { payload_id: "01P".into(), source_class: SourceClass::Decoder });
        assert_eq!(call.input["files"].as_array().map(Vec::len), Some(2));
        assert_eq!(staged_count(&json!({ "staged": 12 })), 12);
        assert_eq!(staged_count(&json!("x")), 0);
    }
}
