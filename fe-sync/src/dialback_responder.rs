//! Dial-back responder and requester over signed `reach_probe` / `reach_result` frames; see `fe-sync/src/AGENTS.md` §reachability.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_policy::RoleLevel;
use fe_runtime::dialback::{DialbackRecord, DIALBACK_FUTURE_SKEW_MS, DIALBACK_MAX_AGE_MS};
use fe_runtime::relay_advertisement::RelayUrlPolicy;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::dialback_prober::{DialbackProber, ProbeRefusal, PROBE_TOTAL_TIMEOUT};
use crate::distributed_query::{unix_now_ms, TopicSender};
use crate::gossip_auth::{self, GossipFrameKind};

/// Probes that may run at once on this node; a request past it is refused as busy.
pub const MAX_CONCURRENT_PROBES: usize = 4;
/// Longest accepted request payload, in bytes (a URL plus JSON framing).
pub const MAX_REQUEST_PAYLOAD_BYTES: usize = 4096;
/// Outstanding local requests held at once.
pub const MAX_PENDING_REQUESTS: usize = 16;
/// Verse topics one local request is broadcast on.
pub const MAX_REQUEST_VERSES: usize = 4;
/// How long [`request_dialback`] waits for a signed answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(PROBE_TOTAL_TIMEOUT.as_secs() + 5);

/// `AppSettings.act_as_prober` (default off), registered at startup like the relay toggles.
static ACT_AS_PROBER: AtomicBool = AtomicBool::new(false);

/// Registers whether this node answers dial-back requests from verse peers (local only, never replicated).
pub fn set_act_as_prober(enabled: bool) {
    ACT_AS_PROBER.store(enabled, Ordering::Relaxed);
}

/// The registered toggle; `false` until a caller registers otherwise.
pub fn act_as_prober() -> bool {
    ACT_AS_PROBER.load(Ordering::Relaxed)
}

/// Payload of a `reach_probe` frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReachProbeRequest {
    url: String,
}

/// Why a request was not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestRefusal {
    /// This node does not act as a prober.
    NotProber,
    /// The requester has no `__peers` declaration in the arrival verse.
    Undeclared,
    /// The requester's role in the arrival verse is below Viewer.
    BelowViewer,
    /// Payload is oversized or not a request.
    MalformedPayload,
    /// [`MAX_CONCURRENT_PROBES`] probes are already running.
    Busy,
    /// The prober refused (url policy, rate limit, address class).
    Probe(ProbeRefusal),
    /// The signed answer could not be built.
    Seal(&'static str),
}

/// The probing edge, faked in tests.
#[async_trait::async_trait]
pub trait ReachProber: Send + Sync {
    async fn probe(&self, requester_did: &str, url: &str) -> Result<DialbackRecord, ProbeRefusal>;
}

#[async_trait::async_trait]
impl ReachProber for DialbackProber {
    async fn probe(&self, requester_did: &str, url: &str) -> Result<DialbackRecord, ProbeRefusal> {
        DialbackProber::probe(self, requester_did, url).await
    }
}

/// Where a signed answer is broadcast, faked in tests.
#[async_trait::async_trait]
pub trait ReplySink: Send + Sync {
    async fn send(&self, frame: bytes::Bytes) -> Result<(), String>;
}

#[async_trait::async_trait]
impl ReplySink for TopicSender {
    async fn send(&self, frame: bytes::Bytes) -> Result<(), String> {
        self.broadcast(frame).await
    }
}

/// Answers dial-back requests: authorizes, probes off the sync loop, broadcasts the signed record.
pub struct DialbackResponder {
    prober: Arc<dyn ReachProber>,
    secret: iroh::SecretKey,
    permits: Arc<Semaphore>,
}

impl DialbackResponder {
    /// Responder probing as `secret` (the key behind the node's did:key).
    pub fn new(secret: iroh::SecretKey) -> Self {
        let prober = DialbackProber::new(SigningKey::from_bytes(&secret.to_bytes()));
        Self::with_prober(secret, Arc::new(prober))
    }

    /// Responder over an injected prober.
    pub fn with_prober(secret: iroh::SecretKey, prober: Arc<dyn ReachProber>) -> Self {
        Self {
            prober,
            secret,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES)),
        }
    }

    /// Only a declared peer holding Viewer or better in the arrival verse may ask.
    pub fn authorize(declared: bool, role: RoleLevel) -> Result<(), RequestRefusal> {
        if !declared {
            return Err(RequestRefusal::Undeclared);
        }
        if !role.is_at_least(RoleLevel::Viewer) {
            return Err(RequestRefusal::BelowViewer);
        }
        Ok(())
    }

    /// Probes the requested url and returns the sealed `reach_result` frame.
    pub async fn answer(
        &self,
        verse_id: &str,
        requester_did: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, RequestRefusal> {
        if payload.len() > MAX_REQUEST_PAYLOAD_BYTES {
            return Err(RequestRefusal::MalformedPayload);
        }
        let request: ReachProbeRequest =
            serde_json::from_slice(payload).map_err(|_| RequestRefusal::MalformedPayload)?;
        let record = self
            .prober
            .probe(requester_did, &request.url)
            .await
            .map_err(RequestRefusal::Probe)?;
        let body = serde_json::to_vec(&record).map_err(|_| RequestRefusal::Seal("record is not serializable"))?;
        gossip_auth::seal(&self.secret, GossipFrameKind::ReachResult, verse_id, unix_now_ms(), body)
            .map_err(RequestRefusal::Seal)
    }

    /// Admits a verified request and answers it on a bounded spawned task; never blocks the sync loop.
    pub fn dispatch(
        self: &Arc<Self>,
        verse_id: &str,
        requester_did: &str,
        payload: &[u8],
        declared: bool,
        role: RoleLevel,
        sink: Arc<dyn ReplySink>,
    ) -> Result<(), RequestRefusal> {
        if !act_as_prober() {
            return Err(RequestRefusal::NotProber);
        }
        Self::authorize(declared, role)?;
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| RequestRefusal::Busy)?;
        let responder = Arc::clone(self);
        let (verse_id, requester_did, payload) = (verse_id.to_owned(), requester_did.to_owned(), payload.to_vec());
        tokio::spawn(async move {
            let _permit = permit;
            match responder.answer(&verse_id, &requester_did, &payload).await {
                Ok(frame) => {
                    if let Err(error) = sink.send(frame.into()).await {
                        tracing::warn!(verse_id, "Failed to broadcast dial-back result: {error}");
                    }
                }
                Err(refusal) => tracing::debug!(verse_id, ?refusal, "Dial-back request not answered"),
            }
        });
        Ok(())
    }
}

/// One local request waiting for its signed answer.
struct PendingDialback {
    url: String,
    reply: oneshot::Sender<Option<DialbackRecord>>,
}

/// Local requests awaiting a `reach_result`, owned by the sync loop.
#[derive(Default)]
pub struct DialbackRequester {
    pending: Vec<PendingDialback>,
}

impl DialbackRequester {
    /// Holds `reply` until a record for `url` arrives; false when the table is full.
    pub fn register(&mut self, url: String, reply: oneshot::Sender<Option<DialbackRecord>>) -> bool {
        self.pending.retain(|entry| !entry.reply.is_closed());
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return false;
        }
        self.pending.push(PendingDialback { url, reply });
        true
    }

    /// Delivers a verified record addressed to `local_did`; returns how many waiters it completed.
    pub fn accept_result(&mut self, local_did: &str, author_did: &str, payload: &[u8], now_ms: u64) -> usize {
        let Ok(record) = serde_json::from_slice::<DialbackRecord>(payload) else {
            return 0;
        };
        let bound = record.prober_did == author_did
            && record.requester_did == local_did
            && record.at_ms <= now_ms.saturating_add(DIALBACK_FUTURE_SKEW_MS)
            && now_ms.saturating_sub(record.at_ms) <= DIALBACK_MAX_AGE_MS
            && record.verify_signature().is_ok();
        if !bound {
            return 0;
        }
        let mut delivered = 0;
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].url == record.url {
                let waiter = self.pending.swap_remove(index);
                if waiter.reply.send(Some(record.clone())).is_ok() {
                    delivered += 1;
                }
            } else {
                index += 1;
            }
        }
        delivered
    }
}

/// A local request handed to the sync loop.
pub struct DialbackRequest {
    pub url: String,
    pub reply: oneshot::Sender<Option<DialbackRecord>>,
}

static REQUEST_SENDER: Mutex<Option<mpsc::UnboundedSender<DialbackRequest>>> = Mutex::new(None);

/// Called by the sync loop at start: routes later [`request_dialback`] calls to the returned receiver.
pub fn install_request_channel() -> mpsc::UnboundedReceiver<DialbackRequest> {
    let (sender, receiver) = mpsc::unbounded_channel();
    *REQUEST_SENDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
    receiver
}

/// Asks verse peers to probe `url` and returns the signed record, or `None` on no sync loop, no answer or a full table.
pub async fn request_dialback(url: &str) -> Option<DialbackRecord> {
    // The prober signs the normalized form, so match on that.
    let normalized = RelayUrlPolicy::default().check(url).ok()?.to_string();
    let sender = REQUEST_SENDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()?;
    let (reply, answer) = oneshot::channel();
    if sender.send(DialbackRequest { url: normalized, reply }).is_err() {
        tracing::warn!("Dial-back request dropped: the sync loop is gone");
        return None;
    }
    tokio::time::timeout(REQUEST_TIMEOUT, answer).await.ok()?.ok()?
}

/// Builds the sealed `reach_probe` frame for `url` on `verse_id`.
pub fn build_request_frame(
    secret: &iroh::SecretKey,
    verse_id: &str,
    url: &str,
) -> Result<Vec<u8>, &'static str> {
    let payload = serde_json::to_vec(&ReachProbeRequest { url: url.to_owned() })
        .map_err(|_| "request is not serializable")?;
    gossip_auth::seal(secret, GossipFrameKind::ReachProbe, verse_id, unix_now_ms(), payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialback_prober::{DialbackClock, DialbackResolver};
    use std::net::IpAddr;
    use std::sync::atomic::AtomicU64;

    const VERSE: &str = "verse-1";
    const NOW_MS: u64 = 1_800_000_000_000;

    struct SigningProber(SigningKey);

    #[async_trait::async_trait]
    impl ReachProber for SigningProber {
        async fn probe(&self, requester_did: &str, url: &str) -> Result<DialbackRecord, ProbeRefusal> {
            Ok(DialbackRecord::sign(&self.0, requester_did, url, NOW_MS, true))
        }
    }

    struct PrivateResolver;

    #[async_trait::async_trait]
    impl DialbackResolver for PrivateResolver {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            Ok(vec!["10.0.0.1".parse().unwrap()])
        }
    }

    struct FixedClock(Arc<AtomicU64>);

    impl DialbackClock for FixedClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn did_of(secret: &iroh::SecretKey) -> String {
        fe_identity::did_key::did_key_from_public_key_bytes(secret.public().as_bytes()).unwrap()
    }

    fn request_payload(url: &str) -> Vec<u8> {
        serde_json::to_vec(&ReachProbeRequest { url: url.to_owned() }).unwrap()
    }

    // Journey: undeclared and sub-Viewer requesters are refused; a Viewer gets a signed record the
    // requester side accepts; a stranger's or replayed-for-someone-else result is ignored; the prober's
    // per-requester rate limit refuses the second probe in a window.
    #[tokio::test]
    async fn dialback_requests_are_authorized_signed_and_rate_limited() {
        assert_eq!(
            DialbackResponder::authorize(false, RoleLevel::Owner),
            Err(RequestRefusal::Undeclared)
        );
        assert_eq!(
            DialbackResponder::authorize(true, RoleLevel::None),
            Err(RequestRefusal::BelowViewer)
        );
        assert_eq!(DialbackResponder::authorize(true, RoleLevel::Viewer), Ok(()));

        let responder_secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let requester_secret = iroh::SecretKey::from_bytes(&[9u8; 32]);
        let (responder_did, requester_did) = (did_of(&responder_secret), did_of(&requester_secret));
        let url = "https://relay.example.org/";

        let responder = DialbackResponder::with_prober(
            responder_secret.clone(),
            Arc::new(SigningProber(SigningKey::from_bytes(&responder_secret.to_bytes()))),
        );
        let request = request_payload(url);
        let sealed = responder.answer(VERSE, &requester_did, &request).await.expect("answered");
        let frame = gossip_auth::SignedGossipFrame::decode(&sealed).expect("decodes");
        assert_eq!(frame.kind, GossipFrameKind::ReachResult);
        gossip_auth::verify_author(&frame).expect("frame signed by the responder");
        assert_eq!(frame.author_did, responder_did);

        let (reply, mut answer) = oneshot::channel();
        let mut requester = DialbackRequester::default();
        assert!(requester.register(url.to_owned(), reply));
        assert_eq!(requester.accept_result("did:key:someone-else", &frame.author_did, &frame.payload, NOW_MS), 0);
        assert!(answer.try_recv().is_err(), "a result for another requester delivers nothing");
        assert_eq!(requester.accept_result(&requester_did, &frame.author_did, &frame.payload, NOW_MS), 1);
        let record = answer.try_recv().expect("delivered").expect("record");
        assert!(record.ok && record.requester_did == requester_did);

        assert_eq!(
            responder.answer(VERSE, &requester_did, b"{\"nope\":1}").await,
            Err(RequestRefusal::MalformedPayload)
        );

        // Real prober with a private-address resolver: the first call is charged then refused by address
        // class, the second from the same requester hits the rate limit.
        let clock = Arc::new(AtomicU64::new(NOW_MS));
        let limited = DialbackResponder::with_prober(
            responder_secret.clone(),
            Arc::new(DialbackProber::with_edges(
                SigningKey::from_bytes(&responder_secret.to_bytes()),
                Box::new(PrivateResolver),
                Box::new(FixedClock(clock)),
            )),
        );
        assert!(matches!(
            limited.answer(VERSE, &requester_did, &request).await,
            Err(RequestRefusal::Probe(ProbeRefusal::ForbiddenAddress(_)))
        ));
        assert_eq!(
            limited.answer(VERSE, &requester_did, &request).await,
            Err(RequestRefusal::Probe(ProbeRefusal::RateLimitedRequester))
        );
    }
}
