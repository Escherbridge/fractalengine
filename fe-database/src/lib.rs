use fe_runtime::messages::{DbCommand, DbResult};
use surrealdb::engine::local::SurrealKv;

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
            let db = surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db")
                .await
                .expect("SurrealDB init");
            db.use_ns("fractalengine").use_db("fractalengine").await.expect("SurrealDB ns/db");
            tracing::info!("SurrealDB ready");
            tx.send(DbResult::Started).ok();
            loop {
                match rx.recv() {
                    Ok(DbCommand::Ping) => {
                        tx.send(DbResult::Pong).ok();
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Database thread shutting down");
            tx.send(DbResult::Stopped).ok();
        });
    })
}
