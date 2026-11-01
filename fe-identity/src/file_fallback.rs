//! Durable file-tier keypair storage — the fallback tier between the OS
//! keystore and an ephemeral identity when the keystore errors (design:
//! fe-identity/src/AGENTS.md §identity-tiers).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::keypair::NodeKeypair;

/// Resolve the opt-in run-local QA storage root; malformed roots fail closed.
pub fn qa_storage_root() -> anyhow::Result<Option<PathBuf>> {
    let Some(raw) = std::env::var_os("FE_QA_STORAGE_ROOT") else {
        return Ok(None);
    };
    let root = PathBuf::from(raw);
    anyhow::ensure!(root.is_absolute(), "FE_QA_STORAGE_ROOT must be absolute");
    let _parents = pin_secret_parents(&root.join(".qa-root-probe"))?;
    Ok(Some(root.canonicalize()?))
}

/// Reject redirected ancestors; Windows handles pin each directory during writes.
pub fn pin_secret_parents(path: &Path) -> anyhow::Result<Vec<std::fs::File>> {
    let absolute = std::path::absolute(path)?;
    anyhow::ensure!(
        !absolute
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "secret paths must not contain parent traversal"
    );
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing secret parent"))?;
    let mut ancestors: Vec<_> = parent.ancestors().collect();
    ancestors.reverse();
    let mut pinned = Vec::new();
    for directory in ancestors {
        if !directory.exists() {
            std::fs::create_dir(directory)?;
        }
        let metadata = std::fs::symlink_metadata(directory)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "secret parent is a symlink"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            let handle = std::fs::OpenOptions::new()
                .access_mode(0x80)
                .share_mode(0x1)
                .custom_flags(0x0220_0000)
                .open(directory)?;
            anyhow::ensure!(
                handle.metadata()?.file_attributes() & 0x400 == 0,
                "secret parent is a reparse point"
            );
            pinned.push(handle);
        }
        #[cfg(not(windows))]
        pinned.push(std::fs::File::open(directory)?);
    }
    Ok(pinned)
}

fn key_file_path(dir: &Path, account: &str) -> PathBuf {
    dir.join(format!("{account}.key"))
}

/// Try to read a seed from `path`. `Ok(None)` means the file does not exist
/// yet (the only case callers should treat as "generate a fresh one");
/// anything else wrong with an EXISTING file (corrupt content, a real I/O
/// error) is `Err` — this tier must never silently treat a damaged file as
/// absent and mint a second, different identity on top of it.
fn try_read_seed(path: &Path) -> anyhow::Result<Option<NodeKeypair>> {
    match std::fs::read_to_string(path) {
        Ok(hex_str) => {
            warn_if_permissions_loosened(path);
            let bytes = hex::decode(hex_str.trim())
                .map_err(|e| anyhow::anyhow!("corrupt file-tier key at {}: {e}", path.display()))?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| {
                anyhow::anyhow!("file-tier key at {} is not 32 bytes", path.display())
            })?;
            Ok(Some(NodeKeypair::from_bytes(&arr)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!(
            "could not read file-tier key at {}: {e}",
            path.display()
        )),
    }
}

/// Load the seed from `<dir>/<account>.key`, or generate and securely
/// persist a new one if the file does not exist yet.
pub fn load_or_generate(dir: &Path, account: &str) -> anyhow::Result<NodeKeypair> {
    let path = key_file_path(dir, account);
    match try_read_seed(&path)? {
        Some(kp) => Ok(kp),
        None => {
            let kp = NodeKeypair::generate();
            create_key_file_securely(&path, &kp)?;
            Ok(kp)
        }
    }
}

/// Like [`load_or_generate`], but migrates a legacy key file (the
/// pre-per-user-dir, CWD-relative `data/identity/` location) into `dir`
/// first. `dir` is authoritative: if it already holds a key, the legacy
/// path is never consulted. Otherwise, a key found at `legacy_dir` is
/// re-stored securely at `dir` and the legacy file is deleted — read-only
/// at the old path (we never write there), one-shot (nothing to migrate on
/// the next launch once this succeeds). A crash between the new write and
/// the old delete leaves the legacy file in place, which is harmless: the
/// next launch finds `dir`'s file via `try_read_seed` and returns without
/// touching `legacy_dir` again; the stale copy can be cleaned up by hand.
pub fn load_or_generate_migrating(
    dir: &Path,
    legacy_dir: &Path,
    account: &str,
) -> anyhow::Result<NodeKeypair> {
    let path = key_file_path(dir, account);
    if let Some(kp) = try_read_seed(&path)? {
        return Ok(kp);
    }
    let legacy_path = key_file_path(legacy_dir, account);
    if let Some(kp) = try_read_seed(&legacy_path)? {
        create_key_file_securely(&path, &kp)?;
        match std::fs::remove_file(&legacy_path) {
            Ok(()) => tracing::info!(
                from = %legacy_path.display(),
                to = %path.display(),
                "migrated the file-tier identity to the per-user data directory"
            ),
            Err(e) => tracing::warn!(
                path = %legacy_path.display(),
                error = %e,
                "migrated identity to the new location but could not remove the legacy file"
            ),
        }
        return Ok(kp);
    }
    let kp = NodeKeypair::generate();
    create_key_file_securely(&path, &kp)?;
    Ok(kp)
}

/// Create `path` holding the seed hex as its only content, owner-only from
/// creation (see [`create_owner_only_file`]).
fn create_key_file_securely(path: &Path, kp: &NodeKeypair) -> anyhow::Result<()> {
    create_owner_only_file(path, hex::encode(kp.seed_bytes()).as_bytes())
}

/// Create `path` (which must not exist) holding `contents`, with permissions
/// restricted AT creation and before any content is written — never a
/// separate step after the fact, so there is no window where the secret sits
/// more broadly readable than intended. `create_new` makes this atomic: a
/// concurrent race loses with `AlreadyExists` instead of silently reopening
/// or truncating whatever the winner wrote. Fails closed (deletes the file,
/// returns `Err`) if owner-only permissions cannot be established — see
/// fe-identity/src/AGENTS.md §identity-tiers "owner-only files".
pub fn create_owner_only_file(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    create_owner_only_file_with(path, |file| file.write_all(contents))
}

fn create_owner_only_file_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    let _parents = pin_secret_parents(path)?;
    #[cfg(windows)]
    let mut file = windows_acl::create_owner_only(path)?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(any(unix, windows)))]
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let result = write(&mut file).map_err(anyhow::Error::from);
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Atomically replace (or create) `path` with `contents`, owner-only: the
/// bytes go to a fresh sibling via [`create_owner_only_file`], which is then
/// renamed over `path`, so a reader sees the old file or the new one, never a
/// partial or broadly-readable one. The temp file is removed on any failure.
pub fn replace_owner_only_file(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    replace_owner_only_file_with(path, |file| file.write_all(contents))
}

/// Stream a trusted source into an owner-only atomic replacement.
pub fn copy_owner_only_file(source: &Path, path: &Path) -> anyhow::Result<()> {
    let mut source = std::fs::File::open(source)?;
    replace_owner_only_file_with(path, |file| std::io::copy(&mut source, file).map(|_| ()))
}

fn replace_owner_only_file_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    let _parents = pin_secret_parents(path)?;
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "secret target is a symlink"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            anyhow::ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "secret target is a reparse point"
            );
        }
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", path.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.{nanos}.tmp", std::process::id()));
    let temp_path = path.with_file_name(temp_name);
    create_owner_only_file_with(&temp_path, write)?;
    if let Err(e) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(anyhow::anyhow!(
            "could not move the new file into place at {}: {e}",
            path.display()
        ));
    }
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

/// Minimal `advapi32` FFI for setting an owner-only, non-inheriting DACL on
/// the file-tier key file. DPAPI (`CryptProtectData`/`CryptUnprotectData`
/// via the `windows` crate) was the preferred design for at-rest encryption,
/// but it is not currently a direct dependency anywhere in this workspace
/// (only a deep transitive one via `keyring`/Tauri, with no DPAPI bindings
/// exposed) and its failure modes are decrypt-time (profile/SID binding
/// edge cases) — exactly the "identity silently stops loading" class of bug
/// this whole F6 fix exists to eliminate. An explicit, fail-closed DACL has
/// no such decrypt-time fragility and needs no new crate: these three
/// `advapi32`/`kernel32` functions have a stable Win32 ABI, so a direct
/// `extern "system"` binding avoids adding `windows`/`windows-sys` as a new
/// dependency just for this. See fe-identity/src/AGENTS.md §identity-tiers.
#[cfg(windows)]
mod windows_acl {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;

    type Bool = i32;
    type DWord = u32;

    #[repr(C)]
    struct SecurityAttributes {
        length: DWord,
        descriptor: *mut c_void,
        inherit: Bool,
    }

    /// SDDL: Protected DACL (no inheritance), one ACE granting
    /// `FILE_ALL_ACCESS` to the Owner (`OW` resolves to whoever owns the
    /// object, i.e. the creating user).
    const OWNER_ONLY_SDDL: &str = "D:P(A;;FA;;;OW)";
    const SDDL_REVISION_1: DWord = 1;
    #[cfg(test)]
    const DACL_SECURITY_INFORMATION: DWord = 0x0000_0004;
    #[cfg(test)]
    const PROTECTED_DACL_SECURITY_INFORMATION: DWord = 0x8000_0000;

    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: DWord,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> Bool;

        #[cfg(test)]
        fn SetFileSecurityW(
            file_name: *const u16,
            security_information: DWord,
            security_descriptor: *mut c_void,
        ) -> Bool;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            access: DWord,
            sharing: DWord,
            attributes: *const SecurityAttributes,
            disposition: DWord,
            flags: DWord,
            template: *mut c_void,
        ) -> *mut c_void;
        fn LocalFree(hmem: *mut c_void) -> *mut c_void;
    }

    fn wide_null(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Create a new file with its protected owner-only DACL in the creation call.
    pub(crate) fn create_owner_only(path: &Path) -> anyhow::Result<std::fs::File> {
        let sddl: Vec<u16> = OWNER_ONLY_SDDL
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let path_w = wide_null(path);
        // SAFETY: buffers and descriptor remain live through CreateFileW; File owns the returned handle.
        unsafe {
            let mut descriptor = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as DWord,
                descriptor,
                inherit: 0,
            };
            let handle = CreateFileW(
                path_w.as_ptr(),
                0x4000_0000,
                0,
                &attributes,
                1,
                0x0020_0080,
                std::ptr::null_mut(),
            );
            let error = std::io::Error::last_os_error();
            LocalFree(descriptor);
            if handle as isize == -1 {
                return Err(error.into());
            }
            Ok(std::fs::File::from_raw_handle(handle))
        }
    }

    /// Restrict `path` to an owner-only, protected (non-inheriting) DACL.
    /// Returns `Err` on ANY failure, including a failure to even build the
    /// security descriptor — the caller must treat that as a hard tier
    /// failure, never silently keep the file at whatever ACL it inherited.
    #[cfg(test)]
    pub(crate) fn restrict_to_owner(path: &Path) -> anyhow::Result<()> {
        let sddl: Vec<u16> = OWNER_ONLY_SDDL
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let path_w = wide_null(path);
        // SAFETY: `sddl`/`path_w` are NUL-terminated UTF-16 buffers kept
        // alive for the duration of each call; `sd` is only ever read by
        // `SetFileSecurityW` and freed exactly once via `LocalFree`, after
        // which it is never dereferenced again.
        unsafe {
            let mut sd: *mut c_void = std::ptr::null_mut();
            let converted = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            );
            if converted == 0 || sd.is_null() {
                return Err(anyhow::anyhow!(
                    "ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let set_ok = SetFileSecurityW(
                path_w.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                sd,
            );
            let set_err = std::io::Error::last_os_error();
            LocalFree(sd);
            if set_ok == 0 {
                return Err(anyhow::anyhow!("SetFileSecurityW failed: {set_err}"));
            }
        }
        Ok(())
    }
}

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

    /// Windows security follow-up: the seed round-trips through the
    /// protected (owner-only, non-inheriting) form, and `icacls` — an
    /// independent tool, not our own FFI — confirms the broad groups a
    /// freshly created temp file would otherwise inherit (`Everyone`,
    /// `BUILTIN\Users`) are absent.
    #[cfg(windows)]
    #[test]
    fn windows_key_file_round_trips_through_the_owner_only_acl() {
        let dir = tempdir();
        let kp = NodeKeypair::generate();
        let path = dir.path().join("node_keypair.key");
        create_key_file_securely(&path, &kp).expect("secure create succeeds");

        let loaded = try_read_seed(&path)
            .expect("read back ok")
            .expect("file present");
        assert_eq!(loaded.seed_bytes(), kp.seed_bytes());

        let replacement = NodeKeypair::generate();
        replace_owner_only_file(&path, hex::encode(replacement.seed_bytes()).as_bytes())
            .expect("replace retains owner-only creation");
        assert_eq!(
            try_read_seed(&path).unwrap().unwrap().seed_bytes(),
            replacement.seed_bytes()
        );
        assert!(create_owner_only_file(&path, b"must not overwrite").is_err());

        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = dir.path().join("redirected");
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .expect("mklink");
        assert!(
            output.status.success(),
            "junction setup failed: {:?}",
            output
        );
        assert!(create_owner_only_file(&junction.join("secret"), b"secret").is_err());
        assert!(replace_owner_only_file(&junction.join("secret"), b"secret").is_err());
        assert!(!target.join("secret").exists());
        std::fs::remove_dir(&junction).expect("unlink junction without traversing it");

        if let Ok(output) = std::process::Command::new("icacls").arg(&path).output() {
            let listing = String::from_utf8_lossy(&output.stdout);
            assert!(
                !listing.contains("Everyone") && !listing.contains("BUILTIN\\Users"),
                "key file must not carry a broad inherited ACL entry: {listing}"
            );
        }
    }

    /// Windows security follow-up: when `restrict_to_owner` itself is
    /// exercised directly, it must succeed on a file this process just
    /// created (owner always holds WRITE_DAC on their own objects).
    #[cfg(windows)]
    #[test]
    fn restrict_to_owner_succeeds_on_a_freshly_created_file() {
        let dir = tempdir();
        let path = dir.path().join("probe.key");
        std::fs::write(&path, b"probe").expect("write probe file");
        windows_acl::restrict_to_owner(&path).expect("restrict_to_owner should succeed");
    }

    /// Migration: a legacy plaintext key file at the old CWD-relative
    /// location is loaded, re-stored securely at the new per-user
    /// directory, and the legacy file is removed — "migrates it" means
    /// the legacy copy is gone, not duplicated.
    #[test]
    fn legacy_key_file_migrates_to_the_new_directory_and_is_removed() {
        let new_dir = tempdir();
        let legacy_dir = tempdir();
        let legacy_kp = NodeKeypair::generate();
        create_key_file_securely(&legacy_dir.path().join("node_keypair.key"), &legacy_kp)
            .expect("seed the legacy location");

        let migrated =
            load_or_generate_migrating(new_dir.path(), legacy_dir.path(), "node_keypair")
                .expect("migration ok");
        assert_eq!(migrated.seed_bytes(), legacy_kp.seed_bytes());
        assert!(
            new_dir.path().join("node_keypair.key").exists(),
            "the key must now live at the new, authoritative location"
        );
        assert!(
            !legacy_dir.path().join("node_keypair.key").exists(),
            "the legacy copy must be removed once migrated"
        );

        // A second launch must be stable: dir already has the key, so the
        // (now-empty) legacy dir is never consulted again.
        let second = load_or_generate_migrating(new_dir.path(), legacy_dir.path(), "node_keypair")
            .expect("second launch ok");
        assert_eq!(second.seed_bytes(), legacy_kp.seed_bytes());
    }

    /// Migration: when NEITHER location has a key yet, a fresh one is
    /// generated directly at the new, authoritative directory.
    #[test]
    fn migrating_loader_generates_fresh_at_the_new_dir_when_neither_exists() {
        let new_dir = tempdir();
        let legacy_dir = tempdir();
        let kp = load_or_generate_migrating(new_dir.path(), legacy_dir.path(), "node_keypair")
            .expect("fresh generate ok");
        assert!(new_dir.path().join("node_keypair.key").exists());
        assert!(!legacy_dir.path().join("node_keypair.key").exists());
        let second = load_or_generate_migrating(new_dir.path(), legacy_dir.path(), "node_keypair")
            .expect("second launch ok");
        assert_eq!(kp.seed_bytes(), second.seed_bytes());
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
