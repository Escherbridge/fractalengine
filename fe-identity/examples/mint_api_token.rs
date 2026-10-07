//! Validation fixture: mint a real API token from a known node keypair seed.
//!
//! The relay (and GUI) verify Bearer JWTs with the node keypair's verifying
//! key; the relay loads that keypair from `FE_SECRET_FRACTALENGINE_NODE_KEYPAIR`
//! (hex seed). A validator that controls the process environment therefore
//! mints tokens with the SAME seed via this fixture — the headless equivalent
//! of the UI's `MintApiToken` flow, using the same `mint_api_token` call.
//!
//! Usage:
//!   mint_api_token --seed-hex <64-hex> --scope <scope> --role <role> \
//!                  [--ttl-secs 3600] [--jti <id>]
//!
//! Prints `did: <did:key>` and `token: <jwt>` (the token's `sub` is the DID).

use fe_identity::api_token::mint_api_token;
use fe_identity::NodeKeypair;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

fn require(name: &str) -> anyhow::Result<String> {
    arg(name).ok_or_else(|| anyhow::anyhow!("missing required argument {name}"))
}

fn main() -> anyhow::Result<()> {
    let seed_hex = require("--seed-hex")?;
    let scope = require("--scope")?;
    let role = require("--role")?;
    let ttl: u64 = arg("--ttl-secs")
        .unwrap_or_else(|| "3600".to_string())
        .parse()?;
    let jti = arg("--jti").unwrap_or_else(|| "validation".to_string());

    let seed = hex::decode(seed_hex.trim())?;
    let arr: [u8; 32] = seed
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("seed must decode to 32 bytes, got {}", v.len()))?;
    let kp = NodeKeypair::from_bytes(&arr)?;
    let token = mint_api_token(&kp, &scope, &role, ttl, &jti)?;
    println!("did: {}", kp.to_did_key());
    println!("token: {}", token);
    Ok(())
}
