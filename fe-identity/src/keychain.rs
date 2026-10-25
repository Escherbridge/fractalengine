use std::sync::Arc;

use crate::keypair::NodeKeypair;
use crate::secret_store::SecretStore;

/// Secret-store service name for all fe-identity key material (rationale + which
/// modules share it: AGENTS.md §secret-store-service).
pub(crate) const SERVICE: &str = "fractalengine";

/// Store a node keypair's 32-byte seed as hex in the given secret store.
pub fn store_keypair(
    store: &Arc<dyn SecretStore>,
    node_id: &str,
    keypair: &NodeKeypair,
) -> anyhow::Result<()> {
    let hex = hex::encode(keypair.seed_bytes());
    store
        .set(SERVICE, node_id, &hex)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Load a node keypair from the secret store by reading the hex-encoded seed.
pub fn load_keypair(store: &Arc<dyn SecretStore>, node_id: &str) -> anyhow::Result<NodeKeypair> {
    let hex = store
        .get(SERVICE, node_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("no keypair found for {node_id}"))?;
    let bytes = hex::decode(&hex)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid key length"))?;
    NodeKeypair::from_bytes(&arr)
}

/// Load a node keypair from the secret store, or generate a new one and persist it.
///
/// Used by both the GUI and relay binaries to ensure a stable node identity
/// across launches. The 32-byte seed is stored as a 64-char hex string.
pub fn load_or_generate_keypair(
    store: &Arc<dyn SecretStore>,
    account: &str,
) -> anyhow::Result<NodeKeypair> {
    match store.get(SERVICE, account) {
        Ok(Some(seed_hex)) => {
            let seed_bytes = hex::decode(&seed_hex)
                .map_err(|e| anyhow::anyhow!("invalid keypair hex in secret store: {e}"))?;
            let seed_array: [u8; 32] = seed_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("secret store keypair seed is not 32 bytes"))?;
            NodeKeypair::from_bytes(&seed_array)
        }
        Ok(None) => {
            let kp = NodeKeypair::generate();
            let seed_hex = hex::encode(kp.seed_bytes());
            store
                .set(SERVICE, account, &seed_hex)
                .map_err(|e| anyhow::anyhow!("secret store set failed: {e}"))?;
            Ok(kp)
        }
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}

/// Delete a node keypair from the secret store.
pub fn delete_keypair(store: &Arc<dyn SecretStore>, node_id: &str) -> anyhow::Result<()> {
    store
        .delete(SERVICE, node_id)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Where a durably-loaded node identity actually came from — the GUI shows a
/// startup banner when this is [`KeypairSource::Ephemeral`] (see
/// fe-identity/src/AGENTS.md §identity-tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypairSource {
    /// Read from (or freshly generated and written to) the OS keystore.
    Keystore,
    /// The OS keystore was unavailable; read from (or written to) the
    /// durable file-tier fallback instead.
    FileFallback,
    /// Both durable tiers failed; this identity exists only for this
    /// process and will NOT survive a restart.
    Ephemeral,
}

/// A loaded node keypair plus which tier it came from.
pub struct TieredKeypair {
    pub keypair: NodeKeypair,
    pub source: KeypairSource,
}

/// Load (or create) a node keypair across three durability tiers: the OS
/// keystore, then a file under `fallback_dir`, then — only if both of those
/// fail — an in-process ephemeral key. Unlike [`load_or_generate_keypair`]
/// this never returns `Err`: the ephemeral tier always succeeds, so the
/// caller instead inspects [`TieredKeypair::source`] to decide whether to
/// warn the user (fe-identity/src/AGENTS.md §identity-tiers).
///
/// `Ok(None)` from the keystore (first run) keeps the existing
/// generate-then-persist behavior unchanged — it never touches the file
/// tier. Only a keystore `Err` (a read failure, OR a write failure during
/// first-run persistence) falls through to the file tier; a file-tier
/// success attempts to heal the identity back into the keystore so a later
/// launch, once the keystore recovers, prefers it again.
pub fn load_or_generate_keypair_durable(
    store: &Arc<dyn SecretStore>,
    account: &str,
    fallback_dir: &std::path::Path,
) -> TieredKeypair {
    match load_or_generate_keypair(store, account) {
        Ok(keypair) => TieredKeypair {
            keypair,
            source: KeypairSource::Keystore,
        },
        Err(keystore_error) => {
            tracing::warn!(
                account,
                error = %keystore_error,
                "keystore unavailable, trying the file-tier identity fallback"
            );
            match crate::file_fallback::load_or_generate(fallback_dir, account) {
                Ok(keypair) => {
                    // Heal-back: best-effort, never fatal — staying on the
                    // file tier is fine if the keystore is still down.
                    match store_keypair(store, account, &keypair) {
                        Ok(()) => tracing::info!(
                            account,
                            "healed the identity back into the OS keystore"
                        ),
                        Err(heal_error) => tracing::debug!(
                            account,
                            error = %heal_error,
                            "heal-back to the OS keystore failed, staying on the file tier"
                        ),
                    }
                    TieredKeypair {
                        keypair,
                        source: KeypairSource::FileFallback,
                    }
                }
                Err(file_error) => {
                    tracing::error!(
                        account,
                        keystore_error = %keystore_error,
                        file_error = %file_error,
                        "both identity tiers failed — generating an EPHEMERAL identity; \
                         existing content may appear read-only"
                    );
                    TieredKeypair {
                        keypair: NodeKeypair::generate(),
                        source: KeypairSource::Ephemeral,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::InMemoryBackend;

    #[test]
    fn roundtrip_store_load_keypair() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let kp = NodeKeypair::generate();
        store_keypair(&store, "test-node", &kp).unwrap();
        let loaded = load_keypair(&store, "test-node").unwrap();
        assert_eq!(kp.seed_bytes(), loaded.seed_bytes());
    }

    #[test]
    fn load_missing_returns_error() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        assert!(load_keypair(&store, "missing").is_err());
    }

    /// A24 (M4 fix M3): the relay's share signer survives a restart ONLY via
    /// the operator-exported env slot. Two fresh `EnvBackend`s (= two process
    /// lifetimes) load the SAME key from `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`;
    /// without it, each lifetime gets a different ephemeral key.
    #[test]
    fn env_backend_share_signer_round_trips_across_restarts() {
        use crate::secret_store::EnvBackend;
        const SLOT: &str = "FE_SECRET_FRACTALENGINE_SHARE_SIGNER";
        let seed = NodeKeypair::generate();
        std::env::set_var(SLOT, hex::encode(seed.seed_bytes()));
        let boot = || {
            let store: Arc<dyn SecretStore> = Arc::new(EnvBackend::new());
            load_or_generate_keypair(&store, "share_signer").unwrap()
        };
        let (first, second) = (boot(), boot());
        std::env::remove_var(SLOT);
        assert_eq!(first.verifying_key(), seed.verifying_key());
        assert_eq!(second.verifying_key(), first.verifying_key());

        let (eph_a, eph_b) = (boot(), boot());
        assert_ne!(
            eph_a.verifying_key(),
            eph_b.verifying_key(),
            "unset slot = ephemeral key per process lifetime"
        );
    }

    #[test]
    fn delete_keypair_removes() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let kp = NodeKeypair::generate();
        store_keypair(&store, "test-node", &kp).unwrap();
        delete_keypair(&store, "test-node").unwrap();
        assert!(load_keypair(&store, "test-node").is_err());
    }

    /// A `SecretStore` whose `get`/`set` always error — simulates an
    /// unavailable OS keystore (e.g. the Windows Credential Manager, locked
    /// Secret Service, etc.) for the file-tier fallback tests.
    struct AlwaysFailingStore;
    impl SecretStore for AlwaysFailingStore {
        fn get(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<String>, crate::secret_store::SecretStoreError> {
            Err(crate::secret_store::SecretStoreError::Backend(
                "simulated keystore outage".to_string(),
            ))
        }
        fn set(
            &self,
            _service: &str,
            _account: &str,
            _value: &str,
        ) -> Result<(), crate::secret_store::SecretStoreError> {
            Err(crate::secret_store::SecretStoreError::Backend(
                "simulated keystore outage".to_string(),
            ))
        }
        fn delete(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<(), crate::secret_store::SecretStoreError> {
            Ok(())
        }
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fe-identity-keychain-test-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create tempdir");
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Identity durability (F6): when the OS keystore errors on every call,
    /// two simulated process launches against the SAME file-tier fallback
    /// directory must still converge on the same DID — the whole point of
    /// having a durable tier between the keystore and an ephemeral identity.
    #[test]
    fn file_tier_fallback_yields_the_same_did_across_two_launches_when_keystore_errors() {
        let dir = TempDir::new("durable");
        let store: Arc<dyn SecretStore> = Arc::new(AlwaysFailingStore);

        let first = load_or_generate_keypair_durable(&store, "node_keypair", &dir.0);
        assert_eq!(first.source, KeypairSource::FileFallback);

        let second = load_or_generate_keypair_durable(&store, "node_keypair", &dir.0);
        assert_eq!(second.source, KeypairSource::FileFallback);

        assert_eq!(
            first.keypair.to_did_key(),
            second.keypair.to_did_key(),
            "both launches must resolve to the same identity via the file tier"
        );
    }

    /// Only when BOTH tiers are unavailable does the identity become
    /// ephemeral (and therefore different across launches).
    #[test]
    fn ephemeral_tier_only_fires_when_both_durable_tiers_are_unavailable() {
        // A directory that can never be created (a file masquerading as a
        // parent dir) makes the file tier fail too.
        let base = TempDir::new("blocked-parent");
        let blocking_file = base.0.join("blocks-the-subdir");
        std::fs::write(&blocking_file, b"not a directory").unwrap();
        let unusable_fallback_dir = blocking_file.join("identity");

        let store: Arc<dyn SecretStore> = Arc::new(AlwaysFailingStore);
        let first = load_or_generate_keypair_durable(&store, "node_keypair", &unusable_fallback_dir);
        let second =
            load_or_generate_keypair_durable(&store, "node_keypair", &unusable_fallback_dir);
        assert_eq!(first.source, KeypairSource::Ephemeral);
        assert_eq!(second.source, KeypairSource::Ephemeral);
        assert_ne!(
            first.keypair.to_did_key(),
            second.keypair.to_did_key(),
            "ephemeral identities are per-process, never durable"
        );
    }
}
