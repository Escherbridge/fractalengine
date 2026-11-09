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

/// One file of an accepted payload as its stored asset row declares it; the bytes must match `size` and `content_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeFile {
    pub asset_id: String,
    pub content_type: String,
    pub size: u64,
    /// Lowercase hex BLAKE3 of the blob, from the asset row (never shown to the guest).
    pub content_hash: String,
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

/// What only the binary can do for a decode: describe and read stored blobs, and call the running instance.
pub trait DecodeHost {
    /// Each asset's stored row in `asset_ids` order (size, type, BLAKE3); reads no blob byte. `Err` when one is not stored here.
    fn declared_files(&mut self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String>;
    /// At most `limit` bytes of `file`'s local blob, through a reader that stops there; `Err` when it is not readable.
    fn read_bounded(&mut self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String>;
    /// Call the instance named by `target`, with `call.stamp` applied to every reading it stages until the call returns.
    fn call_decode(&mut self, target: &CallTarget, call: &DecodeCall) -> Result<DecodeReply, DecodeCallError>;
}

/// Why a payload stayed opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpaqueReason {
    /// Nothing is bound, the binding is `builtin`/`elsewhere`, or the point is unresolved here (OQ-B10: never decoded later).
    Unresolved,
    /// The declared sizes are over the 1 MiB hook cap; no byte was read.
    TooLarge,
    /// A blob is not stored or readable on this peer.
    Unreadable(String),
    /// A blob's bytes do not match its stored size or BLAKE3; nothing of it reaches the guest.
    Mismatch(String),
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
    let files = match read_within_cap(host, &job.asset_ids) {
        Ok(files) => files,
        Err(reason) => return DecodeOutcome::Opaque(reason),
    };
    let call = build_call(&target, job, &files);
    match host.call_decode(&target, &call) {
        Ok(reply) => DecodeOutcome::Decoded { staged: staged_count(&reply.output) },
        Err(error) => DecodeOutcome::Failed(error),
    }
}

/// The payload's bytes: declared sizes are summed against the cap before any read (steer 2026-11-09 02:25), each file is
/// read bounded by its declared size, and kept only when its length and BLAKE3 match the stored row.
pub fn read_within_cap(host: &mut dyn DecodeHost, asset_ids: &[String]) -> Result<Vec<(DecodeFile, Vec<u8>)>, OpaqueReason> {
    let declared = host.declared_files(asset_ids).map_err(OpaqueReason::Unreadable)?;
    if declared.len() != asset_ids.len() {
        return Err(OpaqueReason::Unreadable("the host described another set of assets".into()));
    }
    let total = declared.iter().fold(0u64, |sum, file| sum.saturating_add(file.size));
    if total > MAX_DECODE_BODY_BYTES as u64 {
        return Err(OpaqueReason::TooLarge);
    }
    let mut files = Vec::with_capacity(declared.len());
    for file in declared {
        // One byte past the declared size tells a longer blob from an exact one; the sum above bounds every limit.
        let bytes = host.read_bounded(&file, file.size + 1).map_err(OpaqueReason::Unreadable)?;
        let matches = bytes.len() as u64 == file.size
            && blake3::hash(&bytes).to_hex().as_str() == file.content_hash.to_ascii_lowercase();
        if !matches {
            return Err(OpaqueReason::Mismatch(file.asset_id));
        }
        files.push((file, bytes));
    }
    Ok(files)
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

    /// A store whose rows declare `(size, hash)` and whose blobs hold `bytes`; records every read and its limit.
    struct Store {
        rows: Vec<(String, u64, String)>,
        blobs: Vec<(String, Vec<u8>)>,
        reads: Vec<(String, u64)>,
    }

    impl DecodeHost for Store {
        fn declared_files(&mut self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String> {
            asset_ids
                .iter()
                .map(|id| -> Result<DecodeFile, String> {
                    let (_, size, hash) = self.rows.iter().find(|(row, _, _)| row == id).ok_or(format!("{id} is not stored"))?;
                    Ok(DecodeFile { asset_id: id.clone(), content_type: "application/octet-stream".into(), size: *size, content_hash: hash.clone() })
                })
                .collect()
        }

        fn read_bounded(&mut self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String> {
            self.reads.push((file.asset_id.clone(), limit));
            let (_, bytes) = self.blobs.iter().find(|(id, _)| *id == file.asset_id).ok_or("no blob")?;
            Ok(bytes[..bytes.len().min(limit as usize)].to_vec())
        }

        fn call_decode(&mut self, _target: &CallTarget, _call: &DecodeCall) -> Result<DecodeReply, DecodeCallError> {
            Err(DecodeCallError::Declined("not called here".into()))
        }
    }

    /// The decode journey at the byte seam: the call is host-stamped over concatenated, verified files; an oversized
    /// payload is refused from its declared sizes with no read at all (steer 02:25); a blob longer than its row, or one
    /// with other bytes, never reaches the guest.
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
        let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
        let mut store = Store {
            rows: vec![("a".into(), 2, hash(&[1, 2])), ("b".into(), 2, hash(&[3, 4]))],
            blobs: vec![("a".into(), vec![1, 2]), ("b".into(), vec![3, 4])],
            reads: Vec::new(),
        };
        let files = read_within_cap(&mut store, &job.asset_ids).unwrap();
        assert_eq!(store.reads, [("a".to_string(), 3), ("b".to_string(), 3)], "each read stops one byte past its row's size");
        let call = build_call(&target(), &job, &files);
        assert_eq!(call.body, vec![1, 2, 3, 4]);
        assert_eq!(call.stamp, DecodeStamp { payload_id: "01P".into(), source_class: SourceClass::Decoder });
        assert_eq!(call.input["files"].as_array().map(Vec::len), Some(2));
        assert!(call.input["files"][0].get("content_hash").is_none(), "the guest sees size and type, not the store's hash");
        assert_eq!(staged_count(&json!({ "staged": 12 })), 12);
        assert_eq!(staged_count(&json!("x")), 0);

        // Two files that each fit but together pass 1 MiB: refused from the rows alone, nothing read.
        let half = MAX_DECODE_BODY_BYTES as u64 / 2 + 1;
        let mut oversized = Store {
            rows: vec![("a".into(), half, hash(&[])), ("b".into(), half, hash(&[]))],
            blobs: Vec::new(),
            reads: Vec::new(),
        };
        assert_eq!(read_within_cap(&mut oversized, &job.asset_ids), Err(OpaqueReason::TooLarge));
        assert!(oversized.reads.is_empty(), "an oversized payload is never loaded");

        // A row that understates its blob is cut at size + 1 and refused; so is a blob with other bytes.
        store.rows[1].1 = 1;
        assert_eq!(read_within_cap(&mut store, &job.asset_ids), Err(OpaqueReason::Mismatch("b".into())));
        store.rows[1] = ("b".into(), 2, hash(&[9, 9]));
        assert_eq!(read_within_cap(&mut store, &job.asset_ids), Err(OpaqueReason::Mismatch("b".into())));
    }
}
