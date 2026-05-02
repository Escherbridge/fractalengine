//! OS keychain backend wrapping `keyring::Entry`.

use super::{SecretStore, SecretStoreError};

/// Backend that delegates to the OS credential store via the `keyring` crate.
///
/// This is the default backend for the desktop GUI binary. It stores secrets
/// in the platform-native keychain (e.g. Windows Credential Manager, macOS
/// Keychain, Linux Secret Service).
pub struct OsKeystoreBackend;

impl OsKeystoreBackend {
    /// Create a new OS keystore backend.
    pub fn new() -> Self {
        Self
    }
}

impl Default for OsKeystoreBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for OsKeystoreBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        match entry.get_password() {
            Ok(val) => Ok(Some(val)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretStoreError::Backend(e.to_string())),
        }
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        entry
            .set_password(value)
            .map_err(|e| SecretStoreError::Backend(e.to_string()))
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| SecretStoreError::Backend(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretStoreError::Backend(e.to_string())),
        }
    }
}
