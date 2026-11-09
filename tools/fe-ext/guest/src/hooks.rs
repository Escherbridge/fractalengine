//! Guest helpers for `hooks.on-hook` (bindings.md section 7.3): parsed calls, verdicts and a router.
//! Plain Rust over `serde_json`; the generated glue maps `hook-call` and `hook-output` onto these.

use serde_json::{json, Value};

/// Largest JSON value in or out of one hook call (the host enforces it too).
pub const MAX_HOOK_JSON_BYTES: usize = 64 * 1024;

/// A guest-reported outcome that is not a fault; the host's built-in serves the use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The guest declines (a `scene.*` refusal carries its reason to the caller).
    Refused(String),
    /// The input did not have the shape this hook expects.
    InvalidInput(String),
    /// The guest failed while serving.
    Failed(String),
}

/// One call with its JSON already parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Catalogue id, e.g. `petal.map`.
    pub hook: String,
    /// Binding key; empty for keyless hooks.
    pub key: String,
    /// Scope string of the binding row that selected this call.
    pub binding_scope: String,
    /// The binding's validated config.
    pub config: Value,
    /// Hook-specific input.
    pub input: Value,
    /// Bytes for `ingest.decode`, `asset.import`, `blob.mirror`.
    pub body: Vec<u8>,
}

impl Call {
    /// Parse the wire strings; malformed JSON is `InvalidInput`.
    pub fn parse(
        hook: String,
        key: String,
        binding_scope: String,
        config: &str,
        input: &str,
        body: Vec<u8>,
    ) -> Result<Self, Failure> {
        let parse = |what: &str, text: &str| {
            serde_json::from_str::<Value>(text).map_err(|error| Failure::InvalidInput(format!("{what}: {error}")))
        };
        Ok(Self {
            hook,
            key,
            binding_scope,
            config: parse("config", config)?,
            input: parse("input", input)?,
            body,
        })
    }

    /// True for the `configure` call a service hook gets at start and on every config change.
    pub fn is_configure(&self) -> bool {
        self.input.get("event").and_then(Value::as_str) == Some("configure")
    }
}

/// What a hook returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Hook-specific output JSON.
    pub output: Value,
    /// Bytes for `asset.import`, `blob.source`, `sync.tileset-source`.
    pub body: Vec<u8>,
}

impl Output {
    /// JSON only, no bytes.
    pub fn json(output: Value) -> Self {
        Self { output, body: Vec::new() }
    }

    /// `{}`: the answer for `configure` and for hooks that return nothing.
    pub fn empty() -> Self {
        Self::json(json!({}))
    }

    /// `scene.*` verdict: let the write proceed.
    pub fn allow() -> Self {
        Self::json(json!({ "verdict": "allow" }))
    }

    /// `scene.*` verdict: refuse the write with a reason shown to the caller.
    pub fn refuse(reason: &str) -> Self {
        Self::json(json!({ "verdict": "refuse", "reason": reason }))
    }

    /// Wire form: serialized JSON within the 64 KiB cap, plus the bytes.
    pub fn render(self) -> Result<(String, Vec<u8>), Failure> {
        let text = self.output.to_string();
        if text.len() > MAX_HOOK_JSON_BYTES {
            return Err(Failure::Failed("hook output exceeds 64 KiB".into()));
        }
        Ok((text, self.body))
    }
}

/// A hook implementation.
pub type HookFn = fn(&Call) -> Result<Output, Failure>;

/// Dispatch by hook id to the first matching handler. An id the extension did not register is
/// `Failed`, never `Refused`: a missing handler must not block a `scene.*` write (the built-in serves).
pub fn route(call: &Call, handlers: &[(&str, HookFn)]) -> Result<Output, Failure> {
    match handlers.iter().find(|(hook, _)| *hook == call.hook) {
        Some((_, handler)) => handler(call),
        None => Err(Failure::Failed(format!("hook {} has no handler in this extension", call.hook))),
    }
}

/// True when `key` is served by a `provides[].keys` list: an exact key, a `prefix*` or `*`.
pub fn key_matches(served: &[&str], key: &str) -> bool {
    served.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => pattern.len() == 1 || key.starts_with(prefix),
        None => *pattern == key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse, route and render one call; malformed input and unregistered hooks are guest outcomes.
    #[test]
    fn calls_parse_route_and_render_within_the_caps() {
        fn map(call: &Call) -> Result<Output, Failure> {
            if call.is_configure() {
                return Ok(Output::empty());
            }
            Ok(Output::json(json!({ "terrain": call.config["terrain"].clone() })))
        }
        let handlers: [(&str, HookFn); 1] = [("petal.map", map)];
        let call = |hook: &str, input: &str| {
            Call::parse(hook.into(), String::new(), "PETAL#p1".into(), r#"{"terrain":null}"#, input, Vec::new())
        };

        let configure = call("petal.map", r#"{"event":"configure"}"#).unwrap();
        assert_eq!(route(&configure, &handlers).unwrap(), Output::empty());
        let served = route(&call("petal.map", "{}").unwrap(), &handlers).unwrap();
        assert_eq!(served.render().unwrap().0, r#"{"terrain":null}"#);
        assert!(matches!(route(&call("scene.create", "{}").unwrap(), &handlers), Err(Failure::Failed(_))));
        assert!(matches!(call("petal.map", "{oops"), Err(Failure::InvalidInput(_))));
        assert!(Output::json(Value::String("x".repeat(MAX_HOOK_JSON_BYTES))).render().is_err());

        assert!(key_matches(&["text/*", "image/png"], "text/csv"));
        assert!(key_matches(&["image/png"], "image/png"));
        assert!(!key_matches(&["image/png"], "image/jpeg"));
        assert!(key_matches(&["*"], "anything"));
    }
}
