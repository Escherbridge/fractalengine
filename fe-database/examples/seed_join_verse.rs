//! Validation fixture: seed a relay's SurrealKV store with a joinable verse
//! (+ an optional Editor role row for a remote peer's DID) and READ-BACK rows
//! from a stopped relay's store.
//!
//! The relay opens a verse replica only when the verse row exists in its DB
//! AND the namespace secret resolves from the environment
//! (`FE_SECRET_FRACTALENGINE_VERSE_<ULID>_NS_SECRET_FRACTALENGINE`). REST
//! writes are JWT-gated and `create_verse_handler` generates a secret that
//! only lands in the process's in-memory secret-store override — an operator
//! cannot extract it — so a validator that controls both relays'
//! environments seeds the verse row directly instead, writing the same rows
//! through the same library helpers (`Repo`, same struct types).
//!
//! SurrealKV takes a per-handle file lock: run this only while the host
//! process is STOPPED (same rule as `dump_db`/`inspect_db`, §diagnostics).
//!
//! Usage:
//!   seed_join_verse gen
//!     → prints `verse_id: <ulid>` + `ns_secret: <64-hex>` (fresh values)
//!   seed_join_verse seed --db <path> --verse-id <ulid> --verse-name <name> \
//!       --created-by <did> --ns-secret-hex <64-hex> \
//!       [--role-peer-did <did> --role-scope <scope> --role-level <level>]
//!     → applies the schema, writes the verse row (namespace_id derived from
//!       the secret, exactly like `create_verse_handler`) + optional role row;
//!       prints `namespace_id: <hex>`
//!   seed_join_verse readback --db <path> --table <table> \
//!       [--field <field> --value <value>]
//!     → prints each matching row as one JSON line (all rows when no filter)

use surrealdb::engine::local::SurrealKv;

use fe_database::repo::Db;
use fe_database::schema::{Role, Verse};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

fn require(name: &str) -> anyhow::Result<String> {
    arg(name).ok_or_else(|| anyhow::anyhow!("missing required argument {name}"))
}

async fn open_db(path: &str) -> anyhow::Result<Db> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = surrealdb::Surreal::new::<SurrealKv>(path).await?;
    db.use_ns("fractalengine").use_db("fractalengine").await?;
    Ok(db)
}

fn parse_secret(hex_str: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(hex_str.trim())?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        anyhow::anyhow!("namespace secret must decode to 32 bytes, got {}", v.len())
    })
}

async fn run_gen() -> anyhow::Result<()> {
    let verse_id = ulid::Ulid::new().to_string();
    let mut secret = [0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut secret);
    println!("verse_id: {verse_id}");
    println!("ns_secret: {}", hex::encode(secret));
    Ok(())
}

async fn run_seed() -> anyhow::Result<()> {
    let db_path = require("--db")?;
    let verse_id = require("--verse-id")?;
    let verse_name = require("--verse-name")?;
    let created_by = require("--created-by")?;
    let ns_secret_hex = require("--ns-secret-hex")?;
    let role_peer_did = arg("--role-peer-did");
    let role_scope = arg("--role-scope");
    let role_level = arg("--role-level");

    verse_id.parse::<ulid::Ulid>()?;
    let secret = parse_secret(&ns_secret_hex)?;
    let namespace_id = hex::encode(fe_database::derive_namespace_id(&secret));

    let db = open_db(&db_path).await?;
    fe_database::schema::apply_all(&db).await?;

    // Idempotent: an existing verse row is left untouched (a re-run must not
    // clobber a store the relay already converged).
    let exists = fe_database::repo::Repo::<Verse>::find_by_id(&db, &verse_id).await?;
    if exists.is_some() {
        println!("verse_id: {verse_id} (already present, left as-is)");
    } else {
        fe_database::repo::Repo::<Verse>::create(
            &db,
            &Verse {
                verse_id: verse_id.clone(),
                name: verse_name,
                created_by,
                created_at: chrono::Utc::now().to_rfc3339(),
                namespace_id: Some(namespace_id.clone()),
                default_access: "viewer".to_string(),
                ts_mode: "mirror".to_string(),
                ts_replication_factor: 1,
                ts_bucket_width_ms: fe_runtime::DEFAULT_BUCKET_WIDTH_MS as i64,
            },
        )
        .await?;
        println!("verse_id: {verse_id} (seeded)");
    }
    println!("namespace_id: {namespace_id}");

    if let (Some(peer_did), Some(scope), Some(level)) = (role_peer_did, role_scope, role_level) {
        let mut res = db
            .query("SELECT * FROM role WHERE peer_did = $peer LIMIT 1")
            .bind(("peer", peer_did.clone()))
            .await?;
        let rows: Vec<serde_json::Value> = res.take(0)?;
        if rows.is_empty() {
            fe_database::repo::Repo::<Role>::create(
                &db,
                &Role {
                    peer_did: peer_did.clone(),
                    scope: scope.clone(),
                    role: level.clone(),
                },
            )
            .await?;
            println!("role: seeded peer_did={peer_did} scope={scope} level={level}");
        } else {
            db.query("UPDATE role SET scope = $scope, role = $level WHERE peer_did = $peer")
                .bind(("scope", scope.clone()))
                .bind(("level", level.clone()))
                .bind(("peer", peer_did.clone()))
                .await?
                .check()?;
            println!("role: updated peer_did={peer_did} scope={scope} level={level} (row existed)");
        }
    }
    Ok(())
}

async fn run_readback() -> anyhow::Result<()> {
    let db_path = require("--db")?;
    let table = require("--table")?;
    let field = arg("--field");
    let value = arg("--value");

    const TABLES: &[&str] = &[
        "verse",
        "fractal",
        "petal",
        "node",
        "role",
        "verse_member",
        "iot_reading",
    ];
    if !TABLES.contains(&table.as_str()) {
        anyhow::bail!("table must be one of {TABLES:?}, got {table}");
    }
    let mut sql = format!("SELECT * FROM {table}");
    if let Some(f) = field {
        if !f.chars().all(|c| c.is_ascii_lowercase() || c == '_') || f.len() < 2 {
            anyhow::bail!("field must be a lowercase identifier, got {f}");
        }
        sql.push_str(&format!(" WHERE {f} = $v"));
    }
    let rows = query_rows(&db_path, &sql, value).await?;
    print_rows(&rows);
    Ok(())
}

async fn query_rows(
    db_path: &str,
    sql: &str,
    value: Option<String>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let db = open_db(db_path).await?;
    let mut q = db.query(sql);
    if let Some(v) = value {
        q = q.bind(("v", v));
    }
    let mut res = q.await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows)
}

fn print_rows(rows: &[serde_json::Value]) {
    for row in rows {
        println!("row: {}", serde_json::to_string(row).unwrap_or_default());
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "gen" => run_gen().await,
        "seed" => run_seed().await,
        "readback" => run_readback().await,
        other => anyhow::bail!("unknown subcommand {other:?} (gen | seed | readback)"),
    }
}
