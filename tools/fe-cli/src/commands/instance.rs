//! `fe instance list` (S3, local).

use serde_json::json;

use super::{from_cli_error, ok_outcome, usage_outcome, CommandRequest, CommandResult};
use crate::envelope::EnvelopeContext;
use crate::Env;

/// Runs one `instance` verb.
pub fn run(request: &CommandRequest, env: &Env, working_dir: &std::path::Path) -> CommandResult {
    let outcome = match request.verb.map(|verb| verb.verb) {
        Some("list") => match crate::instance::list(env, working_dir) {
            Ok(instances) => {
                let rows: Vec<_> = instances
                    .iter()
                    .map(|found| {
                        json!({
                            "name": found.name,
                            "api_base": found.api_base,
                            "tokens_file": found.tokens_path.display().to_string(),
                            "admin_instance": found.admin_instance,
                        })
                    })
                    .collect();
                ok_outcome(json!({ "instances": rows }))
            }
            Err(error) => from_cli_error(error),
        },
        _ => usage_outcome("unknown-verb", "usage: fe instance list"),
    };
    CommandResult { outcome, context_after: None, created_ids: Vec::new(), resolved_context: EnvelopeContext::default(), wait: None, resolved_args: None }
}
