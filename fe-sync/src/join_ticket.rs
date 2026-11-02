//! Native ticket admission for addressed invites; see src/AGENTS.md section join-tickets.

use std::{collections::BTreeSet, time::Instant};

use fe_database::join_invite::{JoinRole, VerifiedJoinInvitation, MAX_TICKET_BYTES};
use iroh::TransportAddr;
use iroh_docs::{ticket::DocTicket, Capability};

const MAX_PEERS: usize = 8;
const MAX_ADDRESSES: usize = 16;

/// Parsed native capability with bounded, authenticated bootstrap identity.
#[derive(Clone)]
pub struct NativeJoinTicket {
    encoded: String,
    parsed: DocTicket,
}

impl std::fmt::Debug for NativeJoinTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeJoinTicket")
            .field(
                "namespace_id",
                &hex::encode(self.parsed.capability.id().as_bytes()),
            )
            .field("peer_count", &self.parsed.nodes.len())
            .field("credential", &"[redacted]")
            .finish()
    }
}

impl NativeJoinTicket {
    pub fn parse(
        encoded: &str,
        namespace_id: &str,
        role: JoinRole,
        issuer_did: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            encoded.len() <= MAX_TICKET_BYTES,
            "Document ticket exceeds its size limit"
        );
        let parsed: DocTicket = encoded
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid native document ticket"))?;
        anyhow::ensure!(
            parsed.to_string() == encoded,
            "Noncanonical document ticket"
        );
        anyhow::ensure!(
            hex::encode(parsed.capability.id().as_bytes()) == namespace_id,
            "Document ticket does not match the invitation namespace"
        );
        anyhow::ensure!(
            matches!(
                (&parsed.capability, role),
                (Capability::Read(_), JoinRole::Viewer)
                    | (Capability::Write(_), JoinRole::Editor | JoinRole::Manager)
            ),
            "Document ticket does not match the invited role"
        );
        parsed
            .capability
            .id()
            .into_public_key()
            .map_err(|_| anyhow::anyhow!("Invalid document namespace key"))?;
        let issuer = fe_identity::public_key_from_did_key(issuer_did)
            .map_err(|_| anyhow::anyhow!("Invalid invitation transport issuer"))?;
        anyhow::ensure!(
            !parsed.nodes.is_empty()
                && parsed.nodes.len() <= MAX_PEERS
                && parsed
                    .nodes
                    .iter()
                    .any(|node| node.id.as_bytes() == &issuer.to_bytes()),
            "Document ticket lacks the invitation issuer bootstrap"
        );
        let mut ids = BTreeSet::new();
        for node in &parsed.nodes {
            anyhow::ensure!(ids.insert(node.id), "Document ticket repeats a peer");
            anyhow::ensure!(
                !node.addrs.is_empty() && node.addrs.len() <= MAX_ADDRESSES,
                "Document ticket lacks bounded bootstrap addresses"
            );
            for address in &node.addrs {
                match address {
                    TransportAddr::Ip(address) => anyhow::ensure!(
                        address.port() != 0
                            && !address.ip().is_unspecified()
                            && !address.ip().is_multicast(),
                        "Invalid document peer address"
                    ),
                    TransportAddr::Relay(url) => anyhow::ensure!(
                        url.as_str().len() <= 2048
                            && matches!(url.scheme(), "http" | "https")
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none(),
                        "Invalid document relay address"
                    ),
                    _ => anyhow::bail!("Unsupported document transport address"),
                }
            }
        }
        Ok(Self {
            encoded: encoded.into(),
            parsed,
        })
    }

    pub fn for_invitation(
        invitation: &VerifiedJoinInvitation,
        recipient_did: &str,
        now: u64,
    ) -> anyhow::Result<Self> {
        invitation.revalidate(recipient_did, now)?;
        Self::parse(
            invitation.ticket(),
            &invitation.manifest().namespace_id,
            invitation.role(),
            invitation.issuer_did(),
        )
    }

    pub fn encoded(&self) -> &str {
        &self.encoded
    }
    pub fn peers(&self) -> &[iroh::EndpointAddr] {
        &self.parsed.nodes
    }
    pub(crate) fn capability(&self) -> Capability {
        self.parsed.capability.clone()
    }
}

#[derive(Debug, Clone)]
pub struct ExportJoinTicket {
    pub request_id: String,
    pub verse_id: String,
    pub namespace_id: String,
    pub issuer_did: String,
    pub role: JoinRole,
    pub deadline: Instant,
}

#[derive(Debug, Clone)]
pub struct JoinTicketExported {
    pub request_id: String,
    pub ticket: Result<NativeJoinTicket, &'static str>,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaRequest {
    pub request_id: String,
    pub invitation: VerifiedJoinInvitation,
    pub deadline: Instant,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaOpened {
    pub request_id: String,
    pub outcome: Result<(), &'static str>,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaReadiness {
    pub request_id: String,
    pub outcome: Result<bool, &'static str>,
}
