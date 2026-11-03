//! Owner-authenticated addressed join envelopes; see src/AGENTS.md section join-invitations.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::Signature;
use fe_identity::NodeKeypair;
use serde::{Deserialize, Serialize};

pub const JOIN_PREFIX: &str = "fractalengine://join/v1/";
pub const MAX_INVITE_BYTES: usize = 64 * 1024;
pub const MAX_TICKET_BYTES: usize = 32 * 1024;
pub const MAX_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;
const SIGNING_DOMAIN: &[u8] = b"FractalEngine addressed join invitation v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRole {
    Viewer,
    Editor,
    Manager,
}

impl JoinRole {
    pub fn parse(role: &str) -> anyhow::Result<Self> {
        match role {
            "viewer" => Ok(Self::Viewer),
            "editor" => Ok(Self::Editor),
            "manager" => Ok(Self::Manager),
            _ => anyhow::bail!("Unsupported invitation role"),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Manager => "manager",
        }
    }

    pub fn requires_write(self) -> bool {
        self != Self::Viewer
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinManifest {
    pub verse_id: String,
    pub name: String,
    pub created_by: String,
    pub created_at: String,
    pub namespace_id: String,
    pub default_access: String,
    pub ts_mode: String,
    pub ts_replication_factor: i64,
    pub ts_bucket_width_ms: i64,
}

impl JoinManifest {
    pub fn from_verse(verse: &crate::schema::Verse) -> anyhow::Result<Self> {
        let manifest = Self {
            verse_id: verse.verse_id.clone(),
            name: verse.name.clone(),
            created_by: verse.created_by.clone(),
            created_at: verse.created_at.clone(),
            namespace_id: verse.namespace_id.clone().unwrap_or_default(),
            default_access: verse.default_access.clone(),
            ts_mode: verse.ts_mode.clone(),
            ts_replication_factor: verse.ts_replication_factor,
            ts_bucket_width_ms: verse.ts_bucket_width_ms,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> anyhow::Result<()> {
        bounded_identifier(&self.verse_id)?;
        canonical_did(&self.created_by)?;
        anyhow::ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 1024
                && !self.name.chars().any(char::is_control),
            "Invalid invitation Verse name"
        );
        anyhow::ensure!(
            self.created_at.len() <= 64
                && chrono::DateTime::parse_from_rfc3339(&self.created_at).is_ok(),
            "Invalid invitation creation identity"
        );
        anyhow::ensure!(
            self.namespace_id.len() == 64
                && self
                    .namespace_id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid invitation namespace"
        );
        anyhow::ensure!(
            matches!(self.default_access.as_str(), "viewer" | "none")
                && matches!(self.ts_mode.as_str(), "mirror" | "sharded" | "balanced")
                && (1..=1024).contains(&self.ts_replication_factor)
                && self.ts_bucket_width_ms > 0,
            "Invalid invitation manifest settings"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinInvitation {
    version: u8,
    nonce: String,
    issuer_did: String,
    recipient_did: String,
    scope: String,
    role: JoinRole,
    grant_hlc: u64,
    issued_at: u64,
    expires_at: u64,
    manifest: JoinManifest,
    ticket: String,
    signature: String,
}

impl std::fmt::Debug for JoinInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinInvitation")
            .field("version", &self.version)
            .field("nonce", &self.nonce)
            .field("scope", &self.scope)
            .field("role", &self.role)
            .field("credential", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// A signature-checked envelope; native ticket admission remains a separate fence.
#[derive(Debug, Clone)]
pub struct VerifiedJoinInvitation(JoinInvitation, crate::join_ticket::NativeJoinTicket);

impl VerifiedJoinInvitation {
    pub fn native_ticket(&self) -> &crate::join_ticket::NativeJoinTicket {
        &self.1
    }
    pub fn manifest(&self) -> &JoinManifest {
        &self.0.manifest
    }
    pub fn nonce(&self) -> &str {
        &self.0.nonce
    }
    pub fn scope(&self) -> &str {
        &self.0.scope
    }
    pub fn role(&self) -> JoinRole {
        self.0.role
    }
    pub fn grant_hlc(&self) -> u64 {
        self.0.grant_hlc
    }
    pub(crate) fn signature(&self) -> &str {
        &self.0.signature
    }
    pub fn issuer_did(&self) -> &str {
        &self.0.issuer_did
    }
    pub fn recipient_did(&self) -> &str {
        &self.0.recipient_did
    }
    pub fn ticket(&self) -> &str {
        &self.0.ticket
    }
    pub fn expires_at(&self) -> u64 {
        self.0.expires_at
    }

    pub fn revalidate(&self, recipient_did: &str, now: u64) -> anyhow::Result<()> {
        self.0.verify(recipient_did, now)
    }
}

impl JoinInvitation {
    #[allow(clippy::too_many_arguments)]
    pub fn issue_owner(
        keypair: &NodeKeypair,
        recipient_did: String,
        scope: String,
        role: JoinRole,
        grant_hlc: u64,
        manifest: JoinManifest,
        ticket: String,
        now: u64,
        lifetime_seconds: u64,
    ) -> anyhow::Result<Self> {
        let expires_at = now
            .checked_add(lifetime_seconds)
            .ok_or_else(|| anyhow::anyhow!("Invitation expiry is out of range"))?;
        let mut invitation = Self {
            version: 1,
            nonce: ulid::Ulid::new().to_string(),
            issuer_did: keypair.to_did_key(),
            recipient_did,
            scope,
            role,
            grant_hlc,
            issued_at: now,
            expires_at,
            manifest,
            ticket,
            signature: String::new(),
        };
        invitation.validate(&invitation.recipient_did, now)?;
        crate::join_ticket::NativeJoinTicket::parse(
            &invitation.ticket,
            &invitation.manifest.namespace_id,
            invitation.role,
            &invitation.issuer_did,
        )?;
        invitation.signature = hex::encode(keypair.sign(&invitation.canonical_bytes()?).to_bytes());
        invitation.to_link()?;
        Ok(invitation)
    }

    pub fn to_link(&self) -> anyhow::Result<String> {
        let link = format!(
            "{JOIN_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        );
        anyhow::ensure!(
            link.len() <= MAX_INVITE_BYTES,
            "Invitation exceeds its size limit"
        );
        Ok(link)
    }

    pub fn parse_owner(
        link: &str,
        recipient_did: &str,
        now: u64,
    ) -> anyhow::Result<VerifiedJoinInvitation> {
        anyhow::ensure!(
            link.len() <= MAX_INVITE_BYTES,
            "Invitation exceeds its size limit"
        );
        let encoded = link.strip_prefix(JOIN_PREFIX).ok_or_else(|| {
            anyhow::anyhow!("Unsupported invitation version; request a new addressed invitation")
        })?;
        let raw = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| anyhow::anyhow!("Invalid invitation encoding"))?;
        anyhow::ensure!(
            URL_SAFE_NO_PAD.encode(&raw) == encoded,
            "Noncanonical invitation encoding"
        );
        let invitation: Self = serde_json::from_slice(&raw)
            .map_err(|_| anyhow::anyhow!("Invalid invitation envelope"))?;
        invitation.verify(recipient_did, now)?;
        let native = crate::join_ticket::NativeJoinTicket::parse(
            &invitation.ticket,
            &invitation.manifest.namespace_id,
            invitation.role,
            &invitation.issuer_did,
        )?;
        Ok(VerifiedJoinInvitation(invitation, native))
    }

    fn validate(&self, recipient_did: &str, now: u64) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "Unsupported invitation version");
        canonical_did(&self.issuer_did)?;
        canonical_did(&self.recipient_did)?;
        anyhow::ensure!(
            self.recipient_did == recipient_did,
            "Invitation is addressed to another peer"
        );
        anyhow::ensure!(
            self.recipient_did != self.issuer_did,
            "Invitation recipient is already the Owner"
        );
        anyhow::ensure!(
            self.grant_hlc > 0,
            "Invitation lacks a committed role statement"
        );
        anyhow::ensure!(
            self.nonce.len() == 26
                && ulid::Ulid::from_string(&self.nonce).is_ok_and(|v| v.to_string() == self.nonce),
            "Invalid invitation nonce"
        );
        self.manifest.validate()?;
        anyhow::ensure!(
            self.issuer_did == self.manifest.created_by,
            "Fresh invitation requires the pinned Verse Owner"
        );
        let parts = crate::parse_scope(&self.scope)
            .map_err(|_| anyhow::anyhow!("Invalid invitation scope"))?;
        for id in std::iter::once(&parts.verse_id)
            .chain(parts.fractal_id.iter())
            .chain(parts.petal_id.iter())
        {
            bounded_identifier(id)?;
        }
        anyhow::ensure!(
            parts.verse_id == self.manifest.verse_id
                && self.scope
                    == crate::build_scope(
                        &parts.verse_id,
                        parts.fractal_id.as_deref(),
                        parts.petal_id.as_deref()
                    ),
            "Invitation scope does not match its Verse"
        );
        anyhow::ensure!(
            self.issued_at <= now.saturating_add(30)
                && now < self.expires_at
                && self
                    .expires_at
                    .checked_sub(self.issued_at)
                    .is_some_and(|lifetime| lifetime > 0 && lifetime <= MAX_LIFETIME_SECONDS),
            "Invitation has expired or its lifetime is invalid"
        );
        anyhow::ensure!(
            !self.ticket.is_empty()
                && self.ticket.len() <= MAX_TICKET_BYTES
                && self.ticket.starts_with("doc")
                && self.ticket.is_ascii()
                && !self
                    .ticket
                    .bytes()
                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control()),
            "Invalid invitation ticket encoding"
        );
        Ok(())
    }

    fn verify(&self, recipient_did: &str, now: u64) -> anyhow::Result<()> {
        self.validate(recipient_did, now)?;
        let key = fe_identity::public_key_from_did_key(&self.issuer_did)
            .map_err(|_| anyhow::anyhow!("Invalid invitation issuer"))?;
        let bytes = hex::decode(&self.signature)
            .map_err(|_| anyhow::anyhow!("Invalid invitation signature"))?;
        anyhow::ensure!(
            hex::encode(&bytes) == self.signature,
            "Noncanonical invitation signature"
        );
        let signature = Signature::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid invitation signature"))?;
        key.verify_strict(&self.canonical_bytes()?, &signature)
            .map_err(|_| anyhow::anyhow!("Invitation signature verification failed"))
    }

    fn canonical_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = SIGNING_DOMAIN.to_vec();
        // Fixed-order tuple encoding keeps every signed field unambiguous.
        bytes.extend(serde_json::to_vec(&(
            self.version,
            &self.nonce,
            &self.issuer_did,
            &self.recipient_did,
            &self.scope,
            self.role,
            self.grant_hlc,
            self.issued_at,
            self.expires_at,
            &self.manifest,
            &self.ticket,
        ))?);
        Ok(bytes)
    }
}

pub(crate) fn canonical_did(did: &str) -> anyhow::Result<()> {
    anyhow::ensure!(did.len() <= 128, "Invalid invitation peer identity");
    let key = fe_identity::public_key_from_did_key(did)
        .map_err(|_| anyhow::anyhow!("Invalid invitation peer identity"))?;
    anyhow::ensure!(
        fe_identity::pub_key_to_did_key(&key) == did,
        "Noncanonical invitation peer identity"
    );
    Ok(())
}

pub(crate) fn bounded_identifier(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && !id
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '#' || c == '/'),
        "Invalid invitation hierarchy identity"
    );
    Ok(())
}
