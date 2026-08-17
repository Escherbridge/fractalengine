//! Ed25519 identity-key rotation provisioning primitives for SPEC-2's self-certifying
//! rotation lifecycle (rationale: AGENTS.md §key-provisioning-tracking).

use std::sync::Arc;

use crate::keychain::{self, SERVICE};
use crate::keypair::NodeKeypair;
use crate::secret_store::SecretStore;

/// Outcome of [`load_or_generate_keypair_tracked`] — distinguishes a loaded lineage
/// continuation from a freshly minted, unrelated principal (author-key-lifecycle.md §2.3,
/// conformance case 11).
pub enum KeyProvisioningOutcome {
    /// A previously stored key was loaded from the secret store.
    Existing(NodeKeypair),
    /// No key was found for this account; a new, unrelated principal was generated and stored.
    NewPrincipal(NodeKeypair),
}

impl KeyProvisioningOutcome {
    /// The keypair, regardless of whether it was existing or newly minted.
    pub fn into_keypair(self) -> NodeKeypair {
        match self {
            KeyProvisioningOutcome::Existing(kp) => kp,
            KeyProvisioningOutcome::NewPrincipal(kp) => kp,
        }
    }

    /// True when this call minted a new, unrelated principal rather than loading an existing one.
    pub fn is_new_principal(&self) -> bool {
        matches!(self, KeyProvisioningOutcome::NewPrincipal(_))
    }
}

/// Load a keypair from the secret store, or generate a new one — tracking which happened.
///
/// Same persistence semantics as [`crate::keychain::load_or_generate_keypair`], but the caller
/// can tell a lineage continuation apart from a new principal instead of silently treating a
/// post-secret-loss regeneration as a continuation of the lost key's lineage.
pub fn load_or_generate_keypair_tracked(
    store: &Arc<dyn SecretStore>,
    account: &str,
) -> anyhow::Result<KeyProvisioningOutcome> {
    let existed_before = matches!(store.get(SERVICE, account), Ok(Some(_)));
    let keypair = keychain::load_or_generate_keypair(store, account)?;
    Ok(if existed_before {
        KeyProvisioningOutcome::Existing(keypair)
    } else {
        KeyProvisioningOutcome::NewPrincipal(keypair)
    })
}

/// Mint and store a successor keypair under a distinct account, before any rotation operation
/// is built (author-key-lifecycle.md §5.4 planned recovery, step 1). The predecessor's stored
/// secret is left untouched — retiring it is the caller's separate, explicit
/// [`retire_predecessor_local_use`] step.
pub fn provision_successor_for_planned_rotation(
    store: &Arc<dyn SecretStore>,
    predecessor_account: &str,
    successor_account: &str,
) -> anyhow::Result<NodeKeypair> {
    if predecessor_account == successor_account {
        anyhow::bail!(
            "successor account must be distinct from predecessor account {predecessor_account}"
        );
    }

    // Planned recovery only applies while the predecessor remains available locally.
    keychain::load_keypair(store, predecessor_account)
        .map_err(|e| anyhow::anyhow!("predecessor key unavailable, cannot plan a rotation: {e}"))?;

    let successor = NodeKeypair::generate();
    keychain::store_keypair(store, successor_account, &successor)?;
    Ok(successor)
}

/// Retire local use of a predecessor key: the explicit, separate step after a rotation has
/// been admitted and successor capabilities obtained (author-key-lifecycle.md §5.4, step 3).
///
/// Deleting the local secret does not erase or invalidate the predecessor's historic
/// signatures — only its local availability for signing new operations.
pub fn retire_predecessor_local_use(
    store: &Arc<dyn SecretStore>,
    predecessor_account: &str,
) -> anyhow::Result<()> {
    keychain::delete_keypair(store, predecessor_account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::did_key::did_key_from_public_key;
    use crate::secret_store::InMemoryBackend;

    #[test]
    fn tracked_load_returns_new_principal_once_then_existing() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());

        let first = load_or_generate_keypair_tracked(&store, "acct").unwrap();
        assert!(first.is_new_principal());
        let first_seed = first.into_keypair().seed_bytes();

        let second = load_or_generate_keypair_tracked(&store, "acct").unwrap();
        assert!(!second.is_new_principal());
        assert_eq!(second.into_keypair().seed_bytes(), first_seed);

        let third = load_or_generate_keypair_tracked(&store, "acct").unwrap();
        assert!(!third.is_new_principal());
    }

    #[test]
    fn new_principal_after_simulated_secret_loss_is_unrelated_to_lost_key() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());

        let original = load_or_generate_keypair_tracked(&store, "acct")
            .unwrap()
            .into_keypair();
        let original_seed = original.seed_bytes();
        let original_did = did_key_from_public_key(&original.verifying_key());

        // Simulate local secret loss: the stored entry disappears.
        keychain::delete_keypair(&store, "acct").unwrap();

        let recovered = load_or_generate_keypair_tracked(&store, "acct").unwrap();
        assert!(
            recovered.is_new_principal(),
            "regeneration after secret loss must be reported as a new principal, per §2.3"
        );
        let recovered_kp = recovered.into_keypair();
        assert_ne!(
            recovered_kp.seed_bytes(),
            original_seed,
            "a new principal must never equal the lost key's seed"
        );
        assert_ne!(
            did_key_from_public_key(&recovered_kp.verifying_key()),
            original_did,
            "a new principal must never equal the lost key's DID"
        );
    }

    #[test]
    fn successor_provisioning_leaves_predecessor_readable_until_explicit_retirement() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let predecessor = NodeKeypair::generate();
        keychain::store_keypair(&store, "predecessor", &predecessor).unwrap();

        let successor =
            provision_successor_for_planned_rotation(&store, "predecessor", "successor").unwrap();

        // Predecessor's stored secret is untouched by successor provisioning.
        let still_loadable = keychain::load_keypair(&store, "predecessor").unwrap();
        assert_eq!(still_loadable.seed_bytes(), predecessor.seed_bytes());

        // Successor is stored independently under its own account.
        let loaded_successor = keychain::load_keypair(&store, "successor").unwrap();
        assert_eq!(loaded_successor.seed_bytes(), successor.seed_bytes());
        assert_ne!(loaded_successor.seed_bytes(), predecessor.seed_bytes());

        retire_predecessor_local_use(&store, "predecessor").unwrap();
        assert!(keychain::load_keypair(&store, "predecessor").is_err());

        // Retirement is scoped to the predecessor; the successor remains untouched.
        assert!(keychain::load_keypair(&store, "successor").is_ok());
    }

    #[test]
    fn provision_successor_rejects_identical_predecessor_and_successor_accounts() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        keychain::store_keypair(&store, "acct", &NodeKeypair::generate()).unwrap();
        assert!(provision_successor_for_planned_rotation(&store, "acct", "acct").is_err());
    }

    #[test]
    fn provision_successor_requires_predecessor_to_exist() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        assert!(provision_successor_for_planned_rotation(&store, "missing", "successor").is_err());
    }
}
