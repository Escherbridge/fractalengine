//! Diagnostic: dump nodes, assets, petals from the local SurrealDB.
//! Run with: cargo run -p fe-database --example dump_db
use surrealdb::engine::local::SurrealKv;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db").await?;
    db.use_ns("fractalengine").use_db("fractalengine").await?;

    println!("=== VERSES ===");
    let mut r = db.query("SELECT verse_id, name FROM verse").await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    for v in &rows {
        println!("  {} : {}", v["verse_id"], v["name"]);
    }

    println!("\n=== PETALS ===");
    let mut r = db
        .query("SELECT petal_id, fractal_id, name FROM petal")
        .await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    for p in &rows {
        println!(
            "  {} (fractal={}) : {}",
            p["petal_id"], p["fractal_id"], p["name"]
        );
    }

    println!("\n=== NODES ===");
    let mut r = db.query("SELECT node_id, petal_id, display_name, asset_id, position, elevation FROM node").await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    for n in &rows {
        println!(
            "  node={} petal={} name={} asset_id={} pos={} elev={}",
            n["node_id"],
            n["petal_id"],
            n["display_name"],
            n["asset_id"],
            n["position"],
            n["elevation"]
        );
    }

    println!("\n=== ASSETS (name only) ===");
    let mut r = db
        .query("SELECT asset_id, name, size_bytes FROM asset")
        .await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    for a in &rows {
        println!("  {} : {} ({}b)", a["asset_id"], a["name"], a["size_bytes"]);
    }

    println!("\n=== IMPORTED FILES ON DISK ===");
    let dir = std::path::Path::new("fractalengine/assets/imported");
    if dir.exists() {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            println!("  {}", e.file_name().to_string_lossy());
        }
    } else {
        println!("  (directory does not exist)");
    }

    Ok(())
}
