//! Loopback HTTP client (MCP primary, REST fallback) and reply classification (S2).

use std::time::Duration;

use serde_json::{json, Value};

use crate::envelope::{CliError, ExitCode, Refusal};
use crate::instance::{self, Instance, TokenFile};
use crate::secrets;

/// Per-request ceiling; a request that times out after sending has an unknown outcome.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);


/// Classified result of one call.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub exit: ExitCode,
    /// Stable machine code, e.g. `ok`, `role-denied`, `not-found-or-denied`.
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
    pub retry_after_ms: Option<u64>,
    pub refusal: Refusal,
    pub http_status: Option<u16>,
    /// The server's reply proves the request was refused before admission (nothing ran); see AGENTS.md §retry.
    pub pre_admission: bool,
}

impl Outcome {
    /// A successful outcome carrying `data`.
    pub fn success(data: Option<Value>, http_status: Option<u16>) -> Self {
        Self {
            exit: ExitCode::Ok,
            code: "ok".to_string(),
            message: "ok".to_string(),
            data,
            retry_after_ms: None,
            refusal: Refusal::default(),
            http_status,
            pre_admission: false,
        }
    }

    /// A failed outcome with no data.
    pub fn failure(exit: ExitCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            exit,
            code: code.to_string(),
            message: message.into(),
            data: None,
            retry_after_ms: None,
            refusal: Refusal::default(),
            http_status: None,
            pre_admission: false,
        }
    }

    /// Converts a spine error into an outcome.
    pub fn from_error(error: &CliError) -> Self {
        Self::failure(error.exit, &error.code, error.message.clone())
    }
}

/// Maps an HTTP status, JSON body and raw text to an exit code via the stable server texts.
pub fn classify(http_status: Option<u16>, body: &Value, text: &str) -> Outcome {
    let declared_failure = body.get("ok").and_then(Value::as_bool) == Some(false);
    let status_is_success = http_status.is_none_or(|status| (200..300).contains(&status));
    if status_is_success && !declared_failure {
        let data = if body.get("ok").is_some() {
            body.get("data").filter(|data| !data.is_null()).cloned()
        } else if !body.is_null() {
            Some(body.clone())
        } else if !text.trim().is_empty() {
            Some(Value::String(text.to_string()))
        } else {
            None
        };
        return Outcome::success(data, http_status);
    }

    let message = body
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| body.get("message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| text.trim());
    let message: String = message.chars().take(1000).collect();
    let lowered = message.to_lowercase();
    let status = http_status.unwrap_or(0);

    let mut retry_after_ms = None;
    let (exit, code): (ExitCode, &str) = if lowered.contains("write budget exceeded")
        || lowered.contains("write-budget-exceeded")
        || status == 429
    {
        retry_after_ms = body.get("retry_after_ms").and_then(Value::as_u64).or_else(|| parse_retry_after_ms(&lowered));
        let code = if status == 429 && !lowered.contains("budget") { "rate-limited" } else { "write-budget-exceeded" };
        (ExitCode::Retryable, code)
    } else if status == 504 || lowered.contains("unknown-outcome") || lowered.contains("unknown outcome") {
        (ExitCode::UnknownOutcome, "unknown-outcome")
    } else if lowered.contains("insufficient permissions") || lowered.contains(" needs role ") {
        (ExitCode::RoleDenied, "role-denied")
    } else if status == 404 && (lowered.contains("not available") || lowered.contains("not wired")) {
        (ExitCode::Refused, "not-available")
    } else if lowered.contains("not found") || lowered == DELETE_REFUSED_TEXT || status == 404 {
        (ExitCode::NotFound, "not-found-or-denied")
    } else if status == 401 {
        (ExitCode::Transport, "token-rejected")
    } else if status == 403 {
        if lowered.contains("not available over the api") || lowered.contains("extensions panel") {
            (ExitCode::Refused, "admin-gui-only")
        } else if lowered.contains("polic") || lowered.contains("consent") || lowered.contains("entitlement") || lowered.contains("hook") {
            (ExitCode::Refused, "policy-denied")
        } else {
            (ExitCode::RoleDenied, "role-denied")
        }
    } else if status == 409 {
        (ExitCode::Refused, "refused")
    } else if lowered.contains("is required")
        || lowered.contains("invalid")
        || lowered.contains("unknown tool")
        || matches!(status, 400 | 405 | 413 | 415 | 422)
    {
        (ExitCode::Usage, "invalid-input")
    } else if status >= 500 {
        (ExitCode::ServerFault, "server-fault")
    } else {
        (ExitCode::ServerFault, "server-error")
    };

    let mut refusal = Refusal::default();
    if exit == ExitCode::Refused {
        refusal.reason = Some(message.clone());
        refusal.package_id = body.get("package_id").and_then(Value::as_str).map(str::to_string);
    }
    // Text-only classification never proves pre-admission (the MCP door rewrites write-budget refusals to text).
    Outcome { exit, code: code.to_string(), message, data: None, retry_after_ms, refusal, http_status, pre_admission: false }
}

/// `delete_node`'s refusal text (fe-api mcp/tools.rs): uniform by design, so it is the not-found exit.
const DELETE_REFUSED_TEXT: &str = "operation failed or not permitted";
/// fe-api's uniform not-found text (`rest::NOT_FOUND_OR_DENIED`).
const NOT_FOUND_TEXT: &str = "not found or not permitted";

/// Classifies a REST extension-command reply (`POST .../commands/{command}`) by HTTP status and the body's
/// machine `code` when present; the host text is kept verbatim as the refusal reason.
pub fn classify_extension(http_status: u16, body: &Value, text: &str, retry_after_header_ms: Option<u64>) -> Outcome {
    if (200..300).contains(&http_status) && body.get("ok").and_then(Value::as_bool) != Some(false) {
        return classify(Some(http_status), body, text);
    }
    let message: String = body
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| body.get("message").and_then(Value::as_str))
        .unwrap_or_else(|| text.trim())
        .chars()
        .take(1000)
        .collect();
    // fe-api's ExtError status table (extensions.rs `From<HostRefusal>` / `From<CommandFailure>`).
    let code = body.get("code").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| {
        match http_status {
            400 => "invalid-input",
            401 => "token-rejected",
            403 => "capability-denied",
            404 if message.trim() == NOT_FOUND_TEXT => "not-found",
            404 => "not-available",
            409 => "refused",
            429 => "rate-limited",
            502 => "failed",
            504 => "unknown-outcome",
            _ => "server-fault",
        }
        .to_string()
    });
    let (exit, code): (ExitCode, &str) = match code.as_str() {
        "invalid-input" => (ExitCode::Usage, "invalid-input"),
        "token-rejected" => (ExitCode::Transport, "token-rejected"),
        // The bus's role-floor refusal is a capability denial whose text names the floor (check 4).
        "capability-denied" if message.contains(" needs role ") => (ExitCode::RoleDenied, "role-denied"),
        "capability-denied" | "policy-denied" => (ExitCode::Refused, "capability-denied"),
        "not-found" => (ExitCode::NotFound, "not-found-or-denied"),
        "not-available" => (ExitCode::Refused, "not-available"),
        "refused" => (ExitCode::Refused, "refused"),
        "rate-limited" | "limit-reached" => (ExitCode::Retryable, "rate-limited"),
        "unknown-outcome" => (ExitCode::UnknownOutcome, "unknown-outcome"),
        "failed" => (ExitCode::ServerFault, "failed"),
        _ => (ExitCode::ServerFault, "server-fault"),
    };
    let mut refusal = Refusal::default();
    if matches!(exit, ExitCode::Refused | ExitCode::RoleDenied) {
        refusal.reason = Some(message.clone());
        refusal.package_id = body.get("package_id").and_then(Value::as_str).map(str::to_string);
    }
    let retry_after_ms = (exit == ExitCode::Retryable)
        .then(|| body.get("retry_after_ms").and_then(Value::as_u64).or(retry_after_header_ms))
        .flatten();
    Outcome {
        exit,
        code: code.to_string(),
        message,
        data: None,
        retry_after_ms,
        refusal,
        http_status: Some(http_status),
        // Every 429 on the command route is a bus refusal (gate, checks 6 and 8) taken before the call is queued.
        pre_admission: exit == ExitCode::Retryable && http_status == 429,
    }
}

/// `--retry` may resend only an exit-7 refusal that is a read, or a write the server proved was never admitted.
pub fn retry_allowed(outcome: &Outcome, write: bool) -> bool {
    outcome.exit == ExitCode::Retryable && (outcome.pre_admission || !write)
}

/// Parses `retry after N s` / `retry after N ms` from lower-cased server text.
fn parse_retry_after_ms(lowered: &str) -> Option<u64> {
    let tail = &lowered[lowered.find("retry after ")? + "retry after ".len()..];
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let amount: u64 = digits.parse().ok()?;
    let unit = tail[digits.len()..].trim_start();
    Some(if unit.starts_with("ms") { amount } else { amount.saturating_mul(1000) })
}

/// Loopback-only client bound to one instance.
pub struct Client {
    pub instance: Instance,
}

impl Client {
    /// Builds the client (`no_proxy`, no redirects) after the loopback check.
    pub fn new(instance: Instance) -> Result<Self, CliError> {
        instance::assert_loopback(&instance.api_base)?;
        Ok(Self { instance })
    }

    /// The bearers the instance's token file holds now (empty when it cannot be read).
    pub fn secrets(&self) -> Vec<String> {
        instance::read_tokens(&self.instance).map(|tokens| secrets::loaded(&tokens)).unwrap_or_default()
    }

    /// MCP `tools/call` on `POST /mcp`; `verse_id` selects the token.
    pub async fn call_tool(&self, name: &str, args: Value, verse_id: Option<&str>) -> Outcome {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        let (status, text, _) = match self.send("POST", "/mcp", Some(&request), verse_id, Some(name)).await {
            Ok(reply) => reply,
            Err(outcome) => return outcome,
        };
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 {
            return classify(Some(status), &body, &text);
        }
        if let Some(error) = body.get("error").filter(|error| !error.is_null()) {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("json-rpc error");
            return classify(None, &json!({ "ok": false, "error": message }), message);
        }
        let Some(result) = body.get("result") else {
            return Outcome::failure(ExitCode::ServerFault, "unexpected-reply", "MCP reply had neither result nor error");
        };
        let content = result.get("content").and_then(Value::as_array);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let message = content
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("tool error");
            return classify(None, &json!({ "ok": false, "error": message }), message);
        }
        let data = match content {
            Some(items) if items.len() == 1 && items[0].get("type").and_then(Value::as_str) == Some("text") => {
                let payload = items[0].get("text").and_then(Value::as_str).unwrap_or("");
                Some(serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.to_string())))
            }
            Some(items) => Some(Value::Array(items.clone())),
            None => None,
        };
        Outcome::success(data, Some(status))
    }

    /// REST call under `/api/v1`; `path` starts with `/`.
    pub async fn rest(&self, method: &str, path: &str, body: Option<Value>, verse_id: Option<&str>) -> Outcome {
        if !path.starts_with('/') || path.starts_with("//") || path.contains("://") {
            return Outcome::failure(ExitCode::Internal, "bad-path", "REST path must start with a single '/'");
        }
        let full_path = format!("/api/v1{path}");
        match self.send(method, &full_path, body.as_ref(), verse_id, None).await {
            Ok((status, text, retry_after_ms)) => {
                let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                let mut outcome = classify(Some(status), &parsed, &text);
                if outcome.exit == ExitCode::Retryable && outcome.retry_after_ms.is_none() {
                    outcome.retry_after_ms = retry_after_ms;
                }
                outcome
            }
            Err(outcome) => outcome,
        }
    }

    /// `POST /api/v1/petals/{petal}/extensions/{package}/commands/{command}` with `{"args": ...}`, classified by
    /// status and machine code (`classify_extension`); the ids must already be validated (`ids`).
    pub async fn extension_command(&self, petal_id: &str, package_id: &str, command: &str, args: Value, verse_id: Option<&str>) -> Outcome {
        let path = format!("/api/v1/petals/{petal_id}/extensions/{package_id}/commands/{command}");
        let body = json!({ "args": args });
        match self.send("POST", &path, Some(&body), verse_id, None).await {
            Ok((status, text, retry_after_ms)) => {
                let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                classify_extension(status, &parsed, &text, retry_after_ms)
            }
            Err(outcome) => outcome,
        }
    }

    /// Sends one authenticated request; transport failures come back as ready outcomes. Returns status, body text
    /// and the `Retry-After` header in milliseconds.
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        verse_id: Option<&str>,
        tool: Option<&str>,
    ) -> Result<(u16, String, Option<u64>), Outcome> {
        let tokens = instance::read_tokens(&self.instance).map_err(|error| Outcome::from_error(&error))?;
        let api_base = if tokens.api_base.trim().is_empty() { self.instance.api_base.as_str() } else { tokens.api_base.as_str() };
        instance::assert_loopback(api_base).map_err(|error| Outcome::from_error(&error))?;
        let token = pick_token(&tokens, tool, verse_id).map_err(|error| Outcome::from_error(&error))?;
        // Backstop for every verb: no loaded bearer leaves in a path or body (only in the Authorization header).
        let held = secrets::loaded(&tokens);
        if secrets::text_has_secret(path, &held) || body.is_some_and(|body| secrets::value_has_secret(body, &held)) {
            return Err(Outcome::from_error(&secrets::token_in_args()));
        }

        let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
            .map_err(|_| Outcome::failure(ExitCode::Internal, "bad-method", format!("unsupported HTTP method '{method}'")))?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| Outcome::failure(ExitCode::Internal, "client-build-failed", error.to_string()))?;
        let url = format!("{}{}", api_base.trim().trim_end_matches('/'), path);
        let mut request = http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(transport_failure)?;
        let status = response.status().as_u16();
        let retry_after_ms = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|seconds| seconds.saturating_mul(1000));
        let text = response.text().await.map_err(transport_failure)?;
        Ok((status, text, retry_after_ms))
    }
}

/// Maps a reqwest failure: a refused connection never reached the server (3); anything later is unknown (8).
fn transport_failure(error: reqwest::Error) -> Outcome {
    // Error text can embed the URL but never the bearer header.
    if error.is_builder() {
        Outcome::failure(ExitCode::Internal, "request-build-failed", error.without_url().to_string())
    } else if error.is_connect() {
        Outcome::failure(ExitCode::Transport, "not-running", "cannot connect to the local instance (is it running?)")
    } else if error.is_timeout() {
        Outcome::failure(ExitCode::UnknownOutcome, "unknown-outcome", "request timed out after sending; reconcile by reading")
    } else {
        Outcome::failure(ExitCode::UnknownOutcome, "unknown-outcome", format!("connection failed mid-request: {}", error.without_url()))
    }
}

/// Picks the bearer token: the verse's own token, or the operator/best token when no verse is named.
fn pick_token(tokens: &TokenFile, tool: Option<&str>, verse_id: Option<&str>) -> Result<String, CliError> {
    if let Some(verse_id) = verse_id {
        return tokens.tokens.get(verse_id).cloned().ok_or_else(|| {
            CliError::new(ExitCode::NotFound, "not-found-or-denied", "not found or not permitted")
        });
    }
    let best_verse_token = || {
        tokens
            .tokens
            .iter()
            .max_by_key(|(verse, _)| role_rank(tokens.roles.get(*verse).map(String::as_str)))
            .map(|(_, token)| token.clone())
    };
    let wants_operator = tool.is_some_and(crate::catalog::wants_operator_token);
    let picked = if wants_operator {
        tokens.operator_token.clone().or_else(best_verse_token)
    } else {
        best_verse_token().or_else(|| tokens.operator_token.clone())
    };
    picked.ok_or_else(|| CliError::new(ExitCode::Transport, "no-token", "the token file holds no usable token yet"))
}

/// Owner > manager > editor > viewer > unknown.
pub(crate) fn role_rank(role: Option<&str>) -> u8 {
    match role.map(str::to_ascii_lowercase).as_deref() {
        Some("owner") => 4,
        Some("manager") => 3,
        Some("editor") => 2,
        Some("viewer") => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_real_server_strings_to_the_exit_table() {
        let failure = |status: Option<u16>, message: &str| classify(status, &json!({ "ok": false, "error": message }), message);
        // fe-api's REST error body for the extension command route (`ApiResponse::error`, no machine code yet).
        let extension = |status: u16, message: &str| classify_extension(status, &json!({ "ok": false, "data": null, "error": message }), message, None);
        // (outcome, exit, code, may `--retry` resend it if it was a write)
        let cases: Vec<(Outcome, ExitCode, &str, bool)> = vec![
            (classify(Some(200), &json!({ "ok": true, "data": { "id": "01J" } }), ""), ExitCode::Ok, "ok", false),
            (failure(Some(409), "hook refused the write"), ExitCode::Refused, "refused", false),
            (
                failure(Some(403), "extension administration is not available over the API; use the Extensions panel or fe-ext admin"),
                ExitCode::Refused,
                "admin-gui-only",
                false,
            ),
            (failure(Some(401), ""), ExitCode::Transport, "token-rejected", false),
            (failure(Some(200), "insufficient permissions"), ExitCode::RoleDenied, "role-denied", false),
            (failure(None, "org.example.fleet-sim/spawn needs role Editor"), ExitCode::RoleDenied, "role-denied", false),
            (failure(None, "not found or not permitted"), ExitCode::NotFound, "not-found-or-denied", false),
            (failure(None, "operation failed or not permitted"), ExitCode::NotFound, "not-found-or-denied", false),
            (failure(None, "name is required"), ExitCode::Usage, "invalid-input", false),
            // The MCP door rewrites a write-budget refusal to text: nothing proves it ran before admission.
            (failure(None, "write budget exceeded; retry after 3 s"), ExitCode::Retryable, "write-budget-exceeded", false),
            (failure(Some(504), "unknown-outcome"), ExitCode::UnknownOutcome, "unknown-outcome", false),
            (failure(Some(502), "failed"), ExitCode::ServerFault, "server-fault", false),
            (extension(409, "fleet is parked"), ExitCode::Refused, "refused", false),
            (extension(403, "org.example.fleet-sim/spawn needs role Editor"), ExitCode::RoleDenied, "role-denied", false),
            (extension(403, "org.example.fleet-sim/spawn is not exposed"), ExitCode::Refused, "capability-denied", false),
            (extension(404, "not found or not permitted"), ExitCode::NotFound, "not-found-or-denied", false),
            (extension(404, "no active org.example.fleet-sim at this petal"), ExitCode::Refused, "not-available", false),
            // Every 429 on the command route is a bus refusal before the call is queued: safe to resend.
            (extension(429, "org.example.fleet-sim/spawn is over rate_hz"), ExitCode::Retryable, "rate-limited", true),
            (extension(504, "the command did not answer in time"), ExitCode::UnknownOutcome, "unknown-outcome", false),
            (extension(502, "command result is not JSON"), ExitCode::ServerFault, "failed", false),
            (extension(400, "args must be a JSON object"), ExitCode::Usage, "invalid-input", false),
        ];
        for (outcome, exit, code, write_retry) in cases {
            assert_eq!((outcome.exit, outcome.code.as_str()), (exit, code), "{}", outcome.message);
            assert_eq!(retry_allowed(&outcome, true), write_retry, "write retry: {}", outcome.message);
        }
        // A read refused as retryable may always be resent.
        assert!(retry_allowed(&failure(Some(429), "query rate limit"), false));
        let refused = extension(409, "fleet is parked");
        assert_eq!(refused.refusal.reason.as_deref(), Some("fleet is parked"));
        let retry = failure(None, "write budget exceeded; retry after 3 s");
        assert_eq!(retry.retry_after_ms, Some(3000));
    }
}
