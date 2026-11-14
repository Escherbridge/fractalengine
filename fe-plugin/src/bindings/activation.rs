//! Binding activation: resolved points become petal instances (bindings.md §5.4-5.7); rationale in
//! `fe-plugin/src/bindings/AGENTS.md` §activation.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use fe_runtime::extension_binding::{
    hook_spec, BindingPoint, BindingResolution, HookCallKind, UnresolvedReason, LAZY_IDLE_STOP_MINUTES,
    MAX_BINDING_INSTANCES_PER_INSTALLATION,
};
use serde_json::Value;

use super::resolver::{effective_binding, resolve, BindingSnapshot, LocalFacts, PetalAddress, ResolvedPoint};
use crate::bus::rate::TokenBucket;
use crate::bus::InstanceKey;
use crate::lifecycle::{
    binding_instance_id, ActivationRequest, InstanceOrigin, LifecycleError, PackageCheck, PackageFacts, PluginManager,
    Precondition, RunningInstance, StagedInstance,
};
use crate::tick::{Clock, DeactivateReason};

/// Host configuration for binding-started instances (OQ-B9); the cap never exceeds the catalogue's 64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationLimits {
    /// A lazy instance stops after this long without a call while its petal is not active.
    pub idle_stop_ms: u64,
    /// Binding-started instances per installation on this host, eager ones included; starts past it resolve `capacity`.
    pub max_instances_per_installation: usize,
}

impl Default for ActivationLimits {
    fn default() -> Self {
        Self {
            idle_stop_ms: LAZY_IDLE_STOP_MINUTES * 60_000,
            max_instances_per_installation: MAX_BINDING_INSTANCES_PER_INSTALLATION,
        }
    }
}

/// Why a verse re-resolves (bindings.md §5.6): one resolver run per dirty verse, never per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Trigger {
    /// A binding row applied or withdrawn.
    BindingRow,
    /// A role statement changed in the verse.
    Role,
    /// The verse policy or the local policy changed.
    Policy,
    /// A trust root changed.
    TrustRoot,
    /// Install, update, uninstall, consent, acceptance, enable or disable; retries refused starts.
    Lifecycle,
    /// A kill list was accepted, or the verse Owner overrode one.
    KillList,
    /// The active petal is in a verse this host has not resolved yet.
    Navigation,
    /// A binding instance stopped, so points past the cap may start.
    Capacity,
}

/// One instance to start, or to move to its installation's current build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlan {
    /// [`binding_instance_id`] of the installation and petal.
    pub plugin_id: String,
    pub installation_id: String,
    pub package_id: String,
    pub petal: PetalAddress,
    /// Kept by an eager point: never idle-stopped or evicted, but counted against the cap like any binding instance.
    pub eager: bool,
}

impl InstancePlan {
    /// The bus, network and context key `(petal, package)`.
    pub fn key(&self) -> InstanceKey {
        InstanceKey::new(self.petal.petal_id.clone(), self.package_id.clone())
    }
}

/// One instance to stop after its in-flight writes (OD-E1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopPlan {
    pub plugin_id: String,
    pub reason: DeactivateReason,
}

/// One service-hook `configure` call: one at start, one per config change (bindings.md §7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigurePlan {
    pub key: InstanceKey,
    pub point: BindingPoint,
    /// Scope string of the binding row that selected the instance.
    pub binding_scope: String,
    pub config: Value,
}

/// What one reconcile, navigation, sweep or need asks of the host; [`BindingActivator::apply`] runs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivationPlan {
    pub stops: Vec<StopPlan>,
    pub updates: Vec<InstancePlan>,
    pub starts: Vec<InstancePlan>,
    pub configures: Vec<ConfigurePlan>,
}

impl ActivationPlan {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.stops.is_empty() && self.updates.is_empty() && self.starts.is_empty() && self.configures.is_empty()
    }
}

/// What [`BindingActivator::apply`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    pub started: Vec<String>,
    /// Running instances a binding now shares (a local bind, or one started before a restart of this table).
    pub adopted: Vec<String>,
    pub updated: Vec<String>,
    pub stopped: Vec<String>,
    pub configured: Vec<String>,
    /// Starts and updates refused, with the state their points now show.
    pub refused: Vec<(String, UnresolvedReason, String)>,
    /// `configure` calls that failed: counted as hook faults, the instance keeps running and the call is retried.
    pub faults: Vec<(String, String)>,
}

/// Where the call for one point goes.
#[derive(Debug, Clone, PartialEq)]
pub struct CallTarget {
    pub plugin_id: String,
    pub installation_id: String,
    pub key: InstanceKey,
    /// `HookRequest::binding_scope`.
    pub binding_scope: String,
    /// The binding's config, validated against `config_schema` by the resolver.
    pub config: Value,
}

/// The answer to "who serves this use".
#[derive(Debug, Clone, PartialEq)]
pub enum Need {
    /// No instance call serves it: nothing bound, a `builtin`, `elsewhere` or unresolved answer (the built-in
    /// serves), or an instance-less hook (`portal.open`, offered for a local Open instead).
    Builtin(Option<ResolvedPoint>),
    /// A running instance serves it.
    Call(CallTarget),
    /// The point resolves but its lazy instance is not running: apply this, then ask again.
    Start(ActivationPlan),
}

/// What only the binary can do for activation: read an installed build, stage it, call its hooks.
pub trait BindingLauncher {
    /// The installed build of `installation_id`, verified as R19 judges it; `None` once it is gone.
    fn package(&self, installation_id: &str) -> Option<PackageCheck>;
    /// Instantiate the verified build and stage its context with `PluginContexts::stage_instance(plan.key(), ..)`,
    /// plus its seat, under `plan.plugin_id`.
    fn stage(&mut self, plan: &InstancePlan, package: &PackageFacts, first_activation: bool) -> Result<StagedInstance, String>;
    /// Deliver one `configure` call (`HookRequest::configure`) to the running instance `plugin_id`.
    fn configure(&mut self, plugin_id: &str, call: &ConfigurePlan) -> Result<(), String>;
}

/// Egress token buckets shared by the instances bindings keep for one installation (bindings.md §5.6).
#[derive(Clone, Default)]
pub struct SharedEgressBuckets(Arc<Mutex<EgressGroups>>);

#[derive(Default)]
struct EgressGroups {
    /// Instance -> its installation (the group).
    members: HashMap<InstanceKey, String>,
    /// `(installation_id, endpoint)` -> the bucket every member draws from.
    buckets: HashMap<(String, String), TokenBucket>,
}

impl SharedEgressBuckets {
    fn lock(&self) -> MutexGuard<'_, EgressGroups> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The instance draws from its installation's buckets from now on.
    pub fn join(&self, key: &InstanceKey, installation_id: &str) {
        self.lock().members.insert(key.clone(), installation_id.to_string());
    }

    /// The instance leaves; a group with no member left drops its buckets.
    pub fn leave(&self, key: &InstanceKey) {
        let mut groups = self.lock();
        let Some(group) = groups.members.remove(key) else { return };
        if !groups.members.values().any(|member| *member == group) {
            groups.buckets.retain(|(installation, _), _| *installation != group);
        }
    }

    /// `None`: not a member, its own per-instance bucket applies. `Some(allowed)`: the shared bucket answered.
    pub fn try_take(&self, key: &InstanceKey, endpoint: &str, rate_hz: f64, now_ms: u64) -> Option<bool> {
        let mut groups = self.lock();
        let group = groups.members.get(key)?.clone();
        let bucket = groups
            .buckets
            .entry((group, endpoint.to_string()))
            .or_insert_with(|| TokenBucket::new(rate_hz, now_ms));
        Some(bucket.try_take(now_ms))
    }

    /// Instances drawing from `installation_id`'s buckets.
    pub fn members(&self, installation_id: &str) -> usize {
        self.lock().members.values().filter(|member| member.as_str() == installation_id).count()
    }
}

/// One point's cached answer, plus the deciding binding's `(scope, config)` when it resolves to an extension.
#[derive(Debug, Clone, PartialEq)]
struct CachedPoint {
    resolved: ResolvedPoint,
    binding: Option<(String, Value)>,
}

type PetalPoints = BTreeMap<BindingPoint, CachedPoint>;

/// One verse's resolutions, by petal.
#[derive(Debug, Clone, Default)]
struct VerseCache {
    petals: BTreeMap<String, (PetalAddress, PetalPoints)>,
}

/// A running instance this table tracks: started by a binding, or a local bind a binding shares.
#[derive(Debug, Clone)]
struct BoundInstance {
    key: InstanceKey,
    installation_id: String,
    verse_id: String,
    /// `LocalBind` instances are shared, never stopped here.
    origin: InstanceOrigin,
    keepers: BTreeSet<BindingPoint>,
    eager: bool,
    /// Last `configure` delivered per service point: `(binding_scope, config)`.
    configured: BTreeMap<BindingPoint, (String, Value)>,
    /// Last call, start, or the moment its petal stopped being active.
    last_use_ms: u64,
}

/// What one desired instance needs, read from the cache.
struct Desired {
    plan: InstancePlan,
    keepers: BTreeSet<BindingPoint>,
    services: Vec<ConfigurePlan>,
}

/// A refusal a retry would repeat; cleared by a lifecycle trigger in its verse.
#[derive(Debug, Clone)]
struct Failure {
    verse_id: String,
    reason: UnresolvedReason,
}

/// Per-host activation state: the resolution cache, the instances bindings keep, and the dirty verses.
pub struct BindingActivator {
    clock: Arc<dyn Clock>,
    limits: ActivationLimits,
    verses: HashMap<String, VerseCache>,
    /// Petal id -> verse id, for reads by petal.
    petal_verse: HashMap<String, String>,
    instances: BTreeMap<String, BoundInstance>,
    dirty: BTreeMap<String, BTreeSet<Trigger>>,
    active: Option<PetalAddress>,
    failures: HashMap<String, Failure>,
    /// Updates that rolled back, by instance id -> verse; not retried until a lifecycle trigger.
    update_failures: HashMap<String, String>,
    /// Instance ids that ran `on-install` already.
    activated_once: HashSet<String>,
    egress: SharedEgressBuckets,
}

fn lock(manager: &Mutex<PluginManager>) -> MutexGuard<'_, PluginManager> {
    manager.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `portal.open` binds a web bundle, which has no instance (D-X5: it mounts only on a local Open).
fn starts_instance(hook: &str) -> bool {
    hook != "portal.open"
}

fn is_eager(hook: &str) -> bool {
    hook_spec(hook).is_some_and(|spec| spec.eager)
}

/// Service hooks get `configure` calls; call hooks get one call per use.
fn is_service(hook: &str) -> bool {
    starts_instance(hook) && hook_spec(hook).is_some_and(|spec| spec.call == HookCallKind::Service)
}

/// The deactivate reason a point's new state stops its instance with.
fn stop_reason_for(resolution: Option<&BindingResolution>) -> DeactivateReason {
    match resolution {
        Some(BindingResolution::Unresolved { reason, .. }) => match reason {
            UnresolvedReason::Revoked => DeactivateReason::Revoked,
            UnresolvedReason::PolicyDenied => DeactivateReason::PolicyDenied,
            UnresolvedReason::RoleDenied => DeactivateReason::RoleLost,
            UnresolvedReason::HostOff | UnresolvedReason::Disabled => DeactivateReason::Disabled,
            _ => DeactivateReason::Unbound,
        },
        _ => DeactivateReason::Unbound,
    }
}

/// Revoked > policy-denied > role-lost > disabled > unbound.
fn severity(reason: DeactivateReason) -> u8 {
    match reason {
        DeactivateReason::Revoked => 4,
        DeactivateReason::PolicyDenied => 3,
        DeactivateReason::RoleLost => 2,
        DeactivateReason::Disabled => 1,
        _ => 0,
    }
}

/// A lifecycle refusal as the §5.2 state its points show.
fn refusal_reason(error: &LifecycleError) -> UnresolvedReason {
    match error {
        LifecycleError::Refused(refusal) => match refusal.precondition {
            Precondition::HostOff | Precondition::NotLocalIdentity => UnresolvedReason::HostOff,
            Precondition::PackageInvalid => UnresolvedReason::PackageInvalid,
            Precondition::UntrustedSigner => UnresolvedReason::UntrustedSigner,
            Precondition::Revoked => UnresolvedReason::Revoked,
            Precondition::EntitlementRequired => UnresolvedReason::EntitlementRequired,
            Precondition::PolicyDenied => UnresolvedReason::PolicyDenied,
            Precondition::RoleDenied => UnresolvedReason::RoleDenied,
            Precondition::ConsentMissing => UnresolvedReason::ConsentMissing,
        },
        LifecycleError::HostOff => UnresolvedReason::HostOff,
        LifecycleError::NotAccepted(_) => UnresolvedReason::NotAccepted,
        LifecycleError::InstallationDisabled(_) => UnresolvedReason::Disabled,
        _ => UnresolvedReason::Degraded,
    }
}

/// The kebab-case state name the status counters use.
fn reason_code(reason: UnresolvedReason) -> String {
    serde_json::to_value(reason).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default()
}

fn empty_config() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Every point a verse's rows name, plus `petal.map` when a petal stores terrain (OQ-B6).
fn points_of(snapshot: &BindingSnapshot<'_>) -> BTreeSet<BindingPoint> {
    let mut points: BTreeSet<BindingPoint> = snapshot.rows.iter().map(|row| row.record.point()).collect();
    if !snapshot.stored_terrain_petals.is_empty() {
        points.insert(BindingPoint::new("petal.map", ""));
    }
    points
}

/// The start plan for `key` at one petal: its installation, eagerness and service configures.
fn start_plan(petal: &PetalAddress, points: &PetalPoints, key: &InstanceKey) -> Option<Desired> {
    let mut desired: Option<Desired> = None;
    for (point, cached) in points {
        let BindingResolution::Resolved { package_id, .. } = &cached.resolved.resolution else { continue };
        if *package_id != key.package_id || !starts_instance(&point.hook) {
            continue;
        }
        let entry = desired.get_or_insert_with(|| {
            let installation_id = cached.resolved.detail.installation_id.clone().unwrap_or_default();
            Desired {
                plan: InstancePlan {
                    plugin_id: binding_instance_id(&installation_id, &petal.petal_id),
                    installation_id,
                    package_id: package_id.clone(),
                    petal: petal.clone(),
                    eager: false,
                },
                keepers: BTreeSet::new(),
                services: Vec::new(),
            }
        });
        entry.plan.eager |= is_eager(&point.hook);
        entry.keepers.insert(point.clone());
        if let (true, Some((scope, config))) = (is_service(&point.hook), &cached.binding) {
            entry.services.push(ConfigurePlan {
                key: key.clone(),
                point: point.clone(),
                binding_scope: scope.clone(),
                config: config.clone(),
            });
        }
    }
    desired
}

impl BindingActivator {
    /// `clock` is the worker's (`TickHost::clock`), so idle stops follow the lab's `SimClock` too.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            limits: ActivationLimits::default(),
            verses: HashMap::new(),
            petal_verse: HashMap::new(),
            instances: BTreeMap::new(),
            dirty: BTreeMap::new(),
            active: None,
            failures: HashMap::new(),
            update_failures: HashMap::new(),
            activated_once: HashSet::new(),
            egress: SharedEgressBuckets::default(),
        }
    }

    /// Host limits; the cap is clamped to [`MAX_BINDING_INSTANCES_PER_INSTALLATION`], which the resolver also reads.
    pub fn with_limits(mut self, limits: ActivationLimits) -> Self {
        self.limits = ActivationLimits {
            max_instances_per_installation: limits.max_instances_per_installation.min(MAX_BINDING_INSTANCES_PER_INSTALLATION),
            ..limits
        };
        self
    }

    /// The shared egress buckets, for the network broker and egress connectors.
    pub fn egress_buckets(&self) -> SharedEgressBuckets {
        self.egress.clone()
    }

    /// Queue one verse for re-resolution.
    pub fn mark_dirty(&mut self, verse_id: &str, trigger: Trigger) {
        self.dirty.entry(verse_id.to_string()).or_default().insert(trigger);
    }

    /// Queue every verse this host resolved or runs instances in (trust roots, kill lists, host switch).
    pub fn mark_all_dirty(&mut self, trigger: Trigger) {
        let verses: BTreeSet<String> = self
            .verses
            .keys()
            .cloned()
            .chain(self.instances.values().map(|instance| instance.verse_id.clone()))
            .collect();
        for verse_id in verses {
            self.mark_dirty(&verse_id, trigger);
        }
    }

    /// The verses to re-resolve, each once with its coalesced triggers; a lifecycle trigger clears that verse's
    /// remembered start and update refusals so they are tried again.
    pub fn take_dirty(&mut self) -> Vec<(String, BTreeSet<Trigger>)> {
        let dirty = std::mem::take(&mut self.dirty);
        let retry: BTreeSet<String> = dirty
            .iter()
            .filter(|(_, triggers)| triggers.contains(&Trigger::Lifecycle))
            .map(|(verse_id, _)| verse_id.clone())
            .collect();
        if !retry.is_empty() {
            self.failures.retain(|_, failure| !retry.contains(&failure.verse_id));
            self.update_failures.retain(|_, verse_id| !retry.contains(verse_id));
        }
        dirty.into_iter().collect()
    }

    /// Re-run the resolver for one verse and plan the difference (bindings.md §5.6). `petals` are the verse's
    /// petals, `running` is `PluginManager::running_instances`; nothing here touches the DB or a guest.
    pub fn reconcile(
        &mut self,
        verse_id: &str,
        snapshot: &BindingSnapshot<'_>,
        petals: &[PetalAddress],
        running: &[RunningInstance],
    ) -> ActivationPlan {
        self.sync_running(running);
        let facts = self.patched_facts(snapshot.facts, running);
        let patched = BindingSnapshot {
            rows: snapshot.rows,
            roles: snapshot.roles,
            facts: &facts,
            stored_terrain_petals: snapshot.stored_terrain_petals,
        };
        self.cache_verse(verse_id, &patched, petals, running);
        self.plan_verse(verse_id, &facts, running)
    }

    /// Drop instances governance stopped (kill list, policy, Disable) or a fault degraded.
    fn sync_running(&mut self, running: &[RunningInstance]) {
        let live: HashSet<&str> = running.iter().filter(|instance| !instance.degraded).map(|instance| instance.plugin_id.as_str()).collect();
        let gone: Vec<String> = self.instances.keys().filter(|id| !live.contains(id.as_str())).cloned().collect();
        for plugin_id in gone {
            self.release(&plugin_id);
        }
    }

    /// The binary's facts, with this host's binding-instance counts (the resolver's cap input) filled in here.
    fn patched_facts(&self, facts: &LocalFacts, running: &[RunningInstance]) -> LocalFacts {
        let mut facts = facts.clone();
        for build in &mut facts.builds {
            build.binding_instances = self
                .instances
                .values()
                .filter(|instance| instance.origin == InstanceOrigin::Binding && instance.installation_id == build.installation_id)
                .count();
            build.instance_at_petals = running
                .iter()
                .filter(|instance| instance.installation_id == build.installation_id && !instance.degraded)
                .map(|instance| instance.petal_id.clone())
                .collect();
        }
        facts
    }

    /// Resolve every point at every petal of the verse into the cache. A degraded instance, or a start refusal a
    /// retry would repeat, overrides `resolved` (the built-in serves meanwhile).
    fn cache_verse(&mut self, verse_id: &str, snapshot: &BindingSnapshot<'_>, petals: &[PetalAddress], running: &[RunningInstance]) {
        let points = points_of(snapshot);
        let degraded: HashSet<InstanceKey> = running
            .iter()
            .filter(|instance| instance.degraded)
            .map(|instance| InstanceKey::new(instance.petal_id.clone(), instance.package_id.clone()))
            .collect();
        let mut cache = VerseCache::default();
        for petal in petals.iter().filter(|petal| petal.verse_id == verse_id) {
            let mut answers = PetalPoints::new();
            for point in &points {
                let mut resolved = resolve(snapshot, petal, point);
                let mut binding = None;
                let mut held_state = None;
                if let BindingResolution::Resolved { package_id, scope } = &resolved.resolution {
                    let installation_id = resolved.detail.installation_id.clone().unwrap_or_default();
                    let plugin_id = binding_instance_id(&installation_id, &petal.petal_id);
                    let key = InstanceKey::new(petal.petal_id.clone(), package_id.clone());
                    let held = if degraded.contains(&key) {
                        Some(UnresolvedReason::Degraded)
                    } else {
                        self.failures.get(&plugin_id).map(|failure| failure.reason)
                    };
                    match held {
                        Some(reason) => {
                            held_state = Some(BindingResolution::Unresolved {
                                package_id: package_id.clone(),
                                scope: scope.clone(),
                                reason,
                                offer: None,
                            });
                        }
                        None => {
                            let config = effective_binding(snapshot.rows, snapshot.roles, scope, point)
                                .map_or_else(empty_config, |record| record.config.clone());
                            binding = Some((scope.clone(), config));
                        }
                    }
                }
                if let Some(state) = held_state {
                    resolved.resolution = state;
                }
                answers.insert(point.clone(), CachedPoint { resolved, binding });
            }
            cache.petals.insert(petal.petal_id.clone(), (petal.clone(), answers));
        }
        if let Some(previous) = self.verses.get(verse_id) {
            for petal_id in previous.petals.keys().filter(|petal_id| !cache.petals.contains_key(*petal_id)) {
                self.petal_verse.remove(petal_id);
            }
        }
        for petal_id in cache.petals.keys() {
            self.petal_verse.insert(petal_id.clone(), verse_id.to_string());
        }
        self.verses.insert(verse_id.to_string(), cache);
    }

    /// Every `(petal, package)` the verse's cache wants an instance for.
    fn desired(&self, verse_id: &str) -> BTreeMap<InstanceKey, Desired> {
        let mut desired = BTreeMap::new();
        let Some(cache) = self.verses.get(verse_id) else { return desired };
        for (petal_id, (petal, points)) in &cache.petals {
            let packages: BTreeSet<&String> = points
                .iter()
                .filter(|(point, _)| starts_instance(&point.hook))
                .filter_map(|(_, cached)| match &cached.resolved.resolution {
                    BindingResolution::Resolved { package_id, .. } => Some(package_id),
                    _ => None,
                })
                .collect();
            for package_id in packages {
                let key = InstanceKey::new(petal_id.clone(), package_id.clone());
                if let Some(want) = start_plan(petal, points, &key) {
                    desired.insert(key, want);
                }
            }
        }
        desired
    }

    /// Stops for instances no point keeps, adoption of running local binds, starts (eager now, lazy only at the
    /// active petal), updates on a build change and configures on a config change.
    fn plan_verse(&mut self, verse_id: &str, facts: &LocalFacts, running: &[RunningInstance]) -> ActivationPlan {
        let desired = self.desired(verse_id);
        let mut plan = ActivationPlan::default();
        let now = self.clock.now_ms();
        let active_petal = self.active.as_ref().map(|petal| petal.petal_id.clone());

        let tracked: Vec<(String, InstanceKey, String, InstanceOrigin)> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.verse_id == verse_id)
            .map(|(id, instance)| (id.clone(), instance.key.clone(), instance.installation_id.clone(), instance.origin))
            .collect();
        for (plugin_id, key, installation_id, origin) in tracked {
            let kept = desired
                .get(&key)
                .is_some_and(|want| origin == InstanceOrigin::LocalBind || want.plan.installation_id == installation_id);
            if kept {
                continue;
            }
            match origin {
                InstanceOrigin::Binding => {
                    let reason = self.stop_reason(&plugin_id);
                    plan.stops.push(StopPlan { plugin_id, reason });
                }
                InstanceOrigin::LocalBind => self.release(&plugin_id),
            }
        }

        let stopping: HashSet<String> = plan.stops.iter().map(|stop| stop.plugin_id.clone()).collect();
        let mut planned: HashMap<String, usize> = HashMap::new();
        for (key, want) in desired {
            let serving = self
                .instances
                .iter()
                .find(|(id, instance)| instance.key == key && !stopping.contains(*id))
                .map(|(id, _)| id.clone());
            if let Some(plugin_id) = serving {
                self.keep(&plugin_id, &want, facts, running, &mut plan);
                continue;
            }
            let existing = running.iter().find(|instance| {
                !instance.degraded
                    && !stopping.contains(&instance.plugin_id)
                    && instance.petal_id == key.petal_id
                    && instance.package_id == key.package_id
            });
            if let Some(existing) = existing {
                if existing.origin == InstanceOrigin::LocalBind || existing.installation_id == want.plan.installation_id {
                    self.adopt(existing, &want, now);
                    plan.configures.extend(want.services);
                    continue;
                }
                plan.stops.push(StopPlan { plugin_id: existing.plugin_id.clone(), reason: DeactivateReason::Unbound });
            }
            if want.plan.eager || active_petal.as_deref() == Some(key.petal_id.as_str()) {
                // The cap counts eager instances too (bindings.md §5.6); a held start shows `capacity` until a stop.
                let installation_id = want.plan.installation_id.clone();
                let held = self.binding_count_except(&installation_id, &stopping) + planned.get(&installation_id).copied().unwrap_or(0);
                if held >= self.limits.max_instances_per_installation {
                    self.mark_unresolved(&want.plan, UnresolvedReason::Capacity);
                    continue;
                }
                *planned.entry(installation_id).or_default() += 1;
                plan.starts.push(want.plan);
                plan.configures.extend(want.services);
            }
        }
        plan
    }

    /// A tracked instance stays: refresh its keepers, plan configures that changed, and an update when its
    /// installation's build changed (binding-started only; a local bind updates through the operator's flow).
    fn keep(&mut self, plugin_id: &str, want: &Desired, facts: &LocalFacts, running: &[RunningInstance], plan: &mut ActivationPlan) {
        let Some(instance) = self.instances.get_mut(plugin_id) else { return };
        instance.keepers = want.keepers.clone();
        instance.eager = want.plan.eager;
        for call in &want.services {
            let current = (call.binding_scope.clone(), call.config.clone());
            if instance.configured.get(&call.point) != Some(&current) {
                plan.configures.push(call.clone());
            }
        }
        if instance.origin != InstanceOrigin::Binding || self.update_failures.contains_key(plugin_id) {
            return;
        }
        let wanted = facts.builds.iter().find(|build| build.installation_id == instance.installation_id).map(|build| &build.version);
        let current = running.iter().find(|entry| entry.plugin_id == plugin_id).map(|entry| &entry.version);
        if let (Some(wanted), Some(current)) = (wanted, current) {
            if wanted != current {
                plan.updates.push(want.plan.clone());
            }
        }
    }

    /// Track a running instance a binding now shares (a local bind, or one this table lost track of).
    fn adopt(&mut self, existing: &RunningInstance, want: &Desired, now_ms: u64) {
        let key = InstanceKey::new(existing.petal_id.clone(), existing.package_id.clone());
        self.egress.join(&key, &existing.installation_id);
        self.instances.insert(
            existing.plugin_id.clone(),
            BoundInstance {
                key,
                installation_id: existing.installation_id.clone(),
                verse_id: existing.verse_id.clone(),
                origin: existing.origin,
                keepers: want.keepers.clone(),
                eager: want.plan.eager,
                configured: BTreeMap::new(),
                last_use_ms: now_ms,
            },
        );
    }

    /// Forget one tracked instance and its egress membership (it was stopped, or is a released local bind).
    fn release(&mut self, plugin_id: &str) {
        if let Some(instance) = self.instances.remove(plugin_id) {
            self.egress.leave(&instance.key);
        }
    }

    /// The most severe reason among the instance's former keepers' new states; `unbound` when a point is gone.
    fn stop_reason(&self, plugin_id: &str) -> DeactivateReason {
        let Some(instance) = self.instances.get(plugin_id) else { return DeactivateReason::Unbound };
        let points = self
            .verses
            .get(&instance.verse_id)
            .and_then(|cache| cache.petals.get(&instance.key.petal_id))
            .map(|(_, points)| points);
        instance
            .keepers
            .iter()
            .map(|point| stop_reason_for(points.and_then(|points| points.get(point)).map(|cached| &cached.resolved.resolution)))
            .max_by_key(|reason| severity(*reason))
            .unwrap_or(DeactivateReason::Unbound)
    }

    /// The active petal changed: lazy points there start; instances at the petal left start their idle clock.
    pub fn navigate(&mut self, active: Option<PetalAddress>) -> ActivationPlan {
        let now = self.clock.now_ms();
        let previous = std::mem::replace(&mut self.active, active.clone());
        if let Some(previous) = previous {
            if active.as_ref().is_none_or(|next| next.petal_id != previous.petal_id) {
                for instance in self.instances.values_mut().filter(|instance| instance.key.petal_id == previous.petal_id) {
                    instance.last_use_ms = now;
                }
            }
        }
        let Some(petal) = active else { return ActivationPlan::default() };
        if self.cached_petal(&petal.petal_id).is_none() {
            self.mark_dirty(&petal.verse_id, Trigger::Navigation);
            return ActivationPlan::default();
        }
        let Some((cached_petal, points)) = self.cached_petal(&petal.petal_id) else { return ActivationPlan::default() };
        let mut plan = ActivationPlan::default();
        let packages: BTreeSet<String> = points
            .iter()
            .filter(|(point, _)| starts_instance(&point.hook))
            .filter_map(|(_, cached)| match &cached.resolved.resolution {
                BindingResolution::Resolved { package_id, .. } => Some(package_id.clone()),
                _ => None,
            })
            .collect();
        for package_id in packages {
            let key = InstanceKey::new(petal.petal_id.clone(), package_id);
            if self.tracked_at(&key).is_some() {
                continue;
            }
            if let Some(want) = start_plan(cached_petal, points, &key) {
                plan.starts.push(want.plan);
                plan.configures.extend(want.services);
            }
        }
        plan
    }

    /// Who serves one use of `point` at `petal_id`; a call touches the instance's idle clock.
    pub fn need(&mut self, petal_id: &str, point: &BindingPoint) -> Need {
        let Some((petal, points)) = self.cached_petal(petal_id) else { return Need::Builtin(None) };
        let Some(cached) = points.get(point) else { return Need::Builtin(None) };
        let BindingResolution::Resolved { package_id, .. } = &cached.resolved.resolution else {
            return Need::Builtin(Some(cached.resolved.clone()));
        };
        if !starts_instance(&point.hook) {
            return Need::Builtin(Some(cached.resolved.clone()));
        }
        let key = InstanceKey::new(petal_id.to_string(), package_id.clone());
        let (binding_scope, config) = cached.binding.clone().unwrap_or_else(|| (String::new(), empty_config()));
        let Some(plugin_id) = self.tracked_at(&key) else {
            let want = start_plan(petal, points, &key);
            let mut plan = ActivationPlan::default();
            if let Some(want) = want {
                plan.starts.push(want.plan);
                plan.configures.extend(want.services);
            }
            return Need::Start(plan);
        };
        let now = self.clock.now_ms();
        let installation_id = match self.instances.get_mut(&plugin_id) {
            Some(instance) => {
                instance.last_use_ms = now;
                instance.installation_id.clone()
            }
            None => String::new(),
        };
        Need::Call(CallTarget { plugin_id, installation_id, key, binding_scope, config })
    }

    // Err carries the Builtin resolution by value; boxing would change this public API used by other crates.
    #[allow(clippy::result_large_err)]
    /// [`need`](Self::need), lazily starting the instance when the point resolves (call hooks, bindings.md §5.6).
    pub fn ensure_call_target(
        &mut self,
        petal_id: &str,
        point: &BindingPoint,
        manager: &Mutex<PluginManager>,
        launcher: &mut dyn BindingLauncher,
    ) -> Result<CallTarget, Option<ResolvedPoint>> {
        match self.need(petal_id, point) {
            Need::Call(target) => Ok(target),
            Need::Builtin(resolved) => Err(resolved),
            Need::Start(plan) => {
                self.apply(plan, manager, launcher);
                match self.need(petal_id, point) {
                    Need::Call(target) => Ok(target),
                    Need::Builtin(resolved) => Err(resolved),
                    Need::Start(_) => Err(self.resolution(petal_id, point).cloned()),
                }
            }
        }
    }

    /// Lazy binding instances idle past the limit while their petal is not active stop `unbound`.
    pub fn sweep_idle(&mut self) -> ActivationPlan {
        let now = self.clock.now_ms();
        let active = self.active.as_ref().map(|petal| petal.petal_id.as_str());
        let stops = self
            .instances
            .iter()
            .filter(|(_, instance)| {
                instance.origin == InstanceOrigin::Binding
                    && !instance.eager
                    && Some(instance.key.petal_id.as_str()) != active
                    && now.saturating_sub(instance.last_use_ms) >= self.limits.idle_stop_ms
            })
            .map(|(plugin_id, _)| StopPlan { plugin_id: plugin_id.clone(), reason: DeactivateReason::Unbound })
            .collect();
        ActivationPlan { stops, ..ActivationPlan::default() }
    }

    /// The installation was uninstalled (`PluginManager::uninstall_instance` already stopped and forgot its
    /// instances): drop what this table holds and re-resolve the verses it ran in.
    pub fn forget_installation(&mut self, installation_id: &str) {
        let ids: Vec<(String, String)> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.installation_id == installation_id)
            .map(|(id, instance)| (id.clone(), instance.verse_id.clone()))
            .collect();
        for (plugin_id, verse_id) in ids {
            self.release(&plugin_id);
            self.mark_dirty(&verse_id, Trigger::Lifecycle);
        }
        let prefix = format!("{installation_id}@");
        self.activated_once.retain(|id| !id.starts_with(&prefix));
        self.failures.retain(|id, _| !id.starts_with(&prefix));
        self.update_failures.retain(|id, _| !id.starts_with(&prefix));
    }

    /// Run a plan: stops first (they free capacity and instance keys), then updates, starts and configures.
    pub fn apply(
        &mut self,
        plan: ActivationPlan,
        manager: &Mutex<PluginManager>,
        launcher: &mut dyn BindingLauncher,
    ) -> ApplyReport {
        let mut report = ApplyReport::default();
        for stop in plan.stops {
            let stopped = lock(manager).stop_instance(&stop.plugin_id, stop.reason);
            if let Some(instance) = self.instances.get(&stop.plugin_id) {
                let binding_started = instance.origin == InstanceOrigin::Binding;
                self.release(&stop.plugin_id);
                if binding_started {
                    self.wake_capacity_waits();
                }
            }
            if stopped {
                report.stopped.push(stop.plugin_id);
            }
        }
        for update in plan.updates {
            self.update(update, manager, launcher, &mut report);
        }
        for start in plan.starts {
            self.start(start, manager, launcher, &mut report);
        }
        for call in plan.configures {
            self.configure(call, launcher, &mut report);
        }
        report
    }

    /// Mark every verse with a point waiting on the cap: the cap is per installation, so a stop frees room in any verse.
    fn wake_capacity_waits(&mut self) {
        let waiting: Vec<String> = self
            .verses
            .iter()
            .filter(|(_, cache)| {
                cache.petals.values().any(|(_, points)| {
                    points.values().any(|cached| {
                        matches!(cached.resolved.resolution, BindingResolution::Unresolved { reason: UnresolvedReason::Capacity, .. })
                    })
                })
            })
            .map(|(verse_id, _)| verse_id.clone())
            .collect();
        for verse_id in waiting {
            self.mark_dirty(&verse_id, Trigger::Capacity);
        }
    }

    /// One start: the cap (eager included), the installed build, R19 plus rows 12-13 before staging, then the host start.
    fn start(&mut self, plan: InstancePlan, manager: &Mutex<PluginManager>, launcher: &mut dyn BindingLauncher, report: &mut ApplyReport) {
        if self.tracked_at(&plan.key()).is_some() {
            return;
        }
        let existing = lock(manager).instance_at(&plan.petal.petal_id, &plan.package_id);
        if let Some(existing) = existing {
            if existing.degraded {
                return self.refuse(&plan, UnresolvedReason::Degraded, "the instance at this petal is degraded".into(), false, report);
            }
            if existing.origin == InstanceOrigin::LocalBind || existing.installation_id == plan.installation_id {
                let keepers = self.keepers_at(&plan.petal, &plan.package_id);
                let want = Desired { plan: plan.clone(), keepers, services: Vec::new() };
                let now = self.clock.now_ms();
                self.adopt(&existing, &want, now);
                report.adopted.push(existing.plugin_id);
                return;
            }
            // Another installation's binding instance holds the key: the newest resolution replaces it.
            if lock(manager).stop_instance(&existing.plugin_id, DeactivateReason::Unbound) {
                report.stopped.push(existing.plugin_id.clone());
            }
            self.release(&existing.plugin_id);
        }
        if self.binding_count(&plan.installation_id) >= self.limits.max_instances_per_installation {
            let detail = format!("{} binding instances is the cap for this installation", self.limits.max_instances_per_installation);
            return self.refuse(&plan, UnresolvedReason::Capacity, detail, false, report);
        }
        let Some(check) = launcher.package(&plan.installation_id) else {
            return self.refuse(&plan, UnresolvedReason::NotInstalled, "the installation is gone".into(), false, report);
        };
        let local_did = lock(manager).governance().local_did().to_string();
        let request = ActivationRequest {
            actor_did: &local_did,
            installation_id: &plan.installation_id,
            verse_id: &plan.petal.verse_id,
            petal_id: &plan.petal.petal_id,
            package: &check,
        };
        let checked = lock(manager).check_binding_start(&request);
        if let Err(error) = checked {
            return self.refuse(&plan, refusal_reason(&error), error.to_string(), false, report);
        }
        let PackageCheck::Verified(facts) = &check else {
            return self.refuse(&plan, UnresolvedReason::PackageInvalid, "the build is not verified".into(), false, report);
        };
        let first_activation = !self.activated_once.contains(&plan.plugin_id);
        let staged = match launcher.stage(&plan, facts, first_activation) {
            Ok(staged) => staged,
            Err(detail) => return self.refuse(&plan, UnresolvedReason::Degraded, detail, true, report),
        };
        let started = lock(manager).start_binding_instance(&request, staged);
        match started {
            Ok(()) => {
                let key = plan.key();
                let keepers = self.keepers_at(&plan.petal, &plan.package_id);
                let last_use_ms = self.clock.now_ms();
                self.egress.join(&key, &plan.installation_id);
                self.activated_once.insert(plan.plugin_id.clone());
                self.instances.insert(
                    plan.plugin_id.clone(),
                    BoundInstance {
                        key,
                        installation_id: plan.installation_id.clone(),
                        verse_id: plan.petal.verse_id.clone(),
                        origin: InstanceOrigin::Binding,
                        keepers,
                        eager: plan.eager,
                        configured: BTreeMap::new(),
                        last_use_ms,
                    },
                );
                report.started.push(plan.plugin_id);
            }
            Err(error) => {
                let reason = refusal_reason(&error);
                self.refuse(&plan, reason, error.to_string(), reason == UnresolvedReason::Degraded, report);
            }
        }
    }

    /// Move a binding instance to its installation's current build (P4.3: the running build serves until the
    /// new one is active; a rollback keeps it and is not retried until a lifecycle trigger).
    fn update(&mut self, plan: InstancePlan, manager: &Mutex<PluginManager>, launcher: &mut dyn BindingLauncher, report: &mut ApplyReport) {
        let Some(check) = launcher.package(&plan.installation_id) else { return };
        let PackageCheck::Verified(facts) = &check else {
            self.update_failures.insert(plan.plugin_id.clone(), plan.petal.verse_id.clone());
            return;
        };
        let staged = match launcher.stage(&plan, facts, false) {
            Ok(staged) => staged,
            Err(detail) => {
                self.update_failures.insert(plan.plugin_id.clone(), plan.petal.verse_id.clone());
                report.refused.push((plan.plugin_id, UnresolvedReason::Degraded, detail));
                return;
            }
        };
        let local_did = lock(manager).governance().local_did().to_string();
        let request = ActivationRequest {
            actor_did: &local_did,
            installation_id: &plan.installation_id,
            verse_id: &plan.petal.verse_id,
            petal_id: &plan.petal.petal_id,
            package: &check,
        };
        let outcome = lock(manager).update_instance(&request, staged);
        match outcome {
            Ok(()) => report.updated.push(plan.plugin_id),
            Err(LifecycleError::UpdateAborted(detail)) => {
                self.release(&plan.plugin_id);
                report.refused.push((plan.plugin_id, UnresolvedReason::Degraded, detail));
            }
            Err(error) => {
                self.update_failures.insert(plan.plugin_id.clone(), plan.petal.verse_id.clone());
                report.refused.push((plan.plugin_id, refusal_reason(&error), error.to_string()));
            }
        }
    }

    /// Deliver one `configure`; a start that was refused leaves nothing to configure.
    fn configure(&mut self, call: ConfigurePlan, launcher: &mut dyn BindingLauncher, report: &mut ApplyReport) {
        let Some(plugin_id) = self.tracked_at(&call.key) else { return };
        match launcher.configure(&plugin_id, &call) {
            Ok(()) => {
                if let Some(instance) = self.instances.get_mut(&plugin_id) {
                    instance.configured.insert(call.point.clone(), (call.binding_scope.clone(), call.config.clone()));
                }
                report.configured.push(plugin_id);
            }
            Err(detail) => report.faults.push((plugin_id, detail)),
        }
    }

    /// Record a refused start or update in the report; its points show the refusal (see [`Self::mark_unresolved`]).
    fn refuse(&mut self, plan: &InstancePlan, reason: UnresolvedReason, detail: String, remember: bool, report: &mut ApplyReport) {
        if remember {
            self.failures.insert(plan.plugin_id.clone(), Failure { verse_id: plan.petal.verse_id.clone(), reason });
        }
        self.mark_unresolved(plan, reason);
        report.refused.push((plan.plugin_id.clone(), reason, detail));
    }

    /// The points at that petal serving `package_id` show `reason` until the next re-resolution; the built-in serves.
    fn mark_unresolved(&mut self, plan: &InstancePlan, reason: UnresolvedReason) {
        let offer = match reason {
            UnresolvedReason::ConsentMissing | UnresolvedReason::NotAccepted | UnresolvedReason::Disabled => reason.offer(),
            _ => None,
        };
        let points = self
            .verses
            .get_mut(&plan.petal.verse_id)
            .and_then(|cache| cache.petals.get_mut(&plan.petal.petal_id))
            .map(|(_, points)| points);
        for cached in points.into_iter().flat_map(|points| points.values_mut()) {
            let BindingResolution::Resolved { package_id, scope } = &cached.resolved.resolution else { continue };
            if *package_id != plan.package_id {
                continue;
            }
            let (package_id, scope) = (package_id.clone(), scope.clone());
            cached.resolved.resolution = BindingResolution::Unresolved { package_id, scope, reason, offer };
            cached.binding = None;
        }
    }

    fn cached_petal(&self, petal_id: &str) -> Option<(&PetalAddress, &PetalPoints)> {
        let verse_id = self.petal_verse.get(petal_id)?;
        self.verses
            .get(verse_id)?
            .petals
            .get(petal_id)
            .map(|(petal, points)| (petal, points))
    }

    /// The tracked instance id serving `key`.
    fn tracked_at(&self, key: &InstanceKey) -> Option<String> {
        self.instances.iter().find(|(_, instance)| instance.key == *key).map(|(id, _)| id.clone())
    }

    /// Resolved points at the petal that `package_id` serves.
    fn keepers_at(&self, petal: &PetalAddress, package_id: &str) -> BTreeSet<BindingPoint> {
        self.cached_petal(&petal.petal_id)
            .map(|(_, points)| {
                points
                    .iter()
                    .filter(|(point, cached)| {
                        starts_instance(&point.hook)
                            && matches!(&cached.resolved.resolution, BindingResolution::Resolved { package_id: id, .. } if id == package_id)
                    })
                    .map(|(point, _)| point.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Binding-started instances an installation holds on this host.
    fn binding_count(&self, installation_id: &str) -> usize {
        self.binding_count_except(installation_id, &HashSet::new())
    }

    /// [`binding_count`](Self::binding_count) without the instances a plan is about to stop.
    fn binding_count_except(&self, installation_id: &str, stopping: &HashSet<String>) -> usize {
        self.instances
            .iter()
            .filter(|(plugin_id, instance)| {
                instance.origin == InstanceOrigin::Binding
                    && instance.installation_id == installation_id
                    && !stopping.contains(*plugin_id)
            })
            .count()
    }

    /// This host's answer for one point at one petal, as last resolved.
    pub fn resolution(&self, petal_id: &str, point: &BindingPoint) -> Option<&ResolvedPoint> {
        self.cached_petal(petal_id)?.1.get(point).map(|cached| &cached.resolved)
    }

    /// Every point resolved at the petal (the Bindings view and `GET .../bindings/effective` for the local DID).
    pub fn resolutions_at(&self, petal_id: &str) -> Vec<(BindingPoint, ResolvedPoint)> {
        self.cached_petal(petal_id)
            .map(|(_, points)| points.iter().map(|(point, cached)| (point.clone(), cached.resolved.clone())).collect())
            .unwrap_or_default()
    }

    /// `bindings.resolved` and `bindings.unresolved.<state>` across every cached petal (R11 status).
    pub fn status_rows(&self) -> Vec<(String, u64)> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for cache in self.verses.values() {
            for (_, points) in cache.petals.values() {
                for cached in points.values() {
                    let name = match &cached.resolved.resolution {
                        BindingResolution::Resolved { .. } => "bindings.resolved".to_string(),
                        BindingResolution::Unresolved { reason, .. } => format!("bindings.unresolved.{}", reason_code(*reason)),
                        _ => continue,
                    };
                    *counts.entry(name).or_default() += 1;
                }
            }
        }
        counts.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::resolver::{BuildFacts, BuildIntegrity, PackagePolicy, PolicyVerdict, StoredRow};
    use crate::capability::{CapabilityManifest, CapabilityToken};
    use crate::lifecycle::{ConsentRecord, GovernanceGates, TrustTier};
    use crate::package::UnsignedPolicy;
    use crate::registry::PluginRegistry;
    use crate::scene_host::PluginContexts;
    use crate::tick::{
        FlushReply, GuestLifecycle, Launch, ManualClock, SharedWindow, TickFault, TickHost, TickInfo, TickInstance,
        TickScheduler, TickSink, TickState,
    };
    use crate::transaction::{StagedFlush, StagedWindow};
    use crate::trust_roots::{RevocationEntry, RevocationList, TrustRoots};
    use crate::wasm::WasmEngine;
    use fe_policy::{materialize, EffectiveRoles, GrantedRole, RoleStatement};
    use fe_runtime::extension_binding::{resolution_scopes, BindingMode, BindingOffer, BindingRecord};
    use fe_sdk::manifest_v2::ProvidedHook;
    use serde_json::json;

    const OWNER: &str = "did:key:owner";
    /// This host's DID: a verse Manager, so its own bindings resolve here.
    const LOCAL: &str = "did:key:local";
    /// Another Manager's peer.
    const OTHER: &str = "did:key:other";
    const CONNECTOR: &str = "org.example.bi-push";
    const MAP: &str = "org.example.city-map";

    struct OpenGates;

    impl GovernanceGates for OpenGates {
        fn entitlement(&self, _package: &PackageFacts) -> Result<(), String> {
            Ok(())
        }
        fn policy(&self, _verse_id: &str, _package: &PackageFacts) -> Result<(), String> {
            Ok(())
        }
        fn role(&self, _petal_id: &str, _writes: bool) -> Result<(), String> {
            Ok(())
        }
    }

    /// A guest that accepts every lifecycle hook and stages nothing.
    struct Guest;

    impl TickInstance for Guest {
        fn refuel(&mut self, _fuel: u64) -> Result<(), String> {
            Ok(())
        }
        fn on_tick(&mut self, _info: &TickInfo) -> Result<(), TickFault> {
            Ok(())
        }
        fn on_deactivate(&mut self, _reason: DeactivateReason) {}
    }

    impl TickSink for Guest {
        fn submit(&mut self, _tick: u64, _flush: StagedFlush, reply: crossbeam::channel::Sender<FlushReply>) {
            let _ = reply.send(FlushReply::Refused("no database in this journey".into()));
        }
    }

    impl GuestLifecycle for Guest {
        fn on_install(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn on_activate(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn on_uninstall(&mut self) {}
        fn degraded(&self) -> Option<String> {
            None
        }
    }

    /// The binary's side: verified builds by installation, keyed staging on the real contexts, configure log.
    struct Launcher {
        ticks: TickHost,
        builds: HashMap<String, PackageFacts>,
        configured: Vec<(String, Value)>,
    }

    impl BindingLauncher for Launcher {
        fn package(&self, installation_id: &str) -> Option<PackageCheck> {
            self.builds.get(installation_id).cloned().map(PackageCheck::Verified)
        }

        fn stage(&mut self, plan: &InstancePlan, package: &PackageFacts, first_activation: bool) -> Result<StagedInstance, String> {
            let (commands, _queued) = crossbeam::channel::unbounded();
            let manifest = CapabilityManifest::from_json(r#"{"id":"bound"}"#).map_err(|error| error.to_string())?;
            let token = CapabilityToken::mint(&plan.plugin_id, &manifest);
            let context = self.ticks.contexts().stage_instance(&plan.key(), plan.plugin_id.clone(), token, commands)?;
            let window: SharedWindow = Arc::new(Mutex::new(StagedWindow::new(0, 0)));
            let scheduler = TickScheduler::without_ticks(self.ticks.clock(), window, Box::new(Guest), Box::new(Guest));
            Ok(StagedInstance {
                plugin_id: plan.plugin_id.clone(),
                component_digest: package.component_digest.clone().unwrap_or_default(),
                launch: Launch { scheduler, seat: None, guest: Box::new(Guest), context, first_activation },
            })
        }

        fn configure(&mut self, plugin_id: &str, call: &ConfigurePlan) -> Result<(), String> {
            self.configured.push((plugin_id.to_string(), call.config.clone()));
            Ok(())
        }
    }

    fn package(package_id: &str, content: &str, component: &str) -> PackageFacts {
        PackageFacts {
            package_id: package_id.into(),
            version: "1.0.0".into(),
            content_hash: content.repeat(32),
            component_digest: Some(component.repeat(32)),
            plaintext_digest: None,
            capability_digest: format!("digest-{content}"),
            trust: TrustTier::Publisher,
            writes: false,
            licensed: false,
            manifest: None,
        }
    }

    fn build(installation_id: &str, package_id: &str, hook: &str, config_schema: Option<Value>) -> BuildFacts {
        BuildFacts {
            installation_id: installation_id.into(),
            package_id: package_id.into(),
            version: "1.0.0".into(),
            display_name: None,
            integrity: BuildIntegrity::Ok,
            revoked: false,
            kill_override: false,
            policy: PolicyVerdict::Admits,
            provides: vec![ProvidedHook { hook: hook.into(), keys: None, uses: None, config_schema }],
            writes_scene: false,
            consent_recorded: true,
            disabled: false,
            disabled_petals: BTreeSet::new(),
            degraded: false,
            binding_instances: 0,
            instance_at_petals: BTreeSet::new(),
        }
    }

    /// A version by `author` at `clock`; no package means `inherit` (a clear).
    fn row(scope: &str, hook: &str, key: &str, package_id: Option<&str>, author: &str, clock: u64, config: Value) -> StoredRow {
        StoredRow {
            record: BindingRecord {
                schema_version: 1,
                scope: scope.into(),
                hook: hook.into(),
                key: key.into(),
                mode: if package_id.is_some() { BindingMode::Extension } else { BindingMode::Inherit },
                package_id: package_id.map(str::to_string),
                version_req: None,
                config,
                runs_on: None,
                author_did: author.into(),
                written_at: clock.to_string(),
            },
            written_authorized: true,
        }
    }

    fn petal(petal_id: &str) -> PetalAddress {
        PetalAddress { verse_id: "v1".into(), fractal_id: "f1".into(), petal_id: petal_id.into() }
    }

    fn roles() -> EffectiveRoles {
        let verse_scope = resolution_scopes("v1", "f1", "p1")[2].clone();
        let manager = |peer: &str| RoleStatement {
            scope: verse_scope.clone(),
            peer_did: peer.into(),
            grantor_did: OWNER.into(),
            role: GrantedRole::Manager,
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
            windows: Vec::new(),
        };
        let statements = [manager(LOCAL), manager(OTHER)];
        materialize(OWNER, statements.iter())
    }

    /// The binary's re-resolution: snapshot of this verse, the manager's running instances, then reconcile.
    fn reconcile(activator: &mut BindingActivator, rows: &[StoredRow], facts: &LocalFacts, manager: &Mutex<PluginManager>) -> ActivationPlan {
        let roles = roles();
        let no_terrain = BTreeSet::new();
        let snapshot = BindingSnapshot { rows, roles: &roles, facts, stored_terrain_petals: &no_terrain };
        let running = lock(manager).running_instances();
        activator.reconcile("v1", &snapshot, &[petal("p1"), petal("p2"), petal("p3")], &running)
    }

    fn starts(plan: &ActivationPlan) -> Vec<&str> {
        plan.starts.iter().map(|start| start.plugin_id.as_str()).collect()
    }

    fn state_of(activator: &BindingActivator, petal_id: &str, point: &BindingPoint) -> Option<BindingResolution> {
        activator.resolution(petal_id, point).map(|resolved| resolved.resolution.clone())
    }

    fn revoker() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[5u8; 32])
    }

    fn kill_list(digests: &[&str]) -> RevocationList {
        use ed25519_dalek::Signer;
        let entries = digests
            .iter()
            .map(|digest| RevocationEntry {
                package_id: "renamed".into(),
                content_hash: digest.to_string(),
                reason: "withdrawn".into(),
                revoked_at: "2026-11-09T00:00:00Z".into(),
            })
            .collect();
        let mut list = RevocationList {
            schema_version: 1,
            issued_at: "2026-11-09T00:00:00Z".into(),
            entries,
            signer_key_id: "revoker".into(),
            signature: String::new(),
        };
        list.signature = hex::encode(revoker().sign(&list.signed_bytes()).to_bytes());
        list
    }

    /// Journey (bindings.md §5.3-5.7 on one host): acceptance gates every start; a verse egress connector starts
    /// eagerly in every petal up to the cap (the rest wait at `capacity`), shares one egress bucket and is configured
    /// once per config change; a peer-pinned
    /// connector runs elsewhere; a lazy map starts on navigation, hits the cap, idle-stops and frees the cap;
    /// Disable holds against re-resolution until Enable; a kill list stops the revoked build after its drain
    /// while the Owner's override keeps the other running in its verse only; a clear stops the connector
    /// `unbound`; uninstall forgets every binding instance.
    #[test]
    fn binding_activation_journey() {
        let clock = ManualClock::default();
        clock.set(1_000);
        let host_clock: Arc<dyn Clock> = Arc::new(clock.clone());
        let ticks = TickHost::new(host_clock.clone(), PluginContexts::default());
        let tmp = tempfile::tempdir().unwrap();
        let (commands, _queued) = crossbeam::channel::unbounded();
        let registry = Arc::new(Mutex::new(PluginRegistry::new()));
        let revoker_root = format!(
            "[[publisher]]\nkey_id = \"revoker\"\npublic_key = \"{}\"\nrevocation = true\n",
            hex::encode(revoker().verifying_key().to_bytes())
        );
        let mut manager = PluginManager::new(Arc::new(WasmEngine::new().unwrap()), tmp.path().join("plugins"), registry, commands)
            .unwrap()
            .with_admission(true)
            .with_local_did(LOCAL)
            .with_gates(Arc::new(OpenGates))
            .with_ticks(ticks.clone())
            .with_trust(TrustRoots::from_toml_str(&revoker_root).unwrap(), UnsignedPolicy::Refuse);
        let connector = package(CONNECTOR, "c1", "d1");
        let map = package(MAP, "c2", "d2");
        for (installation_id, build) in [("inst-conn", &connector), ("inst-map", &map)] {
            manager.governance_mut().record_consent(ConsentRecord {
                installation_id: installation_id.into(),
                package_id: build.package_id.clone(),
                content_hash: build.content_hash.clone(),
                capability_digest: build.capability_digest.clone(),
                actor_did: LOCAL.into(),
                at: "2026-11-09T00:00:00Z".into(),
            });
        }
        let manager = Mutex::new(manager);
        let mut launcher = Launcher {
            ticks: ticks.clone(),
            builds: HashMap::from([("inst-conn".to_string(), connector.clone()), ("inst-map".to_string(), map.clone())]),
            configured: Vec::new(),
        };
        let mut activator = BindingActivator::new(host_clock).with_limits(ActivationLimits {
            max_instances_per_installation: 2,
            ..ActivationLimits::default()
        });

        let [_, fractal_scope, verse_scope] = resolution_scopes("v1", "f1", "p1");
        let (warehouse, lake, map_point) = (
            BindingPoint::new("egress.connector", "warehouse"),
            BindingPoint::new("egress.connector", "lake"),
            BindingPoint::new("petal.map", ""),
        );
        let csv = json!({ "format": "csv" });
        let mut rows = vec![
            row(&verse_scope, "egress.connector", "warehouse", Some(CONNECTOR), LOCAL, 10, csv.clone()),
            // One-peer hook: `runs_on: null` reads as its author, another peer.
            row(&verse_scope, "egress.connector", "lake", Some(CONNECTOR), OTHER, 11, json!({})),
            row(&fractal_scope, "petal.map", "", Some(MAP), LOCAL, 12, json!({})),
        ];
        let format_schema =
            json!({ "type": "object", "properties": { "format": { "enum": ["json", "csv"] } }, "additionalProperties": false });
        let mut facts = LocalFacts {
            local_did: LOCAL.into(),
            host_enabled: true,
            is_verse_owner_host: true,
            builds: vec![
                build("inst-conn", CONNECTOR, "egress.connector", Some(format_schema)),
                build("inst-map", MAP, "petal.map", None),
            ],
            accepted: BTreeSet::new(),
            registry: vec![],
            package_policy: PackagePolicy::default(),
            default_access: fe_policy::RoleLevel::None,
            admitted_bundles: BTreeSet::new(),
        };

        // A replicated row alone never makes this host act: nothing is accepted in v1 yet.
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty(), "{plan:?}");
        assert!(matches!(
            state_of(&activator, "p1", &warehouse),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::NotAccepted, offer: Some(BindingOffer::AllowInVerse), .. })
        ));
        assert_eq!(state_of(&activator, "p1", &lake), Some(BindingResolution::Elsewhere { runs_on: OTHER.into() }));

        // "Allow in v1" for the connector: the eager point starts in every petal up to the cap of 2, eager instances
        // included (bindings.md §5.6 "at most N binding-started instances per installation per host"); p3 waits.
        lock(&manager).governance_mut().accept_in_verse("inst-conn", "v1");
        facts.accepted.insert(("inst-conn".into(), "v1".into()));
        activator.mark_dirty("v1", Trigger::Lifecycle);
        assert_eq!(activator.take_dirty(), vec![("v1".to_string(), BTreeSet::from([Trigger::Lifecycle]))]);
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert_eq!(starts(&plan), ["inst-conn@p1", "inst-conn@p2"]);
        assert!(matches!(
            state_of(&activator, "p3", &warehouse),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::Capacity, offer: None, .. })
        ));
        let report = activator.apply(plan, &manager, &mut launcher);
        assert_eq!(report.started.len(), 2, "{report:?}");
        assert_eq!(launcher.configured.len(), 2, "one configure per started service point");
        assert!(launcher.configured.iter().all(|(_, config)| *config == csv));
        assert_eq!(ticks.counters("inst-conn@p2").map(|counters| counters.state), Some(TickState::Running));
        assert!(
            reconcile(&mut activator, &rows, &facts, &manager).is_empty(),
            "re-resolving an unchanged verse plans nothing, and the held start stays held"
        );

        // The petal instances draw from one rate_hz bucket per endpoint; an unbound instance keeps its own.
        let buckets = activator.egress_buckets();
        let (conn_p1, conn_p2) = (InstanceKey::new("p1", CONNECTOR), InstanceKey::new("p2", CONNECTOR));
        assert_eq!(buckets.try_take(&conn_p1, "sink", 1.0, 1_000), Some(true));
        assert_eq!(buckets.try_take(&conn_p2, "sink", 1.0, 1_000), Some(false));
        assert_eq!(buckets.try_take(&InstanceKey::new("p1", MAP), "sink", 1.0, 1_000), None);
        // One instance per (petal, package) per host: a second id cannot stage the connector at p1.
        let token = CapabilityToken::mint("inst-conn", &CapabilityManifest::from_json(r#"{"id":"bound"}"#).unwrap());
        let (sender, _receiver) = crossbeam::channel::unbounded();
        assert!(ticks.contexts().stage_instance(&conn_p1, "inst-conn".into(), token, sender).is_err());

        // The snapshot claims the map is accepted but this host never recorded it: the host gate refuses.
        facts.accepted.insert(("inst-map".into(), "v1".into()));
        activator.mark_dirty("v1", Trigger::Lifecycle);
        activator.take_dirty();
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty(), "a lazy point waits for need: {plan:?}");
        let plan = activator.navigate(Some(petal("p1")));
        assert_eq!(starts(&plan), ["inst-map@p1"]);
        let report = activator.apply(plan, &manager, &mut launcher);
        assert_eq!(report.refused.first().map(|(_, reason, _)| *reason), Some(UnresolvedReason::NotAccepted));
        assert!(matches!(
            state_of(&activator, "p1", &map_point),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::NotAccepted, .. })
        ));

        // Accepted for real: the active petal's lazy point starts, then each petal navigated to.
        lock(&manager).governance_mut().accept_in_verse("inst-map", "v1");
        activator.mark_dirty("v1", Trigger::Lifecycle);
        activator.take_dirty();
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert_eq!(starts(&plan), ["inst-map@p1"]);
        activator.apply(plan, &manager, &mut launcher);
        let plan = activator.navigate(Some(petal("p2")));
        assert_eq!(activator.apply(plan, &manager, &mut launcher).started, ["inst-map@p2"]);
        clock.set(2_000);
        let plan = activator.navigate(Some(petal("p3")));
        let report = activator.apply(plan, &manager, &mut launcher);
        assert_eq!(report.refused.first().map(|(_, reason, _)| *reason), Some(UnresolvedReason::Capacity), "{report:?}");
        assert!(matches!(
            state_of(&activator, "p3", &map_point),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::Capacity, offer: None, .. })
        ));

        // Ten minutes after leaving p1 its lazy instance stops; p2 (left at 2 s) and the eager connectors stay.
        clock.set(1_000 + 600_000);
        let plan = activator.sweep_idle();
        assert_eq!(plan.stops, [StopPlan { plugin_id: "inst-map@p1".into(), reason: DeactivateReason::Unbound }]);
        activator.apply(plan, &manager, &mut launcher);
        assert_eq!(ticks.counters("inst-map@p1").map(|counters| counters.state), Some(TickState::Stopped));
        // The stop freed the cap: the active petal re-resolves and its lazy point starts.
        assert_eq!(activator.take_dirty(), vec![("v1".to_string(), BTreeSet::from([Trigger::Capacity]))]);
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert_eq!(starts(&plan), ["inst-map@p3"]);
        activator.apply(plan, &manager, &mut launcher);
        // A call at p2 goes to its running instance; a call at p1 tries the lazy start and meets the cap.
        assert!(matches!(
            activator.need("p2", &map_point),
            Need::Call(CallTarget { ref plugin_id, ref binding_scope, .. }) if plugin_id == "inst-map@p2" && *binding_scope == fractal_scope
        ));
        assert!(matches!(
            activator.ensure_call_target("p1", &map_point, &manager, &mut launcher),
            Err(Some(ResolvedPoint { resolution: BindingResolution::Unresolved { reason: UnresolvedReason::Capacity, .. }, .. }))
        ));

        // Kill layer a: Disable stops every instance of the installation and holds against re-resolution.
        assert!(lock(&manager).disable_instance("inst-map"));
        activator.mark_dirty("v1", Trigger::Lifecycle);
        activator.take_dirty();
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert_eq!(starts(&plan), ["inst-map@p3"], "the snapshot does not know yet; the host gate does");
        let report = activator.apply(plan, &manager, &mut launcher);
        assert_eq!(report.refused.first().map(|(_, reason, _)| *reason), Some(UnresolvedReason::Disabled));
        assert!(matches!(
            state_of(&activator, "p3", &map_point),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::Disabled, offer: Some(BindingOffer::Enable), .. })
        ));
        lock(&manager).enable_installation("inst-map");
        activator.mark_dirty("v1", Trigger::Lifecycle);
        activator.take_dirty();
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert_eq!(activator.apply(plan, &manager, &mut launcher).started, ["inst-map@p3"]);

        // A config change reaches every running connector as one configure each; nothing restarts.
        let json_format = json!({ "format": "json" });
        rows.push(row(&verse_scope, "egress.connector", "warehouse", Some(CONNECTOR), LOCAL, 20, json_format.clone()));
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty() && plan.stops.is_empty(), "{plan:?}");
        assert_eq!(plan.configures.len(), 2);
        activator.apply(plan, &manager, &mut launcher);
        assert_eq!(launcher.configured.last().map(|(_, config)| config), Some(&json_format));

        // OD-E1: a kill list naming both builds. The Owner overrode the connector in v1 on this host, so only the
        // map stops (after its in-flight commit); the override does not reach v2.
        {
            let mut manager = lock(&manager);
            manager.governance_mut().add_kill_list_override("v1", &connector.content_hash);
            let digests = [connector.component_digest.as_deref().unwrap(), map.component_digest.as_deref().unwrap()];
            assert_eq!(manager.accept_revocation_list(&kill_list(&digests)).unwrap(), ["inst-map@p3"]);
            let check = PackageCheck::Verified(connector.clone());
            let elsewhere = ActivationRequest {
                actor_did: LOCAL,
                installation_id: "inst-conn",
                verse_id: "v2",
                petal_id: "q1",
                package: &check,
            };
            assert!(matches!(
                manager.check_binding_start(&elsewhere),
                Err(LifecycleError::Refused(ref refusal)) if refusal.precondition == Precondition::Revoked
            ));
        }
        assert_eq!(ticks.counters("inst-map@p3").map(|counters| counters.state), Some(TickState::Stopped));
        assert_eq!(ticks.counters("inst-conn@p1").map(|counters| counters.state), Some(TickState::Running));
        facts.builds[0].revoked = true;
        facts.builds[0].kill_override = true;
        facts.builds[1].revoked = true;
        activator.mark_all_dirty(Trigger::KillList);
        activator.take_dirty();
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty() && plan.stops.is_empty(), "a revoked build never restarts: {plan:?}");
        assert!(matches!(
            state_of(&activator, "p3", &map_point),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::Revoked, offer: Some(BindingOffer::Override), .. })
        ));

        // A Manager clears the verse binding: every connector instance stops `unbound`; the shared group dissolves.
        rows.push(row(&verse_scope, "egress.connector", "warehouse", None, LOCAL, 30, json!({})));
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        let stops: Vec<(&str, DeactivateReason)> = plan.stops.iter().map(|stop| (stop.plugin_id.as_str(), stop.reason)).collect();
        assert_eq!(stops, [("inst-conn@p1", DeactivateReason::Unbound), ("inst-conn@p2", DeactivateReason::Unbound)]);
        activator.apply(plan, &manager, &mut launcher);
        assert_eq!(buckets.members("inst-conn"), 0);
        assert_eq!(buckets.try_take(&conn_p1, "sink", 1.0, 700_000), None);
        assert!(activator.status_rows().iter().any(|(name, count)| name == "bindings.unresolved.revoked" && *count == 3));

        // Uninstall forgets every instance ever started for the installation, stopped ones included.
        lock(&manager).uninstall_instance("inst-conn");
        activator.forget_installation("inst-conn");
        assert!(ticks.counters("inst-conn@p1").is_none() && ticks.counters("inst-conn@p3").is_none());

        // R-X-04: a bound web bundle resolves from this host's admitted pairs, offers nothing while unadmitted, and
        // never starts an instance (D-X5: only a local Open mounts it).
        const BUNDLE: &str = "org.example.pump-docs";
        let docs_point = BindingPoint::new("portal.open", "docs");
        rows.push(row(&verse_scope, "portal.open", "docs", Some(BUNDLE), LOCAL, 40, json!({})));
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty(), "{plan:?}");
        assert!(matches!(
            state_of(&activator, "p1", &docs_point),
            Some(BindingResolution::Unresolved { reason: UnresolvedReason::ConsentMissing, offer: None, .. })
        ));
        facts.admitted_bundles.insert((BUNDLE.into(), "h-docs".into()));
        let plan = reconcile(&mut activator, &rows, &facts, &manager);
        assert!(plan.starts.is_empty(), "an admitted bundle still starts nothing: {plan:?}");
        assert!(matches!(
            activator.need("p1", &docs_point),
            Need::Builtin(Some(ResolvedPoint { resolution: BindingResolution::Resolved { .. }, .. }))
        ));
    }
}
