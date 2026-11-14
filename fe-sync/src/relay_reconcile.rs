//! Pure, refcounted relay-map planner for advertised relays (S7, AC-8/AC-9); see `fe-sync/src/AGENTS.md` §relay-advertise.

use std::collections::{BTreeMap, BTreeSet};

use crate::relay_config::RelayAdvertisePolicy;

/// One step the sync thread applies after a plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelayOp {
    /// Add this advertised relay to the endpoint's relay map (never with a token).
    Insert(String),
    /// Drop a relay this reconciler inserted; no verse advertises it any more.
    Remove(String),
    /// Listed and dial-allowlisted only (the full known set is re-listed on every plan).
    Known(String),
}

/// Desired-state reconciler: per-URL refcount of advertising verses, configured relays untouchable.
#[derive(Debug, Clone, Default)]
pub struct RelayReconciler {
    configured: BTreeSet<String>,
    inserted: BTreeMap<String, BTreeSet<String>>,
    known: BTreeSet<String>,
}

impl RelayReconciler {
    /// A reconciler over the operator's configured relays (canonical URL strings).
    pub fn new(configured: BTreeSet<String>) -> Self {
        Self {
            configured,
            inserted: BTreeMap::new(),
            known: BTreeSet::new(),
        }
    }

    /// Plan the relay-map changes for the admitted adverts `advertised` (verse id → URLs) under `policy`.
    pub fn plan(
        &mut self,
        policy: &RelayAdvertisePolicy,
        advertised: &BTreeMap<String, BTreeSet<String>>,
    ) -> Vec<RelayOp> {
        let mut advertisers: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for (verse_id, urls) in advertised {
            for url in urls {
                advertisers
                    .entry(url.as_str())
                    .or_default()
                    .insert(verse_id.clone());
            }
        }

        let mut next_inserted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut next_known: BTreeSet<String> = BTreeSet::new();
        for (url, verses) in advertisers {
            // A configured relay is already in the map with the operator's settings (and token):
            // inserting would replace that entry, so it is never planned at all.
            if self.configured.contains(url) {
                continue;
            }
            match policy {
                RelayAdvertisePolicy::Deny => {}
                RelayAdvertisePolicy::DialOnly => {
                    next_known.insert(url.to_string());
                }
                RelayAdvertisePolicy::Pinned(pinned) => {
                    if pinned.iter().any(|pinned_url| pinned_url == url) {
                        next_inserted.insert(url.to_string(), verses);
                    } else {
                        next_known.insert(url.to_string());
                    }
                }
            }
        }

        let mut ops = Vec::new();
        for url in self.inserted.keys() {
            if !next_inserted.contains_key(url) && !self.configured.contains(url) {
                ops.push(RelayOp::Remove(url.clone()));
            }
        }
        for url in next_inserted.keys() {
            if !self.inserted.contains_key(url) {
                ops.push(RelayOp::Insert(url.clone()));
            }
        }
        ops.extend(next_known.iter().cloned().map(RelayOp::Known));

        self.inserted = next_inserted;
        self.known = next_known;
        ops
    }

    /// Forget an insert the endpoint did not take, so a later plan retries it.
    pub fn forget_insert(&mut self, url: &str) {
        self.inserted.remove(url);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIGURED: &str = "https://configured.example.org/";
    const PINNED: &str = "https://pinned.example.org/";
    const OTHER: &str = "https://other.example.org/";

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

    #[test]
    fn planner_never_removes_a_configured_relay_and_refcounts_across_verses() {
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
}
