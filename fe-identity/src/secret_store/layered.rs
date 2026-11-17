//! Layered secret backend combining read-first environment overrides with a
//! durable run-local owner-only file fallback ([`RunLocalBackend`]).

use std::path::PathBuf;

use super::{EnvBackend, RunLocalBackend, SecretStore, SecretStoreError};

/// Environment-variable precedence layered over durable file-backed secret storage.
///
/// Reads check `FE_SECRET_{SERVICE}_{ACCOUNT}` (and any in-memory override)
/// via [`EnvBackend`] first, falling back to [`RunLocalBackend`] on disk.
/// Runtime writes (`set`) persist durably to disk first and update the
/// in-memory layer; deletes remove from both so deleted secrets stay absent.
pub struct LayeredBackend {
    env: EnvBackend,
    file: RunLocalBackend,
}

impl LayeredBackend {
    /// Open or initialize the durable secret directory under `root` and layer
    /// [`EnvBackend`] read precedence on top.
    ///
    /// Creates `root` if missing before initializing [`RunLocalBackend`].
    pub fn new(root: PathBuf) -> Result<Self, SecretStoreError> {
        std::fs::create_dir_all(&root).map_err(|e| {
            SecretStoreError::Backend(format!(
                "create secret store directory {}: {e}",
                root.display()
            ))
        })?;
        let file = RunLocalBackend::new(root)?;
        Ok(Self {
            env: EnvBackend::new(),
            file,
        })
    }
}

impl SecretStore for LayeredBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        if let Some(value) = self.env.get(service, account)? {
            return Ok(Some(value));
        }
        self.file.get(service, account)
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.file.set(service, account, value)?;
        self.env.set(service, account, value)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        self.file.delete(service, account)?;
        self.env.delete(service, account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDirGuard(PathBuf);

    impl TempDirGuard {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "fe_layered_secret_{label}_{}_{}",
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn layered_backend_persists_runtime_writes_across_reopen() {
        let dir = TempDirGuard::new("persist");
        {
            let store = LayeredBackend::new(dir.0.join("secrets")).expect("init layered backend");
            assert_eq!(store.get("fractalengine", "ns_v1").unwrap(), None);
            store
                .set("fractalengine", "ns_v1", "hex-secret-material")
                .expect("write secret");
            assert_eq!(
                store.get("fractalengine", "ns_v1").unwrap(),
                Some("hex-secret-material".to_string())
            );
        }

        // Simulate process restart with empty in-memory EnvBackend overrides.
        let reopened = LayeredBackend::new(dir.0.join("secrets")).expect("reopen layered backend");
        assert_eq!(
            reopened.get("fractalengine", "ns_v1").unwrap(),
            Some("hex-secret-material".to_string())
        );

        reopened.delete("fractalengine", "ns_v1").expect("delete secret");
        assert_eq!(reopened.get("fractalengine", "ns_v1").unwrap(), None);

        let reopened_after_delete =
            LayeredBackend::new(dir.0.join("secrets")).expect("reopen after delete");
        assert_eq!(
            reopened_after_delete.get("fractalengine", "ns_v1").unwrap(),
            None
        );
    }

    #[test]
    fn layered_backend_env_var_takes_precedence_over_file() {
        let dir = TempDirGuard::new("env_prec");
        let service = format!("LAYER_{}", std::process::id());
        let account = "node_key";
        let env_name = EnvBackend::env_var_name(&service, account);

        let store = LayeredBackend::new(dir.0.clone()).expect("init layered backend");
        store.set(&service, account, "from_file").unwrap();
        std::env::set_var(&env_name, "from_env");

        let reopened = LayeredBackend::new(dir.0.clone()).expect("reopen layered backend");
        assert_eq!(
            reopened.get(&service, account).unwrap(),
            Some("from_env".to_string())
        );

        std::env::remove_var(&env_name);
        assert_eq!(
            reopened.get(&service, account).unwrap(),
            Some("from_file".to_string())
        );
    }
}
