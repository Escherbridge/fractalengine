//! R9-CLI: one operator journey through `fe_cli::run` against a real fe-api router on 127.0.0.1:0, backed by the
//! real DB thread (SurrealKV in a tempdir) and the production API command relay. The only stand-in is the plugin
//! host (`ExtensionHost`), the process edge the CLI never links. See tools/fe-cli/AGENTS.md §journey.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fe_api::extensions::{
    AdminOp, AdminRefusal, ApiCaller, CommandFailure, CommandReply, ExtensionHost, HostRefusal, IngressAnswer,
    IngressCall, IngressRefusal,
};
use fe_api::server::{build_router_with_hosts, ApiHosts, ApiState};
use fe_identity::NodeKeypair;
use fe_runtime::app::{spawn_api_command_relay, PendingApiRequests};
use fe_runtime::blob_store::{canonical_blob_path, BlobHash, BlobStore};
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

const PACKAGE: &str = "org.example.fleet-sim";
const TOKEN_TTL_SECS: u64 = 3600;

// ---------------------------------------------------------------------------
// Process-edge stand-ins: blob files and the plugin host
// ---------------------------------------------------------------------------

/// Content-addressed blob files under a tempdir (the DB thread's blob seam).
struct DirBlobs(PathBuf);

impl BlobStore for DirBlobs {
    fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
        let hash = *blake3::hash(bytes).as_bytes();
        let path = canonical_blob_path(&self.0, &hash);
        std::fs::create_dir_all(path.parent().expect("blob parent"))?;
        std::fs::write(path, bytes)?;
        Ok(hash)
    }
    fn get_blob_path(&self, hash: &BlobHash) -> Option<PathBuf> {
        Some(canonical_blob_path(&self.0, hash)).filter(|path| path.is_file())
    }
    fn has_blob(&self, hash: &BlobHash) -> bool {
        self.get_blob_path(hash).is_some()
    }
    fn remove_blob(&self, hash: &BlobHash) -> anyhow::Result<()> {
        Ok(std::fs::remove_file(canonical_blob_path(&self.0, hash))?)
    }
}

/// One package exposing `spawn` (role floor editor) at `petal`, answering like the bus does; `refuse`, `throttle`
/// and `vanish` answer a guest refusal, a pre-admission rate refusal and an unknown outcome.
struct FakeHost {
    local_did: String,
    petal: Mutex<String>,
    /// Calls that reached `call_command`, per command id.
    calls: Mutex<BTreeMap<String, usize>>,
}

/// The guest's refusal text the CLI must carry verbatim.
const GUEST_REFUSAL: &str = "fleet is parked for maintenance";

fn at_least_editor(caller: &ApiCaller) -> bool {
    use fe_policy::RoleLevel;
    matches!(caller.role_ceiling, RoleLevel::Owner | RoleLevel::Manager | RoleLevel::Editor)
}

impl ExtensionHost for FakeHost {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }
    fn admit_remote(&self, _did: &str) -> bool {
        true
    }
    fn list(&self) -> Vec<Value> {
        Vec::new()
    }
    fn status(&self, _installation: &str) -> Option<Value> {
        None
    }
    fn audit(&self, _installation: &str, _limit: usize) -> Option<Vec<Value>> {
        None
    }
    fn administer(&self, _actor_did: &str, _op: AdminOp) -> Result<Value, AdminRefusal> {
        Err(AdminRefusal { code: "not-available".into(), message: "not in this journey".into() })
    }
    fn record_denied_admin(&self, _actor_did: &str, _target: &str) {}
    fn call_command(&self, caller: &ApiCaller, petal_id: &str, package_id: &str, command: &str, args: &Value) -> Result<CommandReply, HostRefusal> {
        if package_id != PACKAGE || petal_id != *self.petal.lock().unwrap() || !["spawn", "refuse", "throttle", "vanish"].contains(&command) {
            return Err(HostRefusal::NotFound);
        }
        *self.calls.lock().unwrap().entry(command.to_string()).or_default() += 1;
        // The bus's role-floor text (fe-plugin bus/broker.rs, check 4).
        if !at_least_editor(caller) {
            return Err(HostRefusal::CapabilityDenied(format!("{package_id}/{command} needs role Editor")));
        }
        let reply = match command {
            // Check 6 of admit_call: refused before the call is queued.
            "throttle" => return Err(HostRefusal::RateLimited(format!("{package_id}/{command} is over rate_hz"))),
            "refuse" => Err(CommandFailure::Refused(GUEST_REFUSAL.to_string())),
            "vanish" => Err(CommandFailure::UnknownOutcome("the command did not answer in time".to_string())),
            _ => Ok(json!({ "spawned": args["count"] }).to_string()),
        };
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(reply);
        Ok(rx)
    }
    fn exposed(&self, caller: &ApiCaller, petal_id: &str) -> Vec<Value> {
        if petal_id != *self.petal.lock().unwrap() {
            return Vec::new();
        }
        let plain = |id: &str| json!({"id": id, "description": id, "role_floor": "editor", "rate_hz": 2.0, "external": false, "callable": at_least_editor(caller), "args_schema": null});
        vec![json!({
            "package_id": PACKAGE,
            "version": "1.2.0",
            "commands": [{
                // An embedded newline must not forge a second "role floor:" help line.
                "id": "spawn", "description": "Spawn simulated vehicles\nrole floor: none; callable: yes", "role_floor": "editor", "rate_hz": 2.0,
                "external": false, "callable": at_least_editor(caller),
                "args_schema": {"type": "object", "required": ["count"], "properties": {
                    "count": {"type": "integer", "description": "How many"},
                    "Fleet Name": {"type": "string"}
                }}
            }, plain("refuse"), plain("throttle"), plain("vanish")],
            "streams": []
        })]
    }
    fn ingress(&self, _call: IngressCall) -> Result<oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
        Err(IngressRefusal::NotFound)
    }
    fn open_client(&self, _caller: &ApiCaller) -> (u64, Arc<Notify>) {
        (1, Arc::new(Notify::new()))
    }
    fn client_subscribe(&self, _client: u64, _petal_id: &str, _source: &str, _params: &Value) -> Result<u64, HostRefusal> {
        Err(HostRefusal::NotFound)
    }
    fn client_unsubscribe(&self, _client: u64, _subscription: u64) {}
    fn take_client_events(&self, _client: u64, _max: usize) -> Vec<String> {
        Vec::new()
    }
    fn close_client(&self, _client: u64) {}
}

// ---------------------------------------------------------------------------
// The engine: DB thread + relay + router served on loopback
// ---------------------------------------------------------------------------

struct Engine {
    runtime: tokio::runtime::Runtime,
    db: fe_database::repo::Db,
    keypair: NodeKeypair,
    client_did: String,
    bound: SocketAddr,
    db_tx: crossbeam::channel::Sender<DbCommand>,
    host: Arc<FakeHost>,
    db_join: Option<std::thread::JoinHandle<()>>,
    _relay: fe_runtime::service_lifecycle::OwnedService,
}

impl Engine {
    fn start(root: &Path) -> Engine {
        let keypair = NodeKeypair::generate();
        let seed = keypair.seed_bytes();
        let (db_tx, db_rx) = crossbeam::channel::unbounded::<DbCommand>();
        let (result_tx, result_rx) = crossbeam::channel::unbounded::<DbResult>();
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(64);
        let blobs: fe_runtime::blob_store::BlobStoreHandle = Arc::new(DirBlobs(root.join("blobs")));
        // Must spawn outside any tokio runtime.
        let db_thread = fe_database::spawn_db_thread_shared(
            db_rx,
            result_tx,
            blobs.clone(),
            None,
            Some(NodeKeypair::from_bytes(&seed).expect("keypair copy")),
            None,
            Some(entity_change_tx.clone()),
            Some(root.join("db").to_string_lossy().into_owned()),
            None,
        )
        .expect("db thread");

        let pending = PendingApiRequests::default();
        let mut delivery = pending.clone();
        // Mirrors `fe_runtime::app::deliver_pending_api_results` (the headless host's result delivery).
        std::thread::spawn(move || {
            while let Ok(result) = result_rx.recv() {
                if let DbResult::HierarchyLoaded { verses } | DbResult::HierarchyRead { verses } = &result {
                    delivery.deliver_hierarchy(verses.to_vec());
                }
                delivery.try_deliver(result);
            }
        });
        let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded::<ApiCommand>(64);
        let relay = spawn_api_command_relay(api_cmd_rx, db_tx.clone(), pending);

        let state = Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx: tokio::sync::broadcast::channel(16).0,
            entity_change_tx,
            verifying_key: keypair.verifying_key(),
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(Default::default())),
            blob_store: Some(blobs),
            cors_origins: vec!["*".to_string()],
            db_reader: Some(Arc::new(fe_api::db_reader::ReadDb::new(db_thread.shared_db.clone()))),
            terrain_capture: Default::default(),
            query_rate_limiter: tokio::sync::Mutex::new(Default::default()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            share_signer: Arc::new(NodeKeypair::generate()),
            elevated_query_allowed: false,
            upload_quota_bytes: fe_api::upload::DEFAULT_UPLOAD_QUOTA_BYTES,
            screenshot: fe_api::screenshot::ScreenshotSurface::disabled(),
            input: fe_api::input::InputSurface::disabled(),
        });
        let host = Arc::new(FakeHost { local_did: keypair.to_did_key(), petal: Mutex::new(String::new()), calls: Mutex::default() });
        let hosts = ApiHosts { extension: Some(host.clone()), ..ApiHosts::default() };
        let router = build_router_with_hosts(state, hosts);

        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).expect("bind loopback");
        let bound = listener.local_addr().expect("bound");
        runtime.spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let client_did = NodeKeypair::generate().to_did_key();
        Engine { runtime, db: db_thread.shared_db, keypair, client_did, bound, db_tx, host, db_join: Some(db_thread.join_handle), _relay: relay }
    }

    /// Stops the DB thread so the store files close before the tempdir is removed.
    fn shutdown(mut self) {
        let _ = self.db_tx.send(DbCommand::Shutdown);
        if let Some(join) = self.db_join.take() {
            let _ = join.join();
        }
    }

    /// Setup only (not the seam under test): a verse created on the DB thread, as the GUI would.
    fn bootstrap_verse(&self, name: &str) {
        self.db_tx.send(DbCommand::CreateVerse { name: name.to_string() }).expect("db alive");
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            let db = self.db.clone();
            let named = name.to_string();
            let landed = self.runtime.block_on(async move {
                let mut reply = db.query("SELECT VALUE verse_id FROM verse WHERE name = $name").bind(("name", named)).await.expect("verse query");
                !reply.take::<Vec<String>>(0).unwrap_or_default().is_empty()
            });
            if landed {
                return;
            }
        }
        panic!("bootstrap verse never landed");
    }

    fn verse_ids(&self) -> Vec<String> {
        let db = self.db.clone();
        self.runtime.block_on(async move {
            let mut reply = db.query("SELECT VALUE verse_id FROM verse").await.expect("verse query");
            reply.take::<Vec<String>>(0).unwrap_or_default()
        })
    }

    /// The engine's dev-token writer (fe_api::dev_token), re-run as the GUI does when verses appear.
    fn emit_tokens(&self, path: &Path) {
        let verse_ids = self.verse_ids();
        self.runtime
            .block_on(fe_api::dev_token::emit_dev_tokens(path, self.bound, &self.db, &self.keypair, &self.client_did, &verse_ids))
            .expect("emit dev tokens");
    }

    /// A joined viewer's token file: the same node identity, capped at viewer for one verse.
    fn write_viewer_tokens(&self, path: &Path, verse_id: &str) {
        let generation = self.runtime.block_on(fe_database::api_auth::current_generation(&self.db)).expect("generation");
        let jti = fe_database::api_auth::new_jti(&generation).expect("jti");
        let token = fe_identity::api_token::mint_client_api_token(
            &self.keypair,
            &format!("VERSE#{verse_id}"),
            "viewer",
            TOKEN_TTL_SECS,
            &jti,
            &self.client_did,
        )
        .expect("viewer token");
        let document = json!({
            "api_base": format!("http://{}", self.bound),
            "tokens": { verse_id: token },
            "roles": { verse_id: "viewer" },
            "client": self.client_did,
        });
        write_file(path, &document.to_string());
    }
}

// ---------------------------------------------------------------------------
// Driving the CLI in-process
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One `fe` invocation: exit code, the parsed `--json` envelope (Null when stdout is not JSON), and stderr.
fn fe(env: &BTreeMap<String, String>, argv: &[&str]) -> (i32, Value, String) {
    let (stdout, stderr) = (Sink::default(), Sink::default());
    let exit = fe_cli::run(fe_cli::Invocation {
        argv: argv.iter().map(|word| word.to_string()).collect(),
        env: env.clone(),
        stdout: Box::new(stdout.clone()),
        stderr: Box::new(stderr.clone()),
    });
    let out = String::from_utf8(stdout.0.lock().unwrap().clone()).unwrap();
    let err = String::from_utf8(stderr.0.lock().unwrap().clone()).unwrap();
    (exit, serde_json::from_str(out.trim()).unwrap_or(Value::Null), err)
}

/// Asserts the exit code, naming the envelope on failure; returns the envelope.
fn expect(env: &BTreeMap<String, String>, argv: &[&str], exit: i32) -> Value {
    let (got, envelope, stderr) = fe(env, argv);
    assert_eq!(got, exit, "fe {argv:?}\nenvelope: {envelope}\nstderr: {stderr}");
    assert_eq!(envelope["exit"], json!(exit), "fe {argv:?}: envelope exit");
    envelope
}

/// `(x, y, z)` of a `read_node` reply: the row stores x/z as a point and y as `elevation`.
fn position(envelope: &Value) -> [f64; 3] {
    let row = &envelope["data"]["data"];
    let point = row["position"].get("coordinates").unwrap_or(&row["position"]);
    let number = |value: &Value| value.as_f64().unwrap_or_else(|| panic!("not a number in {row}"));
    [number(&point[0]), number(&row["elevation"]), number(&point[1])]
}

/// Writes a token document owner-only, as the engine does (the CLI refuses a group/other-readable one on Unix).
fn write_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn calls(engine: &Engine, command: &str) -> usize {
    engine.host.calls.lock().unwrap().get(command).copied().unwrap_or(0)
}

fn id_of(envelope: &Value) -> String {
    envelope["data"]["id"].as_str().unwrap_or_else(|| panic!("no id in {envelope}")).to_string()
}

#[test]
fn cli_operator_journey() {
    // Snapshot bless happens first so a failing later leg cannot skip it; the drift assertions are the last leg.
    let live: Vec<Value> = fe_api::mcp::tool_specs()
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": (spec.input_schema)(),
                "min_role": format!("{:?}", spec.min_role),
                "scope_rule": format!("{:?}", spec.scope_rule),
            })
        })
        .collect();
    let snapshot_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog").join("mcp-tools.json");
    if std::env::var("FE_CLI_BLESS").as_deref() == Ok("1") {
        let mut text = serde_json::to_string_pretty(&live).unwrap();
        text.push('\n');
        std::fs::write(&snapshot_path, text).expect("bless snapshot");
    }
    let root = tempfile::tempdir().expect("tempdir");
    let engine = Engine::start(root.path());
    engine.bootstrap_verse("Campus");
    let owner_tokens = root.path().join("owner").join("tokens.json");
    engine.emit_tokens(&owner_tokens);
    let journal = root.path().join("journal.jsonl");

    let env = |instance: &Path, agent: &str| -> BTreeMap<String, String> {
        BTreeMap::from([
            ("FE_INSTANCE".to_string(), instance.to_string_lossy().into_owned()),
            ("FE_CONTEXT".to_string(), root.path().join(format!("{agent}-ctx.json")).to_string_lossy().into_owned()),
            ("FE_STATE_DIR".to_string(), root.path().join("state").to_string_lossy().into_owned()),
            ("FE_JOURNAL".to_string(), journal.to_string_lossy().into_owned()),
            ("FE_AGENT_ID".to_string(), agent.to_string()),
            ("FE_CWD".to_string(), root.path().to_string_lossy().into_owned()),
        ])
    };
    let owner = env(&owner_tokens, "owner");

    // 1. A token file pointing anywhere but an IP-literal loopback address is refused before any request (exit 3).
    for remote in ["http://10.0.0.7:8765", "http://localhost:8765"] {
        let file = root.path().join("remote").join("tokens.json");
        write_file(&file, &json!({"api_base": remote, "tokens": {"v": "secret-remote-token"}, "roles": {"v": "owner"}}).to_string());
        let refused = expect(&env(&file, "remote"), &["verse", "list", "--json"], 3);
        assert_eq!(refused["code"], "not-loopback", "{remote}");
    }

    // 2. Select the bootstrap verse by name, then build the hierarchy with context-filled parents.
    let selected = expect(&owner, &["use", "verse", "Campus", "--json"], 0);
    let campus = selected["data"]["context"]["verse"]["id"].as_str().expect("verse id").to_string();
    let north = id_of(&expect(&owner, &["fractal", "create", "North", "--json"], 0));
    expect(&owner, &["use", "fractal", &north, "--json"], 0);
    let plaza = id_of(&expect(&owner, &["petal", "create", "Plaza", "--json"], 0));
    let used = expect(&owner, &["use", "petal", "Plaza", "--json"], 0);
    assert_eq!(used["context"]["petal_id"], json!(plaza), "the context now names the new petal");
    let listed = expect(&owner, &["petal", "list", "--json"], 0);
    assert!(listed["data"]["petals"].as_array().unwrap().iter().any(|petal| petal["id"] == json!(plaza)));

    // 3. Node lifecycle; global flags work before and after the command words.
    let tower = id_of(&expect(&owner, &["--json", "node", "create", "Tower", "--position", "1,2,3"], 0));
    assert_eq!(position(&expect(&owner, &["node", "get", &tower, "--json"], 0)), [1.0, 2.0, 3.0]);
    let moved = expect(&owner, &["node", "move", &tower, "--x", "5", "--wait", "local", "--json"], 0);
    assert_eq!(moved["wait"]["settled"], json!(true), "read back on the same instance: {moved}");
    assert_eq!(position(&expect(&owner, &["--json", "node", "get", &tower], 0)), [5.0, 2.0, 3.0]);
    let set = expect(&owner, &["node", "set", &tower, "color", "red", "--wait", "local", "--json"], 0);
    assert_eq!(set["wait"]["settled"], json!(true), "get_properties shows the value: {set}");

    // 4. FE_WAIT_DEFAULT=peers (a harness default) with no peer instance discoverable degrades writes to local and
    //    says so; reads ignore it; an explicit --wait peers with no peer is refused before the write (exit 6).
    let mut peers_default = owner.clone();
    peers_default.insert("FE_WAIT_DEFAULT".into(), "peers".into());
    let defaulted = expect(&peers_default, &["node", "unset", &tower, "color", "--json"], 0);
    assert_eq!((defaulted["wait"]["requested"].clone(), defaulted["wait"]["settled"].clone()), (json!("peers"), json!(true)));
    assert_eq!(expect(&peers_default, &["node", "get", &tower, "--json"], 0)["wait"], Value::Null);
    let no_peers = expect(&peers_default, &["node", "set", &tower, "k", "v", "--wait", "peers", "--json"], 6);
    assert_eq!(no_peers["code"], "no-peers");

    let deleted = expect(&owner, &["node", "delete", &tower, "--wait", "local", "--json"], 0);
    assert_eq!(deleted["wait"]["settled"], json!(true));
    assert_eq!(expect(&owner, &["node", "get", &tower, "--json"], 5)["code"], "not-found-or-denied");

    // 5. A second verse via `fe verse create`; the engine's token writer then covers it (as the GUI rewrites the file).
    let harbor = id_of(&expect(&owner, &["verse", "create", "Harbor", "--json"], 0));
    engine.emit_tokens(&owner_tokens);
    let docks = id_of(&expect(&owner, &["fractal", "create", "Docks", "--verse", &harbor, "--json"], 0));
    let pier = id_of(&expect(&owner, &["petal", "create", "Pier", "--verse", &harbor, "--fractal", &docks, "--json"], 0));
    let crate_node = id_of(&expect(&owner, &["node", "create", "Crate", "--verse", &harbor, "--petal", &pier, "--json"], 0));

    // 6. A viewer of Campus: writes are role-refused (4); Harbor's node is the uniform not-found (5).
    let viewer_tokens = root.path().join("viewer").join("tokens.json");
    engine.write_viewer_tokens(&viewer_tokens, &campus);
    let viewer = env(&viewer_tokens, "viewer");
    let denied = expect(&viewer, &["node", "create", "Kiosk", "--verse", &campus, "--petal", &plaza, "--json"], 4);
    assert_eq!(denied["message"], "insufficient permissions");
    let hidden = expect(&viewer, &["node", "get", &crate_node, "--verse", &campus, "--json"], 5);
    assert_eq!(hidden["message"], "not found or not permitted");

    // 6b. `--wait peers`: the same read-back runs on every other member of .qa/peers-current with that member's own
    //     token. `viewer` sees the write (converged, 0); `ghost` holds no token for the verse and never does, so the
    //     budget expires (9) while the write itself has landed; a peer name nobody has is refused before sending (6).
    let lab = root.path().join("lab");
    let peerset = root.path().join("peerset");
    let ghost_tokens = root.path().join("ghost").join("tokens.json");
    write_file(&ghost_tokens, &json!({"api_base": format!("http://{}", engine.bound), "tokens": {}, "roles": {}}).to_string());
    let member = |name: &str, tokens: &Path| json!({"name": name, "api_base": format!("http://{}", engine.bound), "tokens_path": tokens.to_string_lossy()});
    let members = json!({"members": [member("owner", &owner_tokens), member("viewer", &viewer_tokens), member("ghost", &ghost_tokens)]});
    write_file(&peerset.join("peers.json"), &members.to_string());
    write_file(&lab.join(".qa").join("peers-current"), &peerset.to_string_lossy());
    let mut peered = owner.clone();
    peered.insert("FE_CWD".into(), lab.to_string_lossy().into_owned());
    let converged = expect(&peered, &["node", "create", "Beacon", "--wait", "peers=viewer", "--json"], 0);
    assert_eq!((converged["wait"]["mode"].clone(), converged["wait"]["settled"].clone()), (json!("peers"), json!(true)), "{converged}");
    assert_eq!(converged["wait"]["peers"][0], json!({"name": "viewer", "settled": true, "ms": converged["wait"]["peers"][0]["ms"].clone()}));
    let beacon = id_of(&converged);
    let late = expect(&peered, &["node", "set", &beacon, "mood", "calm", "--wait", "peers=ghost", "--wait-budget", "1s", "--json"], 9);
    assert_eq!((late["code"].clone(), late["wait"]["settled"].clone(), late["wait"]["peers"][0]["settled"].clone()), (json!("wait-expired"), json!(false), json!(false)), "{late}");
    let landed = expect(&owner, &["node", "props", &beacon, "--json"], 0);
    assert!(landed["data"].to_string().contains("calm"), "the write landed although the wait expired: {landed}");
    assert_eq!(expect(&peered, &["node", "create", "Nobody", "--wait", "peers=nobody", "--json"], 6)["code"], "unknown-peer");
    // Phase-2 rows are table-driven: a dry run shows the planned MCP call (file bytes elided, no token printed).
    let glb = root.path().join("tiny.glb");
    std::fs::write(&glb, b"glTF").unwrap();
    let planned = expect(&owner, &["asset", "upload", "--file", &glb.to_string_lossy(), "--dry-run", "--json"], 0);
    assert_eq!(planned["data"]["planned"]["tool"], "upload_asset");
    assert!(planned["data"]["planned"]["args"]["data_base64"].as_str().unwrap().starts_with('<'), "{planned}");
    let group_plan = expect(&owner, &["group", "delete", "g1", "--dry-run", "--json"], 0);
    assert_eq!((group_plan["data"]["planned"]["args"]["delete"].clone(), group_plan["data"]["planned"]["args"]["group_id"].clone()), (json!(true), json!("g1")));

    // An invite is a credential: a dry run prints it nowhere, in either flag spelling or inside --args-json.
    let secret_invite = "fe-join-v2:CANARY-INVITE-0123456789";
    let join_plan = expect(&owner, &["join", "--invite", secret_invite, "--dry-run", "--json"], 0);
    assert!(!join_plan.to_string().contains("CANARY-INVITE"), "invite printed by a dry run: {join_plan}");
    expect(&owner, &["join", &format!("--invite={secret_invite}"), "--dry-run", "--json"], 0);
    let args_json = format!("{{\"invite\":\"{secret_invite}\"}}");
    expect(&owner, &["join", "--args-json", &args_json, "--dry-run", "--json"], 0);

    // 6c. Replay: record a short script under a seeded context, then re-run it with no LLM. Created ids are re-bound
    //     (the replayed node is a new node); a tampered recorded exit stops the replay at that step (12).
    let recorded = root.path().join("recorded.jsonl");
    write_file(
        &root.path().join("recorder-ctx.json"),
        &json!({"v": 1, "verse": {"id": campus, "name": "Campus"}, "fractal": {"id": north, "name": "North"}, "petal": {"id": plaza, "name": "Plaza"}}).to_string(),
    );
    let mut recorder = env(&owner_tokens, "recorder");
    recorder.insert("FE_JOURNAL".into(), recorded.to_string_lossy().into_owned());
    let pylon = id_of(&expect(&recorder, &["node", "create", "Pylon", "--position", "4,5,6", "--json"], 0));
    expect(&recorder, &["node", "move", &pylon, "--x", "9", "--json"], 0);
    expect(&recorder, &["node", "get", &pylon, "--json"], 0);
    expect(&recorder, &["node", "delete", &pylon, "--json"], 0);
    expect(&recorder, &["node", "get", &pylon, "--json"], 5);
    let replayer = env(&owner_tokens, "replayer");
    let recorded_text = std::fs::read_to_string(&recorded).unwrap();
    let replayed = expect(&replayer, &["replay", &recorded.to_string_lossy(), "--json"], 0);
    assert_eq!(replayed["data"]["replayed"], json!(5), "{replayed}");
    let id_map = replayed["data"]["id_map"].as_object().expect("id map");
    assert!(id_map.get(&pylon).and_then(Value::as_str).is_some_and(|fresh| fresh != pylon), "the replayed create got a new id: {replayed}");
    let mut tampered: Vec<Value> = recorded_text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let last = tampered.len() - 1;
    tampered[last]["exit"] = json!(0);
    let tampered_path = root.path().join("tampered.jsonl");
    std::fs::write(&tampered_path, tampered.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    let diverged = expect(&replayer, &["replay", &tampered_path.to_string_lossy(), "--json"], 12);
    assert_eq!(diverged["code"], "replay-diverged");
    let at = &diverged["data"]["diverged_at"];
    assert_eq!((at["seq"].clone(), at["recorded_exit"].clone(), at["replayed_exit"].clone()), (tampered[last]["seq"].clone(), json!(0), json!(5)), "{diverged}");
    // The same journal converts to a sim scenario: created ids become action references, the fixture petal is named.
    let scenario_path = root.path().join("scenario.json");
    let emitted = expect(&replayer, &["replay", &recorded.to_string_lossy(), "--emit-scenario", &scenario_path.to_string_lossy(), "--scenario-name", "cli-journey", "--json"], 0);
    assert_eq!((emitted["data"]["steps"].clone(), emitted["data"]["actions"].clone()), (json!(4), json!(5)), "{emitted}");
    let scenario: Value = serde_json::from_str(&std::fs::read_to_string(&scenario_path).unwrap()).unwrap();
    assert_eq!(scenario["name"], "cli-journey");
    assert_eq!(scenario["peers"][0], json!({"name": "owner", "role": "owner"}));
    let first_action = &scenario["steps"][0]["actions"][0];
    assert_eq!((first_action["op"].clone(), first_action["args"]["petal_id"].clone()), (json!("create_node"), json!("$fixture.petal_a_id")), "{scenario}");
    assert_eq!(scenario["steps"][0]["expect"][0]["predicate"], "agree");
    assert_eq!(scenario["steps"][1]["actions"][0]["args"]["node_id"], "$c1.id", "later actions name the created node by reference");
    assert_eq!(scenario["steps"][3]["actions"][0]["expect"], "refused", "the recorded not-found read stays a refusal");
    if let Some(keep) = std::env::var_os("FE_CLI_KEEP_SCENARIO") {
        std::fs::copy(&scenario_path, keep).expect("keep emitted scenario");
    }

    // 7. No petal in a fresh context: refused locally with the way to set one (6), nothing sent.
    let mut fresh = owner.clone();
    fresh.insert("FE_CONTEXT".into(), root.path().join("fresh-ctx.json").to_string_lossy().into_owned());
    let missing = expect(&fresh, &["node", "create", "Orphan", "--json"], 6);
    assert_eq!(missing["code"], "missing-context");
    assert!(missing["message"].as_str().unwrap().contains("fe use petal"), "{missing}");

    // 8. `fe x`: discovery lists the package; the owner's call returns the command result; the viewer's
    //    role-floor refusal comes back verbatim (4). Tokens after `x` are the command's own.
    *engine.host.petal.lock().unwrap() = plaza.clone();
    let catalog = expect(&owner, &["x", "--list", "--json"], 0);
    assert_eq!(catalog["data"]["packages"][0]["package_id"], PACKAGE);
    let help = expect(&owner, &["x", "fleet-sim", "spawn", "--help", "--json"], 0);
    let help_text = help["data"]["help"].as_str().unwrap();
    assert!(help_text.contains("Fleet Name"), "args-json-only property is listed: {help}");
    assert_eq!(help_text.lines().filter(|line| line.starts_with("role floor:")).count(), 1, "a description forged a help line: {help_text}");
    let spawned = expect(&owner, &["x", "fleet-sim", "spawn", "--count", "2", "--json"], 0);
    assert_eq!(spawned["data"], json!({"spawned": 2}));
    assert_eq!(spawned["op"]["via"], "rest", "fe x calls go through the REST command route");
    let floor = expect(&viewer, &["--verse", &campus, "--petal", &plaza, "x", "fleet-sim", "spawn", "--count", "2", "--json"], 4);
    assert_eq!(floor["refusal"]["reason"], format!("{PACKAGE}/spawn needs role Editor"));
    assert_eq!(floor["refusal"]["package_id"], PACKAGE);
    // The REST status + code keep each outcome distinct: guest refusal 2 (host text verbatim), pre-admission rate
    // refusal 7 (no retry hint is invented when the host gives none), unknown outcome 8.
    let refused = expect(&owner, &["x", "fleet-sim", "refuse", "--json"], 2);
    assert_eq!((refused["refusal"]["reason"].clone(), refused["refusal"]["package_id"].clone()), (json!(GUEST_REFUSAL), json!(PACKAGE)));
    let throttled = expect(&owner, &["x", "fleet-sim", "throttle", "--json"], 7);
    assert_eq!((throttled["code"].clone(), throttled["retry_after_ms"].clone()), (json!("rate-limited"), Value::Null));
    assert_eq!(expect(&owner, &["x", "fleet-sim", "vanish", "--json"], 8)["code"], "unknown-outcome");
    // --retry resends a pre-admission refusal; an unknown outcome is never resent.
    let (throttles, vanishes) = (calls(&engine, "throttle"), calls(&engine, "vanish"));
    expect(&owner, &["--retry", "1", "x", "fleet-sim", "throttle", "--json"], 7);
    expect(&owner, &["--retry", "1", "x", "fleet-sim", "vanish", "--json"], 8);
    assert_eq!((calls(&engine, "throttle"), calls(&engine, "vanish")), (throttles + 2, vanishes + 1));
    // Ids that would enter a route are checked before anything is sent.
    assert_eq!(expect(&owner, &["--petal", "../extensions", "x", "--list", "--json"], 6)["code"], "invalid-input");

    // 8b. A loaded token never leaves in arguments: the token file itself, an --args-json value, or a core verb's
    //     value all exit 6 `token-in-args` without echoing it, and nothing reaches the host.
    let owner_document: Value = serde_json::from_str(&std::fs::read_to_string(&owner_tokens).unwrap()).unwrap();
    let held = owner_document["tokens"][&campus].as_str().expect("campus token").to_string();
    let spawns = calls(&engine, "spawn");
    let token_file_arg = format!("@{}", owner_tokens.display());
    let leaked_args = json!({"count": 1, "Fleet Name": held}).to_string();
    for argv in [
        vec!["x", "fleet-sim", "spawn", "--args", token_file_arg.as_str(), "--json"],
        vec!["x", "fleet-sim", "spawn", "--args-json", leaked_args.as_str(), "--json"],
        vec!["node", "set", crate_node.as_str(), "note", held.as_str(), "--verse", harbor.as_str(), "--json"],
    ] {
        let leak = expect(&owner, &argv, 6);
        assert_eq!(leak["code"], "token-in-args", "{argv:?}");
        assert!(!leak.to_string().contains(&held), "the refusal echoed the token");
    }
    assert_eq!(calls(&engine, "spawn"), spawns, "a refused call never reached the host");

    // 9. `fe help --json` parses, and every listed command path routes (with its own help) back to itself.
    let catalog_help = expect(&owner, &["help", "--json"], 0);
    let commands = catalog_help["data"]["commands"].as_array().expect("commands");
    assert!(!commands.is_empty());
    for command in commands {
        let path: Vec<&str> = command["path"].as_array().unwrap().iter().map(|word| word.as_str().unwrap()).collect();
        let argv: Vec<&str> = path.iter().copied().chain(["--help", "--json"]).collect();
        let routed = expect(&owner, &argv, 0);
        let paths: Vec<&Value> = routed["data"]["commands"].as_array().unwrap().iter().map(|row| &row["path"]).collect();
        assert!(paths.contains(&&command["path"]), "{path:?} routes to {paths:?}");
    }
    let unknown = expect(&owner, &["node", "mvoe", "--json"], 6);
    assert!(unknown["message"].as_str().unwrap().contains("fe node move"), "{unknown}");

    // 10. The journal: one parseable line per invocation, and no bearer token text anywhere in it.
    let text = std::fs::read_to_string(&journal).expect("journal");
    let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).expect("journal line parses")).collect();
    assert!(lines.len() > 30, "{} lines", lines.len());
    assert!(lines.iter().all(|line| line["envelope_digest"].as_str().is_some_and(|digest| digest.len() == 64)));
    let owner_file: Value = serde_json::from_str(&std::fs::read_to_string(&owner_tokens).unwrap()).unwrap();
    let viewer_file: Value = serde_json::from_str(&std::fs::read_to_string(&viewer_tokens).unwrap()).unwrap();
    for token in owner_file["tokens"].as_object().unwrap().values().chain(viewer_file["tokens"].as_object().unwrap().values()) {
        assert!(!text.contains(token.as_str().unwrap()), "a bearer token reached the journal");
    }
    assert!(!text.contains("secret-remote-token"));
    assert!(!text.contains("CANARY-INVITE"), "an invite reached the journal");
    // The whole journal converts to a scenario (or is refused) without the invite surviving either way.
    let whole_scenario = root.path().join("whole-scenario.json");
    let whole_argv = ["replay", &journal.to_string_lossy().into_owned(), "--emit-scenario", &whole_scenario.to_string_lossy().into_owned(), "--json"].map(String::from);
    let (_, whole_envelope, _) = fe(&replayer, &whole_argv.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(!whole_envelope.to_string().contains("CANARY-INVITE"));
    if let Ok(emitted) = std::fs::read_to_string(&whole_scenario) {
        assert!(!emitted.contains("CANARY-INVITE") && !emitted.contains("fe-join-v2"), "an invite reached an emitted scenario");
    }

    // 11. Drift: the embedded MCP snapshot equals the live tool table (blessed at the top when FE_CLI_BLESS=1).
    let snapshot: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&snapshot_path).unwrap()).unwrap();
    assert!(snapshot == live, "catalog/mcp-tools.json drifted from fe_api::mcp::tool_specs(); rerun with FE_CLI_BLESS=1");
    let embedded: Vec<Value> = fe_cli::catalog::tool_rows().iter().map(|row| serde_json::to_value(row).unwrap()).collect();
    if std::env::var("FE_CLI_BLESS").as_deref() != Ok("1") {
        assert!(embedded == live, "the compiled-in snapshot is stale; rebuild");
    }
    for row in fe_cli::catalog::verb_table() {
        if let fe_cli::catalog::Door::Mcp(tool) = row.door {
            assert!(live.iter().any(|spec| spec["name"] == tool), "{} {} maps to unknown tool {tool}", row.noun, row.verb);
        }
    }
    engine.shutdown();
}
