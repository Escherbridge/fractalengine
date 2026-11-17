//! Headless plugin host for `fe-relay` (`--features plugin-host` + `FE_PLUGIN_HOST_ALLOW=1`).
//!
//! Installs only explicitly pinned sealed packages from `<FE_EXT_CONFIG_DIR>/headless_installs.toml`
//! (checking publisher trust, `content_hash`, and `capability_digest`), executes bound WASM hooks
//! and commands/ingress, bridges `DbCommand::PluginCommit`, and never exposes REST consent.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crossbeam::channel::Sender;
use fe_api::bindings::{BindingHost, BindingListReply, BindingWriteReply, EffectivePoint};
use fe_api::extensions::{
    AdminOp, AdminReply, AdminRefusal, ApiCaller, ApiCallerKind, CommandFailure, CommandReply,
    ExtensionHost, HostRefusal, IngressAnswer, IngressCall, IngressRefusal as ApiIngressRefusal,
};
use fe_api::import_hooks::{
    ImportCall, ImportHooks, ImportOutput, ImportRefusal, ImportReply, IMPORT_HOOK,
};
use fe_database::handlers::device_payload::{set_accepted_payload_sink, AcceptedPayloadEvent};
use fe_database::handlers::extension_policy::{
    record_offer_confirmation, AuditAction, AuditEntry, AuditOutcome, OfferConfirmation,
};
use fe_database::repo::Db;
use fe_database::BlobStoreHandle;
use fe_hexon::package::{PluginPackage, PluginPackageStore, PluginTrust};
use fe_plugin::bindings::decode::DecodeStamp;
use fe_plugin::capability::{CapabilityToken, V2Binding};
use fe_plugin::lifecycle::binding_instance_id;
use fe_plugin::tick::WallClock;
use fe_plugin::trust_roots::TrustRoots;
use fe_plugin::wasm::component::hooks::{HookCallError, HookRefusal};
use fe_plugin::wasm::component::{
    window_for_token, Binding as WitBinding, Caller as WitCaller, CallerKind as WitCallerKind,
    ComponentHost, ExtensionComponent, HookReply, HookRequest, InstanceHandle, InstanceSetup,
};
use fe_plugin::context::PluginContext;
use fe_plugin::PluginCommand;
use fe_policy::{build_scope, parse_scope};
use fe_runtime::extension_binding::{
    resolution_scopes, BindingMode, BindingRecord, BindingResolution, UnresolvedReason,
};
use fe_runtime::extension_trace::{ExtensionTraceBuffer, ExtensionTraceInput};
use fe_runtime::messages::{BindingRefusal, DbCommand};
use fe_runtime::plugin_host::{
    PackageIdentity, PluginAdmission, PluginReply, ReadingsAdmission,
};
use fe_runtime::scene_hook_gate::{
    AugmentOp, SceneBinding, SceneHook, SceneHookBackend, SceneHookCall, SceneHookGate,
    SceneHookReply, SceneHookReplySender,
};
use fe_sdk::manifest_v2::{
    parse_and_validate, ExtensionManifest, ExtensionSourceClass, ProvidedHook,
};
use fe_sdk::plugin_host::{NodeQuery, NodeReader};
use fe_sdk::source_class::SourceClass;
use fe_sdk::NodeSnapshot;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;

const DB_TIMEOUT: Duration = Duration::from_secs(10);
const HOOK_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Host extensions wired onto `fe-api` and `fe-runtime` for headless `fe-relay`.
pub struct RelayPluginParts {
    pub extension_host: Option<Arc<dyn ExtensionHost>>,
    pub binding_host: Option<Arc<dyn BindingHost>>,
    pub import_hooks: Option<Arc<dyn ImportHooks>>,
    pub extension_trace: Option<ExtensionTraceBuffer>,
}

#[derive(Debug, Default, Deserialize)]
struct HeadlessInstallsFile {
    #[serde(default)]
    install: Vec<HeadlessInstallEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct HeadlessInstallEntry {
    installation_id: String,
    package_id: String,
    package_path: PathBuf,
    content_hash: String,
    capability_digest: String,
    #[serde(default)]
    verse_ids: Vec<String>,
    #[serde(default)]
    source_class: Option<String>,
}

#[derive(Clone)]
struct LoadedPackage {
    installation_id: String,
    package_id: String,
    version: String,
    content_hash: String,
    capability_digest: String,
    publisher_did: String,
    source_class: Option<ExtensionSourceClass>,
    accepted_verses: Vec<String>,
    manifest: Arc<ExtensionManifest>,
    component: Arc<ExtensionComponent>,
}

#[derive(Clone, Debug)]
struct PetalMeta {
    verse_id: String,
    fractal_id: String,
    petal_id: String,
    scope: String,
}

struct LiveRelayInstance {
    handle: InstanceHandle,
    context: PluginContext,
    manifest: Arc<ExtensionManifest>,
    content_hash: String,
    petal_scope: String,
    source_class: Option<SourceClass>,
}

struct RelayPluginState {
    local_did: String,
    shared_db: Db,
    blob_store: BlobStoreHandle,
    db_cmd_tx: Sender<DbCommand>,
    rt: tokio::runtime::Runtime,
    trace: ExtensionTraceBuffer,
    component_host: Arc<ComponentHost>,
    packages: RwLock<HashMap<String, LoadedPackage>>,
    packages_by_id: RwLock<HashMap<String, String>>,
    bindings: Arc<RwLock<HashMap<String, BindingRecord>>>,
    petals: RwLock<HashMap<String, PetalMeta>>,
    instances: Mutex<HashMap<(String, String), LiveRelayInstance>>,
    dummy_cmd_tx: Sender<PluginCommand>,
    _dummy_cmd_rx: crossbeam::channel::Receiver<PluginCommand>,
}

/// Initialize headless plugin execution on `fe-relay`.
pub fn setup(
    local_did: String,
    shared_db: Db,
    blob_store: BlobStoreHandle,
    db_cmd_tx: Sender<DbCommand>,
) -> RelayPluginParts {
    let trace = ExtensionTraceBuffer::default();
    if !fe_plugin::plugin_host_allowed_from_env() {
        tracing::info!("fe-relay plugin-host compiled in, but FE_PLUGIN_HOST_ALLOW is unset (inert)");
        return RelayPluginParts {
            extension_host: None,
            binding_host: None,
            import_hooks: None,
            extension_trace: Some(trace),
        };
    }

    let component_host = match ComponentHost::new() {
        Ok(host) => Arc::new(host),
        Err(error) => {
            tracing::error!(%error, "headless ComponentHost init failed; keeping plugin host off");
            return RelayPluginParts {
                extension_host: None,
                binding_host: None,
                import_hooks: None,
                extension_trace: Some(trace),
            };
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("fe-relay-plugin-host")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            tracing::error!(%error, "headless plugin runtime init failed; keeping plugin host off");
            return RelayPluginParts {
                extension_host: None,
                binding_host: None,
                import_hooks: None,
                extension_trace: Some(trace),
            };
        }
    };

    let (dummy_cmd_tx, dummy_cmd_rx) = crossbeam::channel::bounded(16);
    let state = Arc::new(RelayPluginState {
        local_did,
        shared_db,
        blob_store,
        db_cmd_tx,
        rt,
        trace: trace.clone(),
        component_host,
        packages: RwLock::new(HashMap::new()),
        packages_by_id: RwLock::new(HashMap::new()),
        bindings: Arc::new(RwLock::new(HashMap::new())),
        petals: RwLock::new(HashMap::new()),
        instances: Mutex::new(HashMap::new()),
        dummy_cmd_tx,
        _dummy_cmd_rx: dummy_cmd_rx,
    });

    state.load_headless_installs();
    state.refresh_from_db();

    // Refresh bindings & hierarchy periodically so replicated P2P bindings take effect on headless hooks.
    let refresh_state = Arc::clone(&state);
    state.rt.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let state = Arc::clone(&refresh_state);
            let _ = tokio::task::spawn_blocking(move || state.refresh_from_db()).await;
        }
    });

    let hook_backend: Arc<dyn SceneHookBackend> = state.clone();
    if let Err(reason) = SceneHookGate::install(hook_backend) {
        tracing::warn!(%reason, "SceneHookGate already installed on fe-relay");
    }

    let decode_state = Arc::clone(&state);
    if !set_accepted_payload_sink(Box::new(move |event| {
        let state = Arc::clone(&decode_state);
        let rt_handle = state.rt.handle().clone();
        rt_handle.spawn_blocking(move || state.handle_accepted_payload(event));
    })) {
        tracing::warn!("accepted payload sink already installed on fe-relay");
    }

    tracing::info!("fe-relay headless plugin host active (FE_PLUGIN_HOST_ALLOW=1)");
    let extension_host: Arc<dyn ExtensionHost> = state.clone();
    let binding_host: Arc<dyn BindingHost> = state.clone();
    let import_hooks: Arc<dyn ImportHooks> = state;
    RelayPluginParts {
        extension_host: Some(extension_host),
        binding_host: Some(binding_host),
        import_hooks: Some(import_hooks),
        extension_trace: Some(trace),
    }
}

impl RelayPluginState {
    fn config_dir() -> PathBuf {
        std::env::var_os("FE_EXT_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/extensions"))
    }

    fn load_headless_installs(&self) {
        let config_dir = Self::config_dir();
        let dev_flag = cfg!(debug_assertions) && std::env::var_os("FE_PLUGIN_DEV_ALLOW").is_some();
        let trust_path = config_dir.join("trusted_publishers.toml");
        let trust_roots = if trust_path.exists() {
            match TrustRoots::load_with(&trust_path, dev_flag) {
                Ok(roots) => roots,
                Err(error) => {
                    tracing::error!(%error, path = %trust_path.display(), "failed to load trusted_publishers.toml");
                    TrustRoots::default()
                }
            }
        } else {
            TrustRoots::default()
        };

        let installs_path = config_dir.join("headless_installs.toml");
        if !installs_path.exists() {
            tracing::info!(path = %installs_path.display(), "no headless_installs.toml found; zero headless packages installed");
            return;
        }
        let raw_toml = match std::fs::read_to_string(&installs_path) {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(%error, path = %installs_path.display(), "failed to read headless_installs.toml");
                return;
            }
        };
        let doc: HeadlessInstallsFile = match toml::from_str(&raw_toml) {
            Ok(d) => d,
            Err(error) => {
                tracing::error!(%error, path = %installs_path.display(), "failed to parse headless_installs.toml");
                return;
            }
        };

        let store = PluginPackageStore::new(config_dir.join("packages"));
        let allow_dev_unsigned = dev_flag;
        let trust = PluginTrust {
            is_trust_root: &|did| trust_roots.get(did).is_some(),
            allow_unsigned_dev: allow_dev_unsigned,
        };

        for entry in doc.install {
            if let Err(error) = self.install_one(&config_dir, &store, &trust, &entry) {
                tracing::error!(
                    package_id = %entry.package_id,
                    installation_id = %entry.installation_id,
                    %error,
                    "refused headless package install"
                );
                self.trace.record(ExtensionTraceInput {
                    kind: "lifecycle".into(),
                    package_id: entry.package_id.clone(),
                    petal_id: None,
                    hook: None,
                    key: None,
                    outcome: "refused".into(),
                    detail: Some(error),
                    duration_ms: None,
                });
            }
        }
    }

    fn install_one(
        &self,
        config_dir: &Path,
        store: &PluginPackageStore,
        trust: &PluginTrust<'_>,
        entry: &HeadlessInstallEntry,
    ) -> Result<(), String> {
        let pkg_path = if entry.package_path.is_relative() {
            config_dir.join(&entry.package_path)
        } else {
            entry.package_path.clone()
        };
        let bytes = std::fs::read(&pkg_path)
            .map_err(|e| format!("read {}: {e}", pkg_path.display()))?;
        let pkg: PluginPackage = store
            .install(&bytes, Some(&entry.package_id), trust, &|_| false)
            .map_err(|e| format!("verify/store package: {e}"))?;
        let validated = parse_and_validate(&pkg.extension_json)
            .map_err(|e| format!("invalid extension.json: {e}"))?;

        if !pkg.content_hash.eq_ignore_ascii_case(&entry.content_hash) {
            return Err(format!(
                "content_hash pin mismatch (expected {}, got {})",
                entry.content_hash, pkg.content_hash
            ));
        }
        if !validated
            .capability_digest
            .eq_ignore_ascii_case(&entry.capability_digest)
        {
            return Err(format!(
                "capability_digest pin mismatch (expected {}, got {})",
                entry.capability_digest, validated.capability_digest
            ));
        }

        let source_class = match entry.source_class.as_deref() {
            None => None,
            Some("simulation") => Some(ExtensionSourceClass::Simulation),
            Some("device") => Some(ExtensionSourceClass::Device),
            Some("connector") => Some(ExtensionSourceClass::Connector),
            Some(other) => return Err(format!("invalid source_class pin '{other}'")),
        };

        let component = self
            .component_host
            .compile(&pkg.component)
            .map_err(|e| format!("compile wasm component: {e}"))?;
        component
            .check_provides(&validated.manifest)
            .map_err(|e| format!("component hook exports check: {e}"))?;

        for verse_id in &entry.verse_ids {
            let confirmation = OfferConfirmation {
                installation_id: entry.installation_id.clone(),
                package_id: pkg.package_id.clone(),
                content_hash: pkg.content_hash.clone(),
                capability_digest: validated.capability_digest.clone(),
                verse_id: verse_id.clone(),
                source_class: entry.source_class.clone(),
            };
            self.rt
                .block_on(record_offer_confirmation(
                    &self.shared_db,
                    &self.local_did,
                    &confirmation,
                ))
                .map_err(|e| format!("persist consent/acceptance for {verse_id}: {e}"))?;
        }

        let loaded = LoadedPackage {
            installation_id: entry.installation_id.clone(),
            package_id: pkg.package_id.clone(),
            version: pkg.version,
            content_hash: pkg.content_hash,
            capability_digest: validated.capability_digest,
            publisher_did: pkg.publisher_did,
            source_class,
            accepted_verses: entry.verse_ids.clone(),
            manifest: Arc::new(validated.manifest),
            component: Arc::new(component),
        };
        self.packages
            .write()
            .unwrap()
            .insert(entry.installation_id.clone(), loaded);
        self.packages_by_id
            .write()
            .unwrap()
            .insert(pkg.package_id.clone(), entry.installation_id.clone());

        self.trace.record(ExtensionTraceInput {
            kind: "lifecycle".into(),
            package_id: pkg.package_id,
            petal_id: None,
            hook: None,
            key: None,
            outcome: "installed".into(),
            detail: Some(format!("installation={}", entry.installation_id)),
            duration_ms: None,
        });
        Ok(())
    }

    fn refresh_from_db(&self) {
        let Ok((petals, bindings)) = self.rt.block_on(async {
            let mut res = self
                .shared_db
                .query(
                    "SELECT fractal_id, verse_id FROM fractal;\
                     SELECT petal_id, fractal_id FROM petal;\
                     SELECT * OMIT id FROM binding;",
                )
                .await
                .map_err(|e| e.to_string())?;
            let fractals: Vec<Value> = res.take(0).unwrap_or_default();
            let petal_rows: Vec<Value> = res.take(1).unwrap_or_default();
            let binding_vals: Vec<Value> = res.take(2).unwrap_or_default();
            let binding_rows: Vec<BindingRecord> = binding_vals
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();

            let mut fractal_verse = HashMap::new();
            for row in fractals {
                if let (Some(fid), Some(vid)) = (
                    row.get("fractal_id").and_then(Value::as_str),
                    row.get("verse_id").and_then(Value::as_str),
                ) {
                    fractal_verse.insert(fid.to_string(), vid.to_string());
                }
            }
            let mut petals = HashMap::new();
            for row in petal_rows {
                let (Some(pid), Some(fid)) = (
                    row.get("petal_id").and_then(Value::as_str),
                    row.get("fractal_id").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if let Some(vid) = fractal_verse.get(fid) {
                    let scope = build_scope(vid, Some(fid), Some(pid));
                    petals.insert(
                        pid.to_string(),
                        PetalMeta {
                            verse_id: vid.to_string(),
                            fractal_id: fid.to_string(),
                            petal_id: pid.to_string(),
                            scope,
                        },
                    );
                }
            }
            let mut latest: HashMap<String, BindingRecord> = HashMap::new();
            for row in binding_rows {
                let key = format!("{}|{}|{}", row.scope, row.hook, row.key);
                let replace = latest.get(&key).is_none_or(|prev| {
                    let next_clock = row.written_at.parse::<u64>().unwrap_or(0);
                    let prev_clock = prev.written_at.parse::<u64>().unwrap_or(0);
                    next_clock >= prev_clock
                });
                if replace {
                    latest.insert(key, row);
                }
            }
            Ok::<_, String>((petals, latest))
        }) else {
            return;
        };

        *self.petals.write().unwrap() = petals;
        *self.bindings.write().unwrap() = bindings;
    }

    fn loaded_by_package(&self, package_id: &str) -> Option<LoadedPackage> {
        let inst_id = self.packages_by_id.read().unwrap().get(package_id).cloned()?;
        self.packages.read().unwrap().get(&inst_id).cloned()
    }

    fn resolve_binding(&self, petal_id: &str, hook: &str, key: &str) -> Option<BindingRecord> {
        let meta = self.petals.read().unwrap().get(petal_id).cloned()?;
        let scopes = resolution_scopes(&meta.verse_id, &meta.fractal_id, &meta.petal_id);
        let bindings = self.bindings.read().unwrap();
        for scope in scopes {
            let map_key = format!("{scope}|{hook}|{key}");
            if let Some(record) = bindings.get(&map_key) {
                match record.mode {
                    BindingMode::Inherit => continue,
                    BindingMode::Builtin => return None,
                    BindingMode::Extension => return Some(record.clone()),
                }
            }
        }
        None
    }

    fn ensure_instance(
        &self,
        petal_id: &str,
        package_id: &str,
    ) -> Result<(InstanceHandle, PluginContext, String, Option<SourceClass>), String> {
        let key = (petal_id.to_string(), package_id.to_string());
        {
            let guard = self.instances.lock().unwrap();
            if let Some(live) = guard.get(&key) {
                return Ok((
                    live.handle.clone(),
                    live.context.clone(),
                    live.content_hash.clone(),
                    live.source_class,
                ));
            }
        }

        let pkg = self
            .loaded_by_package(package_id)
            .ok_or_else(|| format!("package {package_id} is not installed on fe-relay"))?;
        let meta = self
            .petals
            .read()
            .unwrap()
            .get(petal_id)
            .cloned()
            .ok_or_else(|| format!("unknown petal {petal_id}"))?;
        if !pkg.accepted_verses.is_empty() && !pkg.accepted_verses.contains(&meta.verse_id) {
            return Err(format!(
                "package {package_id} is not accepted in verse {}",
                meta.verse_id
            ));
        }

        let plugin_id = binding_instance_id(&pkg.installation_id, petal_id);
        let v2_binding = V2Binding {
            verse_scope: build_scope(&meta.verse_id, None, None),
            petal_scope: meta.scope.clone(),
            source_class: pkg.source_class,
        };
        let token = CapabilityToken::mint_v2(&plugin_id, &pkg.manifest, &v2_binding)?;
        let context = PluginContext::new(
            plugin_id,
            petal_id.to_string(),
            token.clone(),
            self.dummy_cmd_tx.clone(),
        )
        .with_node_reader(Arc::new(Mutex::new(RelayNodeReader {
            db_cmd_tx: self.db_cmd_tx.clone(),
            admission: build_admission(
                &pkg,
                &meta.scope,
                petal_id,
                &token,
                pkg.source_class.map(to_source_class),
            )?,
        })));

        let source_class = pkg.source_class.map(to_source_class);
        let wit_binding = WitBinding {
            installation_id: pkg.installation_id.clone(),
            package_id: pkg.package_id.clone(),
            content_hash: pkg.content_hash.clone(),
            verse_id: meta.verse_id.clone(),
            fractal_id: meta.fractal_id.clone(),
            petal_id: petal_id.to_string(),
            identity_did: self.local_did.clone(),
            source_class: source_class.map(|c| c.as_str().to_string()),
        };
        let window = window_for_token(&token);
        let setup = InstanceSetup::new(wit_binding, context.clone(), window, Arc::new(WallClock));
        let mut instance = self
            .component_host
            .instantiate(&pkg.component, setup)
            .map_err(|e| e.to_string())?;
        if let Err(error) = instance.on_install().map_err(|f| f.to_string())? {
            return Err(format!("on-install refused: {error}"));
        }
        if let Err(error) = instance.on_activate().map_err(|f| f.to_string())? {
            return Err(format!("on-activate refused: {error}"));
        }
        let handle = InstanceHandle::new(instance);

        let mut guard = self.instances.lock().unwrap();
        let entry = guard.entry(key).or_insert_with(|| LiveRelayInstance {
            handle: handle.clone(),
            context: context.clone(),
            manifest: Arc::clone(&pkg.manifest),
            content_hash: pkg.content_hash.clone(),
            petal_scope: meta.scope.clone(),
            source_class,
        });
        Ok((
            entry.handle.clone(),
            entry.context.clone(),
            entry.content_hash.clone(),
            entry.source_class,
        ))
    }

    fn flush_window_if_non_empty(&self, petal_id: &str, package_id: &str) {
        let key = (petal_id.to_string(), package_id.to_string());
        let Some((handle, context, manifest, content_hash, petal_scope, source_class)) = ({
            let guard = self.instances.lock().unwrap();
            guard.get(&key).map(|live| {
                (
                    live.handle.clone(),
                    live.context.clone(),
                    Arc::clone(&live.manifest),
                    live.content_hash.clone(),
                    live.petal_scope.clone(),
                    live.source_class,
                )
            })
        }) else {
            return;
        };
        let window = match handle.lock() {
            Ok(inst) => inst.window(),
            Err(_) => return,
        };
        let flush = {
            let mut w = window.lock().unwrap();
            if w.is_empty() {
                return;
            }
            w.take()
        };
        let pkg = match self.loaded_by_package(package_id) {
            Some(p) => p,
            None => return,
        };
        let Ok(admission) = build_admission(
            &pkg,
            &petal_scope,
            petal_id,
            context.capabilities(),
            source_class,
        ) else {
            return;
        };
        let (reply_tx, reply_rx) = crossbeam::channel::bounded::<fe_runtime::plugin_host::PluginResult>(1);
        if self
            .db_cmd_tx
            .send(DbCommand::PluginCommit {
                admission,
                ops: flush.ops,
                readings: flush.readings,
                reply: reply_tx,
            })
            .is_err()
        {
            return;
        }
        let outcome = match reply_rx.recv_timeout(DB_TIMEOUT) {
            Ok(Ok(_)) => "committed",
            Ok(Err(_)) | Err(_) => "commit-failed",
        };
        self.trace.record(ExtensionTraceInput {
            kind: "flush".into(),
            package_id: package_id.to_string(),
            petal_id: Some(petal_id.to_string()),
            hook: None,
            key: None,
            outcome: outcome.into(),
            detail: None,
            duration_ms: None,
        });
        let _ = manifest;
        let _ = content_hash;
    }

    fn call_hook_for(
        &self,
        petal_id: &str,
        package_id: &str,
        req: &HookRequest,
        stamp: Option<&DecodeStamp>,
    ) -> Result<HookReply, HookCallError> {
        let (handle, _, _, _) = self
            .ensure_instance(petal_id, package_id)
            .map_err(|e| HookCallError::Guest(HookRefusal::Failed(e)))?;
        let reply = handle.call_hook(req, stamp)?;
        self.flush_window_if_non_empty(petal_id, package_id);
        Ok(reply)
    }

    fn handle_accepted_payload(&self, event: AcceptedPayloadEvent) {
        let Some(binding) = self.resolve_binding(&event.petal_id, "ingest.decode", &event.format_id)
        else {
            return;
        };
        let Some(package_id) = binding.package_id.as_deref() else {
            return;
        };
        let mut files_json = Vec::new();
        let mut body = Vec::new();
        let started = std::time::Instant::now();
        for asset_id in &event.asset_ids {
            let Ok(Some((content_type, hash_hex, size_bytes))) = self.rt.block_on(async {
                let mut res = self
                    .shared_db
                    .query("SELECT content_type, content_hash, size_bytes FROM asset WHERE asset_id = $aid LIMIT 1")
                    .bind(("aid", asset_id.clone()))
                    .await
                    .map_err(|e| e.to_string())?;
                let rows: Vec<Value> = res.take(0).map_err(|e| e.to_string())?;
                Ok::<_, String>(rows.into_iter().next().and_then(|r| {
                    Some((
                        r.get("content_type")?.as_str()?.to_string(),
                        r.get("content_hash")?.as_str()?.to_string(),
                        r.get("size_bytes").and_then(Value::as_u64).unwrap_or(0),
                    ))
                }))
            }) else {
                return;
            };
            let mut hash_bytes = [0u8; 32];
            if hex::decode_to_slice(&hash_hex, &mut hash_bytes).is_err() {
                return;
            }
            let Some(blob_path) = self.blob_store.get_blob_path(&hash_bytes) else {
                return;
            };
            let Ok(bytes) = std::fs::read(&blob_path) else {
                return;
            };
            if blake3::hash(&bytes).as_bytes() != &hash_bytes {
                return;
            }
            body.extend_from_slice(&bytes);
            files_json.push(json!({
                "asset_id": asset_id,
                "content_type": content_type,
                "size": size_bytes,
            }));
        }
        let input = json!({
            "payload_id": event.payload_id,
            "format_id": event.format_id,
            "mime": event.mime,
            "node_id": event.node_id,
            "recorded_at_ms": event.recorded_at_ms,
            "files": files_json,
        });
        let req = HookRequest {
            hook: "ingest.decode".into(),
            key: event.format_id.clone(),
            binding_scope: binding.scope,
            config: binding.config.to_string(),
            input: input.to_string(),
            body,
        };
        let stamp = DecodeStamp::for_payload(&event.payload_id);
        let result = self.call_hook_for(&event.petal_id, package_id, &req, Some(&stamp));
        let duration_ms = started.elapsed().as_millis() as u64;
        self.trace.record(ExtensionTraceInput {
            kind: "decode".into(),
            package_id: package_id.to_string(),
            petal_id: Some(event.petal_id),
            hook: Some("ingest.decode".into()),
            key: Some(event.format_id),
            outcome: if result.is_ok() { "decoded" } else { "failed" }.into(),
            detail: result.err().map(|e| e.to_string()),
            duration_ms: Some(duration_ms),
        });
    }
}

fn to_source_class(c: ExtensionSourceClass) -> SourceClass {
    match c {
        ExtensionSourceClass::Simulation => SourceClass::Simulation,
        ExtensionSourceClass::Device => SourceClass::Device,
        ExtensionSourceClass::Connector => SourceClass::Connector,
    }
}

fn build_admission(
    pkg: &LoadedPackage,
    petal_scope: &str,
    petal_id: &str,
    token: &CapabilityToken,
    source_class: Option<SourceClass>,
) -> Result<PluginAdmission, String> {
    let mut admission = PluginAdmission::new(
        binding_instance_id(&pkg.installation_id, petal_id),
        petal_id.to_string(),
        token.capabilities.clone(),
        token.property_scope.clone(),
        token.verse_scope.clone(),
        token.petal_scope.clone(),
    )?
    .with_package(PackageIdentity::new(
        pkg.package_id.clone(),
        pkg.content_hash.clone(),
    )?)
    .bind_scope(petal_scope.to_string())?;
    if let Some(grants) = &token.grants_v2 {
        if let Some(class) = source_class {
            if !grants.metrics.is_empty() {
                admission =
                    admission.with_readings(ReadingsAdmission::new(class, grants.metrics.clone())?);
            }
        }
        if !grants.scene_write.is_empty() {
            admission = admission.with_scene_writes(grants.scene_write.clone())?;
        }
    }
    Ok(admission)
}

struct RelayNodeReader {
    db_cmd_tx: Sender<DbCommand>,
    admission: PluginAdmission,
}

impl NodeReader for RelayNodeReader {
    fn petal_scope(&self, _petal_id: &str) -> Result<String, String> {
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.db_cmd_tx
            .send(DbCommand::PluginRead {
                admission: self.admission.clone(),
                request: fe_runtime::plugin_host::PluginRead::PetalScope,
                reply: tx,
            })
            .map_err(|_| "db thread unavailable".to_string())?;
        match rx.recv_timeout(DB_TIMEOUT).map_err(|e| e.to_string())? {
            Ok(PluginReply::Scope(s)) => Ok(s),
            Ok(_) => Err("unexpected plugin reply".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn get_node(&self, _petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String> {
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.db_cmd_tx
            .send(DbCommand::PluginRead {
                admission: self.admission.clone(),
                request: fe_runtime::plugin_host::PluginRead::Node(node_id.to_string()),
                reply: tx,
            })
            .map_err(|_| "db thread unavailable".to_string())?;
        match rx.recv_timeout(DB_TIMEOUT).map_err(|e| e.to_string())? {
            Ok(PluginReply::Node(n)) => Ok(n),
            Ok(_) => Err("unexpected plugin reply".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn query_nodes(&self, _petal_id: &str, query: &NodeQuery) -> Result<Vec<NodeSnapshot>, String> {
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.db_cmd_tx
            .send(DbCommand::PluginRead {
                admission: self.admission.clone(),
                request: fe_runtime::plugin_host::PluginRead::Nodes(query.clone()),
                reply: tx,
            })
            .map_err(|_| "db thread unavailable".to_string())?;
        match rx.recv_timeout(DB_TIMEOUT).map_err(|e| e.to_string())? {
            Ok(PluginReply::Nodes(n)) => Ok(n),
            Ok(_) => Err("unexpected plugin reply".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl SceneHookBackend for RelayPluginState {
    fn local_did(&self) -> &str {
        &self.local_did
    }

    fn bound(&self, hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
        let record = self.resolve_binding(petal_id, hook.id(), "")?;
        let pkg_id = record.package_id?;
        let pkg = self.loaded_by_package(&pkg_id)?;
        if !pkg
            .manifest
            .provides
            .iter()
            .any(|p: &ProvidedHook| p.hook == hook.id())
        {
            return None;
        }
        let property_scope = pkg
            .manifest
            .scene
            .as_ref()
            .and_then(|s| s.property_scope.clone())
            .unwrap_or_default();
        let may_augment = pkg.manifest.tick.is_some()
            && pkg
                .manifest
                .scene
                .as_ref()
                .and_then(|s| s.write.as_ref())
                .is_some_and(|w| !w.is_empty());
        Some(SceneBinding {
            package_id: pkg_id,
            binding_scope: record.scope,
            config: record.config,
            property_scope,
            may_augment,
        })
    }

    fn dispatch(&self, call: SceneHookCall, reply: SceneHookReplySender) {
        let state_pkg_id = call.package_id.clone();
        let petal_id = call.petal_id.clone();
        let hook_str = call.hook.id().to_string();
        let req = HookRequest {
            hook: hook_str.clone(),
            key: String::new(),
            binding_scope: call.binding_scope.clone(),
            config: call.config.to_string(),
            input: call.input.to_string(),
            body: Vec::new(),
        };
        let started = std::time::Instant::now();
        // Run synchronously on a worker thread so callers (including DB thread / tokio) never block here.
        let trace = self.trace.clone();
        let answer_handle = self.ensure_instance(&petal_id, &state_pkg_id);
        let rt_handle = self.rt.handle().clone();
        let db_tx = self.db_cmd_tx.clone();
        let packages = self.packages.read().unwrap().get(
            &self
                .packages_by_id
                .read()
                .unwrap()
                .get(&state_pkg_id)
                .cloned()
                .unwrap_or_default(),
        ).cloned();
        let petals = self.petals.read().unwrap().get(&petal_id).cloned();

        rt_handle.spawn_blocking(move || {
            let result = match answer_handle {
                Ok((handle, _, _, _)) => handle.call_hook(&req, None),
                Err(e) => Err(HookCallError::Guest(HookRefusal::Failed(e))),
            };
            let duration_ms = started.elapsed().as_millis() as u64;
            let mapped = match result {
                Ok(hook_reply) => {
                    match serde_json::from_str::<Value>(&hook_reply.output) {
                        Ok(val) => {
                            let decision = val
                                .get("verdict")
                                .or_else(|| val.get("decision"))
                                .and_then(Value::as_str)
                                .unwrap_or("allow");
                            if decision == "refuse" {
                                let reason = val
                                    .get("reason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("refused by extension")
                                    .to_string();
                                trace.record(ExtensionTraceInput {
                                    kind: "scene_hook".into(),
                                    package_id: state_pkg_id,
                                    petal_id: Some(petal_id),
                                    hook: Some(hook_str),
                                    key: None,
                                    outcome: "refused".into(),
                                    detail: Some(reason.clone()),
                                    duration_ms: Some(duration_ms),
                                });
                                SceneHookReply::Refused(reason)
                            } else {
                                trace.record(ExtensionTraceInput {
                                    kind: "scene_hook".into(),
                                    package_id: state_pkg_id,
                                    petal_id: Some(petal_id),
                                    hook: Some(hook_str),
                                    key: None,
                                    outcome: "allow".into(),
                                    detail: None,
                                    duration_ms: Some(duration_ms),
                                });
                                let _ = (db_tx, packages, petals);
                                SceneHookReply::Output(val)
                            }
                        }
                        Err(e) => SceneHookReply::Fault(format!("bad hook JSON: {e}")),
                    }
                }
                Err(HookCallError::Guest(HookRefusal::Refused(r))) => SceneHookReply::Refused(r),
                Err(HookCallError::Guest(HookRefusal::InvalidInput(r) | HookRefusal::Failed(r))) => {
                    SceneHookReply::Declined(r)
                }
                Err(other) => {
                    trace.record(ExtensionTraceInput {
                        kind: "scene_hook".into(),
                        package_id: state_pkg_id,
                        petal_id: Some(petal_id),
                        hook: Some(hook_str),
                        key: None,
                        outcome: "fault".into(),
                        detail: Some(other.to_string()),
                        duration_ms: Some(duration_ms),
                    });
                    SceneHookReply::Fault(other.to_string())
                }
            };
            let _ = reply.send(mapped);
        });
    }

    fn stage_augment(
        &self,
        petal_id: &str,
        package_id: &str,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String> {
        let Some(pkg) = self.loaded_by_package(package_id) else {
            return Err("package not loaded".into());
        };
        let Some(meta) = self.petals.read().unwrap().get(petal_id).cloned() else {
            return Err("unknown petal".into());
        };
        let plugin_id = binding_instance_id(&pkg.installation_id, petal_id);
        let token = CapabilityToken::mint_v2(
            &plugin_id,
            &pkg.manifest,
            &V2Binding {
                verse_scope: build_scope(&meta.verse_id, None, None),
                petal_scope: meta.scope.clone(),
                source_class: pkg.source_class,
            },
        )?;
        let admission = build_admission(
            &pkg,
            &meta.scope,
            petal_id,
            &token,
            pkg.source_class.map(to_source_class),
        )?;
        let pending = ops
            .into_iter()
            .map(|op| fe_sdk::plugin_host::PendingOp::SetProperty {
                node_id: node_id.to_string(),
                key: op.key,
                value: op.value,
            })
            .collect();
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.db_cmd_tx
            .send(DbCommand::PluginCommit {
                admission,
                ops: pending,
                readings: Vec::new(),
                reply: tx,
            })
            .map_err(|_| "db thread unavailable".to_string())?;
        match rx.recv_timeout(DB_TIMEOUT).map_err(|e| e.to_string())? {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl BindingHost for RelayPluginState {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }

    fn put_binding(&self, caller_did: &str, template: BindingRecord) -> BindingWriteReply {
        let (tx, rx) = oneshot::channel();
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        if self
            .db_cmd_tx
            .send(DbCommand::PutBinding {
                caller_did: if caller_did == self.local_did {
                    None
                } else {
                    Some(caller_did.to_string())
                },
                binding: template,
                reply: db_tx,
            })
            .is_err()
        {
            let _ = tx.send(Err(BindingRefusal {
                code: "not-available".into(),
                message: "db thread unavailable".into(),
            }));
            return rx;
        }
        let bindings_lock = self.bindings.clone();
        let trace = self.trace.clone();
        self.rt.spawn_blocking(move || {
            match db_rx.recv_timeout(DB_TIMEOUT) {
                Ok(Ok(saved)) => {
                    let key = format!("{}|{}|{}", saved.scope, saved.hook, saved.key);
                    trace.record(ExtensionTraceInput {
                        kind: "binding".into(),
                        package_id: saved.package_id.clone().unwrap_or_else(|| "builtin".into()),
                        petal_id: None,
                        hook: Some(saved.hook.clone()),
                        key: (!saved.key.is_empty()).then(|| saved.key.clone()),
                        outcome: "put".into(),
                        detail: Some(saved.scope.clone()),
                        duration_ms: None,
                    });
                    bindings_lock.write().unwrap().insert(key, saved.clone());
                    let _ = tx.send(Ok(saved));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(Err(e));
                }
                Err(_) => {
                    let _ = tx.send(Err(BindingRefusal {
                        code: "failed".into(),
                        message: "binding write timed out".into(),
                    }));
                }
            }
        });
        rx
    }

    fn list_bindings(&self, caller_did: &str, scope: &str) -> BindingListReply {
        let (tx, rx) = oneshot::channel();
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        if self
            .db_cmd_tx
            .send(DbCommand::ListBindings {
                scope: scope.to_string(),
                caller_did: if caller_did == self.local_did {
                    None
                } else {
                    Some(caller_did.to_string())
                },
                reply: db_tx,
            })
            .is_err()
        {
            let _ = tx.send(Err(BindingRefusal {
                code: "not-available".into(),
                message: "db thread unavailable".into(),
            }));
            return rx;
        }
        self.rt.spawn_blocking(move || {
            match db_rx.recv_timeout(DB_TIMEOUT) {
                Ok(res) => {
                    let _ = tx.send(res);
                }
                Err(_) => {
                    let _ = tx.send(Err(BindingRefusal {
                        code: "failed".into(),
                        message: "list_bindings timed out".into(),
                    }));
                }
            }
        });
        rx
    }

    fn effective(&self, petal_scope: &str, caller_did: &str) -> Option<Vec<EffectivePoint>> {
        let parts = parse_scope(petal_scope).ok()?;
        let fractal_id = parts.fractal_id.as_deref()?;
        let petal_id = parts.petal_id.as_deref()?;
        let scopes = resolution_scopes(&parts.verse_id, fractal_id, petal_id);
        let mut points: BTreeMap<(String, String), BindingRecord> = BTreeMap::new();
        {
            let guard = self.bindings.read().unwrap();
            for row in guard.values() {
                if scopes.contains(&row.scope) {
                    let key = (row.hook.clone(), row.key.clone());
                    points.entry(key).or_insert_with(|| row.clone());
                }
            }
        }
        // Re-scan in nearest-scope-first order for accurate override semantics.
        let mut ordered = Vec::new();
        let guard = self.bindings.read().unwrap();
        for (hook, key) in points.keys() {
            for scope in &scopes {
                if let Some(row) = guard.get(&format!("{scope}|{hook}|{key}")) {
                    if row.mode == BindingMode::Inherit {
                        continue;
                    }
                    let resolution = if caller_did == self.local_did {
                        match row.mode {
                            BindingMode::Extension => row.package_id.as_ref().map(|pid| {
                                if self.loaded_by_package(pid).is_some() {
                                    BindingResolution::Resolved {
                                        package_id: pid.clone(),
                                        scope: scope.clone(),
                                    }
                                } else {
                                    BindingResolution::Unresolved {
                                        package_id: pid.clone(),
                                        scope: scope.clone(),
                                        reason: UnresolvedReason::NotInstalled,
                                        offer: None,
                                    }
                                }
                            }),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let _ = petal_id;
                    ordered.push(EffectivePoint {
                        hook: hook.clone(),
                        key: key.clone(),
                        scope: scope.clone(),
                        mode: row.mode,
                        package_id: row.package_id.clone(),
                        version_req: row.version_req.clone(),
                        resolution,
                    });
                    break;
                }
            }
        }
        Some(ordered)
    }
}

impl ExtensionHost for RelayPluginState {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }

    fn admit_remote(&self, _did: &str) -> bool {
        true
    }

    fn list(&self) -> Vec<Value> {
        self.packages
            .read()
            .unwrap()
            .values()
            .map(|p| {
                json!({
                    "installation_id": p.installation_id,
                    "package_id": p.package_id,
                    "version": p.version,
                    "content_hash": p.content_hash,
                    "capability_digest": p.capability_digest,
                    "publisher_did": p.publisher_did,
                    "accepted_verses": p.accepted_verses,
                    "status": "installed-headless",
                })
            })
            .collect()
    }

    fn status(&self, installation: &str) -> Option<Value> {
        let guard = self.packages.read().unwrap();
        let pkg = guard
            .get(installation)
            .or_else(|| guard.values().find(|p| p.package_id == installation))?;
        Some(json!({
            "installation_id": pkg.installation_id,
            "package_id": pkg.package_id,
            "version": pkg.version,
            "content_hash": pkg.content_hash,
            "capability_digest": pkg.capability_digest,
            "publisher_did": pkg.publisher_did,
            "accepted_verses": pkg.accepted_verses,
            "status": "installed-headless",
        }))
    }

    fn audit(&self, installation: &str, limit: usize) -> Option<Vec<Value>> {
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.db_cmd_tx
            .send(DbCommand::ListExtensionAudit {
                installation: Some(installation.to_string()),
                verse_id: None,
                limit,
                reply: tx,
            })
            .ok()?;
        match rx.recv_timeout(DB_TIMEOUT) {
            Ok(Ok(rows)) => Some(rows),
            _ => None,
        }
    }

    fn administer(&self, _actor_did: &str, _op: AdminOp) -> Result<Value, AdminRefusal> {
        Err(AdminRefusal {
            code: "not-available".into(),
            message: "interactive extension administration requires the desktop GUI; use headless_installs.toml on fe-relay".into(),
        })
    }

    fn record_denied_admin(&self, actor_did: &str, target: &str) {
        let (tx, _rx) = crossbeam::channel::bounded(1);
        let mut entry = AuditEntry::verse(
            AuditAction::Denial,
            actor_did,
            target,
            AuditOutcome::Denied,
            Some("non-local administration refused".into()),
        );
        entry.target = target.to_string();
        if let Ok(val) = serde_json::to_value(entry) {
            let _ = self.db_cmd_tx.try_send(DbCommand::AppendExtensionAudit {
                entry: val,
                reply: tx,
            });
        }
    }

    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        args: &Value,
    ) -> Result<CommandReply, HostRefusal> {
        if caller.did != self.local_did {
            return Err(HostRefusal::NotFound);
        }
        let started = std::time::Instant::now();
        let (handle, _, _, _) = self
            .ensure_instance(petal_id, package_id)
            .map_err(HostRefusal::NotAvailable)?;
        let wit_caller = WitCaller {
            did: caller.did.clone(),
            kind: match caller.kind {
                ApiCallerKind::Rest => WitCallerKind::Rest,
                ApiCallerKind::Mcp => WitCallerKind::Mcp,
            },
            package_id: None,
            local: true,
        };
        let (tx, rx) = oneshot::channel();
        let command_s = command.to_string();
        let args_s = args.to_string();
        let pkg_id_s = package_id.to_string();
        let petal_s = petal_id.to_string();
        let trace = self.trace.clone();
        self.rt.spawn_blocking(move || {
            let out = handle
                .lock()
                .map_err(|e| HookCallError::Guest(HookRefusal::Failed(e)).to_string())
                .and_then(|mut inst| {
                    inst.on_command(&command_s, &args_s, &wit_caller)
                        .map_err(|fault| fault.to_string())
                });
            let duration_ms = started.elapsed().as_millis() as u64;
            match out {
                Ok(Ok(json_str)) => {
                    trace.record(ExtensionTraceInput {
                        kind: "bus_call".into(),
                        package_id: pkg_id_s,
                        petal_id: Some(petal_s),
                        hook: None,
                        key: Some(command_s),
                        outcome: "ok".into(),
                        detail: None,
                        duration_ms: Some(duration_ms),
                    });
                    let _ = tx.send(Ok(json_str));
                }
                Ok(Err(cmd_err)) => {
                    let msg = format!("{cmd_err:?}");
                    trace.record(ExtensionTraceInput {
                        kind: "bus_call".into(),
                        package_id: pkg_id_s,
                        petal_id: Some(petal_s),
                        hook: None,
                        key: Some(command_s),
                        outcome: "refused".into(),
                        detail: Some(msg.clone()),
                        duration_ms: Some(duration_ms),
                    });
                    let _ = tx.send(Err(CommandFailure::Refused(msg)));
                }
                Err(fault) => {
                    let _ = tx.send(Err(CommandFailure::Failed(fault)));
                }
            }
        });
        Ok(rx)
    }

    fn ingress(
        &self,
        call: IngressCall,
    ) -> Result<oneshot::Receiver<Option<IngressAnswer>>, ApiIngressRefusal> {
        let _ = call;
        Err(ApiIngressRefusal::NotFound)
    }

    fn open_client(&self, _caller: &ApiCaller) -> (u64, Arc<tokio::sync::Notify>) {
        (0, Arc::new(tokio::sync::Notify::new()))
    }

    fn client_subscribe(
        &self,
        _client: u64,
        _petal_id: &str,
        _source: &str,
        _params: &Value,
    ) -> Result<u64, HostRefusal> {
        Err(HostRefusal::NotAvailable("extension streams unsupported on headless relay".into()))
    }

    fn client_unsubscribe(&self, _client: u64, _subscription: u64) {}

    fn take_client_events(&self, _client: u64, _max: usize) -> Vec<String> {
        Vec::new()
    }

    fn close_client(&self, _client: u64) {}

    fn verse_policy(&self, verse_id: &str) -> AdminReply {
        let (tx, rx) = oneshot::channel();
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        if self
            .db_cmd_tx
            .send(DbCommand::GetExtensionPolicy {
                verse_id: verse_id.to_string(),
                reply: db_tx,
            })
            .is_err()
        {
            let _ = tx.send(Err(AdminRefusal {
                code: "not-available".into(),
                message: "db thread unavailable".into(),
            }));
            return rx;
        }
        self.rt.spawn_blocking(move || match db_rx.recv_timeout(DB_TIMEOUT) {
            Ok(Ok(policy)) => {
                let _ = tx.send(Ok(json!({ "policy": policy })));
            }
            Ok(Err(e)) => {
                let _ = tx.send(Err(AdminRefusal {
                    code: "failed".into(),
                    message: e,
                }));
            }
            Err(_) => {
                let _ = tx.send(Err(AdminRefusal {
                    code: "not-available".into(),
                    message: "policy lookup timed out".into(),
                }));
            }
        });
        rx
    }

    fn set_verse_policy(&self, actor_did: &str, verse_id: &str, policy: Value) -> AdminReply {
        let (tx, rx) = oneshot::channel();
        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        if self
            .db_cmd_tx
            .send(DbCommand::SetExtensionPolicy {
                verse_id: verse_id.to_string(),
                policy,
                actor_did: actor_did.to_string(),
                reply: db_tx,
            })
            .is_err()
        {
            let _ = tx.send(Err(AdminRefusal {
                code: "not-available".into(),
                message: "db thread unavailable".into(),
            }));
            return rx;
        }
        self.rt.spawn_blocking(move || match db_rx.recv_timeout(DB_TIMEOUT) {
            Ok(Ok(saved)) => {
                let _ = tx.send(Ok(saved));
            }
            Ok(Err(e)) => {
                let code = if e.contains("role") {
                    "role-denied"
                } else {
                    "invalid-policy"
                };
                let _ = tx.send(Err(AdminRefusal {
                    code: code.into(),
                    message: e,
                }));
            }
            Err(_) => {
                let _ = tx.send(Err(AdminRefusal {
                    code: "not-available".into(),
                    message: "policy update timed out".into(),
                }));
            }
        });
        rx
    }
}

impl ImportHooks for RelayPluginState {
    fn is_bound(&self, petal_id: &str, mime: &str) -> bool {
        self.resolve_binding(petal_id, IMPORT_HOOK, mime).is_some()
    }

    fn import(&self, call: ImportCall) -> Result<ImportReply, ImportRefusal> {
        let binding = self
            .resolve_binding(&call.petal_id, IMPORT_HOOK, &call.mime)
            .ok_or(ImportRefusal::Unbound)?;
        let package_id = binding.package_id.ok_or(ImportRefusal::Unbound)?;
        if call.body.len() > HOOK_MAX_BODY_BYTES {
            return Err(ImportRefusal::Failed);
        }
        let req = HookRequest {
            hook: IMPORT_HOOK.into(),
            key: call.mime.clone(),
            binding_scope: binding.scope,
            config: binding.config.to_string(),
            input: json!({
                "petal_id": call.petal_id,
                "file_name": call.file_name,
                "mime": call.mime,
            })
            .to_string(),
            body: call.body,
        };
        let (tx, rx) = oneshot::channel();
        let instance_res = self.ensure_instance(&call.petal_id, &package_id);
        let trace = self.trace.clone();
        let petal_id = call.petal_id.clone();
        let mime = call.mime.clone();
        let started = std::time::Instant::now();
        self.rt.spawn_blocking(move || {
            let answer = match instance_res {
                Ok((handle, _, _, _)) => handle.call_hook(&req, None),
                Err(e) => Err(HookCallError::Guest(HookRefusal::Failed(e))),
            };
            let duration_ms = started.elapsed().as_millis() as u64;
            match answer {
                Ok(reply) => {
                    let name = serde_json::from_str::<Value>(&reply.output)
                        .ok()
                        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string));
                    trace.record(ExtensionTraceInput {
                        kind: "asset_import".into(),
                        package_id,
                        petal_id: Some(petal_id),
                        hook: Some(IMPORT_HOOK.into()),
                        key: Some(mime),
                        outcome: "ok".into(),
                        detail: None,
                        duration_ms: Some(duration_ms),
                    });
                    let _ = tx.send(Ok(ImportOutput {
                        glb: reply.body,
                        name,
                    }));
                }
                Err(HookCallError::Guest(HookRefusal::Refused(msg) | HookRefusal::InvalidInput(msg))) => {
                    let _ = tx.send(Err(ImportRefusal::Refused(msg)));
                }
                Err(other) => {
                    trace.record(ExtensionTraceInput {
                        kind: "asset_import".into(),
                        package_id,
                        petal_id: Some(petal_id),
                        hook: Some(IMPORT_HOOK.into()),
                        key: Some(mime),
                        outcome: "failed".into(),
                        detail: Some(other.to_string()),
                        duration_ms: Some(duration_ms),
                    });
                    let _ = tx.send(Err(ImportRefusal::Failed));
                }
            }
        });
        Ok(rx)
    }
}
