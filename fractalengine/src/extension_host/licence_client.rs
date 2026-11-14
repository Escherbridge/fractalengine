//! Licence token requests behind the store's "Check licence": one fe-hexon `EntitlementClient` per source, verified
//! against the pinned issuers, and the token filed where governance row 6 reads it (src/AGENTS.md §plugin-host "Licence tokens").

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use fe_format::entitlement::{EntitlementVerifier, KeyringVerifier};
use fe_hexon::entitlement_client::{
    EntitlementClient, FileTokenStore as ClientTokenStore, Licensee, TokenRequestError, TokenStore, TokenTarget, TokenTransport,
};

use super::governance::{load_issuer_keyring, FileTokenStore as GovernanceTokenStore, ENTITLEMENT_ISSUERS_FILE, ENTITLEMENT_TOKEN_DIR};
use super::ops_drain::{LicenceAnswer, LicenceRequest};

/// Subdirectory of the governance config dir holding the client's per-(licensee, package) renewal records.
const CLIENT_TOKEN_DIR: &str = "entitlement_client";

/// A source's client and the transport it was built over.
struct SourceClient {
    transport: Arc<dyn TokenTransport>,
    client: Arc<EntitlementClient>,
}

/// Requests, verifies and files licence tokens for this host's licensee.
pub(crate) struct LicenceTokens {
    verifier: Arc<dyn EntitlementVerifier>,
    /// At least one issuer is pinned; with none, no request is sent.
    pinned: bool,
    client_store: Arc<dyn TokenStore>,
    /// The only store row 6 reads.
    governance_store: GovernanceTokenStore,
    /// One client per source id, kept so a `429` backoff survives between clicks.
    clients: Mutex<HashMap<String, SourceClient>>,
}

impl LicenceTokens {
    /// Pinned issuers from `entitlement_issuers.toml` and both token stores under `config_dir`; an invalid issuers file is an error.
    pub(crate) fn new(config_dir: &Path) -> Result<Self, String> {
        let keyring = load_issuer_keyring(&config_dir.join(ENTITLEMENT_ISSUERS_FILE))?;
        let pinned = !keyring.is_empty();
        Ok(Self {
            verifier: Arc::new(KeyringVerifier::new(keyring)),
            pinned,
            client_store: Arc::new(ClientTokenStore::new(config_dir.join(CLIENT_TOKEN_DIR))),
            governance_store: GovernanceTokenStore::new(config_dir.join(ENTITLEMENT_TOKEN_DIR)),
            clients: Mutex::new(HashMap::new()),
        })
    }

    /// The source's client; rebuilt when the source set handed out a different transport (a reloaded source).
    fn client_for(&self, source_id: &str, transport: &Arc<dyn TokenTransport>) -> Arc<EntitlementClient> {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = clients.get(source_id) {
            if std::ptr::addr_eq(Arc::as_ptr(&known.transport), Arc::as_ptr(transport)) {
                return known.client.clone();
            }
        }
        let client = Arc::new(EntitlementClient::new(transport.clone(), self.verifier.clone(), self.client_store.clone()));
        clients.insert(source_id.to_owned(), SourceClient { transport: transport.clone(), client: client.clone() });
        client
    }

    /// Ask the source's issuer for a token over the listed build; a verified token is filed for row 6 and answers `Held`.
    pub(crate) fn request(&self, request: &LicenceRequest<'_>, licensee: Licensee<'_>, now: u64) -> Result<LicenceAnswer, String> {
        if !self.pinned {
            return Err("not-available: no entitlement issuer is pinned on this host".into());
        }
        let transport = request.transport.as_ref().ok_or("not-available: this source is not reached over http(s)")?;
        // Row 6 looks a token up by the lowercase hash, so the request names that form too.
        let content_hash = request.content_hash.to_ascii_lowercase();
        let target = TokenTarget {
            base: request.base,
            config: request.config.as_ref(),
            package_id: request.package_id,
            content_hash: &content_hash,
        };
        let client = self.client_for(request.source_id, transport);
        match client.token_for(&target, &licensee, now) {
            Ok(acquired) => {
                if let Some(renewal) = &acquired.renewal {
                    tracing::warn!(source = request.source_id, package_id = request.package_id, %renewal, "licence renewal failed; the held token still serves");
                }
                self.governance_store
                    .put(&content_hash, &acquired.held.token)
                    .map_err(|error| format!("token-not-stored: {error}"))?;
                Ok(LicenceAnswer::Held { expires: Some(rfc3339_utc_seconds(acquired.held.exp)) })
            }
            Err(TokenRequestError::NotEntitled) => Ok(LicenceAnswer::Missing),
            Err(TokenRequestError::NotOffered) => Err("not-available: this source offers no licences".into()),
            Err(error) => Err(format!("{}: {error}", error_code(&error))),
        }
    }
}

/// Short kebab code the store details show before the reason.
fn error_code(error: &TokenRequestError) -> &'static str {
    match error {
        TokenRequestError::NotOffered => "not-available",
        TokenRequestError::Location(_) => "refused-location",
        TokenRequestError::NotEntitled => "not-entitled",
        TokenRequestError::ReissueRefused => "reissue-refused",
        TokenRequestError::DeviceLimitReached => "device-limit-reached",
        TokenRequestError::ProofInvalid => "proof-invalid",
        TokenRequestError::UnknownPackage => "unknown-package",
        TokenRequestError::InvalidRequest => "invalid-request",
        TokenRequestError::RateLimited { .. } => "rate-limited",
        TokenRequestError::Unexpected { .. } => "unexpected-answer",
        TokenRequestError::Unreachable(_) => "unreachable",
        TokenRequestError::Request(_) => "request-unsigned",
        TokenRequestError::TokenRejected(_) => "token-rejected",
    }
}

/// RFC 3339 UTC to the second; civil date by Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html#civil_from_days>).
fn rfc3339_utc_seconds(unix_secs: u64) -> String {
    let (hour, minute, second) = ((unix_secs % 86_400) / 3_600, (unix_secs % 3_600) / 60, unix_secs % 60);
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}
