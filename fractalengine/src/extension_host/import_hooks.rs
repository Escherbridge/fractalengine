//! `asset.import` over the binding worker: implements `fe_api::import_hooks::ImportHooks` (R-HOOKS-30).
//! Pinned seam for I-BIN: implement [`ImportWorkerPort`] on the binding hub (`bound` reads the published view,
//! `convert` sends a body-carrying worker signal and returns the worker's reply receiver) and publish
//! [`ImportHostAdapter::new`]`(port).handle()` as `plugin_bridge::import_hooks_handle`. The existing `Call`
//! signal drops the reply body, so I-BIN adds a signal that keeps it. See `src/AGENTS.md` §extension-host.

use std::sync::Arc;

use crossbeam::channel::Receiver;
use fe_api::import_hooks::{
    normalize_mime, ImportCall, ImportHooks, ImportHooksHandle, ImportOutput, ImportRefusal, ImportReply, IMPORT_DEADLINE, IMPORT_HOOK,
    MAX_IMPORT_BYTES,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// What the worker produced for one `asset.import` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerAnswer {
    /// The guest returned `output` JSON (`{"name"?}`) and the GLB `body`.
    Output { output: String, body: Vec<u8> },
    /// The guest declined the file (`refused` or `invalid-input`).
    Refused(String),
    /// The binding moved or no instance serves the point.
    Unbound,
    /// Trap, deadline or host fault.
    Failed,
}

/// The binding hub as this adapter needs it.
pub trait ImportWorkerPort: Send + Sync {
    /// Whether `asset.import` for `mime` has an effective, resolved binding at the petal (published view, never blocks).
    fn bound(&self, petal_id: &str, mime: &str) -> bool;
    /// Queue one call on the worker (lazily starting the serving instance); the answer arrives on the receiver.
    fn convert(&self, petal_id: &str, mime: &str, input: Value, body: Vec<u8>) -> Receiver<WorkerAnswer>;
}

/// [`ImportHooks`] over a worker port.
pub struct ImportHostAdapter {
    port: Arc<dyn ImportWorkerPort>,
}

impl ImportHostAdapter {
    pub fn new(port: Arc<dyn ImportWorkerPort>) -> Self {
        Self { port }
    }

    /// The request extension fe-api layers onto the upload route.
    pub fn handle(self) -> ImportHooksHandle {
        ImportHooksHandle(Arc::new(self))
    }
}

/// The hook input of bindings.md section 6: `{petal_id, file_name, mime}`.
pub fn hook_input(call: &ImportCall) -> Value {
    json!({ "petal_id": call.petal_id, "file_name": call.file_name, "mime": call.mime })
}

/// The display name from the hook's output JSON: a non-empty string, control characters removed, 128 chars at most.
pub fn output_name(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    let name: String = value.get("name")?.as_str()?.chars().filter(|c| !c.is_control()).take(128).collect();
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Turn the worker's answer into the seam's result; an oversized or empty GLB is a failure, not a pass-through.
pub fn map_answer(answer: WorkerAnswer) -> Result<ImportOutput, ImportRefusal> {
    match answer {
        WorkerAnswer::Output { output, body } => {
            if body.is_empty() || body.len() > MAX_IMPORT_BYTES {
                tracing::warn!(len = body.len(), "asset.import guest returned an unusable GLB size");
                return Err(ImportRefusal::Failed);
            }
            Ok(ImportOutput { glb: body, name: output_name(&output) })
        }
        WorkerAnswer::Refused(reason) => Err(ImportRefusal::Refused(reason)),
        WorkerAnswer::Unbound => Err(ImportRefusal::Unbound),
        WorkerAnswer::Failed => Err(ImportRefusal::Failed),
    }
}

impl ImportHooks for ImportHostAdapter {
    fn is_bound(&self, petal_id: &str, mime: &str) -> bool {
        normalize_mime(mime).is_some_and(|mime| self.port.bound(petal_id, &mime))
    }

    fn import(&self, call: ImportCall) -> Result<ImportReply, ImportRefusal> {
        let mime = normalize_mime(&call.mime).ok_or(ImportRefusal::Unbound)?;
        if !self.port.bound(&call.petal_id, &mime) {
            return Err(ImportRefusal::Unbound);
        }
        let input = hook_input(&call);
        let answers = self.port.convert(&call.petal_id, &mime, input, call.body);
        let (reply, receiver) = oneshot::channel();
        let forward = move || {
            let answer = answers.recv_timeout(IMPORT_DEADLINE).unwrap_or(WorkerAnswer::Failed);
            let _ = reply.send(map_answer(answer));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(forward);
            }
            Err(_) => {
                if let Err(error) = std::thread::Builder::new().name("fe-ext-import-reply".into()).spawn(forward) {
                    tracing::warn!(%error, hook = IMPORT_HOOK, "import reply thread did not start");
                    return Err(ImportRefusal::Failed);
                }
            }
        }
        Ok(receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Port {
        bound_mime: &'static str,
        answer: WorkerAnswer,
    }

    impl ImportWorkerPort for Port {
        fn bound(&self, _petal_id: &str, mime: &str) -> bool {
            mime == self.bound_mime
        }

        fn convert(&self, _petal_id: &str, _mime: &str, input: Value, body: Vec<u8>) -> Receiver<WorkerAnswer> {
            assert_eq!(input["file_name"], "part.stl", "the hook input names the file");
            assert_eq!(body, b"stl-bytes");
            let (sender, receiver) = crossbeam::channel::bounded(1);
            sender.send(self.answer.clone()).expect("receiver is held");
            receiver
        }
    }

    fn call(mime: &str) -> ImportCall {
        ImportCall {
            petal_id: "p1".into(),
            file_name: "part.stl".into(),
            mime: mime.into(),
            body: b"stl-bytes".to_vec(),
            caller_did: "did:key:z".into(),
        }
    }

    /// Journey: bound MIME converts with a name, an unbound MIME is refused up front, guest refusal and a bad GLB map.
    #[test]
    fn import_journey_bound_mime_converts_and_failures_map() {
        let converted = WorkerAnswer::Output { output: r#"{"name":"Part\n One"}"#.into(), body: b"glb".to_vec() };
        let adapter = ImportHostAdapter::new(Arc::new(Port { bound_mime: "model/stl", answer: converted }));
        assert!(adapter.is_bound("p1", "Model/STL; charset=x") && !adapter.is_bound("p1", "image/png"));
        let output = adapter.import(call("model/stl")).expect("bound").blocking_recv().expect("answered").expect("converted");
        assert_eq!((output.glb.as_slice(), output.name.as_deref()), (&b"glb"[..], Some("Part One")));
        assert_eq!(adapter.import(call("image/png")).err(), Some(ImportRefusal::Unbound));

        let refused = ImportHostAdapter::new(Arc::new(Port { bound_mime: "model/stl", answer: WorkerAnswer::Refused("not manifold".into()) }));
        let answer = refused.import(call("model/stl")).expect("bound").blocking_recv().expect("answered");
        assert_eq!(answer.err(), Some(ImportRefusal::Refused("not manifold".into())));

        let empty = WorkerAnswer::Output { output: "{}".into(), body: Vec::new() };
        assert_eq!(map_answer(empty).err(), Some(ImportRefusal::Failed), "an empty GLB is a failure");
    }
}
