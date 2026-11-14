//! Embedded members-only iroh relay (FR-1/FR-3, feature `relay-server`); see `AGENTS.md` §peer-relay-server.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iroh::EndpointId;
use iroh_relay::server::{self as relay, Access, AccessControl, ClientRequest, ConnectionId};

use crate::sharding::RelayAdmissionSnapshot;

/// Opt-in switch; only the exact value `1` enables the relay.
pub const RELAY_SERVE_ENV: &str = "FE_RELAY_SERVE";
/// `acme` | `manual`; unset refuses to start (never plaintext).
pub const RELAY_SERVE_CERT_MODE_ENV: &str = "FE_RELAY_SERVE_CERT_MODE";
/// Public hostname for ACME.
pub const RELAY_SERVE_HOSTNAME_ENV: &str = "FE_RELAY_SERVE_HOSTNAME";
/// ACME account contact (an e-mail address).
pub const RELAY_SERVE_ACME_CONTACT_ENV: &str = "FE_RELAY_SERVE_ACME_CONTACT";
/// `1` uses the Let's Encrypt staging directory.
pub const RELAY_SERVE_ACME_STAGING_ENV: &str = "FE_RELAY_SERVE_ACME_STAGING";
/// PEM certificate chain for manual mode (reloaded daily).
pub const RELAY_SERVE_CERT_PATH_ENV: &str = "FE_RELAY_SERVE_CERT_PATH";
/// PEM private key for manual mode.
pub const RELAY_SERVE_KEY_PATH_ENV: &str = "FE_RELAY_SERVE_KEY_PATH";
/// HTTPS listener address.
pub const RELAY_SERVE_HTTPS_BIND_ENV: &str = "FE_RELAY_SERVE_HTTPS_BIND";
/// Plain HTTP listener address (captive-portal probe only).
pub const RELAY_SERVE_HTTP_BIND_ENV: &str = "FE_RELAY_SERVE_HTTP_BIND";
/// Comma-separated served verse ids, or `hosted`.
pub const RELAY_SERVE_VERSES_ENV: &str = "FE_RELAY_SERVE_VERSES";
/// Global live-connection cap enforced by the gate.
pub const RELAY_SERVE_MAX_CLIENTS_ENV: &str = "FE_RELAY_SERVE_MAX_CLIENTS";
/// Per-endpoint live-connection cap enforced by the gate.
pub const RELAY_SERVE_MAX_CONN_PER_ENDPOINT_ENV: &str = "FE_RELAY_SERVE_MAX_CONN_PER_ENDPOINT";
/// Per-client receive rate (bytes per second).
pub const RELAY_SERVE_CLIENT_RX_BPS_ENV: &str = "FE_RELAY_SERVE_CLIENT_RX_BPS";
/// Per-client receive burst (bytes).
pub const RELAY_SERVE_CLIENT_RX_BURST_ENV: &str = "FE_RELAY_SERVE_CLIENT_RX_BURST";
/// `1` | `0`: serve QUIC address discovery (it has no access control).
pub const RELAY_SERVE_QAD_ENV: &str = "FE_RELAY_SERVE_QAD";
/// QUIC address-discovery listener address.
pub const RELAY_SERVE_QAD_BIND_ENV: &str = "FE_RELAY_SERVE_QAD_BIND";
/// Metrics listener; must be loopback.
pub const RELAY_SERVE_METRICS_BIND_ENV: &str = "FE_RELAY_SERVE_METRICS_BIND";
/// `1` adds the reverse proxy's `X-Forwarded-For` to abuse events.
pub const RELAY_SERVE_LOG_IPS_ENV: &str = "FE_RELAY_SERVE_LOG_IPS";
/// Access policy; only `members` (or unset) is accepted, anything open refuses to start.
pub const RELAY_SERVE_ACCESS_ENV: &str = "FE_RELAY_SERVE_ACCESS";

/// Reconciler period for convergent revocation (OD-12).
pub const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);
/// Eviction target after a revoke is materialized (OD-12).
pub const EVICTION_TARGET: Duration = Duration::from_secs(5);
/// One abuse event per endpoint and kind per this window.
pub const ABUSE_LOG_WINDOW: Duration = Duration::from_secs(60);
/// Bound on the abuse-log rate-limit table.
const ABUSE_LOG_CAPACITY: usize = 4096;
/// Denials per minute that make a minute "hot" for the breaker.
pub const BREAKER_DENIALS_PER_MINUTE: u32 = 600;
/// Consecutive hot minutes that trip the breaker.
pub const BREAKER_HOT_MINUTES: u32 = 3;
/// Desktop listener cool-down after a trip.
pub const DESKTOP_BREAKER_COOLDOWN: Duration = Duration::from_secs(600);
const BREAKER_MINUTE: Duration = Duration::from_secs(60);

/// Which defaults apply (spec §4.1 table: operator relay vs desktop peer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayServeProfile {
    /// fe-relay: 1024 clients, 4 per endpoint, 2 MiB/s, QAD on, breaker warns only.
    Operator,
    /// Desktop peer: 64 clients, 2 per endpoint, 512 KiB/s, QAD off, breaker closes the listener.
    Desktop,
}

impl RelayServeProfile {
    fn default_max_clients(self) -> usize {
        match self {
            Self::Operator => 1024,
            Self::Desktop => 64,
        }
    }

    fn default_max_connections_per_endpoint(self) -> usize {
        match self {
            Self::Operator => 4,
            Self::Desktop => 2,
        }
    }

    fn default_client_rx_bytes_per_second(self) -> u32 {
        match self {
            Self::Operator => 2 * 1024 * 1024,
            Self::Desktop => 512 * 1024,
        }
    }

    fn default_client_rx_burst_bytes(self) -> u32 {
        match self {
            Self::Operator => 4 * 1024 * 1024,
            Self::Desktop => 1024 * 1024,
        }
    }

    fn default_qad(self) -> bool {
        matches!(self, Self::Operator)
    }

    fn breaker_policy(self) -> BreakerPolicy {
        match self {
            Self::Operator => BreakerPolicy::WarnOnly,
            Self::Desktop => BreakerPolicy::CloseListener {
                cooldown: DESKTOP_BREAKER_COOLDOWN,
            },
        }
    }
}

/// TLS certificate source; there is no plaintext variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertMode {
    /// Let's Encrypt via ACME (needs inbound TCP 443).
    Acme {
        hostname: String,
        contact: String,
        staging: bool,
    },
    /// Operator-supplied PEM files, reloaded on an interval.
    Manual { cert_path: PathBuf, key_path: PathBuf },
}

/// The verses whose explicit Viewer+ members the relay admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServedVerses {
    /// An explicit verse-id list; every id must be hosted locally.
    Listed(BTreeSet<String>),
    /// Every verse this node hosts, now and later.
    AllHosted,
}

impl ServedVerses {
    fn serves(&self, verse_id: &str) -> bool {
        match self {
            Self::Listed(verses) => verses.contains(verse_id),
            Self::AllHosted => true,
        }
    }
}

/// Live-connection caps the gate enforces (iroh's own accept caps are unimplemented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateLimits {
    pub max_clients: usize,
    pub max_connections_per_endpoint: usize,
}

/// Validated embedded-relay settings read once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayServeConfig {
    pub profile: RelayServeProfile,
    pub cert_mode: CertMode,
    pub https_bind: SocketAddr,
    pub http_bind: SocketAddr,
    pub served_verses: ServedVerses,
    /// Verses this node hosts; set by [`RelayServeConfig::with_hosted_verses`], required by spawn.
    pub hosted_verses: Option<BTreeSet<String>>,
    pub limits: GateLimits,
    pub client_rx_bytes_per_second: NonZeroU32,
    pub client_rx_burst_bytes: NonZeroU32,
    pub qad_bind: Option<SocketAddr>,
    pub metrics_bind: Option<SocketAddr>,
    pub log_client_ips: bool,
}

/// Named refuse-to-start reasons (AC-2) plus spawn failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayServeError {
    NoCertMode,
    UnknownCertMode { raw: String },
    MissingAcmeHostname,
    MissingAcmeContact,
    MissingCertPath,
    MissingKeyPath,
    EmptyVerseSet,
    UnhostedVerse { verse_id: String },
    HostedVersesUnknown,
    OpenAccessRefused { raw: String },
    InvalidValue { variable: &'static str, raw: String },
    MetricsNotLoopback { address: SocketAddr },
    Tls { reason: String },
    Spawn { reason: String },
}

impl RelayServeError {
    /// Stable snake_case name for logs and tests.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NoCertMode => "no_cert_mode",
            Self::UnknownCertMode { .. } => "unknown_cert_mode",
            Self::MissingAcmeHostname => "missing_acme_hostname",
            Self::MissingAcmeContact => "missing_acme_contact",
            Self::MissingCertPath => "missing_cert_path",
            Self::MissingKeyPath => "missing_key_path",
            Self::EmptyVerseSet => "empty_verse_set",
            Self::UnhostedVerse { .. } => "unhosted_verse",
            Self::HostedVersesUnknown => "hosted_verses_unknown",
            Self::OpenAccessRefused { .. } => "open_access_refused",
            Self::InvalidValue { .. } => "invalid_value",
            Self::MetricsNotLoopback { .. } => "metrics_not_loopback",
            Self::Tls { .. } => "tls",
            Self::Spawn { .. } => "spawn",
        }
    }
}

impl std::fmt::Display for RelayServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name();
        match self {
            Self::NoCertMode => write!(
                f,
                "{name}: {RELAY_SERVE_CERT_MODE_ENV} must be `acme` or `manual`; the relay never serves plaintext"
            ),
            Self::UnknownCertMode { raw } => {
                write!(f, "{name}: {RELAY_SERVE_CERT_MODE_ENV}={raw:?} is not `acme` or `manual`")
            }
            Self::MissingAcmeHostname => write!(f, "{name}: ACME needs {RELAY_SERVE_HOSTNAME_ENV}"),
            Self::MissingAcmeContact => write!(f, "{name}: ACME needs {RELAY_SERVE_ACME_CONTACT_ENV}"),
            Self::MissingCertPath => write!(f, "{name}: manual mode needs {RELAY_SERVE_CERT_PATH_ENV}"),
            Self::MissingKeyPath => write!(f, "{name}: manual mode needs {RELAY_SERVE_KEY_PATH_ENV}"),
            Self::EmptyVerseSet => write!(f, "{name}: {RELAY_SERVE_VERSES_ENV} names no served verse"),
            Self::UnhostedVerse { verse_id } => write!(
                f,
                "{name}: verse {verse_id} in {RELAY_SERVE_VERSES_ENV} is not hosted on this node"
            ),
            Self::HostedVersesUnknown => write!(
                f,
                "{name}: the hosted verse set was never supplied (call with_hosted_verses before spawn)"
            ),
            Self::OpenAccessRefused { raw } => write!(
                f,
                "{name}: {RELAY_SERVE_ACCESS_ENV}={raw:?} would admit non-members; only `members` is allowed"
            ),
            Self::InvalidValue { variable, raw } => write!(f, "{name}: {variable}={raw:?} is not valid"),
            Self::MetricsNotLoopback { address } => write!(
                f,
                "{name}: {RELAY_SERVE_METRICS_BIND_ENV}={address} must be a loopback address"
            ),
            Self::Tls { reason } => write!(f, "{name}: {reason}"),
            Self::Spawn { reason } => write!(f, "{name}: {reason}"),
        }
    }
}

impl std::error::Error for RelayServeError {}

type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn trimmed(lookup: EnvLookup<'_>, variable: &str) -> Option<String> {
    lookup(variable)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_socket(
    lookup: EnvLookup<'_>,
    variable: &'static str,
    default: SocketAddr,
) -> Result<SocketAddr, RelayServeError> {
    match trimmed(lookup, variable) {
        None => Ok(default),
        Some(raw) => raw
            .parse()
            .map_err(|_| RelayServeError::InvalidValue { variable, raw }),
    }
}

fn parse_positive(
    lookup: EnvLookup<'_>,
    variable: &'static str,
    default: usize,
) -> Result<usize, RelayServeError> {
    match trimmed(lookup, variable) {
        None => Ok(default),
        Some(raw) => match raw.parse::<usize>() {
            Ok(value) if value > 0 => Ok(value),
            _ => Err(RelayServeError::InvalidValue { variable, raw }),
        },
    }
}

fn parse_non_zero_u32(
    lookup: EnvLookup<'_>,
    variable: &'static str,
    default: u32,
) -> Result<NonZeroU32, RelayServeError> {
    let raw = trimmed(lookup, variable).unwrap_or_else(|| default.to_string());
    raw.parse::<NonZeroU32>()
        .map_err(|_| RelayServeError::InvalidValue { variable, raw })
}

fn parse_switch(
    lookup: EnvLookup<'_>,
    variable: &'static str,
    default: bool,
) -> Result<bool, RelayServeError> {
    match trimmed(lookup, variable).as_deref() {
        None => Ok(default),
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(raw) => Err(RelayServeError::InvalidValue {
            variable,
            raw: raw.to_string(),
        }),
    }
}

impl RelayServeConfig {
    /// Operator-profile config from the process env; `Ok(None)` unless `FE_RELAY_SERVE=1`.
    pub fn from_env() -> Result<Option<Self>, RelayServeError> {
        Self::from_lookup(RelayServeProfile::Operator, &|key: &str| std::env::var(key).ok())
    }

    /// Desktop-profile config from the process env (NET-30 glue).
    pub fn from_env_desktop() -> Result<Option<Self>, RelayServeError> {
        Self::from_lookup(RelayServeProfile::Desktop, &|key: &str| std::env::var(key).ok())
    }

    /// Pure core of [`Self::from_env`]: parses and applies every refuse-to-start rule that needs no DB.
    pub fn from_lookup(
        profile: RelayServeProfile,
        lookup: EnvLookup<'_>,
    ) -> Result<Option<Self>, RelayServeError> {
        match lookup(RELAY_SERVE_ENV).as_deref() {
            Some("1") => {}
            None | Some("") | Some("0") => return Ok(None),
            Some(other) => {
                tracing::warn!(
                    "{RELAY_SERVE_ENV}={other:?} ignored (only `1` enables); no relay listener is bound"
                );
                return Ok(None);
            }
        }

        if let Some(raw) = trimmed(lookup, RELAY_SERVE_ACCESS_ENV) {
            if !raw.eq_ignore_ascii_case("members") {
                return Err(RelayServeError::OpenAccessRefused { raw });
            }
        }

        let cert_mode = match trimmed(lookup, RELAY_SERVE_CERT_MODE_ENV) {
            None => return Err(RelayServeError::NoCertMode),
            Some(raw) => match raw.to_ascii_lowercase().as_str() {
                "acme" => CertMode::Acme {
                    hostname: trimmed(lookup, RELAY_SERVE_HOSTNAME_ENV)
                        .ok_or(RelayServeError::MissingAcmeHostname)?,
                    contact: trimmed(lookup, RELAY_SERVE_ACME_CONTACT_ENV)
                        .ok_or(RelayServeError::MissingAcmeContact)?,
                    staging: parse_switch(lookup, RELAY_SERVE_ACME_STAGING_ENV, false)?,
                },
                "manual" => CertMode::Manual {
                    cert_path: trimmed(lookup, RELAY_SERVE_CERT_PATH_ENV)
                        .map(PathBuf::from)
                        .ok_or(RelayServeError::MissingCertPath)?,
                    key_path: trimmed(lookup, RELAY_SERVE_KEY_PATH_ENV)
                        .map(PathBuf::from)
                        .ok_or(RelayServeError::MissingKeyPath)?,
                },
                _ => return Err(RelayServeError::UnknownCertMode { raw }),
            },
        };

        let served_verses = match trimmed(lookup, RELAY_SERVE_VERSES_ENV) {
            None => return Err(RelayServeError::EmptyVerseSet),
            Some(raw) if raw.eq_ignore_ascii_case("hosted") => ServedVerses::AllHosted,
            Some(raw) => {
                let verses: BTreeSet<String> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|verse| !verse.is_empty())
                    .map(str::to_string)
                    .collect();
                if verses.is_empty() {
                    return Err(RelayServeError::EmptyVerseSet);
                }
                ServedVerses::Listed(verses)
            }
        };

        let metrics_bind = match trimmed(lookup, RELAY_SERVE_METRICS_BIND_ENV) {
            None => None,
            Some(raw) => {
                let address: SocketAddr =
                    raw.parse().map_err(|_| RelayServeError::InvalidValue {
                        variable: RELAY_SERVE_METRICS_BIND_ENV,
                        raw,
                    })?;
                if !address.ip().is_loopback() {
                    return Err(RelayServeError::MetricsNotLoopback { address });
                }
                Some(address)
            }
        };

        let qad = parse_switch(lookup, RELAY_SERVE_QAD_ENV, profile.default_qad())?;
        let qad_default: SocketAddr = (
            std::net::Ipv4Addr::UNSPECIFIED,
            iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT,
        )
            .into();
        let qad_bind = if qad {
            Some(parse_socket(lookup, RELAY_SERVE_QAD_BIND_ENV, qad_default)?)
        } else {
            None
        };

        Ok(Some(Self {
            profile,
            cert_mode,
            https_bind: parse_socket(lookup, RELAY_SERVE_HTTPS_BIND_ENV, ([0, 0, 0, 0], 443).into())?,
            http_bind: parse_socket(lookup, RELAY_SERVE_HTTP_BIND_ENV, ([0, 0, 0, 0], 80).into())?,
            served_verses,
            hosted_verses: None,
            limits: GateLimits {
                max_clients: parse_positive(
                    lookup,
                    RELAY_SERVE_MAX_CLIENTS_ENV,
                    profile.default_max_clients(),
                )?,
                max_connections_per_endpoint: parse_positive(
                    lookup,
                    RELAY_SERVE_MAX_CONN_PER_ENDPOINT_ENV,
                    profile.default_max_connections_per_endpoint(),
                )?,
            },
            client_rx_bytes_per_second: parse_non_zero_u32(
                lookup,
                RELAY_SERVE_CLIENT_RX_BPS_ENV,
                profile.default_client_rx_bytes_per_second(),
            )?,
            client_rx_burst_bytes: parse_non_zero_u32(
                lookup,
                RELAY_SERVE_CLIENT_RX_BURST_ENV,
                profile.default_client_rx_burst_bytes(),
            )?,
            qad_bind,
            metrics_bind,
            log_client_ips: trimmed(lookup, RELAY_SERVE_LOG_IPS_ENV).as_deref() == Some("1"),
        }))
    }

    /// Binds the node's hosted verses; a listed verse it does not host refuses to start (F0-13).
    pub fn with_hosted_verses(mut self, hosted: BTreeSet<String>) -> Result<Self, RelayServeError> {
        match &self.served_verses {
            ServedVerses::Listed(verses) => {
                if let Some(verse_id) = verses.iter().find(|verse| !hosted.contains(*verse)) {
                    return Err(RelayServeError::UnhostedVerse {
                        verse_id: verse_id.clone(),
                    });
                }
            }
            ServedVerses::AllHosted => {
                if hosted.is_empty() {
                    return Err(RelayServeError::EmptyVerseSet);
                }
                tracing::warn!(
                    hosted = hosted.len(),
                    "{RELAY_SERVE_VERSES_ENV}=hosted also serves every verse this node hosts later"
                );
            }
        }
        self.hosted_verses = Some(hosted);
        Ok(self)
    }

    /// Final refuse-to-start check run by [`EmbeddedRelay::spawn`].
    pub fn validate(&self) -> Result<(), RelayServeError> {
        let Some(hosted) = &self.hosted_verses else {
            return Err(RelayServeError::HostedVersesUnknown);
        };
        match &self.served_verses {
            ServedVerses::Listed(verses) if verses.is_empty() => Err(RelayServeError::EmptyVerseSet),
            ServedVerses::Listed(verses) => match verses.iter().find(|verse| !hosted.contains(*verse)) {
                Some(verse_id) => Err(RelayServeError::UnhostedVerse {
                    verse_id: verse_id.clone(),
                }),
                None => Ok(()),
            },
            ServedVerses::AllHosted if hosted.is_empty() => Err(RelayServeError::EmptyVerseSet),
            ServedVerses::AllHosted => Ok(()),
        }
    }
}

/// Gate denial reasons; the string is sent to the client and logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    NotMember,
    PerEndpointCap,
    GlobalCap,
}

impl DenyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotMember => "not_member",
            Self::PerEndpointCap => "per_endpoint_cap",
            Self::GlobalCap => "global_cap",
        }
    }
}

/// Abuse-log event kinds (spec §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AbuseEvent {
    Denied,
    CapHit,
    RevokedDisconnect,
}

impl AbuseEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "relay.denied",
            Self::CapHit => "relay.cap_hit",
            Self::RevokedDisconnect => "relay.revoked_disconnect",
        }
    }
}

/// Breaker edge reported to the reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerTransition {
    Unchanged,
    Tripped,
    Cleared,
}

/// Denial circuit breaker: trips after `BREAKER_HOT_MINUTES` consecutive minutes above the rate.
#[derive(Debug)]
struct DenialBreaker {
    minute_start: Instant,
    denials_this_minute: u32,
    hot_minutes: u32,
    tripped: bool,
}

impl DenialBreaker {
    fn new(now: Instant) -> Self {
        Self {
            minute_start: now,
            denials_this_minute: 0,
            hot_minutes: 0,
            tripped: false,
        }
    }

    fn roll(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.minute_start);
        if elapsed < BREAKER_MINUTE {
            return;
        }
        let whole_minutes = (elapsed.as_secs() / BREAKER_MINUTE.as_secs()) as u32;
        // Idle minutes after the counted one are cool, so only a single elapsed hot minute extends the streak.
        if whole_minutes == 1 && self.denials_this_minute > BREAKER_DENIALS_PER_MINUTE {
            self.hot_minutes += 1;
        } else {
            self.hot_minutes = 0;
        }
        self.denials_this_minute = 0;
        self.minute_start += BREAKER_MINUTE * whole_minutes;
    }

    fn record_denial(&mut self, now: Instant) {
        self.roll(now);
        self.denials_this_minute = self.denials_this_minute.saturating_add(1);
    }

    fn poll(&mut self, now: Instant) -> BreakerTransition {
        self.roll(now);
        let hot = self.hot_minutes >= BREAKER_HOT_MINUTES;
        match (self.tripped, hot) {
            (false, true) => {
                self.tripped = true;
                BreakerTransition::Tripped
            }
            (true, false) => {
                self.tripped = false;
                BreakerTransition::Cleared
            }
            _ => BreakerTransition::Unchanged,
        }
    }

    fn reset(&mut self, now: Instant) {
        *self = Self::new(now);
    }
}

/// One admitted connection, stamped with the snapshot generation it was admitted under.
#[derive(Debug, Clone, Copy)]
struct TrackedConnection {
    connection_id: ConnectionId,
    admitted_generation: u64,
}

#[derive(Debug)]
struct EndpointConnections {
    did: String,
    connections: Vec<TrackedConnection>,
}

#[derive(Debug, Clone, Copy)]
struct PendingEviction {
    since: Instant,
    overdue_warned: bool,
}

#[derive(Debug)]
struct GateState {
    generation: u64,
    admitted: HashSet<String>,
    revoked: HashMap<String, u64>,
    connections: HashMap<EndpointId, EndpointConnections>,
    total_connections: usize,
    pending_eviction: HashMap<EndpointId, PendingEviction>,
    abuse_log_last: HashMap<(AbuseEvent, EndpointId), Instant>,
    breaker: DenialBreaker,
}

impl GateState {
    fn is_admitted(&self, did: &str) -> bool {
        self.admitted.contains(did) && !self.revoked.contains_key(did)
    }

    fn mark_for_eviction(&mut self, endpoint: EndpointId, now: Instant) {
        self.pending_eviction.entry(endpoint).or_insert(PendingEviction {
            since: now,
            overdue_warned: false,
        });
    }

    /// Rate-limits one abuse event per endpoint and kind per [`ABUSE_LOG_WINDOW`].
    fn should_log(&mut self, event: AbuseEvent, endpoint: EndpointId, now: Instant) -> bool {
        let key = (event, endpoint);
        if let Some(last) = self.abuse_log_last.get(&key) {
            if now.saturating_duration_since(*last) < ABUSE_LOG_WINDOW {
                return false;
            }
        }
        if self.abuse_log_last.len() >= ABUSE_LOG_CAPACITY && !self.abuse_log_last.contains_key(&key) {
            self.abuse_log_last
                .retain(|_, last| now.saturating_duration_since(*last) < ABUSE_LOG_WINDOW);
            if self.abuse_log_last.len() >= ABUSE_LOG_CAPACITY {
                return false;
            }
        }
        self.abuse_log_last.insert(key, now);
        true
    }
}

/// Settings the gate needs; built from [`RelayServeConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSettings {
    pub limits: GateLimits,
    pub served_verses: ServedVerses,
    pub log_client_ips: bool,
}

#[cfg(test)]
type AdmitHook = Arc<dyn Fn(&RelayMembershipGate, &str) + Send + Sync>;

/// Members-only access control: admits an endpoint iff its DID holds explicit Viewer+ in a served verse (OD-4).
pub struct RelayMembershipGate {
    settings: GateSettings,
    state: Mutex<GateState>,
    eviction_wake: tokio::sync::Notify,
    #[cfg(test)]
    admit_hook: Mutex<Option<AdmitHook>>,
}

impl std::fmt::Debug for RelayMembershipGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayMembershipGate")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl RelayMembershipGate {
    /// A gate that denies everyone until the first snapshot arrives.
    pub fn new(settings: GateSettings) -> Arc<Self> {
        Arc::new(Self {
            settings,
            state: Mutex::new(GateState {
                generation: 0,
                admitted: HashSet::new(),
                revoked: HashMap::new(),
                connections: HashMap::new(),
                total_connections: 0,
                pending_eviction: HashMap::new(),
                abuse_log_last: HashMap::new(),
                breaker: DenialBreaker::new(Instant::now()),
            }),
            eviction_wake: tokio::sync::Notify::new(),
            #[cfg(test)]
            admit_hook: Mutex::new(None),
        })
    }

    /// The gate for a validated config.
    pub fn for_config(config: &RelayServeConfig) -> Arc<Self> {
        Self::new(GateSettings {
            limits: config.limits,
            served_verses: config.served_verses.clone(),
            log_client_ips: config.log_client_ips,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Current snapshot generation (bumped by every snapshot and revoke).
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Live admitted connections of one endpoint.
    pub fn tracked_connections(&self, endpoint: &EndpointId) -> usize {
        self.lock()
            .connections
            .get(endpoint)
            .map_or(0, |entry| entry.connections.len())
    }

    /// Live admitted connections across all endpoints.
    pub fn total_connections(&self) -> usize {
        self.lock().total_connections
    }

    /// Whether the denial breaker is currently tripped.
    pub fn breaker_tripped(&self) -> bool {
        self.lock().breaker.tripped
    }

    /// Replaces the admitted set (served verses only) and marks every now-unadmitted live endpoint for eviction.
    pub fn apply_snapshot(&self, snapshot: &RelayAdmissionSnapshot) {
        let now = Instant::now();
        let mut admitted = HashSet::new();
        for (verse_id, members) in &snapshot.members_by_verse {
            if self.settings.served_verses.serves(verse_id) {
                admitted.extend(members.iter().cloned());
            }
        }
        let mut evicted = Vec::new();
        {
            let mut state = self.lock();
            state.generation += 1;
            state.revoked.retain(|did, _| !admitted.contains(did));
            state.admitted = admitted;
            let losing: Vec<EndpointId> = state
                .connections
                .iter()
                .filter(|(_, entry)| !state.is_admitted(&entry.did))
                .map(|(endpoint, _)| *endpoint)
                .collect();
            for endpoint in losing {
                state.mark_for_eviction(endpoint, now);
                if state.should_log(AbuseEvent::RevokedDisconnect, endpoint, now) {
                    evicted.push(endpoint);
                }
            }
        }
        for endpoint in &evicted {
            log_abuse(AbuseEvent::RevokedDisconnect, endpoint, None, None);
        }
        self.eviction_wake.notify_one();
    }

    /// Revokes one DID now: denies reconnects until a newer snapshot re-admits it, and evicts its live connections.
    pub fn revoke(&self, did: &str) {
        let now = Instant::now();
        let mut evicted = Vec::new();
        {
            let mut state = self.lock();
            state.generation += 1;
            let generation = state.generation;
            state.revoked.insert(did.to_string(), generation);
            let targets: Vec<EndpointId> = state
                .connections
                .iter()
                .filter(|(_, entry)| entry.did == did)
                .map(|(endpoint, _)| *endpoint)
                .collect();
            for endpoint in targets {
                state.mark_for_eviction(endpoint, now);
                if state.should_log(AbuseEvent::RevokedDisconnect, endpoint, now) {
                    evicted.push(endpoint);
                }
            }
        }
        for endpoint in &evicted {
            log_abuse(AbuseEvent::RevokedDisconnect, endpoint, None, None);
        }
        self.eviction_wake.notify_one();
    }

    /// Admission decision; counts go up only on `Allow` (a denied connection never gets `on_disconnect`).
    fn decide(&self, request: &ClientRequest) -> Result<u64, DenyReason> {
        let endpoint = request.endpoint_id();
        let did = crate::replicator::peer_did_key(&endpoint);
        let now = Instant::now();
        let limits = self.settings.limits;
        let (decision, log_event) = {
            let mut state = self.lock();
            let decision = if !state.is_admitted(&did) {
                Err(DenyReason::NotMember)
            } else if state
                .connections
                .get(&endpoint)
                .is_some_and(|entry| entry.connections.len() >= limits.max_connections_per_endpoint)
            {
                Err(DenyReason::PerEndpointCap)
            } else if state.total_connections >= limits.max_clients {
                Err(DenyReason::GlobalCap)
            } else {
                Ok(state.generation)
            };
            match decision {
                Ok(generation) => {
                    let entry = state
                        .connections
                        .entry(endpoint)
                        .or_insert_with(|| EndpointConnections {
                            did: did.clone(),
                            connections: Vec::new(),
                        });
                    entry.connections.push(TrackedConnection {
                        connection_id: request.connection_id(),
                        admitted_generation: generation,
                    });
                    state.total_connections += 1;
                    (decision, None)
                }
                Err(reason) => {
                    state.breaker.record_denial(now);
                    let event = match reason {
                        DenyReason::NotMember => AbuseEvent::Denied,
                        DenyReason::PerEndpointCap | DenyReason::GlobalCap => AbuseEvent::CapHit,
                    };
                    let log = state.should_log(event, endpoint, now).then_some(event);
                    (decision, log)
                }
            }
        };
        if let (Some(event), Err(reason)) = (log_event, decision) {
            let forwarded = self
                .settings
                .log_client_ips
                .then(|| forwarded_for(request))
                .flatten();
            log_abuse(event, &endpoint, Some(reason), forwarded.as_deref());
        }
        #[cfg(test)]
        if decision.is_ok() {
            let hook = self
                .admit_hook
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(hook) = hook {
                hook(self, &did);
            }
        }
        decision
    }

    /// Endpoints to (re)issue `disconnect` for; drops entries whose connections are all gone.
    fn eviction_targets(&self, now: Instant) -> Vec<EndpointId> {
        let mut overdue = Vec::new();
        let targets = {
            let mut state = self.lock();
            if state.pending_eviction.is_empty() {
                return Vec::new();
            }
            let GateState {
                pending_eviction,
                connections,
                ..
            } = &mut *state;
            pending_eviction.retain(|endpoint, _| connections.contains_key(endpoint));
            for (endpoint, pending) in pending_eviction.iter_mut() {
                if !pending.overdue_warned && now.saturating_duration_since(pending.since) > EVICTION_TARGET {
                    pending.overdue_warned = true;
                    let oldest_generation = connections
                        .get(endpoint)
                        .and_then(|entry| entry.connections.iter().map(|c| c.admitted_generation).min());
                    overdue.push((*endpoint, oldest_generation));
                }
            }
            pending_eviction.keys().copied().collect::<Vec<_>>()
        };
        for (endpoint, admitted_generation) in overdue {
            tracing::warn!(
                endpoint = %endpoint.fmt_short(),
                admitted_generation = ?admitted_generation,
                "relay eviction still pending past the 5 s target; retrying disconnect"
            );
        }
        targets
    }

    fn breaker_poll(&self, now: Instant) -> BreakerTransition {
        self.lock().breaker.poll(now)
    }

    fn breaker_reset(&self) {
        self.lock().breaker.reset(Instant::now());
    }

    #[cfg(test)]
    fn set_admit_hook(&self, hook: Option<AdmitHook>) {
        *self
            .admit_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = hook;
    }
}

impl AccessControl for RelayMembershipGate {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match self.decide(request) {
            Ok(_) => Access::Allow,
            Err(reason) => Access::Deny {
                reason: Some(reason.as_str().to_string()),
            },
        }
    }

    fn on_disconnect(&self, endpoint_id: EndpointId, connection_id: ConnectionId) {
        let mut state = self.lock();
        let mut removed = false;
        let mut endpoint_empty = false;
        if let Some(entry) = state.connections.get_mut(&endpoint_id) {
            let before = entry.connections.len();
            entry.connections.retain(|c| c.connection_id != connection_id);
            removed = entry.connections.len() < before;
            endpoint_empty = entry.connections.is_empty();
        }
        if removed {
            state.total_connections = state.total_connections.saturating_sub(1);
        }
        if endpoint_empty {
            state.connections.remove(&endpoint_id);
            state.pending_eviction.remove(&endpoint_id);
        }
    }
}

/// The reverse proxy's `X-Forwarded-For`; iroh-relay 1.3 exposes no peer socket address to the gate.
fn forwarded_for(request: &ClientRequest) -> Option<String> {
    request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Emits one abuse event; never carries tokens, carries a client address only when the operator opted in.
fn log_abuse(event: AbuseEvent, endpoint: &EndpointId, reason: Option<DenyReason>, forwarded_for: Option<&str>) {
    tracing::warn!(
        target: "fe_sync::relay_server",
        event = event.as_str(),
        reason = reason.map(DenyReason::as_str),
        endpoint_short = %endpoint.fmt_short(),
        forwarded_for,
        "relay abuse event"
    );
}

/// What the reconciler does when the denial breaker trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerPolicy {
    /// fe-relay: warn and keep serving honest members.
    WarnOnly,
    /// Desktop: close the listener for a cool-down (iroh-relay has no pause API).
    CloseListener { cooldown: Duration },
}

struct RelayRuntime {
    server: tokio::sync::Mutex<Option<relay::Server>>,
    config: Option<RelayServeConfig>,
    policy: BreakerPolicy,
    listening: AtomicBool,
    https_addr: Mutex<Option<SocketAddr>>,
}

/// A running embedded relay plus its 250 ms revocation reconciler.
pub struct EmbeddedRelay {
    gate: Arc<RelayMembershipGate>,
    runtime: Arc<RelayRuntime>,
    reconciler: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for EmbeddedRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedRelay")
            .field("listening", &self.is_listening())
            .field("https_addr", &self.https_addr())
            .finish_non_exhaustive()
    }
}

fn acme_cache_dir() -> PathBuf {
    crate::docs_engine::p2p_data_dir().join("relay-acme")
}

async fn build_cert_config(mode: &CertMode) -> Result<relay::CertConfig, RelayServeError> {
    let builder = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| RelayServeError::Tls {
        reason: error.to_string(),
    })?
    .with_no_client_auth();
    match mode {
        CertMode::Acme {
            hostname,
            contact,
            staging,
        } => {
            let contact = if contact.starts_with("mailto:") {
                contact.clone()
            } else {
                format!("mailto:{contact}")
            };
            let acme_config = relay::AcmeConfig::letsencrypt(!*staging)
                .domains(vec![hostname.clone()])
                .contact(vec![contact])
                .cache_path(acme_cache_dir());
            Ok(relay::CertConfig::LetsEncrypt {
                acme_config,
                server_config_builder: builder,
            })
        }
        CertMode::Manual {
            cert_path,
            key_path,
        } => {
            let resolver = relay::reloading_resolver(
                builder.crypto_provider(),
                cert_path.clone(),
                key_path.clone(),
                relay::DEFAULT_CERT_RELOAD_INTERVAL,
            )
            .await
            .map_err(|error| RelayServeError::Tls {
                reason: error.to_string(),
            })?;
            Ok(relay::CertConfig::Manual {
                server_config: builder.with_cert_resolver(resolver),
            })
        }
    }
}

/// TLS always on, `client_rx` set, access = the gate, metrics only on loopback.
async fn build_server_config(
    config: &RelayServeConfig,
    gate: Arc<RelayMembershipGate>,
) -> Result<relay::ServerConfig, RelayServeError> {
    let cert = build_cert_config(&config.cert_mode).await?;
    let mut relay_config = relay::RelayConfig::new(config.http_bind);
    relay_config.tls = Some(relay::TlsConfig::new(config.https_bind, cert));
    let mut rate_limit = relay::ClientRateLimit::new(config.client_rx_bytes_per_second);
    rate_limit.max_burst_bytes = Some(config.client_rx_burst_bytes);
    let mut limits = relay::Limits::default();
    limits.client_rx = Some(rate_limit);
    relay_config.limits = limits;
    relay_config.key_cache_capacity = Some(config.limits.max_clients.max(1024));
    relay_config.access = gate;
    let mut server_config = relay::ServerConfig::default();
    server_config.relay = Some(relay_config);
    server_config.quic = config.qad_bind.map(|bind| relay::QuicConfig::new(bind));
    server_config.metrics_addr = config.metrics_bind;
    Ok(server_config)
}

async fn spawn_server(
    config: &RelayServeConfig,
    gate: Arc<RelayMembershipGate>,
) -> Result<relay::Server, RelayServeError> {
    let server_config = build_server_config(config, gate).await?;
    relay::Server::spawn(server_config)
        .await
        .map_err(|error| RelayServeError::Spawn {
            reason: error.to_string(),
        })
}

impl EmbeddedRelay {
    /// Validates the config (refuse-to-start rules), binds the TLS listener with the gate, starts the reconciler.
    pub async fn spawn(
        config: RelayServeConfig,
        gate: Arc<RelayMembershipGate>,
    ) -> Result<Self, RelayServeError> {
        config.validate()?;
        let server = spawn_server(&config, gate.clone()).await?;
        tracing::warn!(
            https = ?server.https_addr(),
            served_verses = ?config.served_verses,
            qad = config.qad_bind.is_some(),
            "{RELAY_SERVE_ENV}=1: embedded members-only relay is serving; members see this host, \
             and this host sees relayed members' addresses and traffic metadata"
        );
        let policy = config.profile.breaker_policy();
        Ok(Self::start(server, gate, Some(config), policy))
    }

    fn start(
        server: relay::Server,
        gate: Arc<RelayMembershipGate>,
        config: Option<RelayServeConfig>,
        policy: BreakerPolicy,
    ) -> Self {
        let runtime = Arc::new(RelayRuntime {
            https_addr: Mutex::new(server.https_addr()),
            server: tokio::sync::Mutex::new(Some(server)),
            config,
            policy,
            listening: AtomicBool::new(true),
        });
        let reconciler = tokio::spawn(run_reconciler(gate.clone(), runtime.clone()));
        Self {
            gate,
            runtime,
            reconciler,
        }
    }

    /// Test-only: adopt an already-running (self-signed) relay so the reconciler runs against it.
    #[cfg(test)]
    fn adopt_test_server(server: relay::Server, gate: Arc<RelayMembershipGate>) -> Self {
        Self::start(server, gate, None, BreakerPolicy::WarnOnly)
    }

    /// Revokes a DID and wakes the reconciler, which retries `disconnect` until `on_disconnect` fired (OD-12).
    pub fn revoke(&self, did: &str) {
        self.gate.revoke(did);
    }

    /// The membership gate this relay consults.
    pub fn gate(&self) -> &Arc<RelayMembershipGate> {
        &self.gate
    }

    /// False while a desktop breaker cool-down has the listener closed.
    pub fn is_listening(&self) -> bool {
        self.runtime.listening.load(Ordering::Acquire)
    }

    /// The bound HTTPS address while listening.
    pub fn https_addr(&self) -> Option<SocketAddr> {
        *self
            .runtime
            .https_addr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Stops the reconciler and shuts the server down gracefully.
    pub async fn shutdown(self) {
        self.reconciler.abort();
        let server = self.runtime.server.lock().await.take();
        if let Some(server) = server {
            if let Err(error) = server.shutdown().await {
                tracing::warn!(%error, "embedded relay shutdown reported an error");
            }
        }
    }
}

impl Drop for EmbeddedRelay {
    fn drop(&mut self) {
        self.reconciler.abort();
    }
}

/// Convergent revocation loop plus breaker handling; one pass per 250 ms tick or eviction wake-up.
async fn run_reconciler(gate: Arc<RelayMembershipGate>, runtime: Arc<RelayRuntime>) {
    let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut reopen_at: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = gate.eviction_wake.notified() => {}
        }
        let now = Instant::now();
        let targets = gate.eviction_targets(now);
        if !targets.is_empty() {
            let service = runtime
                .server
                .lock()
                .await
                .as_ref()
                .and_then(|server| server.relay_service().cloned());
            if let Some(service) = service {
                for endpoint in targets {
                    // `false` = not registered yet (admit/register race); the next tick retries.
                    service.clients().disconnect(endpoint, None);
                }
            }
        }
        handle_breaker(&gate, &runtime, now, &mut reopen_at).await;
    }
}

async fn handle_breaker(
    gate: &Arc<RelayMembershipGate>,
    runtime: &RelayRuntime,
    now: Instant,
    reopen_at: &mut Option<Instant>,
) {
    if let Some(deadline) = *reopen_at {
        if now < deadline {
            return;
        }
        let Some(config) = runtime.config.as_ref() else {
            *reopen_at = None;
            return;
        };
        match spawn_server(config, gate.clone()).await {
            Ok(server) => {
                *runtime
                    .https_addr
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = server.https_addr();
                *runtime.server.lock().await = Some(server);
                runtime.listening.store(true, Ordering::Release);
                gate.breaker_reset();
                *reopen_at = None;
                tracing::warn!("relay denial breaker cool-down over; listener reopened");
            }
            Err(error) => {
                tracing::warn!(%error, "relay listener reopen failed; retrying after another cool-down");
                if let BreakerPolicy::CloseListener { cooldown } = runtime.policy {
                    *reopen_at = Some(now + cooldown);
                }
            }
        }
        return;
    }
    match (gate.breaker_poll(now), runtime.policy) {
        (BreakerTransition::Tripped, BreakerPolicy::WarnOnly) => tracing::warn!(
            "relay denial circuit breaker tripped; operator relay keeps serving members (absorb floods at the firewall)"
        ),
        (BreakerTransition::Tripped, BreakerPolicy::CloseListener { cooldown }) => {
            let server = runtime.server.lock().await.take();
            runtime.listening.store(false, Ordering::Release);
            *runtime
                .https_addr
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            if let Some(server) = server {
                if let Err(error) = server.shutdown().await {
                    tracing::warn!(%error, "relay listener shutdown reported an error");
                }
            }
            *reopen_at = Some(now + cooldown);
            tracing::warn!(
                cooldown_secs = cooldown.as_secs(),
                "relay denial circuit breaker tripped; listener closed for a cool-down"
            );
        }
        (BreakerTransition::Cleared, _) => tracing::info!("relay denial circuit breaker cleared"),
        (BreakerTransition::Unchanged, _) => {}
    }
}

/// Process-wide gate the sync thread feeds (NET-21); same shape as `relay_config::set_settings_relay_spec`.
static RELAY_GATE: OnceLock<Arc<RelayMembershipGate>> = OnceLock::new();

/// Registers the gate before the sync thread spawns; first call wins (returns false and warns otherwise).
pub fn install_gate(gate: Arc<RelayMembershipGate>) -> bool {
    if RELAY_GATE.set(gate).is_err() {
        tracing::warn!("relay membership gate already installed; ignoring the new gate");
        return false;
    }
    true
}

/// Pushes the latest admission snapshot to the installed gate; no-op without one.
pub fn push_snapshot(snapshot: RelayAdmissionSnapshot) {
    if let Some(gate) = RELAY_GATE.get() {
        gate.apply_snapshot(&snapshot);
    }
}

/// Revokes a DID on the installed gate; no-op without one.
pub fn push_revoke(did: &str) {
    if let Some(gate) = RELAY_GATE.get() {
        gate.revoke(did);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use futures_lite::StreamExt;
    use iroh::endpoint::{RelayStatus, presets};
    use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey, Watcher};

    use super::*;

    const VERSE: &str = "01JVERSE0000000000000000AA";
    const OTHER_VERSE: &str = "01JVERSE0000000000000000BB";
    const WAIT: Duration = Duration::from_secs(20);

    fn did_of(seed: u8) -> String {
        crate::replicator::peer_did_key(&SecretKey::from_bytes(&[seed; 32]).public())
    }

    fn snapshot(verses: &[(&str, &[u8])]) -> RelayAdmissionSnapshot {
        let members_by_verse: BTreeMap<String, BTreeSet<String>> = verses
            .iter()
            .map(|(verse, seeds)| (verse.to_string(), seeds.iter().map(|seed| did_of(*seed)).collect()))
            .collect();
        RelayAdmissionSnapshot {
            members_by_verse,
            ..Default::default()
        }
    }

    fn gate(max_clients: usize, max_connections_per_endpoint: usize) -> Arc<RelayMembershipGate> {
        RelayMembershipGate::new(GateSettings {
            limits: GateLimits {
                max_clients,
                max_connections_per_endpoint,
            },
            served_verses: ServedVerses::Listed([VERSE.to_string()].into()),
            log_client_ips: false,
        })
    }

    async fn relay_client(seed: u8, url: &RelayUrl) -> Endpoint {
        let map: iroh::RelayMap = iroh::RelayConfig::new(url.clone(), None).into();
        Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(&[seed; 32]))
            .relay_mode(RelayMode::Custom(map))
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .clear_ip_transports()
            .bind()
            .await
            .expect("bind relay-only client")
    }

    /// Waits until the home-relay status satisfies `wanted`; false on timeout.
    async fn relay_status_reaches(endpoint: &Endpoint, wanted: impl Fn(&RelayStatus) -> bool) -> bool {
        let mut stream = endpoint.home_relay_status().stream();
        tokio::time::timeout(WAIT, async {
            while let Some(statuses) = stream.next().await {
                if statuses.iter().any(&wanted) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    async fn is_admitted(endpoint: &Endpoint) -> bool {
        relay_status_reaches(endpoint, RelayStatus::is_connected).await
    }

    async fn is_denied_with(endpoint: &Endpoint, reason: &str) -> bool {
        relay_status_reaches(endpoint, |status| status.auth_denied_reason() == Some(reason)).await
    }

    async fn tracked_drops_to_zero(gate: &RelayMembershipGate, endpoint: &EndpointId) -> bool {
        tokio::time::timeout(WAIT, async {
            while gate.tracked_connections(endpoint) > 0 {
                tokio::time::sleep(RECONCILE_INTERVAL).await;
            }
        })
        .await
        .is_ok()
    }

    /// AC-4: explicit Viewer+ admitted (incl. a fresh Viewer with no `__peers` row), others denied by name, caps by name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_admission_and_caps_journey() {
        let gate = gate(3, 1);
        assert!(install_gate(gate.clone()), "first install wins");
        // The snapshot is the gate's only input: it has no default_access/open_to_all field, so an
        // open verse cannot widen admission here (NET-21 owns the fabric-side tripwire).
        push_snapshot(snapshot(&[
            // seeds 1, 2, 3, 4: explicit Viewer+; seed 2 is a fresh Viewer that never published `__peers`.
            (VERSE, &[1, 2, 3, 4]),
            // seed 6 is a member of a verse this relay does not serve.
            (OTHER_VERSE, &[6]),
        ]));
        let (_map, url, server) = iroh::test_utils::run_relay_server_with_access(false, gate.clone())
            .await
            .expect("in-process relay");
        let relay = EmbeddedRelay::adopt_test_server(server, gate.clone());

        let member = relay_client(1, &url).await;
        assert!(is_admitted(&member).await, "explicit Viewer+ member is admitted");
        let fresh_viewer = relay_client(2, &url).await;
        assert!(is_admitted(&fresh_viewer).await, "fresh Viewer without a __peers row is admitted");

        let stranger = relay_client(5, &url).await;
        assert!(is_denied_with(&stranger, "not_member").await, "non-member is denied by name");
        let other_verse_member = relay_client(6, &url).await;
        assert!(
            is_denied_with(&other_verse_member, "not_member").await,
            "a member of an unserved verse is denied"
        );

        let member_second_session = relay_client(1, &url).await;
        assert!(
            is_denied_with(&member_second_session, "per_endpoint_cap").await,
            "a second connection over the per-endpoint cap is denied by name"
        );

        let third = relay_client(3, &url).await;
        assert!(is_admitted(&third).await, "third member fills the global cap");
        let fourth = relay_client(4, &url).await;
        assert!(is_denied_with(&fourth, "global_cap").await, "global cap denies by name");
        assert_eq!(gate.total_connections(), 3, "denied connections are never counted");

        for endpoint in [member, fresh_viewer, stranger, other_verse_member, member_second_session, third, fourth] {
            endpoint.close().await;
        }
        relay.shutdown().await;
    }

    /// AC-5 / OD-12: revoke evicts and denies reconnect, including a revoke between admit and register.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_evicts_including_admit_register_race_journey() {
        let gate = gate(16, 2);
        gate.apply_snapshot(&snapshot(&[(VERSE, &[11, 12])]));
        let (_map, url, server) = iroh::test_utils::run_relay_server_with_access(false, gate.clone())
            .await
            .expect("in-process relay");
        let relay = EmbeddedRelay::adopt_test_server(server, gate.clone());

        let revoked_member = relay_client(11, &url).await;
        assert!(is_admitted(&revoked_member).await);
        let generation_before = gate.generation();
        relay.revoke(&did_of(11));
        assert!(gate.generation() > generation_before, "a revoke bumps the generation");
        assert!(
            tracked_drops_to_zero(&gate, &revoked_member.id()).await,
            "every tracked connection of the revoked DID saw on_disconnect"
        );
        assert!(
            is_denied_with(&revoked_member, "not_member").await,
            "the revoked member's reconnect is denied"
        );

        // Inject the revoke inside the admit path: on_connect has said Allow, Clients::register has not run.
        let racing_did = did_of(12);
        let fired = Arc::new(AtomicBool::new(false));
        let hook_fired = fired.clone();
        let hook_did = racing_did.clone();
        gate.set_admit_hook(Some(Arc::new(move |gate: &RelayMembershipGate, did: &str| {
            if did == hook_did && !hook_fired.swap(true, Ordering::SeqCst) {
                gate.revoke(did);
            }
        })));
        let racing_member = relay_client(12, &url).await;
        assert!(
            is_denied_with(&racing_member, "not_member").await,
            "a connection admitted just before the revoke is still torn down and its reconnect denied"
        );
        assert!(fired.load(Ordering::SeqCst), "the revoke landed inside the admit path");
        assert!(tracked_drops_to_zero(&gate, &racing_member.id()).await);
        gate.set_admit_hook(None);

        revoked_member.close().await;
        racing_member.close().await;
        relay.shutdown().await;
    }

    /// AC-2: every misconfiguration refuses to start with a named error; no opt-in binds nothing.
    #[test]
    fn refuses_to_start_table() {
        let manual = [
            (RELAY_SERVE_ENV, "1"),
            (RELAY_SERVE_CERT_MODE_ENV, "manual"),
            (RELAY_SERVE_CERT_PATH_ENV, "cert.pem"),
            (RELAY_SERVE_KEY_PATH_ENV, "key.pem"),
            (RELAY_SERVE_VERSES_ENV, VERSE),
        ];
        let hosted: BTreeSet<String> = [VERSE.to_string()].into();
        let nothing_hosted: BTreeSet<String> = BTreeSet::new();
        // (row, env overrides on top of `manual` — "" removes a key, hosted set, expected outcome).
        let rows: &[(&str, &[(&str, &str)], Option<&BTreeSet<String>>, Result<bool, &str>)] = &[
            ("unset opt-in binds nothing", &[(RELAY_SERVE_ENV, "")], Some(&hosted), Ok(false)),
            ("opt-in 0 binds nothing", &[(RELAY_SERVE_ENV, "0")], Some(&hosted), Ok(false)),
            ("opt-in true binds nothing", &[(RELAY_SERVE_ENV, "true")], Some(&hosted), Ok(false)),
            ("valid manual config", &[], Some(&hosted), Ok(true)),
            ("no cert mode", &[(RELAY_SERVE_CERT_MODE_ENV, "")], Some(&hosted), Err("no_cert_mode")),
            ("plaintext cert mode", &[(RELAY_SERVE_CERT_MODE_ENV, "none")], Some(&hosted), Err("unknown_cert_mode")),
            ("acme without hostname", &[(RELAY_SERVE_CERT_MODE_ENV, "acme")], Some(&hosted), Err("missing_acme_hostname")),
            ("manual without key", &[(RELAY_SERVE_KEY_PATH_ENV, "")], Some(&hosted), Err("missing_key_path")),
            ("empty verse set", &[(RELAY_SERVE_VERSES_ENV, " , ")], Some(&hosted), Err("empty_verse_set")),
            ("unhosted verse", &[(RELAY_SERVE_VERSES_ENV, OTHER_VERSE)], Some(&hosted), Err("unhosted_verse")),
            ("hosted keyword with nothing hosted", &[(RELAY_SERVE_VERSES_ENV, "hosted")], Some(&nothing_hosted), Err("empty_verse_set")),
            ("allow-all access", &[(RELAY_SERVE_ACCESS_ENV, "all")], Some(&hosted), Err("open_access_refused")),
            ("public metrics bind", &[(RELAY_SERVE_METRICS_BIND_ENV, "0.0.0.0:9090")], Some(&hosted), Err("metrics_not_loopback")),
            ("hosted set never supplied", &[], None, Err("hosted_verses_unknown")),
        ];
        for (row, overrides, hosted_set, expected) in rows {
            let mut env: HashMap<&str, &str> = manual.iter().copied().collect();
            for (key, value) in overrides.iter() {
                env.insert(key, value);
            }
            let lookup = |key: &str| env.get(key).filter(|v| !v.is_empty()).map(|v| v.to_string());
            let outcome = RelayServeConfig::from_lookup(RelayServeProfile::Operator, &lookup).and_then(|config| {
                let Some(config) = config else { return Ok(false) };
                let config = match hosted_set {
                    Some(hosted) => config.with_hosted_verses((**hosted).clone())?,
                    None => config,
                };
                config.validate().map(|()| true)
            });
            match (outcome, expected) {
                (Ok(bound), Ok(want)) => assert_eq!(bound, *want, "{row}"),
                (Err(error), Err(want)) => assert_eq!(error.name(), *want, "{row}: {error}"),
                (other, want) => panic!("{row}: got {other:?}, wanted {want:?}"),
            }
        }
    }
}
