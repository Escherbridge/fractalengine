//! `fe note` (S3, local; journaled by the spine).

use serde_json::{json, Value};

use super::{ok_outcome, usage_outcome, CommandRequest, CommandResult};
use crate::envelope::EnvelopeContext;

/// Longest accepted note, in characters.
const MAX_NOTE_CHARS: usize = 2000;

/// Records a friction note: the text is returned as `data.note` for the spine to journal.
pub fn run(request: &CommandRequest) -> CommandResult {
    let text = match request.args.get("text").and_then(Value::as_str) {
        Some(text) => text.to_string(),
        None => request.positionals.join(" "),
    };
    let text = text.trim().to_string();
    let outcome = if text.is_empty() {
        usage_outcome("invalid-args", "usage: fe note \"<what was confusing>\"")
    } else if text.chars().count() > MAX_NOTE_CHARS {
        usage_outcome("invalid-args", format!("note is longer than {MAX_NOTE_CHARS} characters"))
    } else {
        let mut outcome = ok_outcome(json!({ "note": text }));
        outcome.message = "noted".to_string();
        outcome
    };
    CommandResult { outcome, context_after: None, created_ids: Vec::new(), resolved_context: EnvelopeContext::default(), wait: None }
}
