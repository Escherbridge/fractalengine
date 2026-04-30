use crate::repo::Db;
use fe_runtime::messages::EntityType;

/// Rename a verse, fractal, or petal.
pub(crate) async fn rename_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    new_name: &str,
) -> Result<(), String> {
    let result = match entity_type {
        EntityType::Verse => db.query("UPDATE verse SET name = $name WHERE verse_id = $id")
            .bind(("name", new_name.to_string()))
            .bind(("id", entity_id.to_string()))
            .await,
        EntityType::Fractal => db.query("UPDATE fractal SET name = $name WHERE fractal_id = $id")
            .bind(("name", new_name.to_string()))
            .bind(("id", entity_id.to_string()))
            .await,
        EntityType::Petal => db.query("UPDATE petal SET name = $name WHERE petal_id = $id")
            .bind(("name", new_name.to_string()))
            .bind(("id", entity_id.to_string()))
            .await,
    };
    result.map(|_| ()).map_err(|e| format!("Rename failed: {e}"))
}

/// Delete a verse, fractal, or petal with cascade.
pub(crate) async fn delete_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
) -> Result<(), String> {
    let result = match entity_type {
        EntityType::Verse => async {
            db.query("DELETE FROM node WHERE petal_id IN (SELECT petal_id FROM petal WHERE fractal_id IN (SELECT fractal_id FROM fractal WHERE verse_id = $id))")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM petal WHERE fractal_id IN (SELECT fractal_id FROM fractal WHERE verse_id = $id)")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM fractal WHERE verse_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM role WHERE string::starts_with(scope, $prefix)")
                .bind(("prefix", format!("VERSE#{}", entity_id))).await?;
            db.query("DELETE FROM verse_member WHERE verse_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM verse WHERE verse_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            Ok::<(), surrealdb::Error>(())
        }.await,
        EntityType::Fractal => async {
            db.query("DELETE FROM node WHERE petal_id IN (SELECT petal_id FROM petal WHERE fractal_id = $id)")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM petal WHERE fractal_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM fractal WHERE fractal_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            Ok(())
        }.await,
        EntityType::Petal => async {
            db.query("DELETE FROM node WHERE petal_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            db.query("DELETE FROM petal WHERE petal_id = $id")
                .bind(("id", entity_id.to_string())).await?;
            Ok(())
        }.await,
    };
    result.map_err(|e| format!("Delete failed: {e}"))
}

/// Update a fractal's description.
pub(crate) async fn update_fractal_description_handler(
    db: &Db,
    fractal_id: &str,
    description: &str,
) -> anyhow::Result<()> {
    db.query("UPDATE fractal SET description = $desc WHERE fractal_id = $id")
        .bind(("desc", description.to_string()))
        .bind(("id", fractal_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("Update description failed: {e}"))?;
    Ok(())
}
