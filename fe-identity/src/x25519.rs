//! Independent X25519 device key for the SPEC-3 §10 recipient-device scope-key wrap
//! (rationale: AGENTS.md §x25519-device-key).

use std::sync::Arc;

use multibase::Base;
use rand::rngs::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::keychain::SERVICE;
use crate::secret_store::SecretStore;

/// X25519 did:key multicodec prefix, distinct from Ed25519's `0xed01`.
const X25519_MULTICODEC_PREFIX: [u8; 2] = [0xec, 0x01];

/// A device's X25519 static key pair, generated independently of the node's Ed25519
/// identity key (see AGENTS.md §x25519-device-key for why).
pub struct DeviceKeypair {
    secret: StaticSecret,
}

impl DeviceKeypair {
    /// Generate a fresh X25519 device key from OS randomness.
    pub fn generate() -> Self {
        Self {
            secret: StaticSecret::random_from_rng(OsRng),
        }
    }

    /// Reconstruct a device key from a persisted 32-byte secret.
    pub fn from_bytes(secret_bytes: [u8; 32]) -> Self {
        Self {
            secret: StaticSecret::from(secret_bytes),
        }
    }

    /// The raw 32-byte static secret, for persistence via [`SecretStore`].
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// The 32-byte X25519 public key.
    pub fn public_key(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// This device key's `did:key` identifier (multicodec `0xec01`).
    pub fn to_did_key(&self) -> String {
        did_key_from_x25519_public_key(&self.public_key())
    }
}

/// Convert a raw 32-byte X25519 public key to did:key format.
///
/// Format: `did:key:z` + base58btc(0xec01 || raw_pub_key_bytes). The `0xec01` multicodec
/// prefix identifies X25519 public keys, distinct from Ed25519's `0xed01`
/// (see [`crate::did_key::did_key_from_public_key`]).
pub fn did_key_from_x25519_public_key(pub_key: &[u8; 32]) -> String {
    let mut multicodec = Vec::with_capacity(34);
    multicodec.extend_from_slice(&X25519_MULTICODEC_PREFIX);
    multicodec.extend_from_slice(pub_key);
    let encoded = multibase::encode(Base::Base58Btc, &multicodec);
    format!("did:key:{}", encoded)
}

/// Parse an X25519 did:key string back to a raw 32-byte public key.
pub fn x25519_public_key_from_did_key(did: &str) -> anyhow::Result<[u8; 32]> {
    let encoded = did
        .strip_prefix("did:key:")
        .ok_or_else(|| anyhow::anyhow!("invalid did:key: must start with 'did:key:'"))?;

    let (base, bytes) = multibase::decode(encoded)
        .map_err(|e| anyhow::anyhow!("multibase decode failed: {}", e))?;

    if base != Base::Base58Btc {
        anyhow::bail!("did:key must use base58btc encoding (z prefix)");
    }

    if bytes.len() < 2
        || bytes[0] != X25519_MULTICODEC_PREFIX[0]
        || bytes[1] != X25519_MULTICODEC_PREFIX[1]
    {
        anyhow::bail!("invalid multicodec prefix: expected 0xec 0x01 for X25519");
    }

    let key_bytes: [u8; 32] = bytes[2..].try_into().map_err(|_| {
        anyhow::anyhow!(
            "invalid key length: expected 32 bytes, got {}",
            bytes.len() - 2
        )
    })?;

    Ok(key_bytes)
}

/// Store a device key's 32-byte secret as hex in the given secret store.
///
/// Uses the same `SERVICE` namespace as `keychain.rs`'s Ed25519 identity-key persistence;
/// `account` must be distinct from any identity-key account to avoid colliding in the
/// `(service, account)`-keyed store.
pub fn store_device_keypair(
    store: &Arc<dyn SecretStore>,
    account: &str,
    device_keypair: &DeviceKeypair,
) -> anyhow::Result<()> {
    let hex = hex::encode(device_keypair.secret_bytes());
    store
        .set(SERVICE, account, &hex)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Load a device key from the secret store by reading the hex-encoded secret.
pub fn load_device_keypair(
    store: &Arc<dyn SecretStore>,
    account: &str,
) -> anyhow::Result<DeviceKeypair> {
    let hex = store
        .get(SERVICE, account)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("no device key found for {account}"))?;
    let bytes = hex::decode(&hex)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid device key length"))?;
    Ok(DeviceKeypair::from_bytes(arr))
}

/// Load a device key from the secret store, or generate a new one and persist it.
pub fn load_or_generate_device_keypair(
    store: &Arc<dyn SecretStore>,
    account: &str,
) -> anyhow::Result<DeviceKeypair> {
    match store.get(SERVICE, account) {
        Ok(Some(secret_hex)) => {
            let secret_bytes = hex::decode(&secret_hex)
                .map_err(|e| anyhow::anyhow!("invalid device key hex in secret store: {e}"))?;
            let secret_array: [u8; 32] = secret_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("secret store device key is not 32 bytes"))?;
            Ok(DeviceKeypair::from_bytes(secret_array))
        }
        Ok(None) => {
            let device_keypair = DeviceKeypair::generate();
            let secret_hex = hex::encode(device_keypair.secret_bytes());
            store
                .set(SERVICE, account, &secret_hex)
                .map_err(|e| anyhow::anyhow!("secret store set failed: {e}"))?;
            Ok(device_keypair)
        }
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}

/// Delete a device key from the secret store.
pub fn delete_device_keypair(store: &Arc<dyn SecretStore>, account: &str) -> anyhow::Result<()> {
    store
        .delete(SERVICE, account)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::did_key::did_key_from_public_key;
    use crate::keypair::NodeKeypair;
    use crate::secret_store::InMemoryBackend;

    #[test]
    fn generated_device_key_round_trips_through_secret_store() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let device_keypair = DeviceKeypair::generate();
        store_device_keypair(&store, "device-1", &device_keypair).unwrap();
        let loaded = load_device_keypair(&store, "device-1").unwrap();
        assert_eq!(device_keypair.secret_bytes(), loaded.secret_bytes());
        assert_eq!(device_keypair.public_key(), loaded.public_key());
    }

    #[test]
    fn load_or_generate_device_keypair_persists_on_first_call() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let first = load_or_generate_device_keypair(&store, "device-1").unwrap();
        let second = load_or_generate_device_keypair(&store, "device-1").unwrap();
        assert_eq!(first.secret_bytes(), second.secret_bytes());
    }

    #[test]
    fn delete_device_keypair_removes_entry() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let device_keypair = DeviceKeypair::generate();
        store_device_keypair(&store, "device-1", &device_keypair).unwrap();
        delete_device_keypair(&store, "device-1").unwrap();
        assert!(load_device_keypair(&store, "device-1").is_err());
    }

    #[test]
    fn x25519_did_key_round_trips() {
        let device_keypair = DeviceKeypair::generate();
        let did = device_keypair.to_did_key();
        let recovered = x25519_public_key_from_did_key(&did).unwrap();
        assert_eq!(device_keypair.public_key(), recovered);
    }

    #[test]
    fn x25519_did_key_is_distinguishable_from_ed25519_did_key() {
        let device_keypair = DeviceKeypair::generate();
        let x25519_did = device_keypair.to_did_key();

        let node_keypair = NodeKeypair::generate();
        let ed25519_did = did_key_from_public_key(&node_keypair.verifying_key());

        assert_ne!(x25519_did, ed25519_did);
        // The two multicodec spaces must not accept each other's did:key strings.
        assert!(crate::did_key::public_key_from_did_key(&x25519_did).is_err());
        assert!(x25519_public_key_from_did_key(&ed25519_did).is_err());
    }

    #[test]
    fn wrong_multicodec_prefix_rejected() {
        // Ed25519's 0xed01 prefix must not be accepted by the X25519 decoder.
        let mut fake_bytes = vec![0xed_u8, 0x01];
        fake_bytes.extend_from_slice(&[0u8; 32]);
        let encoded = multibase::encode(Base::Base58Btc, &fake_bytes);
        let did = format!("did:key:{}", encoded);
        assert!(x25519_public_key_from_did_key(&did).is_err());
    }
}
