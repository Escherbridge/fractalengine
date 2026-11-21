//! Instance discovery and loopback token file (S2).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope::{rfc3339_to_unix, unix_millis_now, CliError, ExitCode};
use crate::Env;

/// Dev-token document written by the engine (`fe_api::dev_token`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenFile {
    pub api_base: String,
    /// verse_id -> JWT.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// verse_id -> role.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub operator_token: Option<String>,
}

/// A resolved local instance.
#[derive(Debug, Clone)]
pub struct Instance {
    pub name: String,
    pub api_base: String,
    pub tokens_path: PathBuf,
    /// Admin-channel instance name, if recorded.
    pub admin_instance: Option<String>,
}

/// Resolves `--instance`, `FE_INSTANCE`, `.qa/peers-current`, `.qa/current` in that order.
pub fn resolve(selector: Option<&str>, env: &Env, working_dir: &Path) -> Result<Instance, CliError> {
    let selector = selector
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| env.get("FE_INSTANCE").map(|text| text.trim().to_string()).filter(|text| !text.is_empty()));
    let Some(selector) = selector else {
        return list(env, working_dir)?.into_iter().next().ok_or_else(|| {
            CliError::new(
                ExitCode::Transport,
                "no-instance",
                "no running instance found (set FE_INSTANCE, pass --instance, or start one under .qa/)",
            )
        });
    };

    let looks_like_path = selector.contains('/') || selector.contains('\\') || selector.ends_with(".json");
    if !looks_like_path {
        if let Some(found) = list(env, working_dir)?.into_iter().find(|instance| instance.name == selector) {
            return Ok(found);
        }
    }
    let candidate = {
        let path = PathBuf::from(&selector);
        if path.is_absolute() { path } else { working_dir.join(path) }
    };
    if candidate.is_file() {
        return instance_from_tokens_file(&candidate);
    }
    if candidate.is_dir() {
        let current = candidate.join("current.json");
        if current.is_file() {
            return instance_from_current_json(&current, dir_name(&candidate));
        }
        let tokens = candidate.join("tokens.json");
        if tokens.is_file() {
            return instance_from_tokens_file(&tokens);
        }
    }
    Err(CliError::new(
        ExitCode::Usage,
        "unknown-instance",
        format!("no instance matches '{selector}'; try `fe instance list`"),
    ))
}

/// Lists every discoverable instance (for `fe instance list`).
pub fn list(env: &Env, working_dir: &Path) -> Result<Vec<Instance>, CliError> {
    let _ = env;
    let mut found: Vec<Instance> = Vec::new();
    let Some(qa_dir) = find_qa_dir(working_dir) else {
        return Ok(found);
    };
    let base_dir = qa_dir.parent().unwrap_or(&qa_dir).to_path_buf();

    if let Some(owner_dir) = read_pointer(&qa_dir.join("peers-current"), &base_dir) {
        if let Some(peers) = read_json(&owner_dir.join("peers.json")) {
            for member in peers.get("members").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = member.get("name").and_then(Value::as_str) else { continue };
                let Some(api_base) = member.get("api_base").and_then(Value::as_str) else { continue };
                let run_dir = member.get("run_dir").and_then(Value::as_str).map(PathBuf::from);
                let tokens_path = member
                    .get("tokens_path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .or_else(|| run_dir.map(|dir| dir.join("tokens.json")));
                let Some(tokens_path) = tokens_path else { continue };
                found.push(Instance {
                    name: name.to_string(),
                    api_base: api_base.to_string(),
                    tokens_path,
                    admin_instance: member.get("admin_instance").and_then(Value::as_str).map(str::to_string),
                });
            }
        }
    }

    if let Some(run_dir) = read_pointer(&qa_dir.join("current"), &base_dir) {
        let current = run_dir.join("current.json");
        if let Ok(instance) = instance_from_current_json(&current, "default".to_string()) {
            if !found.iter().any(|existing| existing.api_base == instance.api_base) {
                found.push(instance);
            }
        }
    }
    Ok(found)
}

/// Unix: refuses a token file readable or writable by group/other (exit 3). Windows trusts the engine-written DACL.
pub fn check_token_file_private(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(CliError::new(
                    ExitCode::Transport,
                    "token-file-permissions",
                    format!("token file {} is accessible to group/other (mode {:o}); run chmod 600 on it", path.display(), mode & 0o777),
                ));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Re-reads the token file (called on every request).
pub fn read_tokens(instance: &Instance) -> Result<TokenFile, CliError> {
    let path = &instance.tokens_path;
    check_token_file_private(path)?;
    // The engine replaces the file atomically, but a Windows rename can briefly refuse readers.
    let mut last_error = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<TokenFile>(text.trim_start_matches('\u{feff}')) {
                Ok(tokens) => return check_not_expired(tokens),
                Err(error) => {
                    last_error = Some(CliError::new(
                        ExitCode::Transport,
                        "token-file-invalid",
                        format!("token file {} is not a token document: {error}", path.display()),
                    ));
                }
            },
            Err(error) => {
                last_error = Some(CliError::new(
                    ExitCode::Transport,
                    "token-file-missing",
                    format!("cannot read token file {}: {}", path.display(), error.kind()),
                ));
            }
        }
    }
    Err(last_error.expect("loop ran at least once"))
}

/// Refuses anything but an IP-literal loopback `http://` base (exit 3).
pub fn assert_loopback(api_base: &str) -> Result<(), CliError> {
    match loopback_address(api_base) {
        Ok(_) => Ok(()),
        Err(reason) => Err(CliError::new(
            ExitCode::Transport,
            "not-loopback",
            format!("api_base must be http:// plus an IP-literal loopback address (127.0.0.0/8 or ::1): {reason}"),
        )),
    }
}

/// Parses `http://<ip>[:port]` and returns the address only when it is loopback.
fn loopback_address(api_base: &str) -> Result<IpAddr, &'static str> {
    let base = api_base.trim();
    if base.chars().any(|character| character.is_whitespace() || character.is_control() || matches!(character, '@' | '\\' | '%')) {
        return Err("illegal character");
    }
    let scheme_ok = base.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"));
    if !scheme_ok {
        return Err("scheme must be http://");
    }
    let authority = base[7..].trim_end_matches('/');
    if authority.is_empty() || authority.contains(['/', '?', '#']) {
        return Err("expected host[:port] with no path");
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']').ok_or("unterminated [ipv6]")?;
        let port = match tail {
            "" => None,
            _ => Some(tail.strip_prefix(':').ok_or("bad port separator")?),
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if let Some(port) = port {
        if port.parse::<u16>().is_err() {
            return Err("invalid port");
        }
    }
    let address: IpAddr = if authority.starts_with('[') {
        IpAddr::V6(host.parse::<Ipv6Addr>().map_err(|_| "not an IP literal")?)
    } else {
        IpAddr::V4(host.parse::<Ipv4Addr>().map_err(|_| "not an IPv4 literal (hostnames such as localhost are refused)")?)
    };
    if address.is_loopback() {
        Ok(address)
    } else {
        Err("address is not loopback")
    }
}

fn check_not_expired(tokens: TokenFile) -> Result<TokenFile, CliError> {
    if let Some(expiry) = tokens.expires_at.as_deref().and_then(rfc3339_to_unix) {
        if expiry <= (unix_millis_now() / 1000) as i64 {
            return Err(CliError::new(
                ExitCode::Transport,
                "token-expired",
                "the dev token file has expired; the engine rewrites it while running",
            ));
        }
    }
    Ok(tokens)
}

/// Nearest `.qa` directory at or above `working_dir`.
fn find_qa_dir(working_dir: &Path) -> Option<PathBuf> {
    working_dir
        .ancestors()
        .map(|ancestor| ancestor.join(".qa"))
        .find(|candidate| candidate.is_dir())
}

/// Reads a one-line directory pointer file (`.qa/current`, `.qa/peers-current`).
fn read_pointer(pointer: &Path, base_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(pointer).ok()?;
    let line = text.trim_start_matches('\u{feff}').trim();
    if line.is_empty() {
        return None;
    }
    let path = PathBuf::from(line);
    Some(if path.is_absolute() { path } else { base_dir.join(path) })
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn dir_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "default".to_string())
}

fn instance_from_tokens_file(path: &Path) -> Result<Instance, CliError> {
    check_token_file_private(path)?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        CliError::new(ExitCode::Transport, "token-file-missing", format!("cannot read {}: {}", path.display(), error.kind()))
    })?;
    let document: TokenFile = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|error| {
        CliError::new(ExitCode::Transport, "token-file-invalid", format!("{} is not a token document: {error}", path.display()))
    })?;
    let name = path.parent().map(dir_name).unwrap_or_else(|| "default".to_string());
    Ok(Instance { name, api_base: document.api_base, tokens_path: path.to_path_buf(), admin_instance: None })
}

fn instance_from_current_json(path: &Path, name: String) -> Result<Instance, CliError> {
    let unreadable = |detail: &str| {
        CliError::new(ExitCode::Transport, "instance-record-invalid", format!("{}: {detail}", path.display()))
    };
    let document = read_json(path).ok_or_else(|| unreadable("unreadable or not JSON"))?;
    let api_base = document.get("api_base").and_then(Value::as_str).ok_or_else(|| unreadable("no api_base"))?;
    let run_dir = document.get("run_dir").and_then(Value::as_str).map(PathBuf::from);
    let tokens_path = document
        .get("tokens_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| run_dir.map(|dir| dir.join("tokens.json")))
        .ok_or_else(|| unreadable("no tokens_path"))?;
    Ok(Instance {
        name,
        api_base: api_base.to_string(),
        tokens_path,
        admin_instance: document.get("admin_instance").and_then(Value::as_str).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_predicate_accepts_only_ip_literal_loopback_http() {
        let cases: &[(&str, bool)] = &[
            ("http://127.0.0.1:8765", true),
            ("http://127.9.9.9:80", true),
            ("http://[::1]:8882", true),
            ("http://127.0.0.1", true),
            ("http://localhost:8765", false),
            ("http://0.0.0.0:8765", false),
            ("http://10.0.0.1:8765", false),
            ("http://example.com:8765", false),
            ("https://127.0.0.1:8765", false),
            ("http://127.0.0.1@evil.example:80", false),
            ("http://127.0.0.1:8765/api", false),
            ("http://[::ffff:127.0.0.1]:80", false),
        ];
        for (base, accepted) in cases {
            assert_eq!(assert_loopback(base).is_ok(), *accepted, "{base}");
        }
    }
}
