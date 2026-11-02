//! Local invitation coordination values; see src/AGENTS.md section addressed-join.

use crate::entity_access::EntityAccessTarget;

#[derive(Clone)]
pub struct JoinCredential(String);

impl JoinCredential {
    pub fn new(value: String) -> anyhow::Result<Self> {
        anyhow::ensure!(
            value.len() <= 64 * 1024,
            "Invitation exceeds its size limit"
        );
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for JoinCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JoinCredential([redacted])")
    }
}

#[derive(Debug, Clone)]
pub struct InviteSource {
    pub target: EntityAccessTarget,
    pub recipient_did: String,
    pub role: String,
    pub expiry_hours: u32,
    pub manifest: serde_json::Value,
    pub prepared_at: u64,
    pub nonce: String,
}

#[derive(Debug, Clone)]
pub struct IssuedInvitation {
    pub source: InviteSource,
    pub credential: JoinCredential,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct JoinAdmission {
    pub nonce: String,
    pub verse_id: String,
    pub verse_name: String,
    pub scope: String,
    pub role: String,
}
