//! Seal a `.hexon` with the publisher key: `seal_hexon <in> <out>`, key = 32-byte hex seed
//! in `FE_SECRET_HEXON_SEAL_KEY` (never a file; RUNBOOK §5 Secrets).

use anyhow::{bail, Context, Result};
use ed25519_dalek::SigningKey;
use fe_format::{seal, verify_archive, verifying_key_to_did, IntegrityPolicy};

const KEY_ENV: &str = "FE_SECRET_HEXON_SEAL_KEY";

/// Decode a 64-character hex seed into an Ed25519 signing key.
fn signing_key_from_hex(hex: &str) -> Result<SigningKey> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        bail!("{KEY_ENV} must be 64 hex characters (a 32-byte Ed25519 seed)");
    }
    let mut seed = [0u8; 32];
    for (i, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("{KEY_ENV} is not valid hex"))?;
    }
    Ok(SigningKey::from_bytes(&seed))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        bail!("usage: seal_hexon <in.hexon> <out.hexon>  (key in {KEY_ENV})");
    };
    let key = signing_key_from_hex(
        &std::env::var(KEY_ENV).with_context(|| format!("{KEY_ENV} is not set"))?,
    )?;
    let publisher = verifying_key_to_did(&key.verifying_key());

    let bytes = std::fs::read(input).with_context(|| format!("reading {input}"))?;
    let sealed = seal(&bytes, &key).with_context(|| format!("sealing {input}"))?;
    let status = verify_archive(
        &sealed,
        &IntegrityPolicy::RequireSealed {
            pin: Some(publisher.clone()),
        },
    )
    .context("sealed output failed its own verification")?;
    std::fs::write(output, &sealed).with_context(|| format!("writing {output}"))?;
    println!("sealed {input} -> {output} as {publisher} ({status:?})");
    Ok(())
}
