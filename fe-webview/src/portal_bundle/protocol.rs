//! The `fe-portal` custom-protocol handler (protocol.md §3-4); see `portal_bundle/AGENTS.md` §protocol.

use std::sync::{mpsc, Arc, Mutex, PoisonError};

use fe_format::portal_bundle::is_member_path;
use http::header::{
    HeaderName, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE, RANGE,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderValue, Method, Request, Response, StatusCode};

use super::csp::REFUSAL_POLICY;
use super::mount::{Mount, PortalMounts};
use super::origin;

/// Deny-all Permissions-Policy (protocol.md §4).
const PERMISSIONS_POLICY: &str = "accelerometer=(), camera=(), clipboard-read=(), clipboard-write=(), \
display-capture=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), \
publickey-credentials-get=(), serial=(), usb=(), xr-spatial-tracking=()";
/// Content-Type of every empty error body.
const ERROR_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
/// Host-served paths live under this prefix (protocol.md §3.4).
const RESERVED_PREFIX: &str = "/__fe/";
/// Worker threads reading and verifying members off the Bevy main thread.
const PROTOCOL_WORKERS: usize = 2;

/// Answer one `fe-portal` request (protocol.md §3.2; the first step that answers wins).
pub fn serve(mounts: &PortalMounts, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    // 1. Method.
    let head = request.method() == Method::HEAD;
    if !head && request.method() != Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED, None);
    }
    // 2. Service workers never register on a bundle origin (any Service-Worker header).
    if request.headers().contains_key("service-worker") {
        return empty(StatusCode::FORBIDDEN, None);
    }
    // 3. Origin: the URI exactly as the handler received it (desk check F-3).
    let uri = request.uri().to_string();
    let Some((label, tail)) = origin::split(&uri) else {
        return empty(StatusCode::NOT_FOUND, None);
    };
    // 4. Snapshot: this Arc is the mount for the rest of the request.
    let Some(mount) = mounts.lookup(&label) else {
        return empty(StatusCode::NOT_FOUND, None);
    };
    let policy = Some(mount.csp());
    // 5. Path: never percent-decoded; the member grammar is the traversal defence.
    let path = tail.split(|c: char| c == '?' || c == '#').next().unwrap_or_default();
    let path = if path.is_empty() { "/" } else { path };
    if !path.starts_with('/') || path.contains('%') || path.contains('\\') {
        return empty(StatusCode::NOT_FOUND, policy);
    }
    // 6. Reserved host paths: S2 serves `portal-v1.js` to Admitted mounts, S4 `feeds.json` to Static ones.
    if path.starts_with(RESERVED_PREFIX) || path == "/__fe" {
        return reserved(&mount, path);
    }
    // 7. Member.
    let member = member_for_path(path);
    let Some(mime) = is_member_path(&member)
        .then(|| mount.members.get(&member).copied())
        .flatten()
    else {
        return empty(StatusCode::NOT_FOUND, policy);
    };
    // 8. Read, verified against the seal; a failure is never served, even partly.
    let bytes = match mount.archive.read_member(&member) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return empty(StatusCode::NOT_FOUND, policy),
        Err(error) => {
            tracing::warn!(label = %label, member = %member, "portal bundle member refused: {error:#}");
            mounts.report_fault(&mount);
            return empty(StatusCode::INTERNAL_SERVER_ERROR, policy);
        }
    };
    // 9. Still mounted?
    if !mounts.is_live(mount.mount_id) {
        return empty(StatusCode::GONE, policy);
    }
    // 10-11. Range, then respond.
    let total = bytes.len() as u64;
    let range = request
        .headers()
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .map_or(RangeOutcome::Full, |value| parse_range(value, total));
    match range {
        RangeOutcome::Full => respond(StatusCode::OK, mime, mount.csp(), bytes, head, None),
        RangeOutcome::Partial { start, end } => {
            let content_range = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).ok();
            let slice = bytes
                .get(start as usize..=end as usize)
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            respond(StatusCode::PARTIAL_CONTENT, mime, mount.csp(), slice, head, content_range)
        }
        RangeOutcome::Unsatisfiable => {
            let mut response = empty(StatusCode::RANGE_NOT_SATISFIABLE, policy);
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total}")) {
                response.headers_mut().insert(CONTENT_RANGE, value);
            }
            response
        }
    }
}

/// `/<p>` → `web/<p>`; a trailing `/` (including the root) adds `index.html` (README §3.4).
fn member_for_path(path: &str) -> String {
    let relative = &path[1..];
    if relative.is_empty() || relative.ends_with('/') {
        format!("web/{relative}index.html")
    } else {
        format!("web/{relative}")
    }
}

/// `/__fe/…`: nothing is host-served until S2/S4 add `portal-v1.js` and `feeds.json`.
fn reserved(mount: &Mount, _path: &str) -> Response<Vec<u8>> {
    empty(StatusCode::NOT_FOUND, Some(mount.csp()))
}

/// The outcome of one `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOutcome {
    Full,
    /// Inclusive byte offsets.
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

/// One `bytes=` range → partial; several or malformed → full body; out of bounds → unsatisfiable.
pub fn parse_range(value: &str, total: u64) -> RangeOutcome {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeOutcome::Full;
    };
    if spec.contains(',') {
        return RangeOutcome::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeOutcome::Full;
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        let Ok(suffix) = last.parse::<u64>() else {
            return RangeOutcome::Full;
        };
        if suffix == 0 || total == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        return RangeOutcome::Partial {
            start: total.saturating_sub(suffix),
            end: total - 1,
        };
    }
    let Ok(start) = first.parse::<u64>() else {
        return RangeOutcome::Full;
    };
    if start >= total {
        return RangeOutcome::Unsatisfiable;
    }
    let end = if last.is_empty() {
        total - 1
    } else {
        match last.parse::<u64>() {
            Ok(end) if end >= start => end.min(total - 1),
            _ => return RangeOutcome::Full,
        }
    };
    RangeOutcome::Partial { start, end }
}

/// A served body with every host-authored header (protocol.md §4); `HEAD` keeps the length, drops the body.
fn respond(
    status: StatusCode,
    content_type: &'static str,
    csp: &HeaderValue,
    body: Vec<u8>,
    head: bool,
    content_range: Option<HeaderValue>,
) -> Response<Vec<u8>> {
    let length = body.len() as u64;
    let mut response = Response::new(if head { Vec::new() } else { body });
    *response.status_mut() = status;
    secure_headers(&mut response, content_type, csp, length);
    if let Some(value) = content_range {
        response.headers_mut().insert(CONTENT_RANGE, value);
    }
    response
}

/// An empty error body; `policy` is the mount's CSP when known, else the refusal policy.
fn empty(status: StatusCode, policy: Option<&HeaderValue>) -> Response<Vec<u8>> {
    let refusal = HeaderValue::from_static(REFUSAL_POLICY);
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    secure_headers(&mut response, ERROR_CONTENT_TYPE, policy.unwrap_or(&refusal), 0);
    response
}

fn secure_headers(response: &mut Response<Vec<u8>>, content_type: &'static str, csp: &HeaderValue, length: u64) {
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(CONTENT_SECURITY_POLICY, csp.clone());
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

/// A job for the protocol pool.
pub type ProtocolJob = Box<dyn FnOnce() + Send + 'static>;

/// Small fixed pool that answers protocol requests off the Bevy main thread.
///
/// The threads exit when the pool (owned by the webview's protocol closure) drops.
pub struct ProtocolWorkers {
    sender: mpsc::Sender<ProtocolJob>,
}

impl ProtocolWorkers {
    /// Start the pool; with no thread running, jobs run inline so every request is still answered.
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::channel::<ProtocolJob>();
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..PROTOCOL_WORKERS {
            let receiver = Arc::clone(&receiver);
            let spawned = std::thread::Builder::new()
                .name(format!("fe-portal-protocol-{index}"))
                .spawn(move || loop {
                    let job = {
                        let guard = receiver.lock().unwrap_or_else(PoisonError::into_inner);
                        guard.recv()
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                });
            if let Err(error) = spawned {
                tracing::error!("portal protocol worker {index} failed to start: {error}");
            }
        }
        Self { sender }
    }

    /// Queue `job`; runs it inline when no worker is alive.
    pub fn submit(&self, job: ProtocolJob) {
        if let Err(mpsc::SendError(job)) = self.sender.send(job) {
            job();
        }
    }
}
