//! TCP + rustls connector for host dial-outs (websocket dial, MQTT); plaintext only to loopback.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use super::address::{literal_refusal, resolve_checked};

/// Connect and TLS handshake bound.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A byte stream that is either plain TCP (loopback) or TLS.
pub(super) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// The boxed stream every dial-out speaks over.
pub(super) type Stream = Box<dyn Io>;

/// Where a dial-out connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) tls: bool,
    /// `ws`/`wss` (else `mqtt`/`mqtts`).
    pub(super) websocket: bool,
    /// Without IPv6 brackets.
    pub(super) host: String,
    pub(super) port: u16,
}

/// Exactly `127.0.0.1`, `::1` or `localhost` (manifest-v2.md §egress "Loopback"); callers pass the host unbracketed.
pub(crate) fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

/// Parse a declared `wss`/`ws`/`mqtts`/`mqtt` URL; plaintext beyond loopback and non-public IP literals are refused here too.
pub(super) fn target(url: &str) -> Result<Target, String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("bad endpoint url: {error}"))?;
    let (tls, websocket, default_port) = match parsed.scheme() {
        "wss" => (true, true, 443),
        "ws" => (false, true, 80),
        "mqtts" => (true, false, 8883),
        "mqtt" => (false, false, 1883),
        other => return Err(format!("unsupported scheme {other}")),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("endpoint urls carry no credentials".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("endpoint urls carry no query or fragment".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "endpoint url has no host".to_string())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if !tls && !is_loopback(&host) {
        return Err(format!("plaintext to {host} is refused (loopback only)"));
    }
    if let Some(reason) = literal_refusal(&host) {
        return Err(reason);
    }
    Ok(Target { tls, websocket, host, port: parsed.port().unwrap_or(default_port) })
}

/// Open the stream: resolve once and check every address (net/AGENTS.md §addresses), TCP to exactly those, then TLS
/// (webpki roots, ring provider, verified against the declared name) unless the target is plaintext loopback.
pub(super) async fn connect(target: &Target) -> Result<Stream, String> {
    let addrs = tokio::time::timeout(CONNECT_TIMEOUT, resolve_checked(&target.host, target.port))
        .await
        .map_err(|_| format!("resolving {} timed out", target.host))??;
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addrs.as_slice()))
        .await
        .map_err(|_| format!("connecting to {} timed out", target.host))?
        .map_err(|error| format!("connecting to {} failed: {error}", target.host))?;
    let _ = tcp.set_nodelay(true);
    if !target.tls {
        return Ok(Box::new(tcp));
    }
    let name = ServerName::try_from(target.host.clone()).map_err(|_| format!("{} is not a valid server name", target.host))?;
    let tls = tokio::time::timeout(CONNECT_TIMEOUT, TlsConnector::from(tls_config()?).connect(name, tcp))
        .await
        .map_err(|_| format!("TLS handshake with {} timed out", target.host))?
        .map_err(|error| format!("TLS handshake with {} failed: {error}", target.host))?;
    Ok(Box::new(tls))
}

/// One client config per process; the provider is explicit because the dependency graph enables both ring and aws-lc.
fn tls_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map(|builder| Arc::new(builder.with_root_certificates(roots).with_no_client_auth()))
                .map_err(|error| format!("TLS setup failed: {error}"))
        })
        .clone()
}
