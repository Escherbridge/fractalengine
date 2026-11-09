//! Pure binding resolver (bindings.md §5.1-5.2); notes in `fe-plugin/src/bindings/AGENTS.md` §resolver.

use std::collections::{BTreeMap, BTreeSet};

use fe_policy::{EffectiveRoles, RoleLevel};
use fe_runtime::extension_binding::{
    hook_spec, resolution_scopes, BindingMode, BindingOffer, BindingPoint, BindingRecord, BindingResolution,
    BindingScopeLevel, HookTier, UnresolvedReason, MAX_BINDING_INSTANCES_PER_INSTALLATION,
};
use fe_sdk::manifest_v2::ProvidedHook;
use semver::{Version, VersionReq};
use serde_json::Value;

use crate::bus::schema::validate_args;

/// The petal a point is resolved for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetalAddress {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
}

/// One stored binding version plus the write-time half of admissibility (§4.4).
#[derive(Debug, Clone)]
pub struct StoredRow {
    pub record: BindingRecord,
    /// The author held Manager+ at the scope at `written_at` (`written_while_authorized`, DB plane).
    pub written_authorized: bool,
}

/// Which policy refused a build (§5.2 row 7 names it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVerdict {
    Admits,
    VerseDenies,
    LocalDenies,
}

/// Governance preconditions 3, 4 and 6 for one build, re-read now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildIntegrity {
    Ok,
    PackageInvalid,
    UntrustedSigner,
    EntitlementRequired,
}

/// Local facts about one installed build of a package.
#[derive(Debug, Clone)]
pub struct BuildFacts {
    pub installation_id: String,
    pub package_id: String,
    /// SemVer of the build; an unparsable build never satisfies a requirement.
    pub version: String,
    pub display_name: Option<String>,
    pub integrity: BuildIntegrity,
    /// A kill-list entry matches the build (any of its digests).
    pub revoked: bool,
    /// The verse Owner's audited override for `(verse, content_hash)` exists on this host.
    pub kill_override: bool,
    pub policy: PolicyVerdict,
    pub provides: Vec<ProvidedHook>,
    /// The manifest requests `scene.write`; raises the role floor to Editor.
    pub writes_scene: bool,
    /// A consent record exists for the build's capability digest.
    pub consent_recorded: bool,
    /// Installation is locally disabled (kill layer a).
    pub disabled: bool,
    /// Petals where this installation's instance is locally disabled.
    pub disabled_petals: BTreeSet<String>,
    pub degraded: bool,
    /// Binding-started instances this installation holds on this host.
    pub binding_instances: usize,
    /// Petals where an instance of this installation already runs.
    pub instance_at_petals: BTreeSet<String>,
}

/// A build a configured `RegistrySource` offers (install/update offers only).
#[derive(Debug, Clone)]
pub struct RegistryBuild {
    pub package_id: String,
    pub version: String,
    pub provides: Vec<ProvidedHook>,
}

/// Package-level policy, replicated verse side and local side (§4.2 step 3).
#[derive(Debug, Clone, Default)]
pub struct PackagePolicy {
    pub verse_allow_list: Option<BTreeSet<String>>,
    pub verse_deny: BTreeSet<String>,
    pub local_allow_list: Option<BTreeSet<String>>,
    pub local_deny: BTreeSet<String>,
}

impl PackagePolicy {
    /// True when both policies would admit the package id.
    pub fn admits(&self, package_id: &str) -> bool {
        !self.verse_deny.contains(package_id)
            && !self.local_deny.contains(package_id)
            && self.verse_allow_list.as_ref().map_or(true, |list| list.contains(package_id))
            && self.local_allow_list.as_ref().map_or(true, |list| list.contains(package_id))
    }
}

/// This host's facts for one verse; never replicated.
#[derive(Debug, Clone)]
pub struct LocalFacts {
    pub local_did: String,
    /// Host switch on and Disable all clear (row 1).
    pub host_enabled: bool,
    /// Local DID is the verse `owner_did` (gates the kill-list Override offer).
    pub is_verse_owner_host: bool,
    pub builds: Vec<BuildFacts>,
    /// Verses where this host accepted an installation (per-petal local bind counts, §5.3).
    pub accepted: BTreeSet<(String, String)>,
    pub registry: Vec<RegistryBuild>,
    pub package_policy: PackagePolicy,
}

/// Everything the resolver reads; one verse, one moment.
pub struct BindingSnapshot<'a> {
    pub rows: &'a [StoredRow],
    pub roles: &'a EffectiveRoles,
    pub facts: &'a LocalFacts,
    /// Petals whose own `terrain` column is non-null (implicit `builtin` at the petal, OQ-B6).
    pub stored_terrain_petals: &'a BTreeSet<String>,
}

/// What the surfaces show beside the resolution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolutionDetail {
    /// Level the deciding binding sits at.
    pub source_level: Option<BindingScopeLevel>,
    /// Manifest name when a build is known, else the caller falls back to the package id.
    pub package_name: Option<String>,
    /// Set for `policy-denied`: which policy refused.
    pub policy_source: Option<PolicyVerdict>,
    /// Config of an explicit `builtin` binding (e.g. a `TerrainConfig`).
    pub builtin_config: Option<Value>,
    /// The deciding binding is the petal's stored terrain column.
    pub implicit_terrain: bool,
    pub installation_id: Option<String>,
}

/// A resolution plus its presentation detail.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPoint {
    pub resolution: BindingResolution,
    pub detail: ResolutionDetail,
}

/// Newest admissible version at one `(scope, point)`; `inherit` means none (§4.4).
pub fn effective_binding<'a>(
    rows: &'a [StoredRow],
    roles: &EffectiveRoles,
    scope: &str,
    point: &BindingPoint,
) -> Option<&'a BindingRecord> {
    let newest = rows
        .iter()
        .filter(|row| row.written_authorized)
        .map(|row| &row.record)
        .filter(|record| record.scope == scope && record.hook == point.hook && record.key == point.key)
        .filter(|record| roles.authority(&record.author_did, scope).is_at_least(RoleLevel::Manager))
        .max_by(|left, right| version_order(left).cmp(&version_order(right)))?;
    (newest.mode != BindingMode::Inherit).then_some(newest)
}

/// HLC first, canonical payload bytes as the tiebreak (fe-policy `latest_per_grantor` order).
fn version_order(record: &BindingRecord) -> (u64, Vec<u8>) {
    (record.written_at.parse::<u64>().unwrap_or(0), serde_json::to_vec(record).unwrap_or_default())
}

/// Resolve one point at one petal for this peer (§5.1).
pub fn resolve(snapshot: &BindingSnapshot<'_>, petal: &PetalAddress, point: &BindingPoint) -> ResolvedPoint {
    let scopes = resolution_scopes(&petal.verse_id, &petal.fractal_id, &petal.petal_id);
    let levels = [BindingScopeLevel::Petal, BindingScopeLevel::Fractal, BindingScopeLevel::Verse];
    for (scope, level) in scopes.iter().zip(levels) {
        let binding = effective_binding(snapshot.rows, snapshot.roles, scope, point);
        let Some(record) = binding else {
            if level == BindingScopeLevel::Petal
                && point.hook == "petal.map"
                && snapshot.stored_terrain_petals.contains(&petal.petal_id)
            {
                return ResolvedPoint {
                    resolution: BindingResolution::Builtin { scope: Some(scope.clone()) },
                    detail: ResolutionDetail { source_level: Some(level), implicit_terrain: true, ..Default::default() },
                };
            }
            continue;
        };
        if record.mode == BindingMode::Builtin {
            return ResolvedPoint {
                resolution: BindingResolution::Builtin { scope: Some(scope.clone()) },
                detail: ResolutionDetail {
                    source_level: Some(level),
                    builtin_config: Some(record.config.clone()),
                    ..Default::default()
                },
            };
        }
        return check_local(snapshot, petal, point, scope, level, record);
    }
    ResolvedPoint {
        resolution: BindingResolution::Builtin { scope: None },
        detail: ResolutionDetail::default(),
    }
}

/// Placement (§5.4): explicit `runs_on`, else the author for one-peer hooks, else every peer.
fn placement(record: &BindingRecord) -> Option<String> {
    if record.runs_on.is_some() {
        return record.runs_on.clone();
    }
    hook_spec(&record.hook).filter(|spec| spec.one_peer).map(|_| record.author_did.clone())
}

fn check_local(
    snapshot: &BindingSnapshot<'_>,
    petal: &PetalAddress,
    point: &BindingPoint,
    scope: &str,
    level: BindingScopeLevel,
    record: &BindingRecord,
) -> ResolvedPoint {
    let facts = snapshot.facts;
    let mut detail = ResolutionDetail { source_level: Some(level), ..Default::default() };
    if let Some(did) = placement(record).filter(|did| *did != facts.local_did) {
        return ResolvedPoint { resolution: BindingResolution::Elsewhere { runs_on: did }, detail };
    }
    let package_id = record.package_id.clone().unwrap_or_default();
    let candidates = candidate_builds(facts, &package_id, record.version_req.as_deref());
    let outcome = first_failure(snapshot, petal, point, record, &package_id, &candidates);
    match outcome {
        Ok(build) => {
            detail.package_name = build.display_name.clone();
            detail.installation_id = Some(build.installation_id.clone());
            ResolvedPoint { resolution: BindingResolution::Resolved { package_id, scope: scope.to_string() }, detail }
        }
        Err((reason, build)) => {
            detail.package_name = build.and_then(|b| b.display_name.clone());
            detail.installation_id = build.map(|b| b.installation_id.clone());
            if reason == UnresolvedReason::PolicyDenied {
                detail.policy_source = build.map(|b| b.policy);
            }
            let offer = gated_offer(snapshot, point, record, &package_id, scope, reason);
            ResolvedPoint {
                resolution: BindingResolution::Unresolved {
                    package_id,
                    scope: scope.to_string(),
                    reason,
                    offer,
                },
                detail,
            }
        }
    }
}

/// Installed builds of the package that satisfy `version_req`, highest SemVer first.
fn candidate_builds<'a>(facts: &'a LocalFacts, package_id: &str, version_req: Option<&str>) -> Vec<&'a BuildFacts> {
    let requirement = version_req.map(VersionReq::parse);
    let mut satisfying: Vec<(Version, &BuildFacts)> = facts
        .builds
        .iter()
        .filter(|build| build.package_id == package_id)
        .filter_map(|build| Version::parse(&build.version).ok().map(|version| (version, build)))
        .filter(|(version, _)| match &requirement {
            None => true,
            Some(Ok(req)) => req.matches(version),
            Some(Err(_)) => false,
        })
        .collect();
    satisfying.sort_by(|left, right| right.0.cmp(&left.0));
    satisfying.into_iter().map(|(_, build)| build).collect()
}

type Failure<'a> = (UnresolvedReason, Option<&'a BuildFacts>);

/// Rows 1-14 in order; the highest passing build wins, else the highest build's failure.
fn first_failure<'a>(
    snapshot: &BindingSnapshot<'_>,
    petal: &PetalAddress,
    point: &BindingPoint,
    record: &BindingRecord,
    package_id: &str,
    candidates: &[&'a BuildFacts],
) -> Result<&'a BuildFacts, Failure<'a>> {
    let facts = snapshot.facts;
    if !facts.host_enabled {
        return Err((UnresolvedReason::HostOff, None));
    }
    if hook_spec(&record.hook).map_or(true, |spec| spec.tier != HookTier::V1) {
        return Err((UnresolvedReason::UnknownHook, None));
    }
    if !facts.builds.iter().any(|build| build.package_id == package_id) {
        return Err((UnresolvedReason::NotInstalled, None));
    }
    if candidates.is_empty() {
        return Err((UnresolvedReason::VersionUnsatisfied, None));
    }
    let mut first: Option<Failure<'a>> = None;
    for build in candidates {
        match check_build(snapshot, petal, point, record, build) {
            Ok(()) => return Ok(*build),
            Err(reason) => {
                first.get_or_insert((reason, Some(*build)));
            }
        }
    }
    Err(first.unwrap_or((UnresolvedReason::VersionUnsatisfied, None)))
}

/// Rows 5-14 for one build.
fn check_build(
    snapshot: &BindingSnapshot<'_>,
    petal: &PetalAddress,
    point: &BindingPoint,
    record: &BindingRecord,
    build: &BuildFacts,
) -> Result<(), UnresolvedReason> {
    let facts = snapshot.facts;
    match build.integrity {
        BuildIntegrity::Ok => {}
        BuildIntegrity::PackageInvalid => return Err(UnresolvedReason::PackageInvalid),
        BuildIntegrity::UntrustedSigner => return Err(UnresolvedReason::UntrustedSigner),
        BuildIntegrity::EntitlementRequired => return Err(UnresolvedReason::EntitlementRequired),
    }
    if build.revoked && !build.kill_override {
        return Err(UnresolvedReason::Revoked);
    }
    if build.policy != PolicyVerdict::Admits {
        return Err(UnresolvedReason::PolicyDenied);
    }
    let Some(provided) = provided_entry(&build.provides, point) else {
        return Err(UnresolvedReason::NotProvided);
    };
    if validate_args(provided.config_schema.as_ref(), &record.config).is_err() {
        return Err(UnresolvedReason::InvalidConfig);
    }
    let petal_scope = fe_policy::build_scope(&petal.verse_id, Some(&petal.fractal_id), Some(&petal.petal_id));
    let floor = if build.writes_scene { RoleLevel::Editor } else { RoleLevel::Viewer };
    if !snapshot.roles.role_at(&facts.local_did, &petal_scope).is_at_least(floor) {
        return Err(UnresolvedReason::RoleDenied);
    }
    if !build.consent_recorded {
        return Err(UnresolvedReason::ConsentMissing);
    }
    if !facts.accepted.contains(&(build.installation_id.clone(), petal.verse_id.clone())) {
        return Err(UnresolvedReason::NotAccepted);
    }
    if build.disabled || build.disabled_petals.contains(&petal.petal_id) {
        return Err(UnresolvedReason::Disabled);
    }
    if build.degraded {
        return Err(UnresolvedReason::Degraded);
    }
    let lazy = hook_spec(&point.hook).map_or(true, |spec| !spec.eager);
    if lazy
        && build.binding_instances >= MAX_BINDING_INSTANCES_PER_INSTALLATION
        && !build.instance_at_petals.contains(&petal.petal_id)
    {
        return Err(UnresolvedReason::Capacity);
    }
    Ok(())
}

/// The `provides` entry serving the point: hook equal, and `keys` absent or matching (exact, `prefix*`, `*`).
fn provided_entry<'a>(provides: &'a [ProvidedHook], point: &BindingPoint) -> Option<&'a ProvidedHook> {
    provides.iter().find(|entry| {
        entry.hook == point.hook
            && entry.keys.as_ref().map_or(true, |keys| keys.iter().any(|pattern| key_matches(pattern, &point.key)))
    })
}

fn key_matches(pattern: &str, key: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => pattern == key,
    }
}

/// The reason's single action, kept only when it would not be a dead control (§5.5).
fn gated_offer(
    snapshot: &BindingSnapshot<'_>,
    point: &BindingPoint,
    record: &BindingRecord,
    package_id: &str,
    scope: &str,
    reason: UnresolvedReason,
) -> Option<BindingOffer> {
    let facts = snapshot.facts;
    let offer = reason.offer()?;
    let requirement = record.version_req.as_deref().map(VersionReq::parse);
    let registry_has = |needs_hook: bool| {
        facts.registry.iter().filter(|build| build.package_id == package_id).any(|build| {
            let satisfies = Version::parse(&build.version).is_ok_and(|version| match &requirement {
                None => true,
                Some(Ok(req)) => req.matches(&version),
                Some(Err(_)) => false,
            });
            satisfies && (!needs_hook || provided_entry(&build.provides, point).is_some())
        })
    };
    let allowed = match offer {
        BindingOffer::Install => facts.package_policy.admits(package_id) && registry_has(false),
        BindingOffer::Update => registry_has(reason == UnresolvedReason::NotProvided),
        BindingOffer::Override => facts.is_verse_owner_host,
        BindingOffer::EditBinding => snapshot.roles.role_at(&facts.local_did, scope).is_at_least(RoleLevel::Manager),
        BindingOffer::ReviewPermissions | BindingOffer::AllowInVerse | BindingOffer::Enable => true,
    };
    allowed.then_some(offer)
}

/// Resolve every point of a set for one petal; keyed by point.
pub fn resolve_all(
    snapshot: &BindingSnapshot<'_>,
    petal: &PetalAddress,
    points: &[BindingPoint],
) -> BTreeMap<BindingPoint, ResolvedPoint> {
    points.iter().map(|point| (point.clone(), resolve(snapshot, petal, point))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_policy::{materialize, GrantedRole, RoleStatement};

    const OWNER: &str = "did:key:owner";
    const MANAGER: &str = "did:key:mgr";
    const LOCAL: &str = "did:key:local";
    const PKG: &str = "org.example.city-map";

    fn petal() -> PetalAddress {
        PetalAddress { verse_id: "v1".into(), fractal_id: "f1".into(), petal_id: "p1".into() }
    }

    fn scope_of(level: BindingScopeLevel) -> String {
        let [petal_scope, fractal_scope, verse_scope] = resolution_scopes("v1", "f1", "p1");
        match level {
            BindingScopeLevel::Petal => petal_scope,
            BindingScopeLevel::Fractal => fractal_scope,
            BindingScopeLevel::Verse => verse_scope,
        }
    }

    fn row(level: BindingScopeLevel, hook: &str, key: &str, mode: BindingMode, clock: u64) -> StoredRow {
        StoredRow {
            record: BindingRecord {
                schema_version: 1,
                scope: scope_of(level),
                hook: hook.into(),
                key: key.into(),
                mode,
                package_id: (mode == BindingMode::Extension).then(|| PKG.to_string()),
                version_req: Some(">=1.2.0, <2.0.0".into()),
                config: serde_json::json!({}),
                runs_on: None,
                author_did: MANAGER.into(),
                written_at: clock.to_string(),
            },
            written_authorized: true,
        }
    }

    fn build() -> BuildFacts {
        BuildFacts {
            installation_id: "inst-1".into(),
            package_id: PKG.into(),
            version: "1.3.0".into(),
            display_name: Some("City map".into()),
            integrity: BuildIntegrity::Ok,
            revoked: false,
            kill_override: false,
            policy: PolicyVerdict::Admits,
            provides: vec![ProvidedHook { hook: "petal.map".into(), keys: None, uses: None, config_schema: None }],
            writes_scene: false,
            consent_recorded: true,
            disabled: false,
            disabled_petals: BTreeSet::new(),
            degraded: false,
            binding_instances: 0,
            instance_at_petals: BTreeSet::new(),
        }
    }

    fn facts() -> LocalFacts {
        LocalFacts {
            local_did: LOCAL.into(),
            host_enabled: true,
            is_verse_owner_host: false,
            builds: vec![build()],
            accepted: BTreeSet::from([("inst-1".to_string(), "v1".to_string())]),
            registry: vec![],
            package_policy: PackagePolicy::default(),
        }
    }

    fn roles() -> EffectiveRoles {
        let grant = |peer: &str, role: GrantedRole| RoleStatement {
            scope: scope_of(BindingScopeLevel::Verse),
            peer_did: peer.into(),
            grantor_did: OWNER.into(),
            role,
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let statements = [grant(MANAGER, GrantedRole::Manager), grant(LOCAL, GrantedRole::Editor)];
        materialize(OWNER, statements.iter())
    }

    fn run(rows: &[StoredRow], facts: &LocalFacts, terrain: &[&str], hook: &str, key: &str) -> ResolvedPoint {
        let roles = roles();
        let stored: BTreeSet<String> = terrain.iter().map(|id| id.to_string()).collect();
        let snapshot = BindingSnapshot { rows, roles: &roles, facts, stored_terrain_petals: &stored };
        resolve(&snapshot, &petal(), &BindingPoint::new(hook, key))
    }

    fn reason_of(resolved: &ResolvedPoint) -> Option<UnresolvedReason> {
        match &resolved.resolution {
            BindingResolution::Unresolved { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    // Every §5.2 row fails in order from a world where all rows pass; the offer is the gated action.
    #[test]
    fn local_check_rows_resolve_to_their_state_and_offer() {
        let rows = [row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Extension, 10)];
        let mut registry_facts = facts();
        registry_facts.registry =
            vec![RegistryBuild { package_id: PKG.into(), version: "1.4.0".into(), provides: build().provides }];
        let owner_host = LocalFacts { is_verse_owner_host: true, ..facts() };
        type Mutation = Box<dyn Fn(&mut LocalFacts)>;
        let table: Vec<(&str, Mutation, Option<UnresolvedReason>, Option<BindingOffer>)> = vec![
            ("all pass", Box::new(|_| {}), None, None),
            ("host-off", Box::new(|f| f.host_enabled = false), Some(UnresolvedReason::HostOff), None),
            ("not-installed", Box::new(|f| f.builds.clear()), Some(UnresolvedReason::NotInstalled), None),
            ("version-unsatisfied", Box::new(|f| f.builds[0].version = "2.0.0".into()), Some(UnresolvedReason::VersionUnsatisfied), None),
            ("package-invalid", Box::new(|f| f.builds[0].integrity = BuildIntegrity::PackageInvalid), Some(UnresolvedReason::PackageInvalid), None),
            ("untrusted-signer", Box::new(|f| f.builds[0].integrity = BuildIntegrity::UntrustedSigner), Some(UnresolvedReason::UntrustedSigner), None),
            ("entitlement", Box::new(|f| f.builds[0].integrity = BuildIntegrity::EntitlementRequired), Some(UnresolvedReason::EntitlementRequired), None),
            ("revoked", Box::new(|f| f.builds[0].revoked = true), Some(UnresolvedReason::Revoked), None),
            ("revoked but overridden", Box::new(|f| { f.builds[0].revoked = true; f.builds[0].kill_override = true; }), None, None),
            ("policy-denied", Box::new(|f| f.builds[0].policy = PolicyVerdict::VerseDenies), Some(UnresolvedReason::PolicyDenied), None),
            ("not-provided", Box::new(|f| f.builds[0].provides.clear()), Some(UnresolvedReason::NotProvided), None),
            ("consent-missing", Box::new(|f| f.builds[0].consent_recorded = false), Some(UnresolvedReason::ConsentMissing), Some(BindingOffer::ReviewPermissions)),
            ("not-accepted", Box::new(|f| f.accepted.clear()), Some(UnresolvedReason::NotAccepted), Some(BindingOffer::AllowInVerse)),
            ("disabled", Box::new(|f| f.builds[0].disabled = true), Some(UnresolvedReason::Disabled), Some(BindingOffer::Enable)),
            ("disabled at this petal", Box::new(|f| { f.builds[0].disabled_petals.insert("p1".into()); }), Some(UnresolvedReason::Disabled), Some(BindingOffer::Enable)),
            ("degraded", Box::new(|f| f.builds[0].degraded = true), Some(UnresolvedReason::Degraded), None),
            ("capacity", Box::new(|f| f.builds[0].binding_instances = MAX_BINDING_INSTANCES_PER_INSTALLATION), Some(UnresolvedReason::Capacity), None),
            ("writing extension needs editor", Box::new(|f| { f.builds[0].writes_scene = true; f.local_did = "did:key:viewer".into(); }), Some(UnresolvedReason::RoleDenied), None),
        ];
        for (name, mutate, expected_reason, expected_offer) in table {
            let mut world = facts();
            mutate(&mut world);
            let resolved = run(&rows, &world, &[], "petal.map", "");
            assert_eq!(reason_of(&resolved), expected_reason, "row {name}");
            if let BindingResolution::Unresolved { offer, .. } = &resolved.resolution {
                assert_eq!(*offer, expected_offer, "offer for row {name}");
            } else {
                assert!(resolved.resolution.is_extension(), "row {name} must resolve");
            }
        }
        // Offers that depend on outside facts: registry build, owner host, Manager+ at the scope.
        let mut missing = registry_facts.clone();
        missing.builds.clear();
        let installable = run(&rows, &missing, &[], "petal.map", "");
        assert!(matches!(installable.resolution, BindingResolution::Unresolved { offer: Some(BindingOffer::Install), .. }));
        missing.package_policy.verse_deny.insert(PKG.into());
        let blocked = run(&rows, &missing, &[], "petal.map", "");
        assert!(matches!(blocked.resolution, BindingResolution::Unresolved { offer: None, .. }), "no dead Install control");
        let mut stale = registry_facts.clone();
        stale.builds[0].version = "1.0.0".into();
        let updatable = run(&rows, &stale, &[], "petal.map", "");
        assert!(matches!(updatable.resolution, BindingResolution::Unresolved { offer: Some(BindingOffer::Update), .. }));
        let mut revoked = owner_host.clone();
        revoked.builds[0].revoked = true;
        let overridable = run(&rows, &revoked, &[], "petal.map", "");
        assert!(matches!(overridable.resolution, BindingResolution::Unresolved { offer: Some(BindingOffer::Override), .. }));
        let mut denied = facts();
        denied.builds[0].policy = PolicyVerdict::LocalDenies;
        assert_eq!(run(&rows, &denied, &[], "petal.map", "").detail.policy_source, Some(PolicyVerdict::LocalDenies));
        // unknown-hook: a stretch hook is stored but unknown to this build.
        let stretch = [row(BindingScopeLevel::Verse, "asset.export", "", BindingMode::Extension, 10)];
        assert_eq!(reason_of(&run(&stretch, &facts(), &[], "asset.export", "")), Some(UnresolvedReason::UnknownHook));
    }

    // Nearest scope wins with no fall-through; builtin masks outer; inherit looks outward.
    #[test]
    fn nearest_scope_wins_without_fall_through() {
        let verse = row(BindingScopeLevel::Verse, "petal.map", "", BindingMode::Extension, 5);
        let fractal_builtin = row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Builtin, 6);
        let petal_inherit = row(BindingScopeLevel::Petal, "petal.map", "", BindingMode::Inherit, 7);
        let resolved = run(&[verse.clone(), fractal_builtin.clone(), petal_inherit], &facts(), &[], "petal.map", "");
        assert_eq!(resolved.resolution, BindingResolution::Builtin { scope: Some(scope_of(BindingScopeLevel::Fractal)) });
        // An unresolved petal extension binding falls back to the built-in, not to the verse binding.
        let petal_ext = row(BindingScopeLevel::Petal, "petal.map", "", BindingMode::Extension, 8);
        let mut broken = facts();
        broken.builds[0].consent_recorded = false;
        let resolved = run(&[verse.clone(), petal_ext], &broken, &[], "petal.map", "");
        assert_eq!(reason_of(&resolved), Some(UnresolvedReason::ConsentMissing));
        assert_eq!(resolved.detail.source_level, Some(BindingScopeLevel::Petal));
        // Nothing bound anywhere: the built-in, no scope.
        assert_eq!(run(&[], &facts(), &[], "petal.map", "").resolution, BindingResolution::Builtin { scope: None });
        // Keyed points are independent.
        let keyed = row(BindingScopeLevel::Verse, "ingest.decode", "acme/raw", BindingMode::Builtin, 9);
        assert_eq!(
            run(&[keyed.clone()], &facts(), &[], "ingest.decode", "other").resolution,
            BindingResolution::Builtin { scope: None }
        );
        assert!(matches!(
            run(&[keyed], &facts(), &[], "ingest.decode", "acme/raw").resolution,
            BindingResolution::Builtin { scope: Some(_) }
        ));
    }

    // The stored terrain column is an implicit petal builtin: it beats inherited bindings, not an explicit petal row.
    #[test]
    fn stored_terrain_is_an_implicit_petal_builtin() {
        let verse = row(BindingScopeLevel::Verse, "petal.map", "", BindingMode::Extension, 5);
        let implicit = run(&[verse.clone()], &facts(), &["p1"], "petal.map", "");
        assert_eq!(implicit.resolution, BindingResolution::Builtin { scope: Some(scope_of(BindingScopeLevel::Petal)) });
        assert!(implicit.detail.implicit_terrain);
        let other_petal = run(&[verse.clone()], &facts(), &["p9"], "petal.map", "");
        assert!(other_petal.resolution.is_extension());
        let explicit = row(BindingScopeLevel::Petal, "petal.map", "", BindingMode::Extension, 6);
        assert!(run(&[verse, explicit], &facts(), &["p1"], "petal.map", "").resolution.is_extension());
    }

    // Placement: explicit runs_on and one-peer hooks name another peer; the local author runs it.
    #[test]
    fn placement_reads_runs_on_and_one_peer_defaults() {
        let mut pinned = row(BindingScopeLevel::Verse, "petal.map", "", BindingMode::Extension, 5);
        pinned.record.runs_on = Some("did:key:other".into());
        assert_eq!(
            run(&[pinned], &facts(), &[], "petal.map", "").resolution,
            BindingResolution::Elsewhere { runs_on: "did:key:other".into() }
        );
        let mut connector_facts = facts();
        connector_facts.builds[0].provides =
            vec![ProvidedHook { hook: "egress.connector".into(), keys: None, uses: None, config_schema: None }];
        let connector = row(BindingScopeLevel::Verse, "egress.connector", "bi", BindingMode::Extension, 5);
        assert_eq!(
            run(&[connector.clone()], &connector_facts, &[], "egress.connector", "bi").resolution,
            BindingResolution::Elsewhere { runs_on: MANAGER.into() },
            "null reads as the author for a one-peer hook"
        );
        connector_facts.local_did = MANAGER.into();
        assert!(
            run(&[connector], &connector_facts, &[], "egress.connector", "bi").resolution.is_extension(),
            "the author's own peer runs the local checks and resolves"
        );
    }

    // Admissibility: a demoted author's version drops out, the older version by another author applies.
    #[test]
    fn demoted_author_falls_back_and_highest_semver_wins() {
        let mut by_demoted = row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Builtin, 20);
        by_demoted.record.author_did = "did:key:demoted".into();
        let older = row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Extension, 10);
        let resolved = run(&[by_demoted, older], &facts(), &[], "petal.map", "");
        assert!(resolved.resolution.is_extension(), "newest admissible version applies");
        // Written before the author held Manager: write-time check fails.
        let mut unauthorized = row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Builtin, 30);
        unauthorized.written_authorized = false;
        let rows = [unauthorized, row(BindingScopeLevel::Fractal, "petal.map", "", BindingMode::Extension, 10)];
        assert!(run(&rows, &facts(), &[], "petal.map", "").resolution.is_extension());
        // Several builds: the highest satisfying SemVer serves.
        let mut world = facts();
        let mut newer = build();
        newer.installation_id = "inst-2".into();
        newer.version = "1.9.0".into();
        let mut too_new = build();
        too_new.installation_id = "inst-3".into();
        too_new.version = "2.1.0".into();
        world.builds.extend([newer, too_new]);
        world.accepted.insert(("inst-2".into(), "v1".into()));
        let resolved = run(&[row(BindingScopeLevel::Verse, "petal.map", "", BindingMode::Extension, 5)], &world, &[], "petal.map", "");
        assert_eq!(resolved.detail.installation_id.as_deref(), Some("inst-2"));
    }

    // Key patterns and config schema: keyed provides, wildcard keys, strict {} without a schema.
    #[test]
    fn provides_keys_and_config_schema_gate_the_binding() {
        let mut world = facts();
        world.builds[0].provides =
            vec![ProvidedHook { hook: "asset.import".into(), keys: Some(vec!["image/*".into()]), uses: None, config_schema: None }];
        let matching = row(BindingScopeLevel::Verse, "asset.import", "image/png", BindingMode::Extension, 5);
        assert!(run(&[matching], &world, &[], "asset.import", "image/png").resolution.is_extension());
        let other = row(BindingScopeLevel::Verse, "asset.import", "audio/wav", BindingMode::Extension, 5);
        assert_eq!(reason_of(&run(&[other], &world, &[], "asset.import", "audio/wav")), Some(UnresolvedReason::NotProvided));
        world.local_did = MANAGER.into();
        let mut configured = row(BindingScopeLevel::Verse, "asset.import", "image/png", BindingMode::Extension, 5);
        configured.record.config = serde_json::json!({ "quality": 3 });
        let resolved = run(&[configured], &world, &[], "asset.import", "image/png");
        assert_eq!(reason_of(&resolved), Some(UnresolvedReason::InvalidConfig));
        assert!(matches!(resolved.resolution, BindingResolution::Unresolved { offer: Some(BindingOffer::EditBinding), .. }));
    }
}
