use super::*;

/// One staged instance: its guest, window, key, manifest and lease (live while staged or admitted).
pub(super) struct LiveEntry {
    pub(super) handle: InstanceHandle,
    /// The window its scheduler flushes; scene augments stage here without locking the guest.
    pub(super) window: SharedWindow,
    pub(super) key: InstanceKey,
    pub(super) manifest: Arc<ExtensionManifest>,
    pub(super) lease: PluginLease,
    pub(super) staged_at: Instant,
}

/// Why a host-initiated hook call produced no reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HookFailure {
    /// The guest refused (`hook-error::refused`): a `scene.*` refusal blocks the write.
    Refused(String),
    /// The guest rejected the input or failed: the built-in serves.
    Declined(String),
    /// Not running, a trap, a timeout or bad output.
    Fault(String),
}

/// Whether a `scene.property_scope` admits `key` (`*`, `prefix*` or exact; an empty scope admits nothing).
pub(super) fn property_allowed(scope: &[String], key: &str) -> bool {
    scope.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => pattern == key,
    })
}

/// Instances this host staged, by context id; the serving build is the earliest one whose lease still holds, so a
/// rolled-back update keeps serving from the previous build and a retired one is never called.
#[derive(Clone, Default)]
pub(super) struct LiveInstances(pub(super) Arc<Mutex<HashMap<String, Vec<LiveEntry>>>>);

impl LiveInstances {
    pub(super) fn insert(&self, plugin_id: &str, entry: LiveEntry) {
        lock(&self.0)
            .entry(plugin_id.to_string())
            .or_default()
            .push(entry);
    }

    pub(super) fn serving(entries: &[LiveEntry]) -> Option<&LiveEntry> {
        entries
            .iter()
            .filter(|entry| entry.lease.require_active().is_ok())
            .min_by_key(|entry| entry.staged_at)
    }

    pub(super) fn handle(&self, plugin_id: &str) -> Option<InstanceHandle> {
        let entries = lock(&self.0);
        Self::serving(entries.get(plugin_id)?).map(|entry| entry.handle.clone())
    }

    /// The serving manifest at `key` (ingress role floors).
    pub(super) fn manifest_at(&self, key: &InstanceKey) -> Option<Arc<ExtensionManifest>> {
        let entries = lock(&self.0);
        entries
            .values()
            .filter_map(|candidates| Self::serving(candidates))
            .filter(|entry| entry.key == *key)
            .min_by_key(|entry| entry.staged_at)
            .map(|entry| entry.manifest.clone())
    }

    /// Drop retired builds (revoked leases).
    pub(super) fn prune(&self) {
        let mut entries = lock(&self.0);
        for candidates in entries.values_mut() {
            candidates.retain(|entry| entry.lease.require_active().is_ok());
        }
        entries.retain(|_, candidates| !candidates.is_empty());
    }

    /// One host-initiated hook call (`configure`, `ingest.decode`, `egress.snippet`, `petal.map`) on the serving build.
    pub(super) fn call(
        &self,
        plugin_id: &str,
        request: &HookRequest,
        stamp: Option<&DecodeStamp>,
    ) -> Result<HookReply, HookFailure> {
        let handle = self
            .handle(plugin_id)
            .ok_or_else(|| HookFailure::Fault(format!("{plugin_id} is not running")))?;
        handle
            .call_hook(request, stamp)
            .map_err(|error| match error {
                HookCallError::Guest(HookRefusal::Refused(reason)) => HookFailure::Refused(reason),
                HookCallError::Guest(
                    HookRefusal::InvalidInput(reason) | HookRefusal::Failed(reason),
                ) => HookFailure::Declined(reason),
                other => HookFailure::Fault(other.to_string()),
            })
    }

    /// Stage an allowing `scene.*` guest's `set_property` augments in the serving instance's window, all or none;
    /// every key must sit inside the build's property scope (the flush re-authorizes each op against its token).
    pub(super) fn stage_augment(
        &self,
        key: &InstanceKey,
        node_id: &str,
        ops: Vec<AugmentOp>,
    ) -> Result<(), String> {
        let (window, manifest) = {
            let entries = lock(&self.0);
            entries
                .values()
                .filter_map(|candidates| Self::serving(candidates))
                .filter(|entry| entry.key == *key)
                .min_by_key(|entry| entry.staged_at)
                .map(|entry| (entry.window.clone(), entry.manifest.clone()))
                .ok_or("no running instance serves this binding")?
        };
        let scene = manifest.scene.as_ref();
        let writes_properties = scene
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| kinds.contains(&SceneWrite::SetProperty));
        if !writes_properties || manifest.tick.is_none() {
            return Err("the bound build cannot stage property writes".into());
        }
        let scope = scene
            .and_then(|scene| scene.property_scope.clone())
            .unwrap_or_default();
        let pending = ops
            .into_iter()
            .map(|op| {
                if property_allowed(&scope, &op.key) {
                    Ok(PendingOp::SetProperty {
                        node_id: node_id.to_string(),
                        key: op.key,
                        value: op.value,
                    })
                } else {
                    Err(format!(
                        "property {} is outside the build's property scope",
                        op.key
                    ))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bytes: usize = pending
            .iter()
            .map(|op| serde_json::to_vec(op).map_or(usize::MAX, |encoded| encoded.len()))
            .fold(0, usize::saturating_add);
        let mut window = lock(&window);
        let info = window.info();
        if info.ops + pending.len() > info.max_ops
            || info.bytes.saturating_add(bytes) > info.max_bytes
        {
            return Err("the instance's staged window has no room for the augment".into());
        }
        for op in pending {
            window.stage_op(op).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// `blob.source` / `blob.mirror` calls on the serving builds, made off the worker on the services' call threads.
pub(super) struct LiveHookCaller(pub(super) LiveInstances);

impl HookCaller for LiveHookCaller {
    fn call(&self, target: &CallTarget, request: &HookRequest) -> Result<HookReply, HookCallError> {
        // No serving build (stopped between resolution and call): counted as a failed call, never a pass.
        let handle = self.0.handle(&target.plugin_id).ok_or_else(|| {
            HookCallError::NotServed(format!("{} is not running", target.plugin_id))
        })?;
        handle.call_hook(request, None)
    }
}

/// The `blob.mirror` after-commit queue, set once the blob store is bound (the worker builds the mirror then).
#[derive(Resource, Clone, Default)]
pub(crate) struct ExtensionBlobMirror(pub(super) Arc<OnceLock<MirrorQueue>>);

impl ExtensionBlobMirror {
    /// The queue a blob-commit tap feeds; `None` until the plugin worker built the mirror.
    pub(crate) fn queue(&self) -> Option<&MirrorQueue> {
        self.0.get()
    }

    /// Publish the queue once; false when one is already published.
    pub(crate) fn publish(&self, queue: MirrorQueue) -> bool {
        self.0.set(queue).is_ok()
    }
}

/// The floor an accept-mode ingress endpoint declares (default Editor); `None` when it is not declared.
pub(super) fn endpoint_floor(
    manifest: &ExtensionManifest,
    endpoint: &str,
    websocket: bool,
) -> Option<RoleLevel> {
    manifest
        .ingress
        .iter()
        .find_map(|ingress| match (ingress, websocket) {
            (Ingress::HttpServer(declared), false) if declared.id == endpoint => {
                Some(declared.role_floor)
            }
            (Ingress::WebsocketAccept(declared), true) if declared.id == endpoint => {
                Some(declared.role_floor)
            }
            _ => None,
        })
        .map(|floor| role_level(floor.unwrap_or(RoleFloor::Editor)))
}

// ---------------------------------------------------------------------------
// Staging: verified build -> component instance, context, scheduler and seat
// ---------------------------------------------------------------------------

/// Everything one start or update needs to stage a 2.0 build; shared by the ops drain and the binding launcher.
#[derive(Clone)]
pub(super) struct InstanceStager {
    pub(super) ticks: TickHost,
    pub(super) bus: Bus,
    pub(super) net: Net,
    pub(super) components: Option<Arc<ComponentHost>>,
    pub(super) commands: Sender<PluginCommand>,
    pub(super) local_did: String,
    pub(super) verified: VerifiedBuilds,
    pub(super) petals: PetalDirectory,
    pub(super) live: LiveInstances,
}

/// One build to stage at one petal under one context id.
pub(super) struct StageRequest<'a> {
    pub(super) plugin_id: &'a str,
    pub(super) installation_id: &'a str,
    pub(super) package: &'a PluginPackage,
    pub(super) facts: &'a PackageFacts,
    pub(super) petal: &'a PetalAddress,
    pub(super) first_activation: bool,
}

impl InstanceStager {
    /// Instantiate the verified component and stage its context (keyed `(petal, package)`), scheduler and seat;
    /// nothing runs until `TickHost::launch`/`update`. A failure after staging discards the context.
    pub(super) fn stage(&self, request: &StageRequest<'_>) -> Result<StagedInstance, String> {
        let StageRequest {
            plugin_id,
            package,
            facts,
            petal,
            ..
        } = *request;
        let components = self
            .components
            .as_ref()
            .ok_or("the component host did not start on this host")?;
        let manifest = facts
            .manifest
            .clone()
            .ok_or("a web bundle has no instance")?;
        let judged = facts.component_digest.as_deref().unwrap_or_default();
        if !package.component_digest.eq_ignore_ascii_case(judged)
            || !package
                .content_hash
                .eq_ignore_ascii_case(&facts.content_hash)
        {
            return Err(format!(
                "{plugin_id}: the stored build is not the one governance judged"
            ));
        }
        let identity = PackageIdentity::new(
            facts.package_id.clone(),
            facts.content_hash.to_ascii_lowercase(),
        )?;
        let source_class = manifest
            .readings
            .as_ref()
            .and_then(|r| r.source_class.classes().first().copied());
        let scopes = fe_plugin::host_bridge::v2_binding_for_petal(
            &petal.verse_id,
            Some(petal.fractal_id.as_str()),
            Some(petal.petal_id.as_str()),
            source_class,
        );
        let token = CapabilityToken::mint_v2(plugin_id, &manifest, &scopes)?;
        let component = components
            .compile(&package.component)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        if !component
            .digest()
            .eq_ignore_ascii_case(&package.component_digest)
        {
            return Err(format!(
                "{plugin_id}: the component digest differs from the sealed build"
            ));
        }
        // A manifest that `provides` hooks needs the `hooks` export (bindings.md §7.3; R-HOOKS-02).
        component
            .check_provides(&manifest)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        // The factory reads this while staging below: the admission is stamped with exactly this build.
        self.verified.record(plugin_id, identity);
        let key = InstanceKey::new(petal.petal_id.clone(), facts.package_id.clone());
        let context = self.ticks.contexts().stage_instance(
            &key,
            plugin_id.to_string(),
            token,
            self.commands.clone(),
        )?;
        self.assemble(
            request,
            &manifest,
            components,
            &component,
            key,
            context.clone(),
        )
        .map_err(|error| {
            self.ticks.contexts().discard(&context);
            error
        })
    }

    pub(super) fn assemble(
        &self,
        request: &StageRequest<'_>,
        manifest: &Arc<ExtensionManifest>,
        components: &ComponentHost,
        component: &fe_plugin::wasm::component::ExtensionComponent,
        key: InstanceKey,
        context: fe_plugin::context::PluginContext,
    ) -> Result<StagedInstance, String> {
        let StageRequest {
            plugin_id,
            installation_id,
            facts,
            petal,
            first_activation,
            ..
        } = *request;
        let clock = self.ticks.clock();
        let window = window_for_token(context.capabilities());
        let source_class = context
            .capabilities()
            .grants_v2
            .as_ref()
            .and_then(|grants| grants.readings_class)
            .and_then(|class| serde_json::to_value(class).ok())
            .and_then(|value| value.as_str().map(str::to_string));
        let binding = Binding {
            installation_id: installation_id.to_string(),
            package_id: facts.package_id.clone(),
            content_hash: facts.content_hash.clone(),
            verse_id: petal.verse_id.clone(),
            fractal_id: petal.fractal_id.clone(),
            petal_id: petal.petal_id.clone(),
            identity_did: self.local_did.clone(),
            source_class,
        };
        let broker = HostBroker::new(
            BusBroker::new(self.bus.clone()),
            NetBroker::new(self.net.clone()),
        );
        let mut setup = InstanceSetup::new(binding, context.clone(), window.clone(), clock.clone())
            .with_broker(Arc::new(broker));
        if let Some(kv) = &manifest.kv {
            setup = setup.with_kv(KvSlot::new(Box::new(MemoryKv::new()), kv.max_bytes));
        }
        let instance = components
            .instantiate(component, setup)
            .map_err(|error| format!("{} ({error})", error.code()))?;
        let handle = InstanceHandle::new(instance);
        let budget = context.capabilities().tick_budget().copied();
        let scheduler = TickScheduler::for_budget(
            budget,
            clock,
            window.clone(),
            Box::new(handle.clone()),
            Box::new(ContextTickSink::new(context.clone())),
        );
        let spec = InstanceSpec {
            installation_id: installation_id.to_string(),
            content_hash: facts.content_hash.clone(),
            verse_id: petal.verse_id.clone(),
            petal_id: petal.petal_id.clone(),
            identity_did: self.local_did.clone(),
            manifest: manifest.clone(),
        };
        let seat = InstanceSeat {
            bus: self.bus.clone(),
            spec,
            bus_hooks: Box::new(handle.clone()),
            net: Some(NetSeat {
                net: self.net.clone(),
                hooks: Box::new(handle.clone()),
            }),
        };
        self.live.insert(
            plugin_id,
            LiveEntry {
                handle: handle.clone(),
                key,
                manifest: manifest.clone(),
                window,
                lease: context.revocation_lease(),
                staged_at: Instant::now(),
            },
        );
        Ok(StagedInstance {
            plugin_id: plugin_id.to_string(),
            component_digest: component.digest().to_string(),
            launch: Launch {
                scheduler,
                seat: Some(seat),
                guest: Box::new(handle),
                context,
                first_activation,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Installed builds (the package store as R19 judges it)
// ---------------------------------------------------------------------------

/// One verified build in the package store.
#[derive(Clone)]
pub(super) struct CatalogBuild {
    pub(super) facts: PackageFacts,
    pub(super) name: String,
    /// `None` when no loaded root vouches (row 4 would refuse it).
    pub(super) tier: Option<TrustTier>,
}

/// The package store, re-scanned on a timer; builds are verified once per `(package_id, content_hash)`.
pub(super) struct InstalledCatalog {
    pub(super) store: PluginPackageStore,
    pub(super) governance: ExtensionGovernance,
    pub(super) trust_root: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub(super) allow_dev: bool,
    pub(super) builds: BTreeMap<String, CatalogBuild>,
    /// Every listed `(package_id, content_hash)` checked so far: its build, or when a refused one is re-tried
    /// (the ops drain may still be writing it).
    pub(super) checked: HashMap<(String, String), Result<CatalogBuild, Instant>>,
    pub(super) last_scan: Option<Instant>,
}

impl InstalledCatalog {
    pub(super) fn new(dir: &Path, governance: ExtensionGovernance, allow_dev: bool) -> Self {
        Self {
            store: PluginPackageStore::new(dir.join(PACKAGES_DIR)),
            trust_root: governance.trust_root_predicate(),
            governance,
            allow_dev,
            builds: BTreeMap::new(),
            checked: HashMap::new(),
            last_scan: None,
        }
    }

    /// Re-verify sealed bytes end to end; the package the bytes name must be `package_id`.
    pub(super) fn verify(
        &self,
        package_id: &str,
        content_hash: &str,
    ) -> Result<(PluginPackage, CatalogBuild), String> {
        let bytes = self
            .store
            .get(package_id, content_hash)
            .map_err(|error| format!("package store: {error}"))?
            .ok_or_else(|| format!("the stored build of {package_id} is missing"))?;
        let trust = PluginTrust {
            is_trust_root: &*self.trust_root,
            allow_unsigned_dev: self.allow_dev,
        };
        let package = verify_plugin_package(&bytes, Some(package_id), &trust)
            .map_err(|error| error.to_string())?;
        if !package.content_hash.eq_ignore_ascii_case(content_hash) {
            return Err(format!("the stored build of {package_id} changed on disk"));
        }
        let validated = parse_and_validate(&package.extension_json)
            .map_err(|error| format!("package-invalid ({error})"))?;
        if validated.manifest.id != package.package_id {
            return Err("package-invalid (manifest names another package)".into());
        }
        let licensed = match package.license_json.as_deref() {
            None => false,
            Some(raw) => serde_json::from_slice::<fe_format::license::License>(raw)
                .map_or(true, |license| {
                    matches!(license.license_type, fe_format::license::LicenseType::Paid)
                }),
        };
        let tier = self
            .governance
            .trust()
            .tier_for(package.trusted_by.as_deref());
        let facts = PackageFacts {
            package_id: package.package_id.clone(),
            version: package.version.clone(),
            content_hash: package.content_hash.clone(),
            component_digest: Some(package.component_digest.clone()),
            plaintext_digest: None,
            capability_digest: validated.capability_digest.clone(),
            // Untrusted builds are refused by `check` anyway; never let the facts read as first-party.
            trust: tier.unwrap_or(TrustTier::Publisher),
            writes: PackageFacts::manifest_writes(&validated.manifest),
            licensed,
            manifest: Some(Arc::new(validated.manifest.clone())),
        };
        let build = CatalogBuild {
            facts,
            name: validated.manifest.name.clone(),
            tier,
        };
        Ok((package, build))
    }

    /// List the store; new hashes are verified, the highest version per package id is the installed build.
    pub(super) fn refresh_if_due(&mut self) {
        if self.last_scan.is_some_and(|at| at.elapsed() < CATALOG_SCAN) {
            return;
        }
        self.last_scan = Some(Instant::now());
        let listed = match self.store.list() {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(%error, "extension package store unreadable");
                return;
            }
        };
        let now = Instant::now();
        let mut next: BTreeMap<String, CatalogBuild> = BTreeMap::new();
        let mut listed_keys = std::collections::HashSet::new();
        for (package_id, content_hash) in listed {
            let key = (package_id.clone(), content_hash.clone());
            let due = match self.checked.get(&key) {
                None => true,
                Some(Err(retry_at)) => *retry_at <= now,
                Some(Ok(_)) => false,
            };
            if due {
                let outcome = self
                    .verify(&package_id, &content_hash)
                    .map(|(_, build)| build)
                    .map_err(|reason| {
                        tracing::warn!(%package_id, %reason, "stored extension failed re-verification");
                        now + REFUSAL_RETRY
                    });
                self.checked.insert(key.clone(), outcome);
            }
            if let Some(Ok(build)) = self.checked.get(&key) {
                let newer = next.get(&package_id).map_or(true, |current| {
                    semver_after(&build.facts.version, &current.facts.version)
                });
                if newer {
                    next.insert(package_id, build.clone());
                }
            }
            listed_keys.insert(key);
        }
        self.checked.retain(|key, _| listed_keys.contains(key));
        self.builds = next;
    }

    /// The installed build as R19 judges it (the launcher's `package`).
    pub(super) fn check(&self, installation_id: &str) -> Option<PackageCheck> {
        let build = self.builds.get(installation_id)?;
        Some(match build.tier {
            Some(_) => PackageCheck::Verified(build.facts.clone()),
            None => PackageCheck::Untrusted(format!(
                "no loaded trust root vouches for {installation_id}"
            )),
        })
    }

    /// Fresh sealed bytes of the installed build, verified again before they run.
    pub(super) fn package(&self, installation_id: &str) -> Result<PluginPackage, String> {
        let build = self
            .builds
            .get(installation_id)
            .ok_or_else(|| format!("{installation_id} is not installed"))?;
        self.verify(&build.facts.package_id, &build.facts.content_hash)
            .map(|(package, _)| package)
    }
}

/// Whether SemVer `candidate` sorts after `current` (unparsable versions never win).
pub(super) fn semver_after(candidate: &str, current: &str) -> bool {
    let triplet = |version: &str| -> Option<(u64, u64, u64)> {
        let mut parts = version.split(['-', '+']).next()?.split('.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    };
    matches!((triplet(candidate), triplet(current)), (Some(next), Some(now)) if next > now)
}

/// One build's resolver facts in one verse; instance counts are patched in by the activator.
pub(super) fn build_facts(
    build: &CatalogBuild,
    verse_id: &str,
    petals: &[PetalAddress],
    local: &Governance,
    governance: &ExtensionGovernance,
) -> BuildFacts {
    let facts = &build.facts;
    let manifest = facts.manifest.as_deref();
    let installation_id = facts.package_id.clone();
    let hashes: Vec<&str> = facts.hashes().collect();
    let integrity = if build.tier.is_none() {
        BuildIntegrity::UntrustedSigner
    } else if facts.licensed && governance.entitlement(facts).is_err() {
        BuildIntegrity::EntitlementRequired
    } else {
        BuildIntegrity::Ok
    };
    let policy = match governance.evaluate(verse_id, facts) {
        Ok(()) => PolicyVerdict::Admits,
        Err(reason) if reason.contains("this device") => PolicyVerdict::LocalDenies,
        Err(_) => PolicyVerdict::VerseDenies,
    };
    BuildFacts {
        installation_id: installation_id.clone(),
        package_id: facts.package_id.clone(),
        version: facts.version.clone(),
        display_name: Some(build.name.clone()),
        integrity,
        revoked: governance.is_revoked_any(&hashes),
        kill_override: governance.is_overridden(verse_id, &facts.content_hash),
        policy,
        provides: manifest
            .map(|manifest| manifest.provides.clone())
            .unwrap_or_default(),
        writes_scene: manifest
            .and_then(|manifest| manifest.scene.as_ref())
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| !kinds.is_empty()),
        consent_recorded: local
            .consent(&installation_id)
            .is_some_and(|consent| consent.capability_digest == facts.capability_digest),
        disabled: local.is_locally_disabled(&installation_id, None),
        disabled_petals: petals
            .iter()
            .filter(|petal| local.is_locally_disabled(&installation_id, Some(&petal.petal_id)))
            .map(|petal| petal.petal_id.clone())
            .collect(),
        degraded: false,
        binding_instances: 0,
        instance_at_petals: BTreeSet::new(),
    }
}

/// The binding activator's hands: the installed builds, the stager and the live guests.
pub(super) struct BridgeLauncher<'a> {
    pub(super) catalog: &'a InstalledCatalog,
    pub(super) stager: &'a InstanceStager,
    pub(super) live: &'a LiveInstances,
}

impl BindingLauncher for BridgeLauncher<'_> {
    fn package(&self, installation_id: &str) -> Option<PackageCheck> {
        self.catalog.check(installation_id)
    }

    fn stage(
        &mut self,
        plan: &InstancePlan,
        package: &PackageFacts,
        first_activation: bool,
    ) -> Result<StagedInstance, String> {
        let sealed = self.catalog.package(&plan.installation_id)?;
        self.stager.stage(&StageRequest {
            plugin_id: &plan.plugin_id,
            installation_id: &plan.installation_id,
            package: &sealed,
            facts: package,
            petal: &plan.petal,
            first_activation,
        })
    }

    fn configure(&mut self, plugin_id: &str, call: &ConfigurePlan) -> Result<(), String> {
        let request = HookRequest::configure(
            &call.point.hook,
            &call.point.key,
            &call.binding_scope,
            &call.config.to_string(),
        );
        self.live
            .call(plugin_id, &request, None)
            .map(|_| ())
            .map_err(|failure| format!("{failure:?}"))
    }
}

// ---------------------------------------------------------------------------
// Accepted-payload decode (BND-10b)
// ---------------------------------------------------------------------------

/// Describes and reads accepted payload files for `decode::read_within_cap`, which sums the declared sizes against
/// the cap BEFORE any read and checks each file's length and BLAKE3 (steer 02:25; fe-plugin `src/AGENTS.md`).
#[derive(Clone)]
pub(super) struct PayloadReader {
    pub(super) api: ExtensionApiSlot,
    pub(super) blobs: ExtensionBlobSlot,
}

impl PayloadReader {
    /// Each asset's stored row (size, type, BLAKE3) in `asset_ids` order; reads no blob byte.
    pub(super) fn declared(&self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String> {
        let api = self.api.get().map_err(|error| error.to_string())?.clone();
        let mut vars = HashMap::new();
        vars.insert("ids".to_string(), json!(asset_ids));
        let sql = fe_api::query_guard::with_statement_timeout(
            "SELECT asset_id, content_type, size_bytes, content_hash FROM asset WHERE asset_id IN $ids",
        );
        let rows =
            db_rows_within(&api, sql, vars, DB_REPLY_TIMEOUT).map_err(|error| error.to_string())?;
        let mut declared = Vec::with_capacity(asset_ids.len());
        for asset_id in asset_ids {
            let row = rows
                .iter()
                .find(|row| row.get("asset_id").and_then(Value::as_str) == Some(asset_id.as_str()))
                .ok_or_else(|| format!("asset {asset_id} is not stored on this peer"))?;
            let size = row
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("asset {asset_id} has no size"))?;
            let content_hash = row
                .get("content_hash")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| format!("asset {asset_id} has no content hash"))?;
            let content_type = row
                .get("content_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            declared.push(DecodeFile {
                asset_id: asset_id.clone(),
                content_type,
                size,
                content_hash,
            });
        }
        Ok(declared)
    }

    /// At most `limit` bytes of the file's local blob; the caller compares length and hash.
    pub(super) fn read_bounded(&self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String> {
        let store = self
            .blobs
            .0
            .get()
            .ok_or("the blob store is not bound")?
            .clone();
        let hash = fe_runtime::blob_store::hash_from_hex(&file.content_hash)
            .map_err(|error| error.to_string())?;
        let path = store
            .get_blob_path(&hash)
            .ok_or_else(|| format!("the blob of {} is not on this peer", file.asset_id))?;
        let handle = std::fs::File::open(&path)
            .map_err(|error| format!("{}: {error}", file.asset_id))?;
        let capacity = limit.min(MAX_DECODE_BODY_BYTES as u64 + 1);
        let mut bytes = Vec::with_capacity(usize::try_from(capacity).unwrap_or(0));
        handle
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{}: {error}", file.asset_id))?;
        Ok(bytes)
    }
}

/// `DecodeHost` over the payload reader and the live guests.
pub(super) struct DecodePort<'a> {
    pub(super) reader: &'a PayloadReader,
    pub(super) live: &'a LiveInstances,
}

impl DecodeHost for DecodePort<'_> {
    fn declared_files(&mut self, asset_ids: &[String]) -> Result<Vec<DecodeFile>, String> {
        self.reader.declared(asset_ids)
    }

    fn read_bounded(&mut self, file: &DecodeFile, limit: u64) -> Result<Vec<u8>, String> {
        self.reader.read_bounded(file, limit)
    }

    fn call_decode(
        &mut self,
        target: &CallTarget,
        call: &DecodeCall,
    ) -> Result<DecodeReply, DecodeCallError> {
        let request = HookRequest {
            hook: DECODE_HOOK.to_string(),
            key: call.key.clone(),
            binding_scope: call.binding_scope.clone(),
            config: call.config.to_string(),
            input: call.input.to_string(),
            body: call.body.clone(),
        };
        match self
            .live
            .call(&target.plugin_id, &request, Some(&call.stamp))
        {
            Ok(reply) => serde_json::from_str(&reply.output)
                .map(|output| DecodeReply { output })
                .map_err(|error| DecodeCallError::Fault(format!("decode output: {error}"))),
            Err(HookFailure::Refused(reason) | HookFailure::Declined(reason)) => {
                Err(DecodeCallError::Declined(reason))
            }
            Err(HookFailure::Fault(reason)) => Err(DecodeCallError::Fault(reason)),
        }
    }
}

// ---------------------------------------------------------------------------
// Published views (resolution cache, installations, verse owners)
// ---------------------------------------------------------------------------

/// One bound point at one petal: this host's answer and the row that decided it.
#[derive(Clone)]
pub(super) struct PublishedPoint {
    pub(super) point: BindingPoint,
    pub(super) resolved: ResolvedPoint,
    pub(super) winner: Option<BindingRecord>,
    /// The installed build of a resolved package (its `scene` grant shapes the scene gate).
    pub(super) manifest: Option<Arc<ExtensionManifest>>,
}

/// One petal's published points and its full scope string.
#[derive(Clone)]
pub(super) struct PublishedPetal {
    pub(super) scope: String,
    pub(super) points: Vec<PublishedPoint>,
}

impl PublishedPoint {
    /// The API shape (`GET .../bindings/effective`).
    pub(super) fn effective(&self, petal_scope: &str) -> EffectivePoint {
        let winner = self.winner.as_ref();
        EffectivePoint {
            hook: self.point.hook.clone(),
            key: self.point.key.clone(),
            scope: winner.map_or_else(|| petal_scope.to_string(), |record| record.scope.clone()),
            mode: winner.map_or(BindingMode::Builtin, |record| record.mode),
            package_id: winner.and_then(|record| record.package_id.clone()),
            version_req: winner.and_then(|record| record.version_req.clone()),
            resolution: Some(self.resolved.resolution.clone()),
        }
    }

    /// The winning row when it binds the compiled-in terrain package, which serves from its config, no guest.
    pub(super) fn native_terrain(&self) -> Option<&BindingRecord> {
        self.winner.as_ref().filter(|record| {
            record.mode == BindingMode::Extension
                && record.package_id.as_deref() == Some(TERRAIN_EXTENSION_ID)
        })
    }
}

#[derive(Default)]
pub(super) struct BindingViewInner {
    pub(super) generation: u64,
    pub(super) petals: HashMap<String, PublishedPetal>,
    pub(super) status: Vec<(String, u64)>,
}

/// This host's effective bindings per petal, as the worker last resolved them.
#[derive(Clone, Default)]
pub(super) struct BindingView(pub(super) Arc<RwLock<BindingViewInner>>);

impl BindingView {
    pub(super) fn generation(&self) -> u64 {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
    }

    pub(super) fn points_at(&self, petal_id: &str) -> Option<Vec<EffectivePoint>> {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let petal = inner.petals.get(petal_id)?;
        Some(
            petal
                .points
                .iter()
                .map(|point| point.effective(&petal.scope))
                .collect(),
        )
    }

    /// Who serves `point` at `petal_id`, from the published cache (the terrain bridge's `petal.map` input). A resolved
    /// point answers `Call` even before its lazy instance runs: `ExtensionBindings::call_hook` starts it.
    pub(super) fn need_at(&self, petal_id: &str, point: &BindingPoint) -> Need {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let Some(published) = inner.petals.get(petal_id).and_then(|petal| {
            petal
                .points
                .iter()
                .find(|candidate| candidate.point == *point)
        }) else {
            return Need::Builtin(None);
        };
        if let Some(record) = published.native_terrain() {
            return Need::Call(CallTarget {
                plugin_id: TERRAIN_EXTENSION_ID.to_string(),
                installation_id: TERRAIN_EXTENSION_ID.to_string(),
                key: InstanceKey::new(petal_id.to_string(), TERRAIN_EXTENSION_ID.to_string()),
                binding_scope: record.scope.clone(),
                config: record.config.clone(),
            });
        }
        match &published.resolved.resolution {
            BindingResolution::Resolved { package_id, scope } => Need::Call(CallTarget {
                plugin_id: binding_instance_id(package_id, petal_id),
                installation_id: package_id.clone(),
                key: InstanceKey::new(petal_id.to_string(), package_id.clone()),
                binding_scope: scope.clone(),
                config: published
                    .winner
                    .as_ref()
                    .map_or_else(|| json!({}), |record| record.config.clone()),
            }),
            _ => Need::Builtin(Some(published.resolved.clone())),
        }
    }

    /// The extension resolving `hook` (keyless) at `petal_id`, as the `scene.*` gate reads it; never blocks.
    pub(super) fn scene_binding(&self, petal_id: &str, hook: &str) -> Option<SceneBinding> {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let published = inner
            .petals
            .get(petal_id)?
            .points
            .iter()
            .find(|candidate| candidate.point.hook == hook && candidate.point.key.is_empty())?;
        // A degraded instance keeps its binding in force: the gate dispatches, the call faults, the write is
        // refused naming the package (fail closed). Disable or unbind resolves elsewhere and frees the scope.
        let (package_id, scope) = match &published.resolved.resolution {
            BindingResolution::Resolved { package_id, scope } => (package_id, scope),
            BindingResolution::Unresolved {
                package_id,
                scope,
                reason: UnresolvedReason::Degraded,
                ..
            } => (package_id, scope),
            _ => return None,
        };
        let scene = published
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.scene.clone());
        let writes_properties = scene
            .as_ref()
            .and_then(|scene| scene.write.as_ref())
            .is_some_and(|kinds| kinds.contains(&SceneWrite::SetProperty));
        let ticks = published
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.tick.is_some());
        Some(SceneBinding {
            package_id: package_id.clone(),
            binding_scope: scope.clone(),
            config: published
                .winner
                .as_ref()
                .map_or_else(|| json!({}), |record| record.config.clone()),
            property_scope: scene
                .and_then(|scene| scene.property_scope)
                .unwrap_or_default(),
            may_augment: writes_properties && ticks,
        })
    }

    /// Import judgment: every keyless `scene.*` point at `petal_id` runs here or is the built-in, else unjudgeable.
    pub(super) fn scene_judgeable(&self, petal_id: &str) -> Result<(), ProvisionFailure> {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let petal = inner.petals.get(petal_id).ok_or_else(|| {
            ProvisionFailure::Unavailable("the new petal's bindings were not published".into())
        })?;
        for published in petal
            .points
            .iter()
            .filter(|published| published.point.hook.starts_with("scene.") && published.point.key.is_empty())
        {
            let reason = match &published.resolved.resolution {
                BindingResolution::Resolved { .. } | BindingResolution::Builtin { .. } => continue,
                BindingResolution::Elsewhere { .. } => "the binding runs on another peer".to_string(),
                BindingResolution::Unresolved {
                    package_id, reason, ..
                } => format!("{package_id} cannot run on this host ({reason:?})"),
            };
            return Err(ProvisionFailure::Unjudgeable {
                hook: published.point.hook.clone(),
                reason,
            });
        }
        Ok(())
    }

    /// Whether `point` resolves to an extension at `petal_id` in the published cache; never blocks.
    pub(super) fn is_resolved(&self, petal_id: &str, point: &BindingPoint) -> bool {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        inner.petals.get(petal_id).is_some_and(|petal| {
            petal.points.iter().any(|published| {
                published.point == *point
                    && matches!(published.resolved.resolution, BindingResolution::Resolved { .. })
            })
        })
    }

    pub(super) fn status_rows(&self) -> Vec<(String, u64)> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .status
            .clone()
    }

    /// The `ui.panel` and `egress.snippet` points that resolve at `petal_id` (B10c cache input).
    pub(super) fn slots_at(&self, petal_id: &str) -> (Vec<BoundPanel>, Vec<SnippetTarget>) {
        let inner = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let mut panels = Vec::new();
        let mut targets = Vec::new();
        let points = inner
            .petals
            .get(petal_id)
            .map(|petal| petal.points.as_slice());
        for published in points.unwrap_or_default() {
            let BindingResolution::Resolved { package_id, .. } = &published.resolved.resolution
            else {
                continue;
            };
            let point = &published.point;
            match point.hook.as_str() {
                PANEL_HOOK => panels.push(BoundPanel {
                    panel: point.key.clone(),
                    package_id: package_id.clone(),
                }),
                SNIPPET_HOOK => targets.push(SnippetTarget {
                    package_id: package_id.clone(),
                    key: point.key.clone(),
                }),
                _ => {}
            }
        }
        (panels, targets)
    }

    pub(super) fn replace(&self, petals: HashMap<String, PublishedPetal>, status: Vec<(String, u64)>) {
        let mut inner = self.0.write().unwrap_or_else(PoisonError::into_inner);
        inner.generation += 1;
        inner.petals = petals;
        inner.status = status;
    }
}

/// One installation as the status routes show it.
#[derive(Clone)]
pub(super) struct IndexRow {
    pub(super) package_id: String,
    pub(super) version: String,
    pub(super) name: String,
    pub(super) revoked: bool,
    pub(super) disabled: bool,
    pub(super) consented: bool,
    pub(super) instances: Vec<RunningInstance>,
    /// The installed build (the ui registry re-registers when it changes).
    pub(super) content_hash: String,
    pub(super) manifest: Option<Arc<ExtensionManifest>>,
}

/// Installations and their instances, refreshed by the worker; status reads never take the manager lock.
#[derive(Clone, Default)]
pub(super) struct InstallationIndex(pub(super) Arc<RwLock<BTreeMap<String, IndexRow>>>, pub(super) Arc<AtomicU64>);

impl InstallationIndex {
    /// Moves when a refresh changed which builds are installed or running (the ui registry's cheap check).
    pub(super) fn generation(&self) -> u64 {
        self.1.load(Ordering::Acquire)
    }

    pub(super) fn rows(&self) -> BTreeMap<String, IndexRow> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn row(&self, installation_id: &str) -> Option<IndexRow> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(installation_id)
            .cloned()
    }

    pub(super) fn replace(&self, rows: BTreeMap<String, IndexRow>) {
        let fingerprint = |rows: &BTreeMap<String, IndexRow>| -> Vec<(String, String, Vec<(String, bool)>)> {
            rows.values()
                .map(|row| {
                    let running = row
                        .instances
                        .iter()
                        .map(|instance| (instance.plugin_id.clone(), instance.degraded))
                        .collect();
                    (row.package_id.clone(), row.content_hash.clone(), running)
                })
                .collect()
        };
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if fingerprint(&current) != fingerprint(&rows) {
            self.1.fetch_add(1, Ordering::AcqRel);
        }
        *current = rows;
    }
}

/// What one verse row says about this host: whether the local DID owns it and its `default_access`.
#[derive(Clone, Copy)]
pub(super) struct VerseFacts {
    pub(super) owner: bool,
    pub(super) default_access: RoleLevel,
}

/// Whether the local DID is a verse's Owner (`verse.created_by`) and the verse default access, cached; offers and
/// row 10's fallback only, the DB re-checks overrides and writes.
#[derive(Clone)]
pub(super) struct VerseOwners {
    pub(super) api: ExtensionApiSlot,
    pub(super) local_did: String,
    pub(super) cache: Arc<Mutex<HashMap<String, (VerseFacts, Instant)>>>,
}

impl VerseOwners {
    /// Never takes the manager lock: callers may hold it (ops drain `consent_request`).
    pub(super) fn is_owner(&self, verse_id: &str) -> bool {
        self.facts(verse_id).owner
    }

    /// The verse `default_access` (`viewer` or `none`); `None` when unknown (fail closed, R-X-30).
    pub(super) fn default_access(&self, verse_id: &str) -> RoleLevel {
        self.facts(verse_id).default_access
    }

    pub(super) fn facts(&self, verse_id: &str) -> VerseFacts {
        const UNKNOWN: VerseFacts = VerseFacts { owner: false, default_access: RoleLevel::None };
        if let Some((facts, at)) = lock(&self.cache).get(verse_id).copied() {
            if at.elapsed() < OWNER_CACHE {
                return facts;
            }
        }
        let Ok(api) = self.api.get().cloned() else {
            return UNKNOWN;
        };
        let mut vars = HashMap::new();
        vars.insert("v".to_string(), json!(verse_id));
        let sql = fe_api::query_guard::with_statement_timeout(
            "SELECT created_by, default_access FROM verse WHERE verse_id = $v LIMIT 1",
        );
        let facts = match db_rows_within(&api, sql, vars, OWNER_LOOKUP_TIMEOUT) {
            Ok(rows) => {
                let row = rows.first();
                let text = |name: &str| row.and_then(|row| row.get(name)).and_then(Value::as_str);
                VerseFacts {
                    owner: text("created_by") == Some(self.local_did.as_str()),
                    default_access: text("default_access").map_or(RoleLevel::None, RoleLevel::from),
                }
            }
            Err(error) => {
                tracing::debug!(verse_id, %error, "verse owner lookup failed");
                return UNKNOWN;
            }
        };
        lock(&self.cache).insert(verse_id.to_string(), (facts, Instant::now()));
        facts
    }
}

