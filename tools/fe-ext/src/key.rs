//! Signing keys as a 32-byte ed25519 seed in a hex text file.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::SigningKey;

/// Read a seed file written by [`generate`].
pub fn load(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading key file {}", path.display()))?;
    let seed = hex::decode(text.trim()).map_err(|error| anyhow!("key file is not hex: {error}"))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| anyhow!("key file must hold exactly 32 bytes (64 hex characters)"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Write a fresh key; refuses to overwrite (atomically, `create_new`), owner-only on Unix. Returns the publisher `did:key`.
pub fn generate(path: &Path) -> Result<String> {
    use std::io::Write;

    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => anyhow!("{} already exists; refusing to overwrite a key", path.display()),
        _ => anyhow!("writing key file {}: {error}", path.display()),
    })?;
    file.write_all(format!("{}\n", hex::encode(key.to_bytes())).as_bytes())?;
    Ok(did(&key))
}

/// The `did:key` of a signing key.
pub fn did(key: &SigningKey) -> String {
    fe_format::verifying_key_to_did(&key.verifying_key())
}
