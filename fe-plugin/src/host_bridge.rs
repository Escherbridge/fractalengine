//! Shared host-bridge helpers consumed by both the desktop binary (`fractalengine`)
//! and the standalone relay (`fe-relay`).

use fe_runtime::extension_trace::{ExtensionTraceBuffer, ExtensionTraceInput};
use fe_runtime::scene_hook_gate::SceneHookReply;
use fe_sdk::manifest_v2::ExtensionSourceClass;
use serde_json::Value;

use crate::capability::V2Binding;

/// Build a canonical [`V2Binding`] confining a petal extension instance to one
/// verse scope (`VERSE#<v>`) and one hierarchical petal scope (`VERSE#<v>-FRACTAL#<f>-PETAL#<p>`).
pub fn v2_binding_for_petal(
    verse_id: &str,
    fractal_id: Option<&str>,
    petal_id: Option<&str>,
    source_class: Option<ExtensionSourceClass>,
) -> V2Binding {
    let verse_scope = fe_policy::build_scope(verse_id, None, None);
    let petal_scope = match (fractal_id, petal_id) {
        (Some(fid), pid) => fe_policy::build_scope(verse_id, Some(fid), pid),
        (None, _) => verse_scope.clone(),
    };
    V2Binding {
        verse_scope,
        petal_scope,
        source_class,
    }
}

/// Parse a guest `scene.*` hook JSON payload into a [`SceneHookReply`].
///
/// Explicit `"refuse"` verdicts/decisions map directly to [`SceneHookReply::Refused`];
/// malformed JSON maps to [`SceneHookReply::Fault`]; other JSON values pass through as
/// [`SceneHookReply::Output`] so `fe_runtime::scene_hook_gate` enforces augment schemas.
pub fn scene_hook_reply_from_output(output: &str) -> SceneHookReply {
    match serde_json::from_str::<Value>(output) {
        Ok(val) => {
            let decision = val
                .get("verdict")
                .or_else(|| val.get("decision"))
                .and_then(Value::as_str)
                .unwrap_or("allow");
            if decision == "refuse" {
                let reason = val
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("refused by extension")
                    .to_string();
                SceneHookReply::Refused(reason)
            } else {
                SceneHookReply::Output(val)
            }
        }
        Err(error) => SceneHookReply::Fault(format!("scene hook output: {error}")),
    }
}

/// Record a standardized `scene_hook` trace entry into [`ExtensionTraceBuffer`].
pub fn record_scene_hook_trace(
    trace: &ExtensionTraceBuffer,
    package_id: &str,
    petal_id: &str,
    hook: &str,
    reply: &SceneHookReply,
    duration_ms: Option<u64>,
) {
    let (outcome, detail) = match reply {
        SceneHookReply::Output(_) => ("allow", None),
        SceneHookReply::Refused(reason) => ("refuse", Some(reason.clone())),
        SceneHookReply::Declined(reason) => ("decline", Some(reason.clone())),
        SceneHookReply::Fault(reason) => ("fault", Some(reason.clone())),
    };
    trace.record(ExtensionTraceInput {
        kind: "scene_hook".into(),
        package_id: package_id.to_owned(),
        petal_id: Some(petal_id.to_owned()),
        hook: Some(hook.to_owned()),
        key: None,
        outcome: outcome.into(),
        detail,
        duration_ms,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_binding_builds_verse_and_petal_scopes() {
        let b = v2_binding_for_petal("v1", Some("f1"), Some("p1"), Some(ExtensionSourceClass::Device));
        assert_eq!(b.verse_scope, "VERSE#v1");
        assert_eq!(b.petal_scope, "VERSE#v1-FRACTAL#f1-PETAL#p1");
        assert_eq!(b.source_class, Some(ExtensionSourceClass::Device));

        let no_fractal = v2_binding_for_petal("v1", None, Some("p1"), None);
        assert_eq!(no_fractal.verse_scope, "VERSE#v1");
        assert_eq!(no_fractal.petal_scope, "VERSE#v1");
    }

    #[test]
    fn scene_hook_reply_maps_allow_refuse_and_fault() {
        assert_eq!(
            scene_hook_reply_from_output(r#"{"verdict":"refuse","reason":"zoning conflict"}"#),
            SceneHookReply::Refused("zoning conflict".into())
        );
        assert_eq!(
            scene_hook_reply_from_output(r#"{"decision":"refuse"}"#),
            SceneHookReply::Refused("refused by extension".into())
        );
        assert!(matches!(
            scene_hook_reply_from_output(r#"{"verdict":"allow","augment":[]}"#),
            SceneHookReply::Output(_)
        ));
        assert!(matches!(
            scene_hook_reply_from_output("{not json"),
            SceneHookReply::Fault(_)
        ));
    }

    #[test]
    fn record_scene_hook_trace_captures_outcome_and_detail() {
        let buf = ExtensionTraceBuffer::with_capacity(8);
        let reply = SceneHookReply::Refused("blocked".into());
        record_scene_hook_trace(&buf, "org.example.zoning", "p1", "scene.create", &reply, Some(4));
        let snap = buf.snapshot(None, None, 10);
        assert_eq!(snap.events.len(), 1);
        assert_eq!(snap.events[0].outcome, "refuse");
        assert_eq!(snap.events[0].detail.as_deref(), Some("blocked"));
        assert_eq!(snap.events[0].duration_ms, Some(4));
    }
}
