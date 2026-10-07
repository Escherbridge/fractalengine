//! Read-only inspector for a persisted iroh-docs verse doc (M2 relay e2e
//! read-back instrument, F21 — see fe-database `examples/` for the sibling
//! SurrealKV fixtures).
//!
//! Opens a p2p data dir's persisted doc store (the `docs.redb` a relay or
//! GUI sync thread wrote) read-only — secretless, by the verse's aligned
//! Ed25519 namespace id — and dumps the doc's current entries (latest per
//! key, tombstones included) as JSON lines. With `--expect table/id` the
//! exit code is the assertion (0 = present, 1 = absent), so e2e scripts can
//! prove a row durably entered a stopped process's own doc without any
//! live peer.
//!
//! Lock rule (same as the SurrealKV fixtures): the host process must be
//! STOPPED — the redb store takes an exclusive per-handle file lock.
//!
//! ```text
//! inspect_doc --p2p-dir <dir> --verse-id <ulid> --namespace-id <64hex> [--expect verse/<ulid>]
//! ```

use std::sync::Arc;

use fe_sync::docs_engine::DocsStack;
use fe_sync::endpoint::SyncEndpoint;
use fe_sync::relay_config::RelayConfig;
use fe_sync::replicator::{IrohDocsEngineHolder, IrohDocsReplicator, VerseReplicator};

fn main() -> anyhow::Result<()> {
    let mut p2p_dir: Option<std::path::PathBuf> = None;
    let mut verse_id: Option<String> = None;
    let mut namespace_id: Option<String> = None;
    let mut expect: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--p2p-dir" => p2p_dir = args.next().map(Into::into),
            "--verse-id" => verse_id = args.next(),
            "--namespace-id" => namespace_id = args.next(),
            "--expect" => expect = args.next(),
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    let p2p_dir = p2p_dir.ok_or_else(|| anyhow::anyhow!("--p2p-dir is required"))?;
    let verse_id = verse_id.ok_or_else(|| anyhow::anyhow!("--verse-id is required"))?;
    let namespace_id = namespace_id.ok_or_else(|| anyhow::anyhow!("--namespace-id is required"))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let expect_in_block = expect.clone();
    let found = rt.block_on(async move {
        // Ephemeral key: the secretless read-only open of a persisted doc
        // needs no particular identity. Hermetic by default — the caller
        // controls FE_SYNC_RELAY (loopback "disabled" for e2e runs).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seed = *blake3::hash(format!("{nanos}:{}", std::process::id()).as_bytes()).as_bytes();
        let secret = iroh::SecretKey::from_bytes(&seed);
        let endpoint = SyncEndpoint::new(secret, &RelayConfig::from_env()).await?;
        let stack = Arc::new(DocsStack::spawn(endpoint.inner().clone(), &p2p_dir).await?);
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));
        let reader = IrohDocsReplicator::new(
            verse_id.clone(),
            namespace_id,
            String::new(), // secretless → read-only open of the persisted doc
            "did:key:inspect-doc".to_string(),
            holder,
        );
        reader.open_document().await?;
        let snapshot = reader.snapshot().await?;

        let mut found = false;
        for row in &snapshot {
            let key = format!("{}/{}", row.table, row.record_id);
            let line = serde_json::json!({
                "key": key,
                "tombstone": row.is_tombstone,
                "bytes": row.data.len(),
                "content": String::from_utf8_lossy(&row.data),
            });
            println!("{line}");
            if let Some(want) = &expect_in_block {
                if &key == want {
                    found = true;
                }
            }
        }
        println!(
            "entries={} expected={:?} found={}",
            snapshot.len(),
            expect_in_block,
            found && expect_in_block.is_some()
        );
        Ok::<bool, anyhow::Error>(found)
    })?;

    match (&expect, found) {
        (None, _) => Ok(()),
        (Some(want), true) => {
            eprintln!("FOUND {want}");
            Ok(())
        }
        (Some(want), false) => {
            eprintln!("NOT FOUND {want}");
            std::process::exit(1);
        }
    }
}
