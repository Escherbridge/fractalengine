//! Hexon crate registry DB handlers (rows keyed by `(petal_id, hexon_uri)` and `(hexon_uri, entry_id)`).
//!
//! Handles persisting and removing crate registry entries in SurrealDB.

use surrealdb::engine::local::Db;
use surrealdb::Surreal;

/// Write the crate_registry row for `(petal_id, hexon_uri)`, replacing any earlier one.
#[allow(clippy::too_many_arguments)] // flat DB row writer — params mirror columns
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

    // Delete-then-create in one transaction: re-install of the same pair stays one row, and a failed
    // CREATE never leaves the pair with no row.
    db.query(
        r#"
        BEGIN TRANSACTION;
        DELETE FROM crate_registry WHERE hexon_uri = $hexon_uri AND petal_id = $petal_id;
        CREATE crate_registry SET
            hexon_uri = $hexon_uri,
            manifest_hash = $manifest_hash,
            publisher_did = $publisher_did,
            hexon_type = $hexon_type,
            version = $version,
            name = $name,
            tags = $tags,
            petal_id = $petal_id,
            size_bytes = $size_bytes,
            installed_at = <string> time::now(),
            signature_valid = true;
        COMMIT TRANSACTION;
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
    .await?
    .check()?;

    Ok(())
}

/// Write the crate_entry row for `(hexon_uri, entry_id)`; entries are shared by every petal holding the build.
#[allow(clippy::too_many_arguments)] // flat DB row writer — params mirror columns
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
    db.query(
        r#"
        BEGIN TRANSACTION;
        DELETE FROM crate_entry WHERE hexon_uri = $hexon_uri AND entry_id = $entry_id;
        CREATE crate_entry SET
            entry_id = $entry_id,
            hexon_uri = $hexon_uri,
            kind = $kind,
            asset_hash = $asset_hash,
            format = $format,
            label = $label,
            metadata = IF $metadata = NULL { NONE } ELSE { $metadata };
        COMMIT TRANSACTION;
        "#,
    )
    .bind(("entry_id", entry_id.to_string()))
    .bind(("hexon_uri", hexon_uri.to_string()))
    .bind(("kind", kind.to_string()))
    .bind(("asset_hash", asset_hash.to_string()))
    .bind(("format", format.to_string()))
    .bind(("label", label.to_string()))
    .bind(("metadata", metadata.clone()))
    .await?
    .check()?;

    Ok(())
}

/// Remove a crate and all its entries from every petal.
pub async fn uninstall_crate(db: &Surreal<Db>, hexon_uri: &str) -> Result<(), anyhow::Error> {
    db.query(
        "BEGIN TRANSACTION; \
         DELETE FROM crate_entry WHERE hexon_uri = $hexon_uri; \
         DELETE FROM crate_registry WHERE hexon_uri = $hexon_uri; \
         COMMIT TRANSACTION;",
    )
    .bind(("hexon_uri", hexon_uri.to_string()))
    .await?
    .check()?;

    Ok(())
}

/// Remove one petal's registry row; the shared entry rows go only when no petal holds the build.
pub async fn uninstall_crate_in_petal(
    db: &Surreal<Db>,
    hexon_uri: &str,
    petal_id: &str,
) -> Result<(), anyhow::Error> {
    db.query("DELETE FROM crate_registry WHERE hexon_uri = $hexon_uri AND petal_id = $petal_id")
        .bind(("hexon_uri", hexon_uri.to_string()))
        .bind(("petal_id", petal_id.to_string()))
        .await?
        .check()?;

    let mut remaining = db
        .query("SELECT VALUE petal_id FROM crate_registry WHERE hexon_uri = $hexon_uri LIMIT 1")
        .bind(("hexon_uri", hexon_uri.to_string()))
        .await?;
    let still_held: Vec<String> = remaining.take(0)?;
    if still_held.is_empty() {
        db.query("DELETE FROM crate_entry WHERE hexon_uri = $hexon_uri")
            .bind(("hexon_uri", hexon_uri.to_string()))
            .await?
            .check()?;
    }

    Ok(())
}
