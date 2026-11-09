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

/// How long one extension answer is reused while its input is unchanged (hook deadline, bindings.md section 6).
pub const MAP_CACHE_MS: u64 = 5_000;

/// Calls the bound extension's `petal.map` hook; `Ok` is the hook's output JSON, `Err` a counted fault.
pub trait MapHookCaller {
    fn call_map(&mut self, target: &CallTarget, input: &Value) -> Result<String, String>;
}

/// What drives the active petal's map on this peer.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct MapOutcome {
    pub decision: MapDecision,
    /// `None` = the column (or nothing) serves; `Some` = the binding that decided or failed to.
    pub source: Option<MapSource>,
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

/// Per-petal answer cache keyed by the canonical input, so an unchanged petal never re-calls the guest.
#[derive(Debug, Default)]
pub struct PetalMapResolver {
    cache: HashMap<String, CachedAnswer>,
}

#[derive(Debug, Clone)]
struct CachedAnswer {
    input: String,
    package_id: String,
    at_ms: u64,
    config: Option<TerrainConfig>,
}

impl PetalMapResolver {
    /// Forget everything cached for a petal (binding row arrival, uninstall, role change).
    pub fn invalidate(&mut self, petal_id: &str) {
        self.cache.remove(petal_id);
    }

    /// Decide the active petal's map from the activator's `need` answer.
    pub fn decide(&mut self, need: Need, query: &MapQuery<'_>, caller: &mut dyn MapHookCaller) -> MapOutcome {
        match need {
            Need::Call(target) => self.decide_call(&target, query, caller),
            Need::Builtin(resolved) => decide_builtin(resolved.as_ref(), query),
            // The lazy instance is not up yet: the built-in serves until the caller applies the plan and asks again.
            Need::Start(_) => fallback(query, None),
        }
    }

    fn decide_call(&mut self, target: &CallTarget, query: &MapQuery<'_>, caller: &mut dyn MapHookCaller) -> MapOutcome {
        let package_id = target.key.package_id.as_str();
        let scope = target.binding_scope.clone();
        // The native terrain package serves from the binding's config; it has no guest to call.
        if package_id == TERRAIN_EXTENSION_ID {
            return terrain_package_decision(target, query, scope);
        }
        let input = json!({
            "petal_id": query.petal_id,
            "stored_terrain": query.stored_terrain,
            "installed_tilesets": query.installed_tilesets,
        });
        let canonical = input.to_string();
        if let Some(hit) = self.cache.get(query.petal_id) {
            let fresh = query.now_ms.saturating_sub(hit.at_ms) < MAP_CACHE_MS;
            if fresh && hit.input == canonical && hit.package_id == package_id {
                return outcome_from_answer(hit.config.clone(), package_id, scope, query);
            }
        }
        let answer = match caller.call_map(target, &input).and_then(|text| parse_output(&text)) {
            Ok(config) => config,
            Err(reason) => {
                tracing::warn!(package = package_id, %reason, "petal.map hook failed; built-in serves");
                self.cache.remove(query.petal_id);
                return fallback(query, Some(MapSource::Degraded { package_id: package_id.to_string(), scope }));
            }
        };
        self.cache.insert(
            query.petal_id.to_string(),
            CachedAnswer { input: canonical, package_id: package_id.to_string(), at_ms: query.now_ms, config: answer.clone() },
        );
        outcome_from_answer(answer, package_id, scope, query)
    }
}

/// `{terrain: TerrainConfig | null}` through the strictest reader; `Ok(None)` is an explicit "no map".
fn parse_output(text: &str) -> Result<Option<TerrainConfig>, String> {
    let output: Value = serde_json::from_str(text).map_err(|err| format!("output is not JSON: {err}"))?;
    let terrain = output.get("terrain").ok_or_else(|| "output has no `terrain` key".to_string())?;
    if terrain.is_null() {
        return Ok(None);
    }
    terrain_config_from_petal_json(terrain).map(Some).ok_or_else(|| "terrain failed the strictest reader".to_string())
}

fn assign(config: TerrainConfig, source: MapSource) -> MapOutcome {
    MapOutcome { decision: MapDecision::Assign(Box::new(config)), source: Some(source) }
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
        BindingResolution::Builtin { .. } if resolved.detail.implicit_terrain => {
            MapOutcome { decision: MapDecision::UseStored, source: None }
        }
        BindingResolution::Builtin { scope } => {
            let Some(config) = resolved.detail.builtin_config.as_ref().filter(|c| !is_empty_config(c)) else {
                return fallback(query, None);
            };
            match terrain_config_from_petal_json(config) {
                Some(config) => assign(config, MapSource::Builtin { scope: scope.clone().unwrap_or_default() }),
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
    MapOutcome { decision, source }
}

fn is_empty_config(config: &Value) -> bool {
    config.is_null() || config.as_object().is_some_and(|map| map.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_plugin::bus::InstanceKey;

    struct Fixed(Result<String, String>, u32);
    impl MapHookCaller for Fixed {
        fn call_map(&mut self, _: &CallTarget, _: &Value) -> Result<String, String> {
            self.1 += 1;
            self.0.clone()
        }
    }

    fn target(package: &str) -> CallTarget {
        CallTarget {
            plugin_id: "plugin-1".into(),
            installation_id: "inst-1".into(),
            key: InstanceKey::new("p1", package),
            binding_scope: "VERSE#v-FRACTAL#f".into(),
            config: json!({}),
        }
    }

    // One journey: the extension serves and is cached, then every failure shape degrades to the built-in.
    #[test]
    fn extension_map_is_cached_and_every_failure_degrades_to_the_builtin() {
        let stored = json!({"enabled": true});
        let query = |now_ms| MapQuery { petal_id: "p1", stored_terrain: None, installed_tilesets: &[], now_ms };
        let good = serde_json::to_string(&json!({"terrain": TerrainConfig::default()})).unwrap();
        let mut caller = Fixed(Ok(good), 0);
        let mut resolver = PetalMapResolver::default();

        let first = resolver.decide(Need::Call(target("org.example.map")), &query(0), &mut caller);
        assert!(matches!(first.decision, MapDecision::Assign(_)));
        resolver.decide(Need::Call(target("org.example.map")), &query(1_000), &mut caller);
        assert_eq!(caller.1, 1, "unchanged input inside the window is served from cache");
        resolver.decide(Need::Call(target("org.example.map")), &query(6_000), &mut caller);
        assert_eq!(caller.1, 2);

        for bad in [Err("trap".to_string()), Ok(r#"{"terrain": {"nope": 1}}"#.to_string()), Ok("not json".to_string())] {
            let mut caller = Fixed(bad, 0);
            let with_column = MapQuery { stored_terrain: Some(&stored), ..query(0) };
            let outcome = PetalMapResolver::default().decide(Need::Call(target("org.example.map")), &with_column, &mut caller);
            assert_eq!(outcome.decision, MapDecision::UseStored);
        }

        let mut null_answer = Fixed(Ok(r#"{"terrain": null}"#.into()), 0);
        let outcome = PetalMapResolver::default().decide(Need::Call(target("org.example.map")), &query(0), &mut null_answer);
        assert_eq!(outcome.decision, MapDecision::NoMap);
    }
}
