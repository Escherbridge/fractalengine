//! Writes an iroh 0.35 P2P data dir exactly the way fe-sync's 0.35
//! `DocsStack` + `IrohDocsReplicator` did, for the 1.x store-migration test
//! (fe-sync/src/AGENTS.md §store-migration).
//!
//! Standalone crate (NOT a workspace member; iroh 0.35 cannot coexist with
//! the 1.x workspace), pinned by its own Cargo.lock. The output is gitignored
//! and CI-generated. Regenerate with:
//!   cd fe-sync/tests/fixtures/p2p_store_v035_gen && cargo run --locked -- ../p2p_store_v035
//! (the output dir must not exist yet).

use iroh::protocol::Router;
use iroh_blobs::net_protocol::Blobs;
use iroh_docs::protocol::Docs;
use iroh_docs::{Author, Capability, NamespaceSecret};
use iroh_gossip::net::Gossip;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("usage: v035_fixture_gen <out_dir>"))?,
    );
    anyhow::ensure!(!out.exists(), "{} already exists", out.display());
    std::fs::create_dir_all(&out)?;

    let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
    let endpoint = iroh::Endpoint::builder()
        .secret_key(secret.clone())
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await?;
    let blobs = Blobs::persistent(out.join("blobs")).await?.build(&endpoint);
    let gossip = Gossip::builder().spawn(endpoint.clone()).await?;
    let docs = Docs::persistent(out.clone()).spawn(&blobs, &gossip).await?;
    let author = Author::from_bytes(&secret.to_bytes());
    let author_id = author.id();
    docs.client().authors().import(author).await?;
    docs.client().authors().set_default(author_id).await?;
    let router = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, blobs.clone())
        .accept(iroh_gossip::net::GOSSIP_ALPN, gossip.clone())
        .accept(iroh_docs::ALPN, docs.clone())
        .spawn();

    let ns_secret = NamespaceSecret::from_bytes(&[23u8; 32]);
    let namespace_id = hex::encode(ns_secret.id().as_bytes());
    let doc = docs
        .client()
        .import_namespace(Capability::Write(ns_secret))
        .await?;

    let verse_id = "01J9ZZ0V035AAAAAAAAAAAAAAA";
    let manifest = serde_json::json!({
        "verse_id": verse_id,
        "name": "iroh 0.35 fixture verse",
        "created_by": "did:key:fixture",
        "created_at": "2026-10-11T00:00:00Z",
        "namespace_id": namespace_id,
        "default_access": "viewer",
    })
    .to_string()
    .into_bytes();
    let small = br#"{"node_id":"n1","name":"small row"}"#.to_vec();
    // > 16 KiB: forces the blob out of the inline table into data/{hash}.data.
    let large = serde_json::json!({
        "node_id": "n2",
        "padding": "x".repeat(40 * 1024),
    })
    .to_string()
    .into_bytes();
    let doomed = br#"{"node_id":"n3","name":"deleted row"}"#.to_vec();

    let writes: Vec<(String, Vec<u8>)> = vec![
        (format!("verse/{verse_id}"), manifest),
        ("node/n1".into(), small),
        ("node/n2".into(), large),
        ("node/n3".into(), doomed),
    ];
    for (key, value) in &writes {
        doc.set_bytes(author_id, key.as_bytes().to_vec(), value.clone())
            .await?;
    }
    // Empty-entry tombstone (fe-sync's `write_row` with an empty payload).
    doc.del(author_id, b"node/n3".to_vec()).await?;

    let mut rows = Vec::new();
    for (key, value) in &writes {
        let (data, tombstone): (&[u8], bool) = if key == "node/n3" {
            (&[], true)
        } else {
            (value.as_slice(), false)
        };
        rows.push(serde_json::json!({
            "key": key,
            "len": data.len(),
            "blake3": blake3::hash(data).to_hex().to_string(),
            "tombstone": tombstone,
        }));
    }

    doc.close().await?;
    router.shutdown().await?;
    endpoint.close().await;

    let expected = serde_json::json!({
        "generator": "fe-sync v0.35 fixture (iroh 0.35, iroh-docs 0.35, iroh-blobs 0.35)",
        "endpoint_secret_seed": 7,
        "namespace_secret_seed": 23,
        "verse_id": verse_id,
        "namespace_id": namespace_id,
        "rows": rows,
    });
    std::fs::write(
        out.join("expected.json"),
        serde_json::to_string_pretty(&expected)?,
    )?;
    println!("fixture written to {}", out.display());
    Ok(())
}
