//! `petal.map` hook resolution (bindings.md section 6, OQ-B6); see src/AGENTS.md section extension-host.

use std::collections::HashMap;

use fe_plugin::bindings::activation::{CallTarget, Need};
use fe_plugin::bindings::resolver::ResolvedPoint;
use fe_runtime::extension_binding::{BindingResolution, UnresolvedReason};
use fe_terrain::config::TerrainConfig;
use fe_terrain::extension::TERRAIN_EXTENSION_ID;
use fe_terrain::petal_binding::terrain_config_from_petal_json;
use fe_ui::terrain_map::MapSource;
use serde_json::{json, Value};

/// How long one extension answer is reused while its input is unchanged.
pub const MAP_CACHE_MS: u64 = 5_000;
/// The catalogue deadline of one `petal.map` call (bindings.md section 6); later answers are faults.
pub const MAP_DEADLINE_MS: u64 = 5_000;

/// Starts the bound extension's `petal.map` call off the frame (the binding worker runs it).
pub trait MapHookCaller {
    fn start_map(&mut self, target: &CallTarget, input: &Value) -> Box<dyn MapAnswer>;
}

/// One in-flight `petal.map` call, polled by the frame and never awaited.
pub trait MapAnswer: Send {
    /// The output JSON once answered, `Err` for a fault; `None` while the guest works.
    fn try_answer(&mut self) -> Option<Result<String, String>>;
}

/// What drives the active petal's map on this peer.
#[derive(Debug, Clone)]
pub enum MapDecision {
    /// The petal's `terrain` column serves; the DB echo path (`bridge_petal_terrain`) applies it unchanged.
    UseStored,
    /// Send this config as a `TerrainAssignmentMsg` (a `builtin` binding's config or an extension's output);
    /// never written to the petal row.
    Assign(Box<TerrainConfig>),
    /// Nothing binds a map here and the column is empty.
    NoMap,
}

/// A decision plus the source `PetalMapState` records.
#[derive(Debug, Clone)]
pub struct MapOutcome {
    pub decision: MapDecision,
    /// `None` = the column (or nothing) serves; `Some` = the binding that decided or failed to.
    pub source: Option<MapSource>,
    /// An extension call is in flight: `decision` is the built-in meanwhile and
    /// [`PetalMapResolver::poll`] yields the answer.
    pub awaiting: bool,
}

/// Everything one decision reads, gathered by the terrain bridge on each re-resolution.
pub struct MapQuery<'a> {
    pub petal_id: &'a str,
    /// The petal's stored `terrain` column, if any.
    pub stored_terrain: Option<&'a Value>,
    /// Ids of tilesets installed on this peer.
    pub installed_tilesets: &'a [String],
    pub now_ms: u64,
}

/// Per-petal answer cache keyed by the canonical input, plus the active petal's one in-flight call.
#[derive(Default)]
pub struct PetalMapResolver {
    cache: HashMap<String, CachedAnswer>,
    pending: Option<PendingCall>,
}

#[derive(Debug, Clone)]
struct CachedAnswer {
    input: String,
    package_id: String,
    at_ms: u64,
    config: Option<TerrainConfig>,
}

struct PendingCall {
    petal_id: String,
    input: String,
    package_id: String,
    scope: String,
    started_ms: u64,
    answer: Box<dyn MapAnswer>,
}

impl PetalMapResolver {
    /// Forget everything cached or in flight for a petal (binding row arrival, uninstall, role change).
    pub fn invalidate(&mut self, petal_id: &str) {
        self.cache.remove(petal_id);
        self.forget_pending(petal_id);
    }

    /// True while an extension call is in flight (the frame keeps polling it).
    pub fn is_awaiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Decide the active petal's map from the activator's `need` answer; never blocks.
    pub fn decide(&mut self, need: Need, query: &MapQuery<'_>, caller: &mut dyn MapHookCaller) -> MapOutcome {
        match need {
            Need::Call(target) => self.decide_call(&target, query, caller),
            Need::Builtin(resolved) => {
                self.forget_pending(query.petal_id);
                decide_builtin(resolved.as_ref(), query)
            }
            // The lazy instance is not up yet: the built-in serves until the caller applies the plan and asks again.
            Need::Start(_) => fallback(query, None),
        }
    }

    /// The in-flight call's outcome once answered or past [`MAP_DEADLINE_MS`]; `None` while it runs or when
    /// nothing is in flight for this input (a stale call is dropped). Never blocks.
    pub fn poll(&mut self, query: &MapQuery<'_>) -> Option<MapOutcome> {
        let call = self.pending.as_mut()?;
        if call.petal_id != query.petal_id || call.input != map_input(query).to_string() {
            self.pending = None;
            return None;
        }
        let result = match call.answer.try_answer() {
            Some(result) => result,
            None if query.now_ms.saturating_sub(call.started_ms) >= MAP_DEADLINE_MS => {
                Err(format!("petal.map missed its {MAP_DEADLINE_MS} ms deadline"))
            }
            None => return None,
        };
        let call = self.pending.take()?;
        Some(self.settle(call, result, query))
    }

    fn decide_call(&mut self, target: &CallTarget, query: &MapQuery<'_>, caller: &mut dyn MapHookCaller) -> MapOutcome {
        let package_id = target.key.package_id.as_str();
        let scope = target.binding_scope.clone();
        // OQ-B6: a non-null column is the petal's own builtin and beats every inherited binding.
        if column_wins(query, &scope) {
            self.forget_pending(query.petal_id);
            return fallback(query, None);
        }
        // The native terrain package serves from the binding's config; it has no guest to call.
        if package_id == TERRAIN_EXTENSION_ID {
            self.forget_pending(query.petal_id);
            return terrain_package_decision(target, query, scope);
        }
        let input = map_input(query);
        let canonical = input.to_string();
        if let Some(hit) = self.cache.get(query.petal_id) {
            let fresh = query.now_ms.saturating_sub(hit.at_ms) < MAP_CACHE_MS;
            if fresh && hit.input == canonical && hit.package_id == package_id {
                return outcome_from_answer(hit.config.clone(), package_id, scope, query);
            }
        }
        let in_flight = self.pending.as_ref().is_some_and(|call| {
            call.petal_id == query.petal_id && call.input == canonical && call.package_id == package_id
        });
        if !in_flight {
            let answer = caller.start_map(target, &input);
            self.pending = Some(PendingCall {
                petal_id: query.petal_id.to_string(),
                input: canonical,
                package_id: package_id.to_string(),
                scope,
                started_ms: query.now_ms,
                answer,
            });
        }
        MapOutcome { awaiting: true, ..fallback(query, None) }
    }

    /// Cache a valid answer; any fault, bad output or uninstalled tileset degrades to the built-in.
    fn settle(&mut self, call: PendingCall, result: Result<String, String>, query: &MapQuery<'_>) -> MapOutcome {
        let PendingCall { petal_id, input, package_id, scope, .. } = call;
        match result.and_then(|text| parse_output(&text, query.installed_tilesets)) {
            Ok(config) => {
                self.cache.insert(
                    petal_id,
                    CachedAnswer { input, package_id: package_id.clone(), at_ms: query.now_ms, config: config.clone() },
                );
                outcome_from_answer(config, &package_id, scope, query)
            }
            Err(reason) => {
                tracing::warn!(package = %package_id, %reason, "petal.map hook failed; built-in serves");
                self.cache.remove(&petal_id);
                fallback(query, Some(MapSource::Degraded { package_id, scope }))
            }
        }
    }

    fn forget_pending(&mut self, petal_id: &str) {
        if self.pending.as_ref().is_some_and(|call| call.petal_id == petal_id) {
            self.pending = None;
        }
    }
}

/// The §6 hook input; its string form is the cache and in-flight key.
fn map_input(query: &MapQuery<'_>) -> Value {
    json!({
        "petal_id": query.petal_id,
        "stored_terrain": query.stored_terrain,
        "installed_tilesets": query.installed_tilesets,
    })
}

/// OQ-B6: the petal's own non-null column wins unless the deciding binding sits at this very petal.
fn column_wins(query: &MapQuery<'_>, binding_scope: &str) -> bool {
    let has_column = query.stored_terrain.is_some_and(|terrain| !terrain.is_null());
    let petal_scoped = fe_database::parse_scope(binding_scope)
        .ok()
        .and_then(|parts| parts.petal_id)
        .is_some_and(|petal| petal == query.petal_id);
    has_column && !petal_scoped
}

/// `{terrain: TerrainConfig | null}` through the strictest reader, naming only installed tilesets;
/// `Ok(None)` is an explicit "no map".
fn parse_output(text: &str, installed: &[String]) -> Result<Option<TerrainConfig>, String> {
    let output: Value = serde_json::from_str(text).map_err(|err| format!("output is not JSON: {err}"))?;
    let terrain = output.get("terrain").ok_or_else(|| "output has no `terrain` key".to_string())?;
    if terrain.is_null() {
        return Ok(None);
    }
    let config =
        terrain_config_from_petal_json(terrain).ok_or_else(|| "terrain failed the strictest reader".to_string())?;
    if let Some(missing) = config.tileset_hexon_uris.iter().find(|uri| !installed.contains(uri)) {
        return Err(format!("terrain names tileset {missing} that is not installed on this peer"));
    }
    Ok(Some(config))
}

fn assign(config: TerrainConfig, source: MapSource) -> MapOutcome {
    MapOutcome { decision: MapDecision::Assign(Box::new(config)), source: Some(source), awaiting: false }
}

fn outcome_from_answer(config: Option<TerrainConfig>, package_id: &str, scope: String, query: &MapQuery<'_>) -> MapOutcome {
    match config {
        Some(config) => assign(config, MapSource::Extension { package_id: package_id.to_string(), scope }),
        None => fallback(query, None),
    }
}

/// The native terrain package serves `petal.map` from the binding's config, else from the column.
fn terrain_package_decision(target: &CallTarget, query: &MapQuery<'_>, scope: String) -> MapOutcome {
    let config = (!is_empty_config(&target.config)).then(|| terrain_config_from_petal_json(&target.config)).flatten();
    match config {
        Some(config) => assign(config, MapSource::Builtin { scope }),
        None => fallback(query, None),
    }
}

fn decide_builtin(resolved: Option<&ResolvedPoint>, query: &MapQuery<'_>) -> MapOutcome {
    let Some(resolved) = resolved else { return fallback(query, None) };
    match &resolved.resolution {
        // The column is the implicit petal-scope builtin (OQ-B6): leave the DB echo path in charge.
        BindingResolution::Builtin { .. } if resolved.detail.implicit_terrain => fallback(query, None),
        BindingResolution::Builtin { scope } => {
            let scope = scope.clone().unwrap_or_default();
            if column_wins(query, &scope) {
                return fallback(query, None);
            }
            let Some(config) = resolved.detail.builtin_config.as_ref().filter(|c| !is_empty_config(c)) else {
                return fallback(query, None);
            };
            match terrain_config_from_petal_json(config) {
                Some(config) => assign(config, MapSource::Builtin { scope }),
                None => {
                    tracing::warn!("petal.map builtin config failed the strictest reader; stored column serves");
                    fallback(query, None)
                }
            }
        }
        BindingResolution::Unresolved { package_id, scope, reason, .. } => fallback(
            query,
            Some(MapSource::Unresolved { package_id: package_id.clone(), scope: scope.clone(), reason: unresolved_label(*reason) }),
        ),
        BindingResolution::Elsewhere { .. } | BindingResolution::Resolved { .. } => fallback(query, None),
    }
}

fn unresolved_label(reason: UnresolvedReason) -> String {
    serde_json::to_value(reason).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "unresolved".into())
}

/// The built-in behaviour (the column, else no map), carrying why an extension did not serve.
fn fallback(query: &MapQuery<'_>, source: Option<MapSource>) -> MapOutcome {
    let decision = if query.stored_terrain.is_some_and(|terrain| !terrain.is_null()) { MapDecision::UseStored } else { MapDecision::NoMap };
    MapOutcome { decision, source, awaiting: false }
}

fn is_empty_config(config: &Value) -> bool {
    config.is_null() || config.as_object().is_some_and(|map| map.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_plugin::bus::InstanceKey;

    const FRACTAL: &str = "VERSE#v-FRACTAL#f";
    const PETAL: &str = "VERSE#v-FRACTAL#f-PETAL#p1";

    /// An answer ready on the first poll.
    struct Ready(Option<Result<String, String>>);
    impl MapAnswer for Ready {
        fn try_answer(&mut self) -> Option<Result<String, String>> {
            self.0.take()
        }
    }

    /// A guest that never answers.
    struct Silent;
    impl MapAnswer for Silent {
        fn try_answer(&mut self) -> Option<Result<String, String>> {
            None
        }
    }

    /// Counts started calls; `None` starts a silent one.
    struct Fixed(Option<Result<String, String>>, u32);
    impl MapHookCaller for Fixed {
        fn start_map(&mut self, _: &CallTarget, _: &Value) -> Box<dyn MapAnswer> {
            self.1 += 1;
            match self.0.clone() {
                Some(answer) => Box::new(Ready(Some(answer))),
                None => Box::new(Silent),
            }
        }
    }

    fn target(package: &str, scope: &str) -> CallTarget {
        CallTarget {
            plugin_id: "plugin-1".into(),
            installation_id: "inst-1".into(),
            key: InstanceKey::new("p1", package),
            binding_scope: scope.into(),
            config: json!({}),
        }
    }

    fn answer_naming(tileset: &str) -> String {
        let config = TerrainConfig { tileset_hexon_uris: vec![tileset.into()], ..TerrainConfig::default() };
        serde_json::to_string(&json!({ "terrain": config })).unwrap()
    }

    // One journey: the call runs off the frame and is cached, the column beats inherited bindings, and every
    // failure shape (including a silent guest and an uninstalled tileset) degrades to the built-in.
    #[test]
    fn extension_map_is_cached_and_every_failure_degrades_to_the_builtin() {
        let stored = json!({"enabled": true});
        let installed = vec!["ts-1".to_string()];
        let query = |now_ms| MapQuery { petal_id: "p1", stored_terrain: None, installed_tilesets: &installed, now_ms };
        let good = answer_naming("ts-1");
        let mut caller = Fixed(Some(Ok(good.clone())), 0);
        let mut resolver = PetalMapResolver::default();

        let first = resolver.decide(Need::Call(target("org.example.map", FRACTAL)), &query(0), &mut caller);
        assert!(first.awaiting && matches!(first.decision, MapDecision::NoMap), "the built-in serves while the call runs");
        let answered = resolver.poll(&query(10)).expect("the answer is ready");
        assert!(!answered.awaiting && matches!(answered.decision, MapDecision::Assign(_)));
        assert!(matches!(answered.source, Some(MapSource::Extension { .. })));
        let cached = resolver.decide(Need::Call(target("org.example.map", FRACTAL)), &query(1_000), &mut caller);
        assert!(matches!(cached.decision, MapDecision::Assign(_)));
        assert_eq!(caller.1, 1, "unchanged input inside the window is served from cache");
        resolver.decide(Need::Call(target("org.example.map", FRACTAL)), &query(6_000), &mut caller);
        assert_eq!(caller.1, 2);

        let with_column = MapQuery { stored_terrain: Some(&stored), ..query(0) };
        let mut counting = Fixed(Some(Ok(good)), 0);
        let inherited = PetalMapResolver::default().decide(Need::Call(target("org.example.map", FRACTAL)), &with_column, &mut counting);
        assert!(!inherited.awaiting && matches!(inherited.decision, MapDecision::UseStored));
        assert_eq!(counting.1, 0, "OQ-B6: the column beats an inherited binding without a call");
        let petal_bound = PetalMapResolver::default().decide(Need::Call(target("org.example.map", PETAL)), &with_column, &mut counting);
        assert!(petal_bound.awaiting && counting.1 == 1, "a binding at the petal itself still runs");

        let failures = [
            (Some(Err("trap".to_string())), 10),
            (Some(Ok(r#"{"terrain": {"nope": 1}}"#.to_string())), 10),
            (Some(Ok("not json".to_string())), 10),
            (Some(Ok(answer_naming("ts-9"))), 10),
            (None, MAP_DEADLINE_MS),
        ];
        for (bad, at_ms) in failures {
            let mut caller = Fixed(bad, 0);
            let mut resolver = PetalMapResolver::default();
            resolver.decide(Need::Call(target("org.example.map", FRACTAL)), &query(0), &mut caller);
            let outcome = resolver.poll(&query(at_ms)).expect("settled by an answer or the deadline");
            assert!(matches!(outcome.decision, MapDecision::NoMap));
            assert!(matches!(outcome.source, Some(MapSource::Degraded { .. })));
        }

        let mut null_answer = Fixed(Some(Ok(r#"{"terrain": null}"#.into())), 0);
        let mut resolver = PetalMapResolver::default();
        resolver.decide(Need::Call(target("org.example.map", FRACTAL)), &query(0), &mut null_answer);
        let outcome = resolver.poll(&query(10)).expect("answered");
        assert!(matches!(outcome.decision, MapDecision::NoMap) && outcome.source.is_none());
    }
}
