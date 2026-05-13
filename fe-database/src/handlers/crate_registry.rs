//! Hexon crate registry DB handlers.
//!
//! Handles persisting and removing crate registry entries in SurrealDB.

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

/// Insert a crate_registry row for an installed hexon.
pub async fn install_crate(
    db: &Surreal<Db>,
    hexon_uri: &str,
    manifest_hash: &str,
    publisher_did: &str,
    hexon_type: &str,
    version: &str,
    name: &str,
    tags: &[String],
    petal_id: &str,
    size_bytes: u64,
) -> Result<(), anyhow::Error> {
    let tags_json = serde_json::to_string(tags)?;

    // Use UPSERT to handle re-installation gracefully
    db.query(
        r#"
        UPSERT INTO crate_registry SET
            hexon_uri = $hexon_uri,
            manifest_hash = $manifest_hash,
            publisher_did = $publisher_did,
            hexon_type = $hexon_type,
            version = $version,
            name = $name,
            tags = $tags,
            petal_id = $petal_id,
            size_bytes = $size_bytes,
            installed_at = time::now(),
            signature_valid = true
        "#,
    )
    .bind(("hexon_uri", hexon_uri.to_string()))
    .bind(("manifest_hash", manifest_hash.to_string()))
    .bind(("publisher_did", publisher_did.to_string()))
    .bind(("hexon_type", hexon_type.to_string()))
    .bind(("version", version.to_string()))
    .bind(("name", name.to_string()))
    .bind(("tags", tags_json))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("size_bytes", size_bytes as i64))
    .await?;

    Ok(())
}

/// Insert a crate_entry row for a single asset within an installed hexon.
pub async fn install_crate_entry(
    db: &Surreal<Db>,
    entry_id: &str,
    hexon_uri: &str,
    kind: &str,
    asset_hash: &str,
    format: &str,
    label: &str,
    metadata: &serde_json::Value,
) -> Result<(), anyhow::Error> {
    let metadata_json = serde_json::to_string(metadata)?;

    db.query(
        r#"
        UPSERT INTO crate_entry SET
            entry_id = $entry_id,
            hexon_uri = $hexon_uri,
            kind = $kind,
            asset_hash = $asset_hash,
            format = $format,
            label = $label,
            metadata = $metadata
        "#,
    )
    .bind(("entry_id", entry_id.to_string()))
    .bind(("hexon_uri", hexon_uri.to_string()))
    .bind(("kind", kind.to_string()))
    .bind(("asset_hash", asset_hash.to_string()))
    .bind(("format", format.to_string()))
    .bind(("label", label.to_string()))
    .bind(("metadata", metadata_json))
    .await?;

    Ok(())
}

/// Remove a crate and all its entries from the registry.
pub async fn uninstall_crate(
    db: &Surreal<Db>,
    hexon_uri: &str,
) -> Result<(), anyhow::Error> {
    // Delete entries first (foreign key relationship)
    db.query("DELETE FROM crate_entry WHERE hexon_uri = $hexon_uri")
        .bind(("hexon_uri", hexon_uri.to_string()))
        .await?;

    // Delete the crate registry row
    db.query("DELETE FROM crate_registry WHERE hexon_uri = $hexon_uri")
        .bind(("hexon_uri", hexon_uri.to_string()))
        .await?;

    Ok(())
}
