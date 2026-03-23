use fe_runtime::messages::{DbCommand, DbResult};
use surrealdb::engine::local::{Db, SurrealKv};

pub mod admin;
pub mod atlas;
pub mod model_url_meta;
pub mod op_log;
pub mod queries;
pub mod rbac;
pub mod schema;
pub mod space_manager;
pub mod types;

pub use atlas::*;
pub use model_url_meta::ModelUrlMeta;
pub use types::*;

#[derive(bevy::prelude::Resource, Clone)]
pub struct DbHandle(pub std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>);

pub fn spawn_db_thread(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
) -> std::thread::JoinHandle<()> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_db_thread must not be called from within a Tokio runtime"
    );
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build database Tokio runtime");
        rt.block_on(async {
            tracing::info!("Database thread started, initialising SurrealDB");
            let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
                surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db")
                    .await
                    .expect("SurrealDB init");
            db.use_ns("fractalengine")
                .use_db("fractalengine")
                .await
                .expect("SurrealDB ns/db");
            rbac::apply_schema(&db)
                .await
                .expect("Schema application failed");
            tracing::info!("SurrealDB ready");
            tx.send(DbResult::Started).ok();
            #[allow(clippy::while_let_loop)]
            loop {
                match rx.recv() {
                    Ok(DbCommand::Ping) => {
                        tx.send(DbResult::Pong).ok();
                    }
                    Ok(DbCommand::Seed) => {
                        match seed_default_data(&db).await {
                            Ok((petal_name, rooms)) => {
                                tx.send(DbResult::Seeded { petal_name, rooms }).ok();
                            }
                            Err(e) => {
                                tracing::error!("Seed failed: {e}");
                                tx.send(DbResult::Error(format!("Seed failed: {e}"))).ok();
                            }
                        }
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Database thread shutting down");
            tx.send(DbResult::Stopped).ok();
        });
    })
}

async fn seed_default_data(db: &surrealdb::Surreal<Db>) -> anyhow::Result<(String, Vec<String>)> {
    let node_id = types::NodeId("local-node".to_string());
    let petal_name = "Genesis Petal";
    let room_names = vec!["Lobby", "Workshop", "Gallery"];

    let petal_id = queries::create_petal(db, petal_name, &node_id).await?;
    tracing::info!("Seeded petal: {petal_name} ({})", petal_id.0);

    for room_name in &room_names {
        queries::create_room(db, &petal_id, room_name).await?;
        tracing::info!("Seeded room: {room_name}");
    }

    // Assign owner role to the local node
    let _: Option<serde_json::Value> = db
        .create("role")
        .content(serde_json::json!({
            "node_id": node_id.0,
            "petal_id": petal_id.0.to_string(),
            "role": "owner",
        }))
        .await?;
    tracing::info!("Assigned owner role to {}", node_id.0);

    Ok((
        petal_name.to_string(),
        room_names.iter().map(|s| s.to_string()).collect(),
    ))
}
