//! Durable file-tier keypair storage — the fallback tier between the OS
//! keystore and an ephemeral identity when the keystore errors (design:
//! fe-identity/src/AGENTS.md §identity-tiers).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::keypair::NodeKeypair;

fn key_file_path(dir: &Path, account: &str) -> PathBuf {
    dir.join(format!("{account}.key"))
}

/// Load the seed from `<dir>/<account>.key`, or generate and securely persist
/// a new one if the file does not exist yet. Any OTHER I/O error (permission
/// denied, a corrupt/truncated file, a lost creation race) is returned —
/// this tier is meant to be durable, so a failure must be visible to the
/// caller (which falls through to the loud-ephemeral path) rather than
/// silently minting or exposing key material.
pub fn load_or_generate(dir: &Path, account: &str) -> anyhow::Result<NodeKeypair> {
    let path = key_file_path(dir, account);
    match std::fs::read_to_string(&path) {
        Ok(hex_str) => {
            warn_if_permissions_loosened(&path);
            let bytes = hex::decode(hex_str.trim())
                .map_err(|e| anyhow::anyhow!("corrupt file-tier key at {}: {e}", path.display()))?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| {
                anyhow::anyhow!("file-tier key at {} is not 32 bytes", path.display())
            })?;
            NodeKeypair::from_bytes(&arr)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let kp = NodeKeypair::generate();
            create_key_file_securely(&path, &kp)?;
            Ok(kp)
        }
        Err(e) => Err(anyhow::anyhow!(
            "could not read file-tier key at {}: {e}",
            path.display()
        )),
    }
}

/// Create `path` holding the seed hex as its only content, with permissions
/// restricted AT creation — never a separate `chmod` after the fact, so
/// there is no window where the key sits world-readable. `create_new` makes
/// this atomic: a concurrent first-run race (two launches racing to create
/// the same file) loses with an `AlreadyExists` error instead of silently
/// reopening or truncating whatever the winner wrote.
#[cfg(unix)]
fn create_key_file_securely(path: &Path, kp: &NodeKeypair) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(hex::encode(kp.seed_bytes()).as_bytes())?;
    Ok(())
}

/// Windows has no POSIX "other" principal to leak to — the app-data
/// directory a caller passes in already carries a per-user NTFS ACL
/// inherited from the user profile, so no extra ACL tightening is
/// attempted here (ACL APIs are also unavailable through `std`). Creation
/// is still atomic (`create_new`) and every I/O error propagates — a failed
/// create is a hard failure here too, never a silent reopen.
#[cfg(not(unix))]
fn create_key_file_securely(path: &Path, kp: &NodeKeypair) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(hex::encode(kp.seed_bytes()).as_bytes())?;
    Ok(())
}

/// On load, warn (never fail — this is advisory) if something outside this
/// module has loosened the key file's mode past owner-only.
#[cfg(unix)]
fn warn_if_permissions_loosened(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            tracing::warn!(
                path = %path.display(),
                mode = format!("{mode:o}"),
                "file-tier identity key has loosened permissions (expected 0600)"
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_permissions_loosened(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same seed across two simulated process launches — the durability
    /// property this tier exists for.
    #[test]
    fn same_account_round_trips_the_same_seed_across_two_launches() {
        let dir = tempdir();
        let first = load_or_generate(dir.path(), "node_keypair").expect("first launch");
        let second = load_or_generate(dir.path(), "node_keypair").expect("second launch");
        assert_eq!(first.seed_bytes(), second.seed_bytes());
    }

    #[test]
    fn different_accounts_get_different_files_and_seeds() {
        let dir = tempdir();
        let a = load_or_generate(dir.path(), "node_keypair").expect("a");
        let b = load_or_generate(dir.path(), "share_signer").expect("b");
        assert_ne!(a.seed_bytes(), b.seed_bytes());
    }

    #[test]
    fn a_corrupt_key_file_errors_instead_of_silently_minting_a_new_identity() {
        let dir = tempdir();
        std::fs::write(dir.path().join("node_keypair.key"), "not-hex!!").expect("write corrupt");
        assert!(load_or_generate(dir.path(), "node_keypair").is_err());
    }

    /// Security: a lost creation race must be refused, never a silent
    /// reopen/truncate of the winner's file — `create_new`'s atomicity is
    /// the whole point of creating-with-permissions in one syscall instead
    /// of write-then-chmod.
    #[test]
    fn create_new_refuses_to_reopen_an_existing_key_file() {
        let dir = tempdir();
        let path = dir.path().join("node_keypair.key");
        let first = NodeKeypair::generate();
        create_key_file_securely(&path, &first).expect("first create succeeds");
        let before = std::fs::read_to_string(&path).expect("read back");

        let second = NodeKeypair::generate();
        let result = create_key_file_securely(&path, &second);
        assert!(
            result.is_err(),
            "a second create_new on an existing file must be refused, not silently applied"
        );

        let after = std::fs::read_to_string(&path).expect("read back after the refused attempt");
        assert_eq!(
            before, after,
            "the original key file content must be untouched by the refused attempt"
        );
    }

    /// Security: permissions are correct from the moment the file exists —
    /// there must be no window where a freshly created key is world/group
    /// readable.
    #[cfg(unix)]
    #[test]
    fn created_key_file_has_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        let kp = NodeKeypair::generate();
        let path = dir.path().join("node_keypair.key");
        create_key_file_securely(&path, &kp).expect("create");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be created owner-only (0600)");
    }

    /// Minimal temp-dir helper (no `tempfile` dependency in this crate):
    /// a process-unique subdirectory under `std::env::temp_dir()`, cleaned
    /// up on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "fe-identity-file-fallback-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        TempDir(dir)
    }
}
