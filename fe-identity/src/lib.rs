pub mod api_token;
pub mod did_key;
pub mod jwt;
pub mod keychain;
pub mod keypair;
pub mod resource;
pub mod rotation_provisioning;
pub mod secret_store;
pub mod x25519;

pub use api_token::{mint_api_token, verify_api_token, ApiClaims};
pub use did_key::{did_key_from_public_key, pub_key_to_did_key, public_key_from_did_key};
pub use jwt::FractalClaims;
pub use keychain::load_or_generate_keypair;
pub use keypair::NodeKeypair;
pub use resource::NodeIdentity;
pub use rotation_provisioning::{
    load_or_generate_keypair_tracked, provision_successor_for_planned_rotation,
    retire_predecessor_local_use, KeyProvisioningOutcome,
};
#[cfg(feature = "keyring")]
pub use secret_store::OsKeystoreBackend;
pub use secret_store::{EnvBackend, InMemoryBackend, SecretStore, SecretStoreError};
pub use x25519::{
    delete_device_keypair, did_key_from_x25519_public_key, load_device_keypair,
    load_or_generate_device_keypair, store_device_keypair, x25519_public_key_from_did_key,
    DeviceKeypair,
};
