//! Per-verse timeseries replication settings (M2/F6 — A13): the DB-thread
//! persist + re-emit seam for the settings surface. See
//! `fe-database/src/AGENTS.md` §timeseries-settings.

use fe_runtime::timeseries::VerseTimeseriesSettings;

use crate::repo::Db;
use crate::{BlobStoreHandle, ReplicationSender};

/// Set a verse's timeseries replication settings (mode / durability slider
/// R / shard bucket width), then re-emit the verse manifest so the change
/// converges through the existing seams: peers apply the updated verse row
/// through the normal inbound path, and the sync thread's fabric parses the
/// settings off the outbound write (F6).
///
/// `pub` (not `pub(crate)`) for the same reason as
/// `replicated_row::apply_replicated_row_handler`: the test harness's
/// simplified DB loop calls the real handler against its in-memory DB.
#[allow(clippy::too_many_arguments)]
pub async fn set_verse_timeseries_settings_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    caller_did: &str,
    verse_id: &str,
    mode: &str,
    replication_factor: u32,
    bucket_width_ms: u64,
) -> anyhow::Result<VerseTimeseriesSettings> {
    // Validate BEFORE writing: a bad value returns its original command error
    // and never creates a persisted row (§log-first-commit precondition
    // discipline, even though this path stays outside the op-log seam).
    let settings = VerseTimeseriesSettings::sanitized(mode, replication_factor, bucket_width_ms)
        .map_err(anyhow::Error::msg)?;
    // Gate parity (RP-2 step 5): verse governance needs Manager+ at the VERSE scope.
    crate::rbac::require_manage_role(db, caller_did, &crate::build_scope(verse_id, None, None))
        .await?;

    db.query(
        "UPDATE verse SET ts_mode = $mode, ts_replication_factor = $r, \
         ts_bucket_width_ms = $w WHERE verse_id = $id",
    )
    .bind(("mode", settings.mode.as_str()))
    .bind(("r", settings.replication_factor as i64))
    .bind(("w", settings.bucket_width_ms as i64))
    .bind(("id", verse_id.to_string()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("set timeseries settings statement failed: {e}"))?;

    // READ-BACK the durable row (the handler's Ok is never the proof) and
    // re-emit it as the verse manifest — the manifest row is the single
    // source the sync plane parses, so the persisted truth and the published
    // truth cannot drift. A verse_id that matched nothing surfaces here as
    // a plain not-found error.
    let mut res = db
        .query("SELECT * FROM verse WHERE verse_id = $id")
        .bind(("id", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(row) = rows.into_iter().next() else {
        anyhow::bail!("verse '{verse_id}' not found");
    };
    let persisted = VerseTimeseriesSettings::from_verse_row(&row);
    if persisted != settings {
        anyhow::bail!(
            "timeseries settings for verse '{verse_id}' did not persist \
             (requested {}/{}/{}, read back {}/{}/{})",
            settings.mode.as_str(),
            settings.replication_factor,
            settings.bucket_width_ms,
            persisted.mode.as_str(),
            persisted.replication_factor,
            persisted.bucket_width_ms
        );
    }
    publish_manifest_row(db, repl_tx, blob_store, caller_did, verse_id).await;

    tracing::info!(
        verse_id,
        mode = persisted.mode.as_str(),
        replication_factor = persisted.replication_factor,
        bucket_width_ms = persisted.bucket_width_ms,
        "Set verse timeseries replication settings (manifest re-emitted)"
    );
    Ok(persisted)
}

/// Re-emit the committed verse row as the clocked manifest; a failure warns, never fails the write.
async fn publish_manifest_row(
    db: &Db,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    local_did: &str,
    verse_id: &str,
) {
    if let Err(error) = crate::field_clock::publish_clocked_row(
        db, blob_store, repl_tx, local_did, verse_id, "verse", verse_id, true,
    )
    .await
    {
        tracing::warn!(verse_id, "committed verse manifest clocked publish failed: {error:#}");
    }
}

/// Set a verse's `default_access` (Manager+ at the verse scope) and re-emit the manifest so
/// the change propagates deliberately, the same way settings do (RP-2 step 5, A3).
pub async fn set_verse_default_access_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    caller_did: &str,
    verse_id: &str,
    default_access: &str,
) -> anyhow::Result<()> {
    crate::role_manager::set_default_access(db, caller_did, verse_id, default_access).await?;
    let mut res = db
        .query("SELECT * FROM verse WHERE verse_id = $id")
        .bind(("id", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(row) = rows.into_iter().next() else {
        anyhow::bail!("verse '{verse_id}' not found");
    };
    if row
        .get("default_access")
        .and_then(serde_json::Value::as_str)
        != Some(default_access)
    {
        anyhow::bail!("default_access for verse '{verse_id}' did not persist");
    }
    publish_manifest_row(db, repl_tx, blob_store, caller_did, verse_id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Repo;
    use crate::schema::Verse;
    use fe_runtime::blob_store::mock::MockBlobStore;

    const OWNER: &str = "did:key:z6MkOwner";

    async fn schema_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    fn mock_store() -> (BlobStoreHandle, std::sync::Arc<MockBlobStore>) {
        let store = std::sync::Arc::new(MockBlobStore::new());
        (store.clone(), store)
    }

    fn verse_row(verse_id: &str) -> Verse {
        Verse {
            verse_id: verse_id.to_string(),
            name: "v".to_string(),
            created_by: OWNER.to_string(),
            created_at: "2026-10-07T00:00:00Z".to_string(),
            namespace_id: Some("ab".repeat(32)),
            default_access: "viewer".to_string(),
            ts_mode: "mirror".to_string(),
            ts_replication_factor: 1,
            ts_bucket_width_ms: fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS as i64,
            view_hidden: None,
        }
    }

    async fn read_settings(db: &Db, verse_id: &str) -> VerseTimeseriesSettings {
        let mut res = db
            .query("SELECT * FROM verse WHERE verse_id = $id")
            .bind(("id", verse_id.to_string()))
            .await
            .expect("select");
        let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
        VerseTimeseriesSettings::from_verse_row(rows.first().expect("one row"))
    }

    #[tokio::test]
    async fn settings_apply_and_read_back() {
        let db = schema_db().await;
        let (store, _mock) = mock_store();
        Repo::<Verse>::create(&db, &verse_row("v1"))
            .await
            .expect("seed");

        let applied = set_verse_timeseries_settings_handler(
            &db, &store, None, OWNER, "v1", "balanced", 3, 3_600_000,
        )
        .await
        .expect("set");
        assert_eq!(
            applied.mode,
            fe_runtime::timeseries::TimeseriesMode::Balanced
        );

        // READ-BACK: the handler's Ok is not the proof, the row is.
        let persisted = read_settings(&db, "v1").await;
        assert_eq!(persisted, applied);
    }

    #[tokio::test]
    async fn invalid_values_are_rejected_without_persisting() {
        let db = schema_db().await;
        let (store, _mock) = mock_store();
        Repo::<Verse>::create(&db, &verse_row("v2"))
            .await
            .expect("seed");

        for (mode, r, w) in [
            ("replicated", 1u32, 1u64),
            ("mirror", 0, 1),
            ("mirror", 1, 0),
        ] {
            let err =
                set_verse_timeseries_settings_handler(&db, &store, None, OWNER, "v2", mode, r, w)
                    .await
                    .expect_err("must reject");
            assert!(!err.to_string().is_empty(), "rejection carries a reason");
        }
        // Nothing was written.
        let persisted = read_settings(&db, "v2").await;
        assert_eq!(persisted, VerseTimeseriesSettings::default());
    }

    #[tokio::test]
    async fn unknown_verse_is_an_error() {
        let db = schema_db().await;
        let (store, _mock) = mock_store();
        let err = set_verse_timeseries_settings_handler(
            &db, &store, None, OWNER, "missing", "mirror", 1, 1,
        )
        .await
        .expect_err("missing verse must error");
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn settings_change_re_emits_the_verse_manifest() {
        let db = schema_db().await;
        let (store, mock) = mock_store();
        Repo::<Verse>::create(&db, &verse_row("v3"))
            .await
            .expect("seed");

        let (tx, rx) = crossbeam::channel::bounded::<crate::ReplicationEvent>(8);
        set_verse_timeseries_settings_handler(
            &db,
            &store,
            Some(&tx),
            OWNER,
            "v3",
            "sharded",
            1,
            86_400_000,
        )
        .await
        .expect("set");

        let event = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("event");
        assert_eq!(event.verse_id, "v3");
        assert_eq!(event.table, "verse");
        assert_eq!(event.record_id, "v3");
        // The blob the manifest re-emission points at carries the new settings.
        let bytes = mock.bytes_for(&event.content_hash).expect("blob bytes");
        let row: serde_json::Value =
            serde_json::from_slice(bytes.as_slice()).expect("manifest json");
        assert_eq!(row["ts_mode"], serde_json::json!("sharded"));
        assert_eq!(row["ts_replication_factor"], serde_json::json!(1));
        assert_eq!(row["ts_bucket_width_ms"], serde_json::json!(86_400_000));
        // The manifest is published through the clocked seam: the changed key is signed by the local author.
        assert_eq!(row["_fe_clock"]["author"], OWNER);
        assert!(
            row["_fe_clock"]["own"]["ts_mode"]["hlc"].is_string(),
            "ts_mode carries its own clocked version"
        );
    }
}
