//! In-process fe-api integration harness — see src/AGENTS.md §api-harness.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use tower::ServiceExt;

use fe_api::server::{build_router, ApiState};
use fe_identity::NodeKeypair;
use fe_runtime::messages::ApiCommand;

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

/// Default TTL for harness-minted API tokens (1 hour).
const TOKEN_TTL_SECS: u64 = 3600;

/// IDs of a seeded verse → fractal → petal chain.
#[derive(Debug, Clone)]
pub struct SeededHierarchy {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
}

impl SeededHierarchy {
    /// Verse-level scope string covering the whole chain (`VERSE#<id>`).
    pub fn verse_scope(&self) -> String {
        format!("VERSE#{}", self.verse_id)
    }
}

/// A live in-process fe-api instance: real router + real auth middleware over
/// an in-memory SurrealDB. Requests go through `tower::ServiceExt::oneshot`.
pub struct ApiHarness {
    pub router: axum::Router,
    pub state: Arc<ApiState>,
    /// Same handle as `state.db_reader` — use for direct seeding/assertions.
    pub db: Arc<Db>,
    /// Signs API tokens; its verifying key is installed in `ApiState`.
    pub keypair: Arc<NodeKeypair>,
    /// Backing dir for the harness blob store; deleted on drop.
    pub blob_dir: tempfile::TempDir,
    // Held so ApiCommand sends fail fast-free (channel stays open; nothing consumes).
    _api_cmd_rx: crossbeam::channel::Receiver<ApiCommand>,
}

impl ApiHarness {
    /// Spin up a fresh harness: Mem SurrealDB + full schema + real router.
    pub async fn spawn() -> Result<Self> {
        let db: Db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .context("in-memory SurrealDB")?;
        db.use_ns("test").use_db("test").await.context("ns/db")?;
        fe_database::schema::apply_all(&db)
            .await
            .context("apply schema")?;
        fe_database::api_token_store::apply_api_token_schema(&db)
            .await
            .context("api token schema")?;
        let db = Arc::new(db);

        let keypair = Arc::new(NodeKeypair::generate());

        let blob_dir = tempfile::tempdir().context("blob tempdir")?;
        let blob_store: fe_runtime::blob_store::BlobStoreHandle = Arc::new(
            fe_sync::FsBlobStore::new(blob_dir.path().join("blobs")).context("blob store")?,
        );

        let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded::<ApiCommand>(64);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(16);

        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key: keypair.verifying_key(),
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: Some(blob_store),
            cors_origins: vec!["*".to_string()],
            db_reader: Some(db.clone()),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            share_signer: keypair.clone(),
        });

        let router = build_router(state.clone());

        Ok(Self {
            router,
            state,
            db,
            keypair,
            blob_dir,
            _api_cmd_rx: api_cmd_rx,
        })
    }

    /// DID of the harness identity (matches token `sub` and seeded `created_by`).
    pub fn did(&self) -> String {
        self.keypair.to_did_key()
    }

    // -- auth -----------------------------------------------------------------

    /// Mint a real signed API token the router's auth middleware accepts.
    pub fn mint_token(&self, scope: &str, max_role: &str) -> String {
        fe_identity::api_token::mint_api_token(
            &self.keypair,
            scope,
            max_role,
            TOKEN_TTL_SECS,
            &ulid::Ulid::new().to_string(),
        )
        .expect("mint api token")
    }

    // -- requests -------------------------------------------------------------

    /// Fire an arbitrary request through the full router (no network).
    pub async fn request(&self, req: Request<Body>) -> Response {
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router oneshot")
    }

    /// GET `path`; `token` adds a Bearer header. Returns (status, lenient JSON body).
    pub async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method("GET").uri(path);
        if let Some(t) = token {
            builder = builder.header("authorization", format!("Bearer {t}"));
        }
        let resp = self
            .request(builder.body(Body::empty()).expect("build request"))
            .await;
        let status = resp.status();
        (status, body_json_lenient(resp).await)
    }

    /// POST a JSON body to `path`. Returns (status, lenient JSON body).
    pub async fn post_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("authorization", format!("Bearer {t}"));
        }
        let resp = self
            .request(
                builder
                    .body(Body::from(body.to_string()))
                    .expect("build request"),
            )
            .await;
        let status = resp.status();
        (status, body_json_lenient(resp).await)
    }

    // -- seeding (same statements the fe-database handlers write) -------------

    /// Seed a verse row; returns its ULID.
    pub async fn seed_verse(&self, name: &str) -> Result<String> {
        let verse_id = ulid::Ulid::new().to_string();
        self.db
            .query(
                "CREATE verse CONTENT { verse_id: $vid, name: $name, \
                 created_by: $did, created_at: $now }",
            )
            .bind(("vid", verse_id.clone()))
            .bind(("name", name.to_string()))
            .bind(("did", self.did()))
            .bind(("now", now_rfc3339()))
            .await?
            .check()?;
        Ok(verse_id)
    }

    /// Seed a fractal row under `verse_id`; returns its ULID.
    pub async fn seed_fractal(&self, verse_id: &str, name: &str) -> Result<String> {
        let fractal_id = ulid::Ulid::new().to_string();
        self.db
            .query(
                "CREATE fractal CONTENT { fractal_id: $fid, verse_id: $vid, \
                 owner_did: $did, name: $name, created_at: $now }",
            )
            .bind(("fid", fractal_id.clone()))
            .bind(("vid", verse_id.to_string()))
            .bind(("did", self.did()))
            .bind(("name", name.to_string()))
            .bind(("now", now_rfc3339()))
            .await?
            .check()?;
        Ok(fractal_id)
    }

    /// Seed a petal row under `fractal_id`; returns its ULID.
    /// `terrain` (optional) is stored as the petal's terrain config JSON.
    pub async fn seed_petal(
        &self,
        fractal_id: &str,
        name: &str,
        terrain: Option<serde_json::Value>,
    ) -> Result<String> {
        let petal_id = ulid::Ulid::new().to_string();
        // Omit `terrain` entirely when absent (NONE), matching CreatePetal writes.
        let terrain_clause = if terrain.is_some() {
            ", terrain: $terrain"
        } else {
            ""
        };
        let sql = format!(
            "CREATE petal CONTENT {{ petal_id: $pid, fractal_id: $fid, name: $name, \
             node_id: $did, created_at: $now{terrain_clause} }}"
        );
        let mut q = self
            .db
            .query(sql)
            .bind(("pid", petal_id.clone()))
            .bind(("fid", fractal_id.to_string()))
            .bind(("name", name.to_string()))
            .bind(("did", self.did()))
            .bind(("now", now_rfc3339()));
        if let Some(t) = terrain {
            q = q.bind(("terrain", t));
        }
        q.await?.check()?;
        Ok(petal_id)
    }

    /// Seed a node at `[x, y, z]` (y = elevation) with optional properties.
    /// Geometry uses the mandatory cast — fe-database/src/AGENTS.md §geometry-inserts.
    pub async fn seed_node(
        &self,
        petal_id: &str,
        display_name: &str,
        position: [f64; 3],
        properties: Option<serde_json::Value>,
    ) -> Result<String> {
        let node_id = ulid::Ulid::new().to_string();
        let props_clause = if properties.is_some() {
            ", properties: $props"
        } else {
            ""
        };
        let sql = format!(
            "CREATE node CONTENT {{ node_id: $nid, petal_id: $pid, display_name: $name, \
             position: <geometry<point>> [$x, $z], elevation: $ele, \
             rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0], \
             interactive: true, created_at: $now{props_clause} }}"
        );
        let mut q = self
            .db
            .query(sql)
            .bind(("nid", node_id.clone()))
            .bind(("pid", petal_id.to_string()))
            .bind(("name", display_name.to_string()))
            .bind(("x", position[0]))
            .bind(("z", position[2]))
            .bind(("ele", position[1]))
            .bind(("now", now_rfc3339()));
        if let Some(p) = properties {
            q = q.bind(("props", p));
        }
        q.await?.check()?;
        Ok(node_id)
    }

    /// Seed a full verse → fractal → petal chain with generated names.
    pub async fn seed_hierarchy(&self) -> Result<SeededHierarchy> {
        let verse_id = self.seed_verse("Test Verse").await?;
        let fractal_id = self.seed_fractal(&verse_id, "Test Fractal").await?;
        let petal_id = self.seed_petal(&fractal_id, "Test Petal", None).await?;
        Ok(SeededHierarchy {
            verse_id,
            fractal_id,
            petal_id,
        })
    }
}

/// Collect a response body as raw bytes.
pub async fn body_bytes(resp: Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body")
        .to_vec()
}

/// Parse a response body as JSON; non-JSON (empty, parquet, …) → `Value::Null`.
pub async fn body_json_lenient(resp: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(resp).await).unwrap_or(serde_json::Value::Null)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
