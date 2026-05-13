use tracing::instrument;

use fe_query::{DeleteBuilder, Filter, QueryBuilder, UpdateBuilder};

use crate::query_helpers::exec_query;
use crate::repo::Db;
use fe_runtime::messages::EntityType;

/// Rename a verse, fractal, or petal.
#[instrument(skip(db))]
pub(crate) async fn rename_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    new_name: &str,
) -> Result<(), String> {
    let (table, id_field) = match entity_type {
        EntityType::Verse => ("verse", "verse_id"),
        EntityType::Fractal => ("fractal", "fractal_id"),
        EntityType::Petal => ("petal", "petal_id"),
    };
    let q = UpdateBuilder::update(table)
        .set("name", new_name)
        .where_clause(Filter::eq(id_field, entity_id))
        .build();
    exec_query(db, &q)
        .await
        .map(|_| ())
        .map_err(|e| format!("Rename failed: {e}"))
}

/// Delete a verse, fractal, or petal with cascade.
#[instrument(skip(db))]
pub(crate) async fn delete_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
) -> Result<(), String> {
    let result = match entity_type {
        EntityType::Verse => {
            async {
                // DELETE nodes under all petals under all fractals in this verse
                let fractals_sub = QueryBuilder::new()
                    .select(&["fractal_id"])
                    .from("fractal")
                    .filter(Filter::eq("verse_id", entity_id))
                    .build();
                let petals_sub = QueryBuilder::new()
                    .select(&["petal_id"])
                    .from("petal")
                    .filter(Filter::in_subquery("fractal_id", fractals_sub.clone()))
                    .build();

                let q = DeleteBuilder::delete_from("node")
                    .where_clause(Filter::in_subquery("petal_id", petals_sub))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("petal")
                    .where_clause(Filter::in_subquery("fractal_id", fractals_sub))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("fractal")
                    .where_clause(Filter::eq("verse_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("role")
                    .where_clause(Filter::starts_with("scope", format!("VERSE#{}", entity_id)))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("verse_member")
                    .where_clause(Filter::eq("verse_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("verse")
                    .where_clause(Filter::eq("verse_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                Ok::<(), surrealdb::Error>(())
            }
            .await
        }
        EntityType::Fractal => {
            async {
                let petals_sub = QueryBuilder::new()
                    .select(&["petal_id"])
                    .from("petal")
                    .filter(Filter::eq("fractal_id", entity_id))
                    .build();

                let q = DeleteBuilder::delete_from("node")
                    .where_clause(Filter::in_subquery("petal_id", petals_sub))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("petal")
                    .where_clause(Filter::eq("fractal_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("fractal")
                    .where_clause(Filter::eq("fractal_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                Ok(())
            }
            .await
        }
        EntityType::Petal => {
            async {
                let q = DeleteBuilder::delete_from("node")
                    .where_clause(Filter::eq("petal_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                let q = DeleteBuilder::delete_from("petal")
                    .where_clause(Filter::eq("petal_id", entity_id))
                    .build();
                exec_query(db, &q).await?;

                Ok(())
            }
            .await
        }
    };
    result.map_err(|e| format!("Delete failed: {e}"))
}

/// Update a fractal's description.
#[instrument(skip(db))]
pub(crate) async fn update_fractal_description_handler(
    db: &Db,
    fractal_id: &str,
    description: &str,
) -> anyhow::Result<()> {
    let q = UpdateBuilder::update("fractal")
        .set("description", description)
        .where_clause(Filter::eq("fractal_id", fractal_id))
        .build();
    exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("Update description failed: {e}"))?;
    Ok(())
}
