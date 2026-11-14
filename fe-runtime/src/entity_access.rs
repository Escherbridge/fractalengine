//! Target-bound local hierarchy access requests; see src/AGENTS.md.

use crate::messages::{ApiTokenInfo, EntityType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityAccessTarget {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub scope: String,
}

#[derive(Debug, Clone)]
pub enum EntityAccessAction {
    Load {
        peer_dids: Vec<String>,
    },
    Assign {
        peer_did: String,
        role: String,
    },
    Revoke {
        peer_did: String,
    },
    GenerateInvite {
        role: String,
        expiry_hours: u32,
    },
    MintToken {
        max_role: String,
        ttl_hours: u32,
        client_did: Option<String>,
    },
    RevokeToken {
        jti: String,
        sub: String,
        scope: String,
    },
}

#[derive(Clone)]
pub struct EntityAccessSnapshot {
    pub local_did: String,
    pub local_role: String,
    pub roles: Vec<(String, String)>,
    pub tokens: Vec<ApiTokenInfo>,
    pub tokens_truncated: bool,
    pub notice: Option<String>,
    pub invite_link: Option<String>,
    pub minted: Option<EntityMintedToken>,
    /// Admitted role statements at exactly this scope, newest first, at most 256.
    pub grants: Vec<EntityGrantRecord>,
}

/// One current (latest) role statement per `(scope, peer, grantor)` key; not a full op log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityGrantRecord {
    pub peer_did: String,
    pub grantor_did: String,
    pub role: String,
    pub revoked: bool,
    pub hlc: u64,
}

#[derive(Clone)]
pub struct EntityMintedToken {
    pub token: String,
    pub client: String,
    pub expires_at: String,
}

impl std::fmt::Debug for EntityMintedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntityMintedToken")
            .field("token", &"[redacted]")
            .field("client", &self.client)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl std::fmt::Debug for EntityAccessSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntityAccessSnapshot")
            .field("local_role", &self.local_role)
            .field("roles", &self.roles)
            .field("token_count", &self.tokens.len())
            .field(
                "invite_link",
                &self.invite_link.as_ref().map(|_| "[redacted]"),
            )
            .field("minted", &self.minted)
            .field("grant_count", &self.grants.len())
            .finish_non_exhaustive()
    }
}
