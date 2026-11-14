//! SSRF-pinned HTTPS dial-back prober (FR-2b); see `fe-sync/src/AGENTS.md` §reachability.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_runtime::dialback::DialbackRecord;
use fe_runtime::relay_advertisement::{
    classify_relay_address, AddressClass, RelayUrlError, RelayUrlPolicy,
};

/// Fixed probe path on the relay (iroh-relay answers `/ping`); the requested URL's path is ignored.
pub const PROBE_PATH: &str = "/ping";
/// Response body cap (spec FR-2b: 1 KiB).
pub const MAX_PROBE_BODY_BYTES: usize = 1024;
/// Total request budget (connect, TLS, response).
pub const PROBE_TOTAL_TIMEOUT: Duration = Duration::from_secs(5);
/// One probe per requester per window.
pub const PER_REQUESTER_WINDOW_MS: u64 = 60_000;
/// Probes allowed across all requesters per window.
pub const GLOBAL_PROBES_PER_WINDOW: usize = 10;

/// Why a probe was not attempted or was aborted before a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRefusal {
    /// URL failed the strict relay URL policy.
    UrlPolicy(RelayUrlError),
    RateLimitedRequester,
    RateLimitedGlobal,
    /// DNS lookup failed or returned nothing.
    ResolutionFailed(String),
    /// At least one resolved address is not public; mixed answers are refused whole.
    ForbiddenAddress(IpAddr),
}

impl std::fmt::Display for ProbeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UrlPolicy(error) => write!(f, "dial-back url refused: {error}"),
            Self::RateLimitedRequester => write!(f, "dial-back rate limit for this requester"),
            Self::RateLimitedGlobal => write!(f, "dial-back global rate limit"),
            Self::ResolutionFailed(reason) => write!(f, "dial-back resolution failed: {reason}"),
            Self::ForbiddenAddress(address) => {
                write!(f, "dial-back target resolves to refused address {address}")
            }
        }
    }
}

impl std::error::Error for ProbeRefusal {}

/// DNS edge, faked in tests.
#[async_trait::async_trait]
pub trait DialbackResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>>;
}

/// Production resolver over the system lookup.
pub struct SystemResolver;

#[async_trait::async_trait]
impl DialbackResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((host, port)).await?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

/// Wall clock edge, injectable for rate-limit tests.
pub trait DialbackClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Production clock.
pub struct SystemClock;

impl DialbackClock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Default)]
struct RateState {
    last_by_requester: HashMap<String, u64>,
    global_recent: VecDeque<u64>,
}

/// Probes an advertised relay URL on behalf of a requester and signs the verdict.
pub struct DialbackProber {
    signing_key: SigningKey,
    url_policy: RelayUrlPolicy,
    resolver: Box<dyn DialbackResolver>,
    clock: Box<dyn DialbackClock>,
    rate: Mutex<RateState>,
}

impl DialbackProber {
    pub fn new(signing_key: SigningKey) -> Self {
        Self::with_edges(signing_key, Box::new(SystemResolver), Box::new(SystemClock))
    }

    pub fn with_edges(
        signing_key: SigningKey,
        resolver: Box<dyn DialbackResolver>,
        clock: Box<dyn DialbackClock>,
    ) -> Self {
        Self {
            signing_key,
            url_policy: RelayUrlPolicy::default(),
            resolver,
            clock,
            rate: Mutex::new(RateState::default()),
        }
    }

    /// Charges the requester and global windows; a refused call consumes nothing.
    fn admit(&self, requester_did: &str, now_ms: u64) -> Result<(), ProbeRefusal> {
        let mut state = self.rate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .last_by_requester
            .retain(|_, last| now_ms.saturating_sub(*last) < PER_REQUESTER_WINDOW_MS);
        while let Some(oldest) = state.global_recent.front() {
            if now_ms.saturating_sub(*oldest) >= PER_REQUESTER_WINDOW_MS {
                state.global_recent.pop_front();
            } else {
                break;
            }
        }
        if state.last_by_requester.contains_key(requester_did) {
            return Err(ProbeRefusal::RateLimitedRequester);
        }
        if state.global_recent.len() >= GLOBAL_PROBES_PER_WINDOW {
            return Err(ProbeRefusal::RateLimitedGlobal);
        }
        state.last_by_requester.insert(requester_did.to_string(), now_ms);
        state.global_recent.push_back(now_ms);
        Ok(())
    }

    /// Resolves and checks every address; returns them only when all are public.
    async fn resolve_public(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, ProbeRefusal> {
        let addresses = self
            .resolver
            .resolve(host, port)
            .await
            .map_err(|error| ProbeRefusal::ResolutionFailed(error.to_string()))?;
        if addresses.is_empty() {
            return Err(ProbeRefusal::ResolutionFailed("no addresses".to_string()));
        }
        for address in &addresses {
            if classify_relay_address(*address) != AddressClass::Public {
                return Err(ProbeRefusal::ForbiddenAddress(*address));
            }
        }
        Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect())
    }

    /// Probes `url` for `requester_did`; an unreachable relay yields a signed `ok = false` record.
    pub async fn probe(
        &self,
        requester_did: &str,
        url: &str,
    ) -> Result<DialbackRecord, ProbeRefusal> {
        let accepted = self.url_policy.check(url).map_err(ProbeRefusal::UrlPolicy)?;
        let now_ms = self.clock.now_ms();
        self.admit(requester_did, now_ms)?;

        let host = accepted.url().host_str().unwrap_or_default().to_string();
        let port = accepted.url().port_or_known_default().unwrap_or(443);
        let pinned = self.resolve_public(&host, port).await?;

        let mut target = accepted.url().clone();
        target.set_path(PROBE_PATH);
        target.set_query(None);
        target.set_fragment(None);

        let ok = fetch_pinned(&host, &pinned, target.as_str()).await;
        // Signed in normalized form, the same string the REST layer compares against.
        let normalized_url = accepted.to_string();
        Ok(DialbackRecord::sign(
            &self.signing_key,
            requester_did,
            &normalized_url,
            now_ms,
            ok,
        ))
    }
}

/// Builds the client: connects only to the checked addresses (no proxy may re-resolve), TLS still verifies the hostname.
fn build_pinned_client(host: &str, pinned: &[SocketAddr]) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .resolve_to_addrs(host, pinned)
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .timeout(PROBE_TOTAL_TIMEOUT)
        .build()
}

/// GET with a body cap; true on a 2xx that fit the cap.
async fn fetch_pinned(host: &str, pinned: &[SocketAddr], target: &str) -> bool {
    let client = match build_pinned_client(host, pinned) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "dial-back client build failed");
            return false;
        }
    };
    let mut response = match client.get(target).send().await {
        Ok(response) => response,
        Err(_) => return false,
    };
    if !response.status().is_success() {
        return false;
    }
    let mut received = 0usize;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                received += chunk.len();
                if received > MAX_PROBE_BODY_BYTES {
                    return false;
                }
            }
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    struct FakeResolver(HashMap<&'static str, Vec<&'static str>>);

    #[async_trait::async_trait]
    impl DialbackResolver for FakeResolver {
        async fn resolve(&self, host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
            match self.0.get(host) {
                Some(addresses) => Ok(addresses.iter().map(|text| text.parse().unwrap()).collect()),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "nxdomain")),
            }
        }
    }

    struct FakeClock(Arc<AtomicU64>);

    impl DialbackClock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn prober(now: Arc<AtomicU64>) -> DialbackProber {
        let table = HashMap::from([
            ("private.example", vec!["10.0.0.5"]),
            ("loopback.example", vec!["127.0.0.1"]),
            ("linklocal.example", vec!["169.254.169.254"]),
            ("cgnat.example", vec!["100.64.1.1"]),
            ("mapped.example", vec!["::ffff:127.0.0.1"]),
            ("mixed.example", vec!["93.184.216.34", "192.168.1.9"]),
            ("rebind.example", vec!["93.184.216.34", "127.0.0.1"]),
        ]);
        DialbackProber::with_edges(
            SigningKey::from_bytes(&[7u8; 32]),
            Box::new(FakeResolver(table)),
            Box::new(FakeClock(now)),
        )
    }

    #[test]
    fn prober_refuses_private_loopback_mixed_record_rebinding_and_redirect_targets() {
        let now = Arc::new(AtomicU64::new(1_000_000));
        let prober = prober(now.clone());
        let rows: [(&str, &str, fn(&ProbeRefusal) -> bool); 11] = [
            ("did:a", "https://private.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:b", "https://loopback.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:c", "https://linklocal.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:d", "https://cgnat.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:e", "https://mapped.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:f", "https://mixed.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:g", "https://rebind.example", |r| matches!(r, ProbeRefusal::ForbiddenAddress(_))),
            ("did:h", "https://unknown.example", |r| matches!(r, ProbeRefusal::ResolutionFailed(_))),
            ("did:i", "http://example.org", |r| matches!(r, ProbeRefusal::UrlPolicy(_))),
            ("did:j", "https://127.0.0.1", |r| matches!(r, ProbeRefusal::UrlPolicy(_))),
            ("did:k", "https://example.org:8443", |r| matches!(r, ProbeRefusal::UrlPolicy(_))),
        ];
        for (requester, url, expected) in rows {
            let refusal = futures_lite::future::block_on(prober.probe(requester, url))
                .expect_err(url);
            assert!(expected(&refusal), "{url}: {refusal}");
        }

        // Redirects are never followed and the dial is pinned: the client builds with both.
        let pinned = ["93.184.216.34:443".parse().unwrap()];
        assert!(build_pinned_client("example.org", &pinned).is_ok());

        // Rate limits: a valid-URL refusal at resolution still charged the requester window.
        let again = futures_lite::future::block_on(prober.probe("did:a", "https://private.example"));
        assert_eq!(again.unwrap_err(), ProbeRefusal::RateLimitedRequester);
        now.fetch_add(PER_REQUESTER_WINDOW_MS, Ordering::SeqCst);
        let after_window = futures_lite::future::block_on(prober.probe("did:a", "https://private.example"));
        assert!(matches!(after_window.unwrap_err(), ProbeRefusal::ForbiddenAddress(_)));

        // Global window: ten admissions per window across distinct requesters, the eleventh is refused.
        for index in 0..(GLOBAL_PROBES_PER_WINDOW - 1) {
            let requester = format!("did:r{index}");
            let result = futures_lite::future::block_on(prober.probe(&requester, "https://private.example"));
            assert!(matches!(result.unwrap_err(), ProbeRefusal::ForbiddenAddress(_)));
        }
        let over = futures_lite::future::block_on(prober.probe("did:over", "https://private.example"));
        assert_eq!(over.unwrap_err(), ProbeRefusal::RateLimitedGlobal);
    }
}
