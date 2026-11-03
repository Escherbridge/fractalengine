//! Detached Manager authority certificates; see src/AGENTS.md section join-delegation.

use ed25519_dalek::Signature;
use fe_identity::NodeKeypair;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::join_invite::{bounded_identifier, canonical_did, JoinManifest, MAX_LIFETIME_SECONDS};

pub const MAX_DELEGATION_DEPTH: usize = 8;
pub const MAX_DELEGATION_BYTES: usize = 16 * 1024;
const SIGNING_DOMAIN: &[u8] = b"FractalEngine detached Manager delegation v1\0";
const ID_DOMAIN: &[u8] = b"FractalEngine detached Manager delegation identity v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerseIdentity {
    pub verse_id: String,
    pub owner_did: String,
    pub created_at: String,
    pub namespace_id: String,
}

impl VerseIdentity {
    pub fn from_manifest(manifest: &JoinManifest) -> Self {
        Self {
            verse_id: manifest.verse_id.clone(),
            owner_did: manifest.created_by.clone(),
            created_at: manifest.created_at.clone(),
            namespace_id: manifest.namespace_id.clone(),
        }
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        bounded_identifier(&self.verse_id)?;
        canonical_did(&self.owner_did)?;
        anyhow::ensure!(
            self.created_at.len() <= 64
                && chrono::DateTime::parse_from_rfc3339(&self.created_at).is_ok()
                && self.namespace_id.len() == 64
                && self
                    .namespace_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "Invalid delegation Verse identity"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerDelegation {
    version: u8,
    nonce: String,
    issuer_did: String,
    audience_did: String,
    scope: String,
    grant_hlc: u64,
    issued_at: u64,
    expires_at: u64,
    identity: VerseIdentity,
    parent_id: Option<String>,
    signature: String,
}

impl ManagerDelegation {
    fn canonical_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = SIGNING_DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(&(
            self.version,
            &self.nonce,
            &self.issuer_did,
            &self.audience_did,
            &self.scope,
            "manager",
            self.grant_hlc,
            self.issued_at,
            self.expires_at,
            &self.identity,
            &self.parent_id,
        ))?);
        Ok(bytes)
    }

    fn content_id(&self) -> anyhow::Result<String> {
        let mut hash = blake3::Hasher::new();
        hash.update(ID_DOMAIN);
        hash.update(&self.canonical_bytes()?);
        hash.update(self.signature.as_bytes());
        Ok(hash.finalize().to_hex().to_string())
    }

    fn verify(&self, identity: &VerseIdentity, now: u64) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "Unsupported delegation version");
        anyhow::ensure!(
            self.signature.len() == 128,
            "Invalid delegation signature length"
        );
        identity.validate()?;
        anyhow::ensure!(
            &self.identity == identity,
            "Delegation Verse identity changed"
        );
        canonical_did(&self.issuer_did)?;
        canonical_did(&self.audience_did)?;
        anyhow::ensure!(
            self.issuer_did != self.audience_did && self.audience_did != identity.owner_did,
            "Delegation cannot assign Owner or self authority"
        );
        validate_scope(&self.scope, identity)?;
        anyhow::ensure!(
            self.grant_hlc > 0
                && self.nonce.len() == 26
                && ulid::Ulid::from_string(&self.nonce)
                    .is_ok_and(|nonce| nonce.to_string() == self.nonce),
            "Delegation lacks a canonical committed grant identity"
        );
        anyhow::ensure!(
            self.issued_at <= now.saturating_add(30)
                && now < self.expires_at
                && self
                    .expires_at
                    .checked_sub(self.issued_at)
                    .is_some_and(|lifetime| lifetime > 0 && lifetime <= MAX_LIFETIME_SECONDS),
            "Delegation has expired or its lifetime is invalid"
        );
        let bytes = hex::decode(&self.signature)
            .map_err(|_| anyhow::anyhow!("Invalid delegation signature"))?;
        anyhow::ensure!(
            hex::encode(&bytes) == self.signature,
            "Noncanonical delegation signature"
        );
        let signature = Signature::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid delegation signature"))?;
        fe_identity::public_key_from_did_key(&self.issuer_did)
            .map_err(|_| anyhow::anyhow!("Invalid delegation issuer"))?
            .verify_strict(&self.canonical_bytes()?, &signature)
            .map_err(|_| anyhow::anyhow!("Delegation signature verification failed"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DelegationChain(Vec<ManagerDelegation>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportingGrant {
    pub scope: String,
    pub peer_did: String,
    pub grantor_did: String,
    pub hlc: u64,
}

/// Signature proof only; the database must separately fence every current grant.
#[derive(Debug, Clone)]
pub struct VerifiedDelegationChain(DelegationChain);

impl VerifiedDelegationChain {
    pub fn chain(&self) -> &DelegationChain {
        &self.0
    }
    pub fn proof_id(&self) -> anyhow::Result<String> {
        self.last().content_id()
    }
    pub fn holder(&self) -> &str {
        &self.last().audience_did
    }
    pub fn scope(&self) -> &str {
        &self.last().scope
    }
    pub fn identity(&self) -> &VerseIdentity {
        &self.last().identity
    }
    pub fn expires_at(&self) -> u64 {
        self.last().expires_at
    }
    pub fn issued_at(&self) -> u64 {
        self.last().issued_at
    }
    pub fn supporting_grants(&self) -> impl Iterator<Item = SupportingGrant> + '_ {
        self.0 .0.iter().map(|certificate| SupportingGrant {
            scope: certificate.scope.clone(),
            peer_did: certificate.audience_did.clone(),
            grantor_did: certificate.issuer_did.clone(),
            hlc: certificate.grant_hlc,
        })
    }
    fn last(&self) -> &ManagerDelegation {
        self.0
             .0
            .last()
            .expect("verified nonempty delegation chain")
    }
}

impl DelegationChain {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn matches_recipient_grant(
        &self,
        parent: Option<&Self>,
        issuer: &str,
        scope: &str,
        hlc: u64,
        issued_at: u64,
        expires_at: u64,
    ) -> bool {
        self.0.last().is_some_and(|last| {
            last.issuer_did == issuer
                && last.scope == scope
                && last.grant_hlc == hlc
                && last.issued_at == issued_at
                && last.expires_at == expires_at
                && self.0[..self.0.len() - 1] == parent.map_or(&[][..], |chain| &chain.0[..])
        })
    }

    pub fn verify(
        &self,
        holder: &str,
        identity: &VerseIdentity,
        required_scope: &str,
        now: u64,
    ) -> anyhow::Result<VerifiedDelegationChain> {
        anyhow::ensure!(
            !self.0.is_empty() && self.0.len() <= MAX_DELEGATION_DEPTH,
            "Delegation depth exceeds its bound"
        );
        canonical_did(holder)?;
        validate_scope(required_scope, identity)?;
        let mut issuer = identity.owner_did.as_str();
        let mut parent: Option<&ManagerDelegation> = None;
        let mut seen = BTreeSet::from([issuer]);
        for certificate in &self.0 {
            certificate.verify(identity, now)?;
            anyhow::ensure!(
                certificate.issuer_did == issuer,
                "Delegation does not follow its Owner root"
            );
            anyhow::ensure!(
                seen.insert(certificate.audience_did.as_str()),
                "Delegation contains an authority cycle"
            );
            if let Some(parent) = parent {
                anyhow::ensure!(
                    certificate.parent_id.as_ref() == Some(&parent.content_id()?)
                        && crate::scope_contains(&parent.scope, &certificate.scope)
                        && certificate.issued_at >= parent.issued_at
                        && certificate.expires_at <= parent.expires_at,
                    "Delegation exceeds its parent authority"
                );
            } else {
                anyhow::ensure!(
                    certificate.parent_id.is_none(),
                    "Root delegation has an unexpected parent"
                );
            }
            issuer = &certificate.audience_did;
            parent = Some(certificate);
        }
        let last = parent.expect("bounded nonempty delegation chain");
        anyhow::ensure!(
            issuer == holder && crate::scope_contains(&last.scope, required_scope),
            "Delegation does not authorize this issuer and scope"
        );
        anyhow::ensure!(
            serde_json::to_vec(self)?.len() <= MAX_DELEGATION_BYTES,
            "Delegation proof exceeds its size limit"
        );
        Ok(VerifiedDelegationChain(self.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        signer: &NodeKeypair,
        audience_did: String,
        scope: String,
        grant_hlc: u64,
        identity: VerseIdentity,
        now: u64,
        expires_at: u64,
        parent: Option<&VerifiedDelegationChain>,
    ) -> anyhow::Result<VerifiedDelegationChain> {
        identity.validate()?;
        canonical_did(&audience_did)?;
        validate_scope(&scope, &identity)?;
        let issuer = signer.to_did_key();
        let mut certificates = if let Some(parent) = parent {
            parent.chain().verify(&issuer, &identity, &scope, now)?;
            parent.0 .0.clone()
        } else {
            anyhow::ensure!(
                issuer == identity.owner_did,
                "Delegation requires an Owner-rooted parent proof"
            );
            Vec::new()
        };
        anyhow::ensure!(
            certificates.len() < MAX_DELEGATION_DEPTH,
            "Delegation depth exhausted; request Owner reauthorization"
        );
        let mut certificate = ManagerDelegation {
            version: 1,
            nonce: ulid::Ulid::new().to_string(),
            issuer_did: issuer,
            audience_did,
            scope,
            grant_hlc,
            issued_at: now,
            expires_at,
            identity: identity.clone(),
            parent_id: parent.map(VerifiedDelegationChain::proof_id).transpose()?,
            signature: String::new(),
        };
        certificate.signature =
            hex::encode(signer.sign(&certificate.canonical_bytes()?).to_bytes());
        let holder = certificate.audience_did.clone();
        let scope = certificate.scope.clone();
        certificates.push(certificate);
        Self(certificates).verify(&holder, &identity, &scope, now)
    }
}

pub(crate) fn validate_scope(scope: &str, identity: &VerseIdentity) -> anyhow::Result<()> {
    anyhow::ensure!(scope.len() <= 512, "Invalid delegation scope");
    let parts =
        crate::parse_scope(scope).map_err(|_| anyhow::anyhow!("Invalid delegation scope"))?;
    for id in std::iter::once(&parts.verse_id)
        .chain(parts.fractal_id.iter())
        .chain(parts.petal_id.iter())
    {
        bounded_identifier(id)?;
    }
    anyhow::ensure!(
        parts.verse_id == identity.verse_id
            && scope
                == crate::build_scope(
                    &parts.verse_id,
                    parts.fractal_id.as_deref(),
                    parts.petal_id.as_deref()
                ),
        "Delegation scope does not match its Verse"
    );
    Ok(())
}
