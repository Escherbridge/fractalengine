//! Read-only DB inspector: table counts, petal/node distribution, property
//! histograms, disk size. See fe-database/src/AGENTS.md §diagnostics.
//! Run: FE_DB_PATH=data/fractalengine.db cargo run -p fe-database --example inspect_db
use surrealdb::engine::local::{Db, SurrealKv};
use surrealdb::Surreal;

/// Recursive on-disk size of a file or directory, in bytes.
fn dir_size(path: &std::path::Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    std::fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum())
        .unwrap_or(0)
}

/// `SELECT count() FROM <table> GROUP ALL` -> total row count.
async fn count_table(db: &Surreal<Db>, table: &str) -> anyhow::Result<i64> {
    let mut r = db
        .query(format!("SELECT count() FROM {table} GROUP ALL"))
        .await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    Ok(rows.first().and_then(|v| v["count"].as_i64()).unwrap_or(0))
}

/// Run a `SELECT <k> AS k, count() AS n ... GROUP BY k` query and return
/// (k, n) pairs sorted by count descending.
async fn grouped(db: &Surreal<Db>, sql: &str) -> anyhow::Result<Vec<(String, i64)>> {
    let mut r = db.query(sql).await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    let mut out: Vec<(String, i64)> = rows
        .iter()
        .map(|v| {
            let k = match &v["k"] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => "(none)".to_string(),
                other => other.to_string(),
            };
            (k, v["n"].as_i64().unwrap_or(0))
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.1));
    Ok(out)
}

fn print_top(label: &str, rows: &[(String, i64)], limit: usize) {
    println!("\n=== {label} ===");
    if rows.is_empty() {
        println!("  (no rows)");
    }
    for (k, n) in rows.iter().take(limit) {
        println!("  {n:>10}  {k}");
    }
    if rows.len() > limit {
        println!("  ... {} more groups", rows.len() - limit);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::var("FE_DB_PATH").unwrap_or_else(|_| "data/fractalengine.db".to_string());
    let disk = dir_size(std::path::Path::new(&path));
    println!("DB path: {path}");
    println!(
        "DB size on disk: {disk} bytes ({:.2} MB)",
        disk as f64 / 1_048_576.0
    );

    let db = Surreal::new::<SurrealKv>(path.as_str()).await?;
    db.use_ns("fractalengine").use_db("fractalengine").await?;

    // Discover tables via INFO FOR DB.
    let mut r = db.query("INFO FOR DB").await?;
    let info: Option<serde_json::Value> = r.take(0)?;
    let mut tables: Vec<String> = info
        .as_ref()
        .and_then(|i| i["tables"].as_object())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    tables.sort();

    println!("\n=== TABLES ({}) ===", tables.len());
    for t in &tables {
        match count_table(&db, t).await {
            Ok(n) => println!("  {n:>10}  {t}"),
            Err(e) => println!("       ERR   {t}: {e}"),
        }
    }

    // Top petals by node count, joined with petal names.
    let mut r = db.query("SELECT petal_id, name FROM petal").await?;
    let petals: Vec<serde_json::Value> = r.take(0)?;
    let name_of = |id: &str| -> String {
        petals
            .iter()
            .find(|p| p["petal_id"].as_str() == Some(id))
            .and_then(|p| p["name"].as_str())
            .unwrap_or("?")
            .to_string()
    };
    let by_petal = grouped(
        &db,
        "SELECT petal_id AS k, count() AS n FROM node GROUP BY k",
    )
    .await?;
    println!("\n=== TOP 10 PETALS BY NODE COUNT ===");
    for (id, n) in by_petal.iter().take(10) {
        println!("  {n:>10}  {id}  ({})", name_of(id));
    }

    // Node histograms: candidate kind/type properties, asset usage, property keys.
    for (label, sql) in [
        (
            "NODES BY properties.kind",
            "SELECT properties.kind AS k, count() AS n FROM node GROUP BY k",
        ),
        (
            "NODES BY properties.type",
            "SELECT properties.type AS k, count() AS n FROM node GROUP BY k",
        ),
        (
            "NODES BY asset_id",
            "SELECT asset_id AS k, count() AS n FROM node GROUP BY k",
        ),
    ] {
        match grouped(&db, sql).await {
            Ok(rows) => print_top(label, &rows, 15),
            Err(e) => println!("\n=== {label} ===\n  ERR: {e}"),
        }
    }

    // Histogram of property KEY names across all nodes (cheap: names only).
    let mut r = db
        .query("SELECT object::keys(properties) AS keys FROM node WHERE properties != NONE")
        .await?;
    let rows: Vec<serde_json::Value> = r.take(0)?;
    let mut key_hist: std::collections::HashMap<String, i64> = Default::default();
    for row in &rows {
        if let Some(keys) = row["keys"].as_array() {
            for k in keys.iter().filter_map(|k| k.as_str()) {
                *key_hist.entry(k.to_string()).or_default() += 1;
            }
        }
    }
    let mut key_rows: Vec<(String, i64)> = key_hist.into_iter().collect();
    key_rows.sort_by_key(|x| std::cmp::Reverse(x.1));
    print_top(
        "NODE PROPERTY KEYS (nodes carrying each key)",
        &key_rows,
        25,
    );

    // Largest embedded gpx_points arrays (big persisted tracks).
    let mut r = db
        .query(
            "SELECT node_id, petal_id, display_name, \
             array::len(properties.gpx_points ?? []) AS n \
             FROM node WHERE properties.gpx_points != NONE",
        )
        .await?;
    let mut gpx: Vec<serde_json::Value> = r.take(0)?;
    gpx.sort_by_key(|v| -v["n"].as_i64().unwrap_or(0));
    println!("\n=== TOP 10 NODES BY gpx_points LENGTH ===");
    if gpx.is_empty() {
        println!("  (no gpx_points properties)");
    }
    for g in gpx.iter().take(10) {
        println!(
            "  {:>8} pts  node={} petal={} name={}",
            g["n"], g["node_id"], g["petal_id"], g["display_name"]
        );
    }

    // node_log: ops histogram + hottest nodes.
    match grouped(&db, "SELECT op AS k, count() AS n FROM node_log GROUP BY k").await {
        Ok(rows) => print_top("NODE_LOG BY op", &rows, 15),
        Err(e) => println!("\n=== NODE_LOG BY op ===\n  ERR: {e}"),
    }
    match grouped(
        &db,
        "SELECT node_id AS k, count() AS n FROM node_log GROUP BY k",
    )
    .await
    {
        Ok(rows) => print_top("TOP NODES BY node_log ROWS", &rows, 10),
        Err(e) => println!("\n=== TOP NODES BY node_log ROWS ===\n  ERR: {e}"),
    }

    // Assets: per-row sizes + total.
    let mut r = db
        .query("SELECT asset_id, name, size_bytes, content_type FROM asset")
        .await?;
    let mut assets: Vec<serde_json::Value> = r.take(0)?;
    assets.sort_by_key(|v| -v["size_bytes"].as_i64().unwrap_or(0));
    let total: i64 = assets
        .iter()
        .map(|a| a["size_bytes"].as_i64().unwrap_or(0))
        .sum();
    println!(
        "\n=== ASSETS ({} rows, {:.2} MB declared) ===",
        assets.len(),
        total as f64 / 1_048_576.0
    );
    for a in assets.iter().take(15) {
        println!(
            "  {:>10} b  {}  {}  {}",
            a["size_bytes"], a["asset_id"], a["content_type"], a["name"]
        );
    }

    println!("\ndone.");
    Ok(())
}
