//! Pure relay-map planner over three source tiers (Configured / Default / Advertised); see `fe-sync/src/AGENTS.md` §relay-advertise, §relay-trust.

use std::collections::{BTreeMap, BTreeSet};

use fe_runtime::relay_advertisement::{validate_advertised_relay_url_with, AdvertisedRelayPolicy};

use crate::relay_config::{configured_urls, RelayAdvertisePolicy, RelayConfig, RelayHealth};
use crate::relay_trust::AdoptionMode;

/// One step the sync thread applies after a plan (inserts come first, so a demoted tier is restored before anything leaves).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelayOp {
    /// Add this relay to the endpoint's relay map: an advertised one (never with a token) or a restored Default one.
    Insert(String),
    /// Drop a relay this reconciler put in the map, or demote a Default relay while a covered relay is home.
    Remove(String),
    /// Listed and dial-allowlisted only (the full known set is re-listed on every plan).
    Known(String),
}

/// The tier a relay URL belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySource {
    /// `FE_SYNC_RELAY` or Settings: never inserted, removed or demoted by the planner.
    Configured,
    /// n0's public fleet in Default mode: absent only while a covered relay is connected as home.
    Default,
    /// Admitted adverts of open verses, refcounted by advertising verse.
    Advertised,
}

/// Everything one tiered plan reads; the planner performs no I/O.
#[derive(Debug, Clone)]
pub struct RelayReconcileInputs<'a> {
    /// Local advertisement policy (never replicated).
    pub policy: &'a RelayAdvertisePolicy,
    /// Admitted adverts of open verses: verse id to canonical URLs.
    pub advertised: &'a BTreeMap<String, BTreeSet<String>>,
    /// The home relay iroh reports as Connected; `None` when disconnected or the 30 s status tick says not connected.
    pub home_connected: Option<String>,
    /// `relay_trust::evaluate_coverage` over the participating verses' healthy, admitted adverts.
    pub coverage: BTreeSet<String>,
    /// `AppSettings.auto_home_relay` (OD-13 toggle, default on).
    pub auto_home: bool,
    /// Current home-relay health; Degraded or Unreachable re-inserts the Default tier.
    pub home_health: RelayHealth,
}

/// Desired-state reconciler: refcounted advertised relays, a demotable Default tier, an untouchable Configured tier.
#[derive(Debug, Clone, Default)]
pub struct RelayReconciler {
    configured: BTreeSet<String>,
    defaults: BTreeSet<String>,
    defaults_present: BTreeSet<String>,
    relays_enabled: bool,
    inserted: BTreeMap<String, BTreeSet<String>>,
    known: BTreeSet<String>,
    adoption: BTreeMap<String, AdoptionMode>,
}

impl RelayReconciler {
    /// A reconciler over the operator's configured relays (canonical URL strings) and no Default tier.
    pub fn new(configured: BTreeSet<String>) -> Self {
        Self::with_tiers(configured, BTreeSet::new())
    }

    /// A reconciler over explicit Configured and Default tiers; a URL in both is Configured.
    pub fn with_tiers(configured: BTreeSet<String>, defaults: BTreeSet<String>) -> Self {
        let defaults: BTreeSet<String> = defaults.difference(&configured).cloned().collect();
        Self {
            defaults_present: defaults.clone(),
            configured,
            defaults,
            relays_enabled: true,
            ..Self::default()
        }
    }

    /// The tiers implied by the relay config: Default mode yields the Default tier, Custom the Configured tier, Disabled neither.
    pub fn from_relay_config(config: &RelayConfig) -> Self {
        match config {
            RelayConfig::Default => Self::with_tiers(BTreeSet::new(), configured_urls(config)),
            RelayConfig::Custom(_) => Self::new(configured_urls(config)),
            RelayConfig::Disabled => Self {
                relays_enabled: false,
                ..Self::default()
            },
        }
    }

    /// Plan without the home-relay inputs: no automatic adoption and no Default demotion.
    pub fn plan(
        &mut self,
        policy: &RelayAdvertisePolicy,
        advertised: &BTreeMap<String, BTreeSet<String>>,
    ) -> Vec<RelayOp> {
        self.plan_tiers(&RelayReconcileInputs {
            policy,
            advertised,
            home_connected: None,
            coverage: BTreeSet::new(),
            auto_home: false,
            home_health: RelayHealth::Unknown,
        })
    }

    /// Plan the relay-map changes for every tier from one snapshot of the inputs.
    pub fn plan_tiers(&mut self, inputs: &RelayReconcileInputs<'_>) -> Vec<RelayOp> {
        let policy = inputs.policy;
        let auto_home = inputs.auto_home
            && self.relays_enabled
            && !matches!(policy, RelayAdvertisePolicy::Deny);

        let mut advertisers: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for (verse_id, urls) in inputs.advertised {
            for url in urls {
                advertisers
                    .entry(url.as_str())
                    .or_default()
                    .insert(verse_id.clone());
            }
        }

        let mut next_inserted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut next_known: BTreeSet<String> = BTreeSet::new();
        let mut next_adoption: BTreeMap<String, AdoptionMode> = BTreeMap::new();
        for (url, verses) in advertisers {
            // Configured and Default relays are already in the map with the operator's (or n0's)
            // entry: inserting would replace it, so an advert naming one is never planned at all.
            if self.configured.contains(url) || self.defaults.contains(url) {
                continue;
            }
            let mode = match policy {
                RelayAdvertisePolicy::Deny => continue,
                RelayAdvertisePolicy::Pinned(pinned)
                    if pinned.iter().any(|pinned_url| pinned_url == url) =>
                {
                    AdoptionMode::Pinned
                }
                _ if auto_home && inputs.coverage.contains(url) && auto_home_candidate(url) => {
                    AdoptionMode::HomeEligible
                }
                _ => AdoptionMode::DialOnly,
            };
            next_adoption.insert(url.to_string(), mode);
            if mode == AdoptionMode::DialOnly {
                next_known.insert(url.to_string());
            } else {
                next_inserted.insert(url.to_string(), verses);
            }
        }

        // OD-13: n0's fleet leaves the map only while a covered relay that is ALREADY in the map is the
        // connected, healthy home; any failure signal makes this false and restores the tier in this pass.
        let demote_defaults = auto_home
            && !self.defaults.is_empty()
            && inputs.home_health == RelayHealth::Healthy
            && inputs.home_connected.as_deref().is_some_and(|home| {
                inputs.coverage.contains(home)
                    && self.inserted.contains_key(home)
                    && matches!(
                        next_adoption.get(home),
                        Some(AdoptionMode::HomeEligible | AdoptionMode::Pinned)
                    )
                    && auto_home_candidate(home)
            });
        let next_defaults: BTreeSet<String> = if demote_defaults {
            BTreeSet::new()
        } else {
            self.defaults.clone()
        };

        let mut ops = Vec::new();
        for url in next_defaults.difference(&self.defaults_present) {
            ops.push(RelayOp::Insert(url.clone()));
        }
        for url in next_inserted.keys() {
            if !self.inserted.contains_key(url) {
                ops.push(RelayOp::Insert(url.clone()));
            }
        }
        for url in self.inserted.keys() {
            if !next_inserted.contains_key(url)
                && !self.configured.contains(url)
                && !self.defaults.contains(url)
            {
                ops.push(RelayOp::Remove(url.clone()));
            }
        }
        for url in self.defaults_present.difference(&next_defaults) {
            ops.push(RelayOp::Remove(url.clone()));
        }
        ops.extend(next_known.iter().cloned().map(RelayOp::Known));

        self.inserted = next_inserted;
        self.known = next_known;
        self.adoption = next_adoption;
        self.defaults_present = next_defaults;
        ops
    }

    /// Forget an insert the endpoint did not take (advertised or restored Default), so a later plan retries it.
    pub fn forget_insert(&mut self, url: &str) {
        self.inserted.remove(url);
        self.defaults_present.remove(url);
    }

    /// `true` when `url` is an advertised relay this reconciler put in the relay map.
    pub fn is_inserted(&self, url: &str) -> bool {
        self.inserted.contains_key(url)
    }

    /// Advertised relays currently in the relay map, with the verses advertising each.
    pub fn inserted(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.inserted
    }

    /// Advertised relays that are listed and dial-allowlisted but not in the relay map.
    pub fn known(&self) -> &BTreeSet<String> {
        &self.known
    }

    /// The operator-configured relays this reconciler never inserts or removes.
    pub fn configured(&self) -> &BTreeSet<String> {
        &self.configured
    }

    /// n0's Default relays (empty outside Default mode).
    pub fn defaults(&self) -> &BTreeSet<String> {
        &self.defaults
    }

    /// `true` while the Default tier is demoted out of the relay map.
    pub fn defaults_demoted(&self) -> bool {
        self.defaults_present.len() < self.defaults.len()
    }

    /// Adoption mode of every advertised URL from the last plan.
    pub fn adoption(&self) -> &BTreeMap<String, AdoptionMode> {
        &self.adoption
    }

    /// The tier `url` belongs to, if any.
    pub fn source_of(&self, url: &str) -> Option<RelaySource> {
        if self.configured.contains(url) {
            Some(RelaySource::Configured)
        } else if self.defaults.contains(url) {
            Some(RelaySource::Default)
        } else if self.inserted.contains_key(url) || self.known.contains(url) {
            Some(RelaySource::Advertised)
        } else {
            None
        }
    }
}

/// Only a public https relay is ever adopted as home automatically, whatever the private-relay opt-in says.
fn auto_home_candidate(url: &str) -> bool {
    validate_advertised_relay_url_with(url, AdvertisedRelayPolicy::STRICT)
        .is_ok_and(|relay| relay.home_eligible())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIGURED: &str = "https://configured.example.org/";
    const PINNED: &str = "https://pinned.example.org/";
    const OTHER: &str = "https://other.example.org/";
    const COVERED: &str = "https://covered.example.org/";
    const PRIVATE_COVERED: &str = "https://10.0.0.5/";
    const DEFAULT_A: &str = "https://default-a.example.org/";
    const DEFAULT_B: &str = "https://default-b.example.org/";

    /// Xorshift64 generator for hand-rolled property sequences (no dev-dependency).
    struct Xorshift(u64);

    impl Xorshift {
        fn seeded(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        fn next(&mut self) -> u64 {
            let mut state = self.0;
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            self.0 = state;
            state
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn chance(&mut self) -> bool {
            self.next() & 1 == 1
        }

        fn subset(&mut self, items: &[&str]) -> BTreeSet<String> {
            let mut chosen = BTreeSet::new();
            for item in items {
                if self.chance() {
                    chosen.insert(item.to_string());
                }
            }
            chosen
        }

        /// A home relay drawn from what is in the map, or none (disconnected).
        fn home(&mut self, relay_map: &BTreeSet<String>) -> Option<String> {
            if relay_map.is_empty() || self.below(4) == 0 {
                return None;
            }
            relay_map.iter().nth(self.below(relay_map.len())).cloned()
        }
    }

    const HEALTHS: [RelayHealth; 4] = [
        RelayHealth::Unknown,
        RelayHealth::Healthy,
        RelayHealth::Degraded,
        RelayHealth::Unreachable,
    ];

    fn adverts(rows: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        rows.iter()
            .map(|(verse, urls)| {
                (
                    verse.to_string(),
                    urls.iter().map(|url| url.to_string()).collect(),
                )
            })
            .collect()
    }

    fn tier_inputs<'a>(
        policy: &'a RelayAdvertisePolicy,
        advertised: &'a BTreeMap<String, BTreeSet<String>>,
        home: Option<&str>,
        coverage: &[&str],
        auto_home: bool,
        home_health: RelayHealth,
    ) -> RelayReconcileInputs<'a> {
        RelayReconcileInputs {
            policy,
            advertised,
            home_connected: home.map(str::to_string),
            coverage: set(coverage),
            auto_home,
            home_health,
        }
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn inserts(ops: &[RelayOp]) -> Vec<&str> {
        ops.iter()
            .filter_map(|op| match op {
                RelayOp::Insert(url) => Some(url.as_str()),
                _ => None,
            })
            .collect()
    }

    fn removes(ops: &[RelayOp]) -> Vec<&str> {
        ops.iter()
            .filter_map(|op| match op {
                RelayOp::Remove(url) => Some(url.as_str()),
                _ => None,
            })
            .collect()
    }

    fn knowns(ops: &[RelayOp]) -> Vec<&str> {
        ops.iter()
            .filter_map(|op| match op {
                RelayOp::Known(url) => Some(url.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Apply a plan to a simulated relay map; an insert over an existing entry (a token-bearing one, say) fails.
    fn apply_ops(relay_map: &mut BTreeSet<String>, ops: &[RelayOp], context: &str) {
        let first_remove = ops.iter().position(|op| matches!(op, RelayOp::Remove(_)));
        let last_insert = ops.iter().rposition(|op| matches!(op, RelayOp::Insert(_)));
        if let (Some(first_remove), Some(last_insert)) = (first_remove, last_insert) {
            assert!(last_insert < first_remove, "{context}: a removal ran before an insert: {ops:?}");
        }
        for op in ops {
            match op {
                RelayOp::Insert(url) => assert!(
                    relay_map.insert(url.clone()),
                    "{context}: inserted {url} over an existing relay-map entry"
                ),
                RelayOp::Remove(url) => {
                    relay_map.remove(url);
                }
                RelayOp::Known(_) => {}
            }
        }
    }

    #[test]
    fn planner_never_removes_a_configured_relay_url() {
        let configured: BTreeSet<String> = [CONFIGURED.to_string()].into();
        let pinned = RelayAdvertisePolicy::Pinned(vec![PINNED.to_string(), CONFIGURED.to_string()]);
        let dial_only = RelayAdvertisePolicy::DialOnly;
        let deny = RelayAdvertisePolicy::Deny;

        // (policy, adverts after this step, expected inserts, expected removes, expected known)
        type Step<'a> = (
            &'a RelayAdvertisePolicy,
            BTreeMap<String, BTreeSet<String>>,
            Vec<&'a str>,
            Vec<&'a str>,
            Vec<&'a str>,
        );
        let flows: Vec<(&str, Vec<Step>)> = vec![
            (
                "dial-only lists without inserting",
                vec![(
                    &dial_only,
                    adverts(&[("verse-a", &[PINNED, OTHER])]),
                    vec![],
                    vec![],
                    vec![OTHER, PINNED],
                )],
            ),
            (
                "pinned inserts only pinned and advertised relays",
                vec![(
                    &pinned,
                    adverts(&[("verse-a", &[OTHER]), ("verse-b", &[PINNED])]),
                    vec![PINNED],
                    vec![],
                    vec![OTHER],
                )],
            ),
            (
                "deny inserts and lists nothing",
                vec![(
                    &deny,
                    adverts(&[("verse-a", &[PINNED, OTHER])]),
                    vec![],
                    vec![],
                    vec![],
                )],
            ),
            (
                "same url in two verses survives one withdrawal",
                vec![
                    (&pinned, adverts(&[("verse-a", &[PINNED]), ("verse-b", &[PINNED])]), vec![PINNED], vec![], vec![]),
                    (&pinned, adverts(&[("verse-b", &[PINNED])]), vec![], vec![], vec![]),
                    (&pinned, adverts(&[]), vec![], vec![PINNED], vec![]),
                    (&pinned, adverts(&[("verse-a", &[PINNED])]), vec![PINNED], vec![], vec![]),
                ],
            ),
            (
                "configured url is never inserted (its token entry would be replaced) nor removed",
                vec![
                    (&pinned, adverts(&[("verse-a", &[CONFIGURED]), ("verse-b", &[CONFIGURED])]), vec![], vec![], vec![]),
                    (&pinned, adverts(&[]), vec![], vec![], vec![]),
                    (&dial_only, adverts(&[("verse-a", &[CONFIGURED])]), vec![], vec![], vec![]),
                    (&dial_only, adverts(&[]), vec![], vec![], vec![]),
                ],
            ),
        ];

        for (flow, steps) in flows {
            let mut reconciler = RelayReconciler::new(configured.clone());
            for (index, (policy, advertised, want_inserts, want_removes, want_known)) in
                steps.into_iter().enumerate()
            {
                let ops = reconciler.plan(policy, &advertised);
                assert_eq!(inserts(&ops), want_inserts, "{flow} step {index} inserts");
                assert_eq!(removes(&ops), want_removes, "{flow} step {index} removes");
                assert_eq!(knowns(&ops), want_known, "{flow} step {index} known");
                // Invariants on every plan: configured relays are untouchable, a removal
                // means no verse advertises that URL any more, the known set is readable.
                assert!(
                    !ops.iter().any(|op| matches!(op, RelayOp::Insert(url) | RelayOp::Remove(url) if configured.contains(url))),
                    "{flow} step {index} touched a configured relay: {ops:?}"
                );
                for url in removes(&ops) {
                    assert!(
                        advertised.values().all(|urls| !urls.contains(url)),
                        "{flow} step {index} removed {url} while still advertised"
                    );
                }
                assert_eq!(
                    reconciler.known().iter().map(String::as_str).collect::<Vec<_>>(),
                    want_known,
                    "{flow} step {index} known set"
                );
            }
        }

        // Property: random sequences over every input; Configured always stays, and an advertised URL
        // leaves the advertised book (inserted or known) only when no open verse advertises it.
        let universe = [CONFIGURED, DEFAULT_A, PINNED, COVERED, OTHER, PRIVATE_COVERED];
        let policies = [dial_only.clone(), pinned.clone(), deny.clone()];
        for seed in 1..=64u64 {
            let mut rng = Xorshift::seeded(seed);
            let mut reconciler =
                RelayReconciler::with_tiers(configured.clone(), set(&[DEFAULT_A, CONFIGURED]));
            assert_eq!(reconciler.defaults(), &set(&[DEFAULT_A]), "a url in both tiers is Configured");
            let mut relay_map = set(&[CONFIGURED, DEFAULT_A]);
            for step in 0..40 {
                let context = format!("seed {seed} step {step}");
                let policy = &policies[rng.below(policies.len())];
                let advertised: BTreeMap<String, BTreeSet<String>> = [
                    ("verse-a".to_string(), rng.subset(&universe)),
                    ("verse-b".to_string(), rng.subset(&universe)),
                ]
                .into();
                let inputs = RelayReconcileInputs {
                    policy,
                    advertised: &advertised,
                    home_connected: rng.home(&relay_map),
                    coverage: rng.subset(&universe),
                    auto_home: rng.chance(),
                    home_health: HEALTHS[rng.below(HEALTHS.len())],
                };
                let ops = reconciler.plan_tiers(&inputs);
                apply_ops(&mut relay_map, &ops, &context);

                assert!(relay_map.contains(CONFIGURED), "{context}: configured relay left the map");
                assert!(
                    !ops.iter().any(|op| matches!(op, RelayOp::Insert(url) | RelayOp::Remove(url) if url == CONFIGURED)),
                    "{context}: touched a configured relay: {ops:?}"
                );
                assert_eq!(reconciler.source_of(CONFIGURED), Some(RelaySource::Configured));
                for url in [PINNED, COVERED, OTHER, PRIVATE_COVERED] {
                    let refcount = advertised.values().filter(|urls| urls.contains(url)).count();
                    let in_book = reconciler.is_inserted(url) || reconciler.known().contains(url);
                    if refcount == 0 || matches!(policy, RelayAdvertisePolicy::Deny) {
                        assert!(!in_book && !relay_map.contains(url), "{context}: {url} kept without an advertiser");
                    } else {
                        assert!(in_book, "{context}: {url} dropped while {refcount} verse(s) advertise it");
                        assert_eq!(
                            relay_map.contains(url),
                            reconciler.is_inserted(url),
                            "{context}: {url} map membership disagrees with its adoption"
                        );
                    }
                    if reconciler.is_inserted(url) {
                        assert_ne!(url, PRIVATE_COVERED, "{context}: a private relay entered the map automatically");
                    }
                }
            }
        }

        // The policy is parsed locally from the environment text; anything invalid is dial-only.
        let policy_cases: [(&str, RelayAdvertisePolicy); 7] = [
            ("", RelayAdvertisePolicy::DialOnly),
            ("DialOnly", RelayAdvertisePolicy::DialOnly),
            ("deny", RelayAdvertisePolicy::Deny),
            (
                "pinned:https://pinned.example.org, https://pinned.example.org",
                RelayAdvertisePolicy::Pinned(vec![PINNED.to_string()]),
            ),
            ("pinned:", RelayAdvertisePolicy::DialOnly),
            ("pinned:not a url", RelayAdvertisePolicy::DialOnly),
            ("allow-everything", RelayAdvertisePolicy::DialOnly),
        ];
        for (raw, expected) in policy_cases {
            assert_eq!(RelayAdvertisePolicy::parse(raw), expected, "policy {raw:?}");
        }
        let disabled = crate::relay_config::RelayConfig::Disabled;
        assert_eq!(pinned.clone().effective_for(&disabled), RelayAdvertisePolicy::DialOnly);
    }

    #[test]
    fn default_relays_are_absent_only_while_a_covered_relay_is_connected_home() {
        let dial_only = RelayAdvertisePolicy::DialOnly;
        let both_verses = adverts(&[("verse-a", &[COVERED]), ("verse-b", &[COVERED])]);
        let third_verse_joined =
            adverts(&[("verse-a", &[COVERED]), ("verse-b", &[COVERED]), ("verse-c", &[])]);
        let insert = |url: &str| RelayOp::Insert(url.to_string());
        let remove = |url: &str| RelayOp::Remove(url.to_string());
        let known = |url: &str| RelayOp::Known(url.to_string());

        // AC-17 journey: adopt, demote once home, re-insert on every failure signal, inserts before removes.
        // (label, inputs, expected ops, Default tier demoted afterwards)
        let mut reconciler = RelayReconciler::with_tiers(BTreeSet::new(), set(&[DEFAULT_A, DEFAULT_B]));
        let healthy = RelayHealth::Healthy;
        let journey: Vec<(&str, RelayReconcileInputs<'_>, Vec<RelayOp>, bool)> = vec![
            ("covered relay adopted, n0 still home", tier_inputs(&dial_only, &both_verses, Some(DEFAULT_A), &[COVERED], true, healthy), vec![insert(COVERED)], false),
            ("covered relay connected home demotes n0", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, healthy), vec![remove(DEFAULT_A), remove(DEFAULT_B)], true),
            ("still home: no churn", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, healthy), vec![], true),
            ("joining an unadvertised verse breaks coverage", tier_inputs(&dial_only, &third_verse_joined, Some(COVERED), &[], true, healthy), vec![insert(DEFAULT_A), insert(DEFAULT_B), remove(COVERED), known(COVERED)], false),
            ("leaving it re-adopts", tier_inputs(&dial_only, &both_verses, Some(DEFAULT_B), &[COVERED], true, healthy), vec![insert(COVERED)], false),
            ("home again", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, healthy), vec![remove(DEFAULT_A), remove(DEFAULT_B)], true),
            ("30 s tick reports not connected", tier_inputs(&dial_only, &both_verses, None, &[COVERED], true, healthy), vec![insert(DEFAULT_A), insert(DEFAULT_B)], false),
            ("home again", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, healthy), vec![remove(DEFAULT_A), remove(DEFAULT_B)], true),
            ("health degraded", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, RelayHealth::Degraded), vec![insert(DEFAULT_A), insert(DEFAULT_B)], false),
            ("home again", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], true, healthy), vec![remove(DEFAULT_A), remove(DEFAULT_B)], true),
            ("toggle turned off: dial-only, n0 back", tier_inputs(&dial_only, &both_verses, Some(COVERED), &[COVERED], false, healthy), vec![insert(DEFAULT_A), insert(DEFAULT_B), remove(COVERED), known(COVERED)], false),
        ];
        for (index, (label, step_inputs, want, want_demoted)) in journey.iter().enumerate() {
            let ops = reconciler.plan_tiers(step_inputs);
            assert_eq!(&ops, want, "journey step {index}: {label}");
            assert_eq!(reconciler.defaults_demoted(), *want_demoted, "journey step {index}: {label} demotion flag");
        }
        assert_eq!(reconciler.adoption().get(COVERED), Some(&AdoptionMode::DialOnly));
        assert_eq!(reconciler.source_of(DEFAULT_A), Some(RelaySource::Default));
        assert_eq!(reconciler.source_of(COVERED), Some(RelaySource::Advertised));

        // A covered private relay is never adopted automatically; under Disabled nothing is.
        let private = adverts(&[("verse-a", &[PRIVATE_COVERED])]);
        let mut strict = RelayReconciler::with_tiers(BTreeSet::new(), set(&[DEFAULT_A]));
        let ops = strict.plan_tiers(&tier_inputs(&dial_only, &private, Some(DEFAULT_A), &[PRIVATE_COVERED], true, healthy));
        assert_eq!(ops, vec![known(PRIVATE_COVERED)]);
        let mut disabled = RelayReconciler::from_relay_config(&RelayConfig::Disabled);
        let ops = disabled.plan_tiers(&tier_inputs(&dial_only, &both_verses, None, &[COVERED], true, healthy));
        assert_eq!(ops, vec![known(COVERED)]);
        let custom = RelayReconciler::from_relay_config(&RelayConfig::Custom(vec![CONFIGURED.to_string()]));
        assert!(custom.defaults().is_empty() && custom.configured().len() == 1);

        // Property: over random sequences the Default tier is all-or-nothing, and absent only while
        // auto-home is on, health is Healthy, and the connected home is a covered public relay in the map.
        let universe = [COVERED, OTHER, PRIVATE_COVERED, DEFAULT_A];
        let pinned_other = RelayAdvertisePolicy::Pinned(vec![OTHER.to_string()]);
        let deny = RelayAdvertisePolicy::Deny;
        let policies = [&dial_only, &pinned_other, &deny];
        let defaults = set(&[DEFAULT_A, DEFAULT_B]);
        for seed in 1..=64u64 {
            let mut rng = Xorshift::seeded(seed ^ 0xD3FA_u64);
            let mut reconciler = RelayReconciler::with_tiers(BTreeSet::new(), defaults.clone());
            let mut relay_map = defaults.clone();
            for step in 0..60 {
                let context = format!("seed {seed} step {step}");
                let policy = policies[rng.below(policies.len())];
                let advertised: BTreeMap<String, BTreeSet<String>> = [
                    ("verse-a".to_string(), rng.subset(&universe)),
                    ("verse-b".to_string(), rng.subset(&universe)),
                ]
                .into();
                // Coverage is biased toward the realistic case so demotion is actually reached.
                let coverage = if rng.chance() { set(&[COVERED]) } else { rng.subset(&universe) };
                let step_inputs = RelayReconcileInputs {
                    policy,
                    advertised: &advertised,
                    home_connected: rng.home(&relay_map),
                    coverage,
                    auto_home: rng.below(4) != 0,
                    home_health: HEALTHS[rng.below(HEALTHS.len())],
                };
                let ops = reconciler.plan_tiers(&step_inputs);
                apply_ops(&mut relay_map, &ops, &context);

                let present = defaults.iter().filter(|url| relay_map.contains(*url)).count();
                assert!(present == 0 || present == defaults.len(), "{context}: Default tier split: {relay_map:?}");
                if present == 0 {
                    let home = step_inputs.home_connected.as_deref();
                    let covered_home = home.is_some_and(|home| {
                        step_inputs.coverage.contains(home)
                            && relay_map.contains(home)
                            && reconciler.is_inserted(home)
                            && !defaults.contains(home)
                            && home != PRIVATE_COVERED
                    });
                    assert!(
                        step_inputs.auto_home
                            && step_inputs.home_health == RelayHealth::Healthy
                            && !matches!(policy, RelayAdvertisePolicy::Deny)
                            && covered_home,
                        "{context}: Default relays absent without a covered relay connected home ({home:?})"
                    );
                }
                assert_eq!(reconciler.defaults_demoted(), present == 0, "{context}: demotion flag");
            }
        }
    }
}
