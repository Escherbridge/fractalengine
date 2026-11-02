//! Fenced local replica catalog repair; see handlers/AGENTS.md (local-catalog).

use fe_runtime::messages::EntityType;
use serde_json::{Value, json};

use crate::repo::Db;
use crate::{BlobStoreHandle, ReplicationSender, replicate_row};

const CATALOG_LIMIT: usize = 1024;

/// Publish a committed Petal through the guarded catalog seam used by startup repair.
pub(crate) async fn publish_petal(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    petal: &str,
) -> anyhow::Result<()> {
    if sender.is_none() {
        return Ok(());
    }
    let scope = super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("committed Petal ancestry is unavailable"))?;
    let parts = crate::parse_scope(&scope)?;
    publish_member(
        db,
        blobs,
        sender,
        actor,
        &parts.verse_id,
        EntityType::Petal,
        petal,
    )
    .await
}

async fn publish_member(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    verse: &str,
    kind: EntityType,
    id: &str,
) -> anyhow::Result<()> {
    let access = super::entity::prepare_parent_mutation(db, kind, id, actor).await?;
    let (table, select) = match kind {
        EntityType::Verse => (
            "verse",
            "SELECT verse_id, name, created_by, created_at, namespace_id, \
            default_access, ts_mode, ts_replication_factor, ts_bucket_width_ms \
            FROM verse WHERE verse_id=$catalog_target AND verse_id=$catalog_verse",
        ),
        EntityType::Fractal => (
            "fractal",
            "SELECT fractal_id, verse_id, owner_did, name, description, created_at \
            FROM fractal WHERE fractal_id=$catalog_target AND verse_id=$catalog_verse",
        ),
        EntityType::Petal => (
            "petal",
            "SELECT petal_id, fractal_id, name, node_id, bounds, created_at, description, visibility, tags, \
            hexon_manifest, terrain, visibility_groups, render_distance \
            FROM petal WHERE petal_id=$catalog_target AND fractal_id IN \
            (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$catalog_verse)",
        ),
    };
    let select = format!("{select} TIMEOUT 5s");
    let rows = access
        .read_rows(
            db,
            &select,
            vec![
                ("catalog_target".into(), json!(id)),
                ("catalog_verse".into(), json!(verse)),
            ],
        )
        .await?;
    anyhow::ensure!(
        rows.len() == 1,
        "catalog target ancestry is missing or ambiguous"
    );
    let bytes = serde_json::to_vec(&crate::repo::strip_json_nulls(
        rows.into_iter().next().unwrap(),
    ))?;
    replicate_row(sender, blobs, verse, table, id, &bytes);
    Ok(())
}

/// Re-publish only current stored catalog rows the actual local identity may manage.
pub(crate) async fn reemit(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    verse: &str,
) -> anyhow::Result<usize> {
    if sender.is_none() {
        return Ok(0);
    }
    let scope = crate::build_scope(verse, None, None);
    if !crate::role_manager::resolve_role(db, actor, &scope)
        .await?
        .is_at_least(fe_policy::RoleLevel::Manager)
    {
        return Ok(0);
    }
    let mut response = db.query(
        "SELECT fractal_id FROM fractal WHERE verse_id=$catalog_verse LIMIT 1025 TIMEOUT 5s; \
         SELECT petal_id FROM petal WHERE fractal_id IN \
         (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$catalog_verse) LIMIT 1025 TIMEOUT 5s",
    ).bind(("catalog_verse", verse.to_owned())).await?.check()?;
    let fractals: Vec<Value> = response.take(0)?;
    let petals: Vec<Value> = response.take(1)?;
    anyhow::ensure!(
        fractals.len() <= CATALOG_LIMIT && petals.len() <= CATALOG_LIMIT,
        "local catalog exceeds the 1024 member recovery limit"
    );
    publish_member(db, blobs, sender, actor, verse, EntityType::Verse, verse).await?;
    let mut published = 1;
    for (kind, field, members) in [
        (EntityType::Fractal, "fractal_id", fractals),
        (EntityType::Petal, "petal_id", petals),
    ] {
        for member in members {
            let id = member[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid stored catalog identity"))?;
            let scope = match kind {
                EntityType::Fractal => super::crud::resolve_fractal_scope_handler(db, id).await?,
                EntityType::Petal => super::crud::resolve_petal_scope_handler(db, id).await?,
                EntityType::Verse => unreachable!(),
            };
            let Some(scope) = scope else {
                continue;
            };
            if !crate::role_manager::resolve_role(db, actor, &scope)
                .await?
                .is_at_least(fe_policy::RoleLevel::Manager)
            {
                continue;
            }
            publish_member(db, blobs, sender, actor, verse, kind, id).await?;
            published += 1;
        }
    }
    Ok(published)
}
