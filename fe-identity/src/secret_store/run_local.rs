//! Run-local persistent secret storage; see ../AGENTS.md QA harness security.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::RwLock;

use super::{SecretStore, SecretStoreError};

/// One-process secret store persisted owner-only under an explicit run directory.
pub struct RunLocalBackend {
    path: PathBuf,
    access: RwLock<()>,
}

impl RunLocalBackend {
    /// Create a backend whose only persistent file is inside `root`.
    pub fn new(root: PathBuf) -> Result<Self, SecretStoreError> {
        if !root.is_absolute() {
            return Err(SecretStoreError::Backend(
                "run-local secret root must be absolute".into(),
            ));
        }
        let path = root.join("secrets.json");
        let _parents = crate::file_fallback::pin_secret_parents(&path).map_err(backend_error)?;
        Ok(Self {
            path: root
                .canonicalize()
                .map_err(backend_error)?
                .join("secrets.json"),
            access: RwLock::new(()),
        })
    }

    fn key(service: &str, account: &str) -> String {
        format!(
            "{}:{}",
            hex::encode(service.as_bytes()),
            hex::encode(account.as_bytes())
        )
    }

    fn load(&self) -> Result<BTreeMap<String, String>, SecretStoreError> {
        let _parents =
            crate::file_fallback::pin_secret_parents(&self.path).map_err(backend_error)?;
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_symlink() {
                return Err(backend_error("secret store is a symlink"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(backend_error("secret store is a reparse point"));
                }
            }
        }
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(backend_error),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(backend_error(error)),
        }
    }

    fn save(&self, values: &BTreeMap<String, String>) -> Result<(), SecretStoreError> {
        let bytes = serde_json::to_vec(values).map_err(backend_error)?;
        crate::file_fallback::replace_owner_only_file(&self.path, &bytes).map_err(backend_error)
    }
}

fn backend_error(error: impl std::fmt::Display) -> SecretStoreError {
    SecretStoreError::Backend(error.to_string())
}

impl SecretStore for RunLocalBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        let _access = self.access.read().map_err(backend_error)?;
        Ok(self.load()?.get(&Self::key(service, account)).cloned())
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError> {
        let _access = self.access.write().map_err(backend_error)?;
        let mut values = self.load()?;
        values.insert(Self::key(service, account), value.to_owned());
        self.save(&values)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        let _access = self.access.write().map_err(backend_error)?;
        let mut values = self.load()?;
        if values.remove(&Self::key(service, account)).is_some() {
            self.save(&values)?;
        }
        Ok(())
    }
}
