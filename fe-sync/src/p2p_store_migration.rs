//! One-time on-disk migration of an iroh 0.35 P2P data dir to the iroh 1.x
//! stack's store format. See `fe-sync/src/AGENTS.md` §store-migration.
//!
//! iroh-docs 0.35 / iroh-blobs 0.35 wrote their redb stores (`docs.redb`,
//! `blobs/blobs.db`) in redb file format v2; iroh-docs 0.101 / iroh-blobs
//! 0.103 open them with redb 4, which refuses v2 files. redb 2.6 is the last
//! release that can upgrade a v2 file to v3, so this module runs it — once,
//! behind a marker file, after a full backup copy — BEFORE the stack opens
//! the stores. iroh-docs 0.101's own `redb-v2-migration` feature then
//! rewrites the tuple tables on first open; the blobs tables, data files and
//! outboards need no conversion.
//!
//! Blocking filesystem/redb work: callers run [`migrate_legacy_p2p_store`]
//! on the blocking pool (§sync-thread-blocking-io).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Marker file (inside the P2P data dir) recording the store format.
pub const STORE_FORMAT_MARKER: &str = ".fe-store-format";

/// Marker content for a dir whose stores are in the iroh 1.x format.
pub const STORE_FORMAT_IROH1: &str = "iroh1";

/// The redb stores a 0.35 data dir may hold, relative to the dir.
const LEGACY_REDB_FILES: [&str; 2] = ["docs.redb", "blobs/blobs.db"];

/// What [`migrate_legacy_p2p_store`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The marker already says the dir is in the iroh 1.x format.
    AlreadyCurrent,
    /// No legacy store existed — the marker was written for a fresh dir.
    Fresh,
    /// Legacy stores were upgraded after a full backup copy.
    Migrated {
        /// The untouched pre-migration copy of the whole dir (the rollback).
        backup_dir: PathBuf,
        /// Store files whose redb file format was actually upgraded v2→v3.
        upgraded: Vec<PathBuf>,
    },
}

/// Migrate `dir` (an existing P2P data dir) to the iroh 1.x store format.
///
/// - Marker present and current → no-op ([`MigrationOutcome::AlreadyCurrent`]).
/// - No legacy redb store present → writes the marker ([`MigrationOutcome::Fresh`]),
///   so stores the 1.x stack creates are never mistaken for legacy ones.
/// - Otherwise: copies the whole dir to `<dir>.v035.bak-<unix_ts>` (via a
///   `.partial` sibling renamed on completion; never
///   moved or deleted — it is the rollback), upgrades each legacy redb file
///   with redb 2.6's `Database::upgrade` (v2→v3 file format), then writes
///   the marker. Any failure is returned loudly and the marker is NOT
///   written, so the next start retries from the untouched backup state.
///
/// `dir` must already exist.
pub fn migrate_legacy_p2p_store(dir: &Path) -> Result<MigrationOutcome> {
    let marker = dir.join(STORE_FORMAT_MARKER);
    if let Ok(content) = std::fs::read_to_string(&marker) {
        if content.trim() == STORE_FORMAT_IROH1 {
            return Ok(MigrationOutcome::AlreadyCurrent);
        }
        anyhow::bail!(
            "unrecognized P2P store format marker {:?} in {}",
            content.trim(),
            marker.display()
        );
    }

    let legacy: Vec<PathBuf> = LEGACY_REDB_FILES
        .iter()
        .map(|rel| dir.join(rel))
        .filter(|path| path.is_file())
        .collect();
    if legacy.is_empty() {
        write_marker(&marker)?;
        return Ok(MigrationOutcome::Fresh);
    }

    // Copy into a `.partial` sibling and rename only once complete, so an
    // interrupted copy can never be mistaken for the rollback.
    let backup_dir = backup_path(dir)?;
    let partial = backup_dir.with_file_name(format!(
        "{}.partial",
        backup_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    copy_dir_recursive(dir, &partial)
        .and_then(|()| std::fs::rename(&partial, &backup_dir))
        .with_context(|| {
            format!(
                "backing up legacy P2P store {} to {}",
                dir.display(),
                backup_dir.display()
            )
        })?;
    tracing::warn!(
        p2p_dir = %dir.display(),
        backup = %backup_dir.display(),
        "Migrating an iroh 0.35 P2P store to the iroh 1.x format (backup copy written first)"
    );

    let mut upgraded = Vec::new();
    for path in legacy {
        let mut db = redb_v2::Database::open(&path)
            .with_context(|| format!("opening legacy redb store {}", path.display()))?;
        let did_upgrade = db
            .upgrade()
            .with_context(|| format!("upgrading redb file format of {}", path.display()))?;
        drop(db);
        tracing::info!(store = %path.display(), upgraded = did_upgrade, "redb store checked");
        if did_upgrade {
            upgraded.push(path);
        }
    }

    write_marker(&marker)?;
    Ok(MigrationOutcome::Migrated {
        backup_dir,
        upgraded,
    })
}

fn write_marker(marker: &Path) -> Result<()> {
    std::fs::write(marker, STORE_FORMAT_IROH1)
        .with_context(|| format!("writing store format marker {}", marker.display()))
}

/// `<dir>.v035.bak-<unix_ts>` beside `dir`.
fn backup_path(dir: &Path) -> Result<PathBuf> {
    let name = dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("P2P data dir {} has no file name", dir.display()))?
        .to_string_lossy()
        .into_owned();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut candidate = dir.with_file_name(format!("{name}.v035.bak-{ts}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.with_file_name(format!("{name}.v035.bak-{ts}-{n}"));
        n += 1;
    }
    Ok(candidate)
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generated (never committed) 0.35-written fixture (see AGENTS.md §store-migration).
    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/p2p_store_v035")
    }

    /// Skips the test, or fails it when `FE_REQUIRE_STORE_FIXTURE=1` (AGENTS.md §store-migration).
    fn skip_unless_required(reason: &str) {
        if std::env::var("FE_REQUIRE_STORE_FIXTURE").is_ok_and(|v| v == "1") {
            panic!("FE_REQUIRE_STORE_FIXTURE=1 forbids skipping: {reason}");
        }
        eprintln!("skipping: {reason}");
    }

    #[test]
    fn fresh_dir_gets_marker_and_second_run_is_a_no_op() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("p2p");
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(
            migrate_legacy_p2p_store(&dir).unwrap(),
            MigrationOutcome::Fresh
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(STORE_FORMAT_MARKER)).unwrap(),
            STORE_FORMAT_IROH1
        );
        assert_eq!(
            migrate_legacy_p2p_store(&dir).unwrap(),
            MigrationOutcome::AlreadyCurrent
        );
        // A fresh dir is never backed up.
        let siblings: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(siblings.len(), 1, "no backup dir beside a fresh store");
    }

    #[test]
    fn unrecognized_marker_fails_loudly() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join(STORE_FORMAT_MARKER), "iroh9").unwrap();
        assert!(migrate_legacy_p2p_store(tmp.path()).is_err());
    }

    /// The 0.35 fixture migrates, the stack opens it, and every row reads
    /// back with the right bytes and tombstone flag; a second run is a no-op.
    #[tokio::test]
    async fn legacy_v035_fixture_migrates_and_reads_back() {
        let fixture = fixture_dir();
        let expected_path = fixture.join("expected.json");
        if !expected_path.is_file() {
            skip_unless_required(&format!(
                "0.35 store fixture not generated ({}); run the p2p_store_v035_gen generator \
                 (fe-sync/src/AGENTS.md §store-migration)",
                fixture.display()
            ));
            return;
        }
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&expected_path).unwrap()).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("p2p");
        copy_dir_recursive(&fixture, &dir).unwrap();
        std::fs::remove_file(dir.join("expected.json")).unwrap();

        let outcome = migrate_legacy_p2p_store(&dir).unwrap();
        let MigrationOutcome::Migrated { backup_dir, .. } = &outcome else {
            panic!("expected a migration, got {outcome:?}");
        };
        assert!(
            backup_dir.join("docs.redb").is_file(),
            "backup is a full copy"
        );
        assert_eq!(
            migrate_legacy_p2p_store(&dir).unwrap(),
            MigrationOutcome::AlreadyCurrent,
            "second run is a no-op"
        );

        let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let endpoint = match crate::endpoint::SyncEndpoint::new(
            secret,
            &crate::relay_config::RelayConfig::Disabled,
        )
        .await
        {
            Ok(ep) => ep,
            Err(e) => {
                skip_unless_required(&format!("read-back: endpoint bind failed ({e})"));
                return;
            }
        };
        let stack = std::sync::Arc::new(
            crate::docs_engine::DocsStack::spawn(endpoint.inner().clone(), &dir)
                .await
                .expect("stack opens the migrated store"),
        );
        let holder = std::sync::Arc::new(crate::replicator::IrohDocsEngineHolder::online(
            stack.clone(),
        ));
        let reader = crate::replicator::IrohDocsReplicator::new(
            expected["verse_id"].as_str().unwrap().to_string(),
            expected["namespace_id"].as_str().unwrap().to_string(),
            String::new(),
            "did:key:migration-test".to_string(),
            holder,
        );
        reader.open_document().await.expect("secretless reopen");
        let snapshot = crate::replicator::VerseReplicator::snapshot(&reader)
            .await
            .expect("snapshot");

        let rows = expected["rows"].as_array().unwrap();
        assert_eq!(snapshot.len(), rows.len(), "every fixture row reads back");
        for row in rows {
            let key = row["key"].as_str().unwrap();
            let got = snapshot
                .iter()
                .find(|c| format!("{}/{}", c.table, c.record_id) == key)
                .unwrap_or_else(|| panic!("row {key} missing after migration"));
            assert_eq!(
                got.is_tombstone,
                row["tombstone"].as_bool().unwrap(),
                "{key}"
            );
            assert_eq!(got.data.len() as u64, row["len"].as_u64().unwrap(), "{key}");
            assert_eq!(
                blake3::hash(&got.data).to_hex().as_str(),
                row["blake3"].as_str().unwrap(),
                "{key}"
            );
        }
        crate::replicator::VerseReplicator::close(&reader)
            .await
            .ok();
        stack.shutdown().await;
        endpoint.shutdown().await;
    }
}
