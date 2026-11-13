//! Egress (spec R9): declared outbound HTTP requests and MQTT publishes; the guest names an endpoint, never a host.

use std::sync::Arc;
use std::time::Duration;

use fe_sdk::manifest_v2::{Egress, HttpEgress, Method, Qos};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use tokio::sync::mpsc::error::TrySendError;

use super::address::{literal_refusal, CheckedResolver};
use super::connect::is_loopback;
use super::mqtt::{topic_matches, valid_topic_name};
use super::mqtt_session::MqttPublish;
use super::{
    auth_header, host_rate, host_size, is_credential_header, is_host_owned_header, Delivery, EndpointState, Net, NetState,
    HOST_RATE_CEILING_HZ, MAX_EGRESS_IN_FLIGHT,
};
use crate::bindings::egress::take_egress_token;
use crate::bus::rate::TokenBucket;
use crate::bus::{HostError, InstanceKey};

const DEFAULT_MAX_REQUEST_BYTES: u64 = 1024 * 1024;
const MAX_REQUEST_CEILING: u64 = 8 * 1024 * 1024;
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_RESPONSE_CEILING: u64 = 8 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_REDIRECTS: usize = 5;
const MAX_EGRESS_RATE_HZ: f64 = 10.0;
const MAX_REQUEST_HEADERS: usize = 64;
const MAX_PATH_BYTES: usize = 2_048;
const MAX_QUERY_PAIRS: usize = 64;
const MAX_URL_BYTES: usize = 8_192;
/// Percent-encodings that could decode into a separator, a dot segment, a NUL, another escape (double encoding),
/// a servlet `;` path parameter (`..%3b/` behind a decoding proxy), or an overlong UTF-8 dot or slash (`%c0%ae`).
const REFUSED_ENCODINGS: [&str; 8] = ["%2e", "%2f", "%5c", "%25", "%00", "%3b", "%c0", "%c1"];

/// WIT `egress.outgoing-request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingRequest {
    pub method: Method,
    /// Relative to the endpoint's base url; may not leave it.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// WIT `egress.incoming-response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A validated request ready to run off the worker.
struct HttpJob {
    client: reqwest::Client,
    method: reqwest::Method,
    url: reqwest::Url,
    headers: HeaderMap,
    body: Vec<u8>,
    max_response: u64,
}

/// A refusal plus `(actor, installation)` when it is an audited denial.
type Refused = (HostError, Option<(String, String)>);

/// A refusal that is not audited.
fn plain(error: HostError) -> Refused {
    (error, None)
}

impl Net {
    /// WIT `egress.http-request`: checked now, run off the worker, answered through `on-egress-result`.
    pub fn http_request(&self, key: &InstanceKey, endpoint: &str, request: OutgoingRequest) -> Result<u64, HostError> {
        let (id, generation, job) = self.prepare_http(key, endpoint, request).map_err(|refused| self.refuse_egress(key, endpoint, refused))?;
        let Some(runtime) = self.sources.runtime.clone() else {
            self.finish_egress(key, generation, id, Err(HostError::NotAvailable("this host has no network runtime".into())));
            return Ok(id);
        };
        let net = self.clone();
        let key = key.clone();
        runtime.spawn(async move {
            let outcome = perform(job).await;
            net.finish_egress(&key, generation, id, outcome);
        });
        Ok(id)
    }

    /// Declared endpoint, declared method, contained url, guest headers, size, rate, in-flight, credential, client.
    fn prepare_http(&self, key: &InstanceKey, endpoint: &str, request: OutgoingRequest) -> Result<(u64, u64, HttpJob), Refused> {
        let now = self.now();
        let mut state = self.lock();
        let NetState { instances, next_id, .. } = &mut *state;
        let instance = instances.get_mut(key).ok_or_else(|| plain(HostError::Revoked("instance is not active".into())))?;
        let who = Some((instance.spec.identity_did.clone(), instance.spec.installation_id.clone()));
        let declared = instance.spec.manifest.egress.iter().find_map(|egress| match egress {
            Egress::Http(http) if http.id == endpoint => Some(http.clone()),
            _ => None,
        });
        let Some(declared) = declared else {
            return Err((HostError::CapabilityDenied(format!("egress endpoint {endpoint} is not declared")), who));
        };
        if !declared.methods.contains(&request.method) {
            return Err((HostError::CapabilityDenied(format!("{:?} is not declared for {endpoint}", request.method)), who));
        }
        let url = endpoint_url(&declared.url, &request.path, &request.query).map_err(|error| plain(HostError::InvalidInput(error)))?;
        let mut headers =
            request_headers(&request.headers, request.content_type.as_deref()).map_err(|error| plain(HostError::InvalidInput(error)))?;
        let max_request = declared.max_request_bytes.unwrap_or(DEFAULT_MAX_REQUEST_BYTES).min(MAX_REQUEST_CEILING);
        if request.body.len() as u64 > max_request {
            return Err(plain(HostError::BudgetExceeded(format!("request body is over max_request_bytes ({max_request})"))));
        }
        let rate_hz = host_rate(declared.rate_hz, MAX_EGRESS_RATE_HZ);
        let own = instance.egress_buckets.entry(endpoint.to_string()).or_insert_with(|| TokenBucket::new(rate_hz, now));
        let allowed = take_egress_token(&self.sources.shared_egress, key, endpoint, rate_hz, now, own);
        if !allowed {
            return Err(plain(HostError::RateLimited(format!("{endpoint} is over rate_hz"))));
        }
        if instance.egress_in_flight >= MAX_EGRESS_IN_FLIGHT {
            return Err(plain(HostError::BudgetExceeded(format!("at most {MAX_EGRESS_IN_FLIGHT} requests in flight"))));
        }
        let missing = || plain(HostError::NotAvailable("credential-missing".into()));
        match self.credential(&key.package_id, declared.auth.as_ref()) {
            Ok(None) => {}
            Ok(Some((scheme, credential, header))) => {
                let (name, value) = auth_header(scheme, &credential, header.as_deref()).ok_or_else(missing)?;
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| missing())?;
                let mut value = HeaderValue::from_str(&value).map_err(|_| missing())?;
                value.set_sensitive(true);
                headers.insert(name, value);
            }
            Err(_) => return Err(missing()),
        }
        let client = instance
            .egress_clients
            .get(endpoint)
            .cloned()
            .ok_or_else(|| plain(HostError::NotAvailable(format!("{endpoint} has no client on this host"))))?;
        instance.egress_in_flight += 1;
        instance.counters.egress_requests += 1;
        let id = *next_id;
        *next_id += 1;
        let job = HttpJob {
            client,
            method: reqwest_method(request.method),
            url,
            headers,
            body: request.body,
            max_response: declared.max_response_bytes.unwrap_or(DEFAULT_MAX_RESPONSE_BYTES).min(MAX_RESPONSE_CEILING),
        };
        Ok((id, instance.generation, job))
    }

    /// Count every refusal of a live instance, audit the denials (R9 AC), and return the error.
    fn refuse_egress(&self, key: &InstanceKey, endpoint: &str, (error, who): Refused) -> HostError {
        if let Some(instance) = self.lock().instances.get_mut(key) {
            instance.counters.egress_refused += 1;
        }
        if let (true, Some((actor, installation))) = (error.is_denial(), who) {
            self.audit(self.denial(&actor, Some(installation), format!("{}/egress/{endpoint}", key.package_id), error.code()));
        }
        error
    }

    /// Hand a finished request to the guest's inbox (dropped if the instance stopped meanwhile).
    fn finish_egress(&self, key: &InstanceKey, generation: u64, request: u64, outcome: Result<IncomingResponse, HostError>) {
        self.with_generation(key, generation, |state| {
            if let Some(instance) = state.instances.get_mut(key) {
                instance.egress_in_flight = instance.egress_in_flight.saturating_sub(1);
                if outcome.is_err() {
                    instance.counters.egress_failed += 1;
                }
                instance.push(Delivery::EgressResult { request, outcome });
            }
        });
    }

    /// WIT `egress.mqtt-publish`: a declared broker, a topic matching a declared filter, within size, QoS and rate; queued.
    pub fn mqtt_publish(&self, key: &InstanceKey, endpoint: &str, topic: &str, payload: Vec<u8>, qos: Qos, retain: bool) -> Result<(), HostError> {
        let sender = self.prepare_publish(key, endpoint, topic, payload.len(), qos, retain).map_err(|refused| self.refuse_egress(key, endpoint, refused))?;
        sender.try_send(MqttPublish { topic: topic.to_string(), payload, qos, retain }).map_err(|error| {
            let error = match error {
                TrySendError::Full(_) => HostError::RateLimited(format!("{endpoint}'s publish queue is full")),
                TrySendError::Closed(_) => HostError::NotAvailable(format!("{endpoint} is not connected")),
            };
            self.refuse_egress(key, endpoint, plain(error))
        })?;
        if let Some(instance) = self.lock().instances.get_mut(key) {
            instance.counters.mqtt_published += 1;
        }
        Ok(())
    }

    fn prepare_publish(
        &self,
        key: &InstanceKey,
        endpoint: &str,
        topic: &str,
        payload_len: usize,
        qos: Qos,
        retain: bool,
    ) -> Result<tokio::sync::mpsc::Sender<MqttPublish>, Refused> {
        let now = self.now();
        let mut state = self.lock();
        let instance = state.instances.get_mut(key).ok_or_else(|| plain(HostError::Revoked("instance is not active".into())))?;
        let who = Some((instance.spec.identity_did.clone(), instance.spec.installation_id.clone()));
        let declared = instance.spec.manifest.egress.iter().find_map(|egress| match egress {
            Egress::Mqtt(mqtt) if mqtt.id == endpoint => Some(mqtt.clone()),
            _ => None,
        });
        let Some(declared) = declared else {
            return Err((HostError::CapabilityDenied(format!("egress endpoint {endpoint} is not declared")), who));
        };
        if !valid_topic_name(topic) {
            return Err(plain(HostError::InvalidInput(format!("{topic:?} is not a topic name"))));
        }
        if !declared.topics.iter().any(|filter| topic_matches(filter, topic)) {
            return Err((HostError::CapabilityDenied(format!("{topic} matches no declared topic filter")), who));
        }
        let max_payload = host_size(declared.max_payload_bytes);
        if payload_len > max_payload {
            return Err(plain(HostError::BudgetExceeded(format!("payload is over max_payload_bytes ({max_payload})"))));
        }
        if qos == Qos::AtLeastOnce && declared.qos != Some(Qos::AtLeastOnce) {
            return Err(plain(HostError::InvalidInput("at-least-once was not declared for this endpoint".into())));
        }
        if retain && declared.retain != Some(true) {
            return Err(plain(HostError::InvalidInput("retain was not declared for this endpoint".into())));
        }
        let rate_hz = host_rate(declared.rate_hz, HOST_RATE_CEILING_HZ);
        let own = instance.egress_buckets.entry(endpoint.to_string()).or_insert_with(|| TokenBucket::new(rate_hz, now));
        let allowed = take_egress_token(&self.sources.shared_egress, key, endpoint, rate_hz, now, own);
        if !allowed {
            return Err(plain(HostError::RateLimited(format!("{endpoint} is over rate_hz"))));
        }
        if instance.endpoints.get(endpoint) == Some(&EndpointState::CredentialMissing) {
            return Err(plain(HostError::NotAvailable("credential-missing".into())));
        }
        instance
            .mqtt
            .get(endpoint)
            .and_then(|session| session.outbound.clone())
            .ok_or_else(|| plain(HostError::NotAvailable(format!("{endpoint} has no session on this host"))))
    }
}

/// One reqwest client per HTTP egress endpoint: no proxy, no referer, bounded time, redirects only inside the base,
/// and every address checked by [`CheckedResolver`] (an IP literal is checked here: reqwest never resolves one).
pub(super) fn build_client(http: &HttpEgress) -> Result<reqwest::Client, String> {
    let base = reqwest::Url::parse(&http.url).map_err(|error| format!("bad egress url: {error}"))?;
    let host = base
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase())
        .ok_or_else(|| "egress url has no host".to_string())?;
    let loopback = is_loopback(&host);
    if !(base.scheme() == "https" || (base.scheme() == "http" && loopback)) {
        return Err("egress must be https, or http to loopback".into());
    }
    if !base.username().is_empty() || base.password().is_some() || base.query().is_some() || base.fragment().is_some() {
        return Err("egress urls carry no credentials, query or fragment".into());
    }
    if let Some(reason) = literal_refusal(&host) {
        return Err(reason);
    }
    let timeout = http.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
    let redirect_base = base.clone();
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        if within_base(attempt.url(), &redirect_base) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    reqwest::Client::builder()
        .redirect(policy)
        .referer(false)
        .timeout(Duration::from_millis(timeout))
        .connect_timeout(Duration::from_secs(10))
        .dns_resolver(Arc::new(CheckedResolver))
        // A manifest-named server never negotiates h2 here (RUSTSEC-2026-0258 stays off this path; deny.toml).
        .http1_only()
        .no_proxy()
        .https_only(!loopback)
        .user_agent("FractalEngine-extension-host")
        .build()
        .map_err(|error| format!("egress client: {error}"))
}

/// Same scheme, host and port, no userinfo, a path at or under the base path, and no servlet dot segment (`..;`).
pub(super) fn within_base(url: &reqwest::Url, base: &reqwest::Url) -> bool {
    let prefix = base.path().trim_end_matches('/');
    url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port_or_known_default() == base.port_or_known_default()
        && url.username().is_empty()
        && url.password().is_none()
        && !url.path().split('/').any(|segment| matches!(segment.split(';').next(), Some(".") | Some("..")))
        && (prefix.is_empty() || url.path() == prefix || url.path().starts_with(&format!("{prefix}/")))
}

/// `base` + `path` + `query`; any path that could leave the base (dot segments, encodings, authority) is refused.
pub(super) fn endpoint_url(base: &str, path: &str, query: &[(String, String)]) -> Result<reqwest::Url, String> {
    if path.len() > MAX_PATH_BYTES {
        return Err(format!("path is over {MAX_PATH_BYTES} bytes"));
    }
    if query.len() > MAX_QUERY_PAIRS {
        return Err(format!("at most {MAX_QUERY_PAIRS} query parameters"));
    }
    let lower = path.to_ascii_lowercase();
    // `..;/` is a dot segment to servlet containers, so a segment's part before `;` counts.
    let leaves = path.contains("://")
        || path.starts_with("//")
        || path.contains(['\\', '?', '#', '@'])
        || path.chars().any(|c| c.is_control() || c.is_whitespace())
        || path.split('/').any(|segment| matches!(segment.split(';').next(), Some(".") | Some("..")))
        || REFUSED_ENCODINGS.iter().any(|encoded| lower.contains(encoded));
    if leaves {
        return Err(format!("path {path:?} leaves the endpoint base"));
    }
    let base = reqwest::Url::parse(base).map_err(|error| format!("bad egress url: {error}"))?;
    let mut url = base.clone();
    let relative = path.trim_start_matches('/');
    if !relative.is_empty() {
        let prefix = base.path().trim_end_matches('/');
        url.set_path(&format!("{prefix}/{relative}"));
    }
    url.set_query(None);
    url.set_fragment(None);
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(format!("the url is over {MAX_URL_BYTES} bytes"));
    }
    if !within_base(&url, &base) {
        return Err(format!("path {path:?} leaves the endpoint base"));
    }
    Ok(url)
}

/// Guest headers minus everything the host owns; a credential or host-owned header refuses the whole call.
fn request_headers(headers: &[(String, String)], content_type: Option<&str>) -> Result<HeaderMap, String> {
    if headers.len() > MAX_REQUEST_HEADERS {
        return Err(format!("at most {MAX_REQUEST_HEADERS} headers"));
    }
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.trim().to_ascii_lowercase();
        if is_credential_header(&lower) || is_host_owned_header(&lower) {
            return Err(format!("header {name} is set by the host"));
        }
        let header_name = HeaderName::from_bytes(lower.as_bytes()).map_err(|_| format!("invalid header name {name}"))?;
        let header_value = HeaderValue::from_str(value).map_err(|_| format!("invalid value for header {name}"))?;
        map.append(header_name, header_value);
    }
    if let Some(kind) = content_type {
        map.insert(CONTENT_TYPE, HeaderValue::from_str(kind).map_err(|_| "invalid content type".to_string())?);
    }
    Ok(map)
}

/// Response headers a guest may see: never a credential (`set-cookie`, `authorization`, `x-fe-*`) or a framing header.
pub(super) fn response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| !is_credential_header(name.as_str()) && !is_host_owned_header(name.as_str()))
        .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str().to_string(), value.to_string())))
        .take(MAX_REQUEST_HEADERS)
        .collect()
}

fn reqwest_method(method: Method) -> reqwest::Method {
    match method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    }
}

/// Run one request; a response over the cap is refused, never cut, and a timeout is an unknown outcome.
async fn perform(job: HttpJob) -> Result<IncomingResponse, HostError> {
    let mut response = job
        .client
        .request(job.method, job.url)
        .headers(job.headers)
        .body(job.body)
        .send()
        .await
        .map_err(transport_error)?;
    if response.content_length().is_some_and(|length| length > job.max_response) {
        return Err(HostError::BudgetExceeded("response is over max_response_bytes".into()));
    }
    let status = response.status().as_u16();
    let content_type = response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(str::to_string);
    let headers = response_headers(response.headers());
    let mut body = Vec::new();
    // Counted as the bytes arrive (after any decompression), so the cap holds before buffering more.
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if (body.len() + chunk.len()) as u64 > job.max_response {
            return Err(HostError::BudgetExceeded("response is over max_response_bytes".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(IncomingResponse { status, content_type, headers, body })
}

/// A connect failure never reached the server (`backend`); a timeout after sending may have (`unknown-outcome`).
fn transport_error(error: reqwest::Error) -> HostError {
    if error.is_connect() {
        HostError::Backend(format!("connect failed: {error}"))
    } else if error.is_timeout() {
        HostError::UnknownOutcome(format!("timed out: {error}"))
    } else {
        HostError::Backend(error.to_string())
    }
}
