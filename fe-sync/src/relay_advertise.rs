//! Read-time, role-gated `__relays` advertisement book (FR-7); see `fe-sync/src/AGENTS.md` §relay-advertise.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use fe_policy::RoleLevel;
use fe_runtime::relay_advertisement::{
    parse_row, verify_row_at, AdvertRefusal, RelayAdvertisementRow, RowParse,
    MAX_ADVERTS_PER_VERSE, MAX_ADVERT_TTL_MS,
};

pub use fe_runtime::relay_advertisement::{
    validate_advertised_relay_url, RELAY_ADVERT_ROW_VERSION, RELAY_ADVERT_TABLE,
};

use crate::relay_config::RelayAdvertSignedMode;
use crate::relay_trust::select_bounded;

/// Tolerated clock skew for `issued_at_ms` (spec §4.4 item 5: `issued_at <= now + 60 s`).
pub const ADVERT_ISSUED_SKEW_MS: u64 = 60_000;

/// Bound on quarantined advertisements across all verses (same posture as `__shards`).
pub const RELAY_QUARANTINE_CAP: usize = 1024;

/// Bound on remembered withdrawals per verse.
pub const RELAY_TOMBSTONE_PER_VERSE_CAP: usize = 64;

/// Bound on remembered withdrawals across all verses.
pub const RELAY_TOMBSTONE_CAP: usize = 1024;

/// Bound on quarantined advertisements one author holds in one verse.
pub const RELAY_QUARANTINE_PER_AUTHOR_CAP: usize = 8;

/// Minimum verse-scope role an advertiser must hold when the row is read.
pub const RELAY_ADVERTISER_FLOOR: RoleLevel = RoleLevel::Manager;

/// A change to the set of admitted advertisements, consumed by the relay reconciler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvertChange {
    Admitted {
        verse_id: String,
        record_id: String,
        url: String,
    },
    Withdrawn {
        verse_id: String,
        record_id: String,
        url: String,
    },
}

/// One role-eligible advertisement; `url` is the validator's normalized form.
#[derive(Debug, Clone)]
struct AdmittedAdvert {
    author_did: String,
    url: String,
    row: RelayAdvertisementRow,
}

/// A well-formed, self-attributed row whose author lacked the role floor when read.
#[derive(Debug, Clone)]
struct QuarantinedAdvert {
    verse_id: String,
    record_id: String,
    author_did: String,
    url: String,
    row: RelayAdvertisementRow,
    /// Arrival order, for oldest-first eviction across authors.
    arrival: u64,
}

/// A Manager+ withdrawal remembered so an older cross-author row arriving later cannot resurrect the record.
#[derive(Debug, Clone)]
struct TombstoneMemo {
    tombstoner_did: String,
    seen_at_ms: u64,
    arrival: u64,
}

/// Per-verse eligible advertisements, the bounded visible subset emitted as changes, and a per-author bounded quarantine.
#[derive(Debug, Default)]
pub struct RelayAdvertisementBook {
    /// verse id → record id → structurally valid row whose author holds the floor.
    admitted: BTreeMap<String, BTreeMap<String, AdmittedAdvert>>,
    /// verse id → record id → URL last emitted as admitted (unexpired, within `MAX_ADVERTS_PER_VERSE`).
    visible: BTreeMap<String, BTreeMap<String, String>>,
    signed_mode: RelayAdvertSignedMode,
    /// verse id → author DID → pending adverts, oldest first, one per record id.
    quarantine: BTreeMap<String, BTreeMap<String, VecDeque<QuarantinedAdvert>>>,
    quarantine_len: usize,
    quarantine_arrivals: u64,
    quarantine_evicted: u64,
    quarantine_author_evicted: u64,
    /// verse id → record id → remembered Manager+ withdrawal (bounded per verse and overall).
    tombstones: BTreeMap<String, BTreeMap<String, TombstoneMemo>>,
    tombstone_len: usize,
    tombstone_arrivals: u64,
    tombstone_evicted: u64,
}

/// Outcome of the structural checks every row passes before the role gate.
enum Structural {
    Valid { url: String, row: RelayAdvertisementRow },
    Refused,
}

impl RelayAdvertisementBook {
    /// A book under the process signed-advert mode (`FE_SYNC_RELAY_ADVERT_SIGNED`).
    pub fn new() -> Self {
        Self::with_signed_mode(RelayAdvertSignedMode::process())
    }

    /// A book under an explicit signed-advert mode.
    pub fn with_signed_mode(signed_mode: RelayAdvertSignedMode) -> Self {
        Self {
            signed_mode,
            ..Self::default()
        }
    }

    /// Judge one inbound `__relays` entry now; see [`Self::observe_at`].
    pub fn observe(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        data: &[u8],
        is_tombstone: bool,
        role_of: impl Fn(&str, &str) -> RoleLevel,
    ) -> Vec<AdvertChange> {
        self.observe_at(verse_id, record_id, author_did, data, is_tombstone, role_of, unix_now_ms())
    }

    /// Judge one inbound `__relays` entry at `now_ms`; `author_did` is the `SignedEntry` author, `role_of(did, scope)` the fabric's `role_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn observe_at(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        data: &[u8],
        is_tombstone: bool,
        role_of: impl Fn(&str, &str) -> RoleLevel,
        now_ms: u64,
    ) -> Vec<AdvertChange> {
        let scope = verse_scope(verse_id);
        let mut changes = Vec::new();

        if is_tombstone || data.is_empty() {
            self.observe_withdrawal(verse_id, record_id, author_did, &scope, &role_of, now_ms);
            self.publish(verse_id, now_ms, &mut changes);
            return changes;
        }

        // The author's own earlier pending row for this record is superseded by this one.
        self.remove_pending(verse_id, author_did, record_id);

        match judge_structure(verse_id, record_id, author_did, data, now_ms, self.signed_mode) {
            Structural::Valid { url, row } => {
                if self.tombstone_refuses(verse_id, record_id, author_did, &row) {
                    tracing::debug!(
                        verse_id,
                        record_id,
                        author = author_did,
                        "relay advertisement refused: a Manager+ withdrawal of this record was seen at or after its issue time"
                    );
                    return changes;
                }
                if role_of(author_did, &scope).is_at_least(RELAY_ADVERTISER_FLOOR) {
                    self.make_eligible(verse_id, record_id, author_did, url, row);
                } else {
                    if self.admitted_author(verse_id, record_id) == Some(author_did) {
                        self.retire(verse_id, record_id);
                    }
                    tracing::debug!(
                        verse_id,
                        record_id,
                        author = author_did,
                        "relay advertisement quarantined: advertiser is below Manager at the verse scope"
                    );
                    self.quarantine(QuarantinedAdvert {
                        verse_id: verse_id.to_string(),
                        record_id: record_id.to_string(),
                        author_did: author_did.to_string(),
                        url,
                        row,
                        arrival: 0,
                    });
                }
            }
            Structural::Refused => {
                // A refused rewrite by the same author retires that author's admission; others' stay.
                if self.admitted_author(verse_id, record_id) == Some(author_did) {
                    self.retire(verse_id, record_id);
                }
            }
        }

        self.publish(verse_id, now_ms, &mut changes);
        if self.is_held_over_bound(verse_id, record_id, now_ms) {
            tracing::warn!(
                verse_id,
                record_id,
                refusal = AdvertRefusal::OverBound.name(),
                limit = MAX_ADVERTS_PER_VERSE,
                "relay advertisement held back: the verse already has its bound of older adverts"
            );
        }
        changes
    }

    /// Re-judge after `did`'s role changed: demoted admissions are withdrawn and quarantined, promoted quarantine admitted.
    pub fn rejudge(
        &mut self,
        did: &str,
        role_of: impl Fn(&str, &str) -> RoleLevel,
    ) -> Vec<AdvertChange> {
        let now_ms = unix_now_ms();
        let mut touched: BTreeSet<String> = BTreeSet::new();

        let demoted: Vec<(String, String)> = self
            .admitted
            .iter()
            .flat_map(|(verse_id, records)| {
                records
                    .iter()
                    .filter(|(_, advert)| advert.author_did == did)
                    .map(move |(record_id, _)| (verse_id.clone(), record_id.clone()))
            })
            .filter(|(verse_id, _)| {
                !role_of(did, &verse_scope(verse_id)).is_at_least(RELAY_ADVERTISER_FLOOR)
            })
            .collect();
        for (verse_id, record_id) in demoted {
            if let Some(advert) = self.retire(&verse_id, &record_id) {
                touched.insert(verse_id.clone());
                self.quarantine(QuarantinedAdvert {
                    verse_id,
                    record_id,
                    author_did: advert.author_did,
                    url: advert.url,
                    row: advert.row,
                    arrival: 0,
                });
            }
        }

        // Only `did`'s own pending adverts can change verdict when `did`'s role moves.
        let promoted_in: Vec<String> = self
            .quarantine
            .iter()
            .filter(|(_, authors)| authors.contains_key(did))
            .map(|(verse_id, _)| verse_id.clone())
            .filter(|verse_id| {
                role_of(did, &verse_scope(verse_id)).is_at_least(RELAY_ADVERTISER_FLOOR)
            })
            .collect();
        for verse_id in promoted_in {
            let Some(authors) = self.quarantine.get_mut(&verse_id) else {
                continue;
            };
            let Some(pending) = authors.remove(did) else {
                continue;
            };
            if authors.is_empty() {
                self.quarantine.remove(&verse_id);
            }
            self.quarantine_len -= pending.len();
            for entry in pending {
                let QuarantinedAdvert {
                    verse_id,
                    record_id,
                    author_did,
                    url,
                    row,
                    ..
                } = entry;
                if self.tombstone_refuses(&verse_id, &record_id, &author_did, &row) {
                    continue;
                }
                self.make_eligible(&verse_id, &record_id, &author_did, url, row);
            }
            touched.insert(verse_id);
        }

        let mut changes = Vec::new();
        for verse_id in touched {
            self.publish(&verse_id, now_ms, &mut changes);
        }
        changes
    }

    /// Drop every eligible and quarantined row expired at `now_ms`, withdrawing the visible ones (the expiry timer).
    pub fn expire(&mut self, now_ms: u64) -> Vec<AdvertChange> {
        for records in self.admitted.values_mut() {
            records.retain(|_, advert| !is_expired(&advert.row, now_ms));
        }
        self.admitted.retain(|_, records| !records.is_empty());

        let mut removed = 0;
        for authors in self.quarantine.values_mut() {
            authors.retain(|_, pending| {
                let before = pending.len();
                pending.retain(|entry| !is_expired(&entry.row, now_ms));
                removed += before - pending.len();
                !pending.is_empty()
            });
        }
        self.quarantine.retain(|_, authors| !authors.is_empty());
        self.quarantine_len -= removed;

        let mut dropped_memos = 0;
        for records in self.tombstones.values_mut() {
            let before = records.len();
            records.retain(|_, memo| memo.seen_at_ms.saturating_add(MAX_ADVERT_TTL_MS) >= now_ms);
            dropped_memos += before - records.len();
        }
        self.tombstones.retain(|_, records| !records.is_empty());
        self.tombstone_len -= dropped_memos;

        let verses: BTreeSet<String> = self
            .visible
            .keys()
            .chain(self.admitted.keys())
            .cloned()
            .collect();
        let mut changes = Vec::new();
        for verse_id in verses {
            self.publish(&verse_id, now_ms, &mut changes);
        }
        changes
    }

    /// Withdraw every admission and quarantined row of a verse whose replica closed.
    pub fn close_verse(&mut self, verse_id: &str) -> Vec<AdvertChange> {
        if let Some(authors) = self.quarantine.remove(verse_id) {
            self.quarantine_len -= authors.values().map(VecDeque::len).sum::<usize>();
        }
        if let Some(records) = self.tombstones.remove(verse_id) {
            self.tombstone_len -= records.len();
        }
        self.admitted.remove(verse_id);
        let Some(records) = self.visible.remove(verse_id) else {
            return Vec::new();
        };
        records
            .into_iter()
            .map(|(record_id, url)| AdvertChange::Withdrawn {
                verse_id: verse_id.to_string(),
                record_id,
                url,
            })
            .collect()
    }

    /// Normalized admitted relay URLs per verse, excluding any advert that has expired since it was admitted.
    pub fn urls_by_verse(&self) -> BTreeMap<String, BTreeSet<String>> {
        let now_ms = unix_now_ms();
        self.visible
            .iter()
            .filter_map(|(verse_id, records)| {
                let urls: BTreeSet<String> = records
                    .iter()
                    .filter(|(record_id, _)| self.is_live(verse_id, record_id, now_ms))
                    .map(|(_, url)| url.clone())
                    .collect();
                (!urls.is_empty()).then(|| (verse_id.clone(), urls))
            })
            .collect()
    }

    /// Admitted rows of one verse, in record-id order.
    pub fn admitted(&self, verse_id: &str) -> Vec<&RelayAdvertisementRow> {
        let Some(records) = self.visible.get(verse_id) else {
            return Vec::new();
        };
        records
            .keys()
            .filter_map(|record_id| {
                self.admitted
                    .get(verse_id)
                    .and_then(|eligible| eligible.get(record_id))
                    .map(|advert| &advert.row)
            })
            .collect()
    }

    /// How many advertisements are waiting on their author's role (diagnostics/tests).
    pub fn quarantine_len(&self) -> usize {
        self.quarantine_len
    }

    /// How many quarantined advertisements were evicted under either cap (diagnostics/tests).
    pub fn quarantine_evicted(&self) -> u64 {
        self.quarantine_evicted
    }

    /// How many of those evictions hit the per-author cap (diagnostics/tests).
    pub fn quarantine_author_evicted(&self) -> u64 {
        self.quarantine_author_evicted
    }

    /// How many remembered withdrawals are held (diagnostics/tests).
    pub fn tombstone_len(&self) -> usize {
        self.tombstone_len
    }

    /// How many remembered withdrawals were evicted under either cap (diagnostics/tests).
    pub fn tombstone_evicted(&self) -> u64 {
        self.tombstone_evicted
    }

    /// True when a remembered withdrawal by another author outranks this row; otherwise a superseding row clears the memo.
    fn tombstone_refuses(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        row: &RelayAdvertisementRow,
    ) -> bool {
        let Some(memo) = self.tombstones.get(verse_id).and_then(|records| records.get(record_id)) else {
            return false;
        };
        let refused = memo.tombstoner_did != author_did
            && row.issued_at_ms.map_or(true, |issued| issued <= memo.seen_at_ms);
        if !refused {
            self.forget_tombstone(verse_id, record_id);
        }
        refused
    }

    fn forget_tombstone(&mut self, verse_id: &str, record_id: &str) {
        let Some(records) = self.tombstones.get_mut(verse_id) else {
            return;
        };
        if records.remove(record_id).is_some() {
            self.tombstone_len -= 1;
        }
        if records.is_empty() {
            self.tombstones.remove(verse_id);
        }
    }

    /// Remember a Manager+ withdrawal, evicting the oldest memo under the per-verse then the global cap.
    fn remember_tombstone(&mut self, verse_id: &str, record_id: &str, tombstoner_did: &str, now_ms: u64) {
        self.forget_tombstone(verse_id, record_id);
        let in_verse = self.tombstones.get(verse_id).map_or(0, BTreeMap::len);
        if in_verse >= RELAY_TOMBSTONE_PER_VERSE_CAP {
            let oldest = self.tombstones.get(verse_id).and_then(|records| {
                records.iter().min_by_key(|(_, memo)| memo.arrival).map(|(id, _)| id.clone())
            });
            if let Some(oldest) = oldest {
                self.forget_tombstone(verse_id, &oldest);
                self.tombstone_evicted += 1;
            }
        }
        if self.tombstone_len >= RELAY_TOMBSTONE_CAP {
            let oldest = self
                .tombstones
                .iter()
                .flat_map(|(verse, records)| {
                    records.iter().map(move |(id, memo)| (memo.arrival, verse.clone(), id.clone()))
                })
                .min();
            if let Some((_, verse, id)) = oldest {
                self.forget_tombstone(&verse, &id);
                self.tombstone_evicted += 1;
            }
        }
        self.tombstone_arrivals += 1;
        self.tombstones.entry(verse_id.to_string()).or_default().insert(
            record_id.to_string(),
            TombstoneMemo {
                tombstoner_did: tombstoner_did.to_string(),
                seen_at_ms: now_ms,
                arrival: self.tombstone_arrivals,
            },
        );
        self.tombstone_len += 1;
    }

    fn observe_withdrawal(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        scope: &str,
        role_of: &impl Fn(&str, &str) -> RoleLevel,
        now_ms: u64,
    ) {
        let author_is_manager = role_of(author_did, scope).is_at_least(RELAY_ADVERTISER_FLOOR);
        if author_is_manager {
            self.remember_tombstone(verse_id, record_id, author_did, now_ms);
            self.drop_quarantined_record(verse_id, record_id);
        } else {
            self.remove_pending(verse_id, author_did, record_id);
        }
        let may_withdraw = self
            .admitted_author(verse_id, record_id)
            .map(|admitted_by| admitted_by == author_did || author_is_manager);
        match may_withdraw {
            Some(true) => {
                self.retire(verse_id, record_id);
            }
            Some(false) => tracing::warn!(
                verse_id,
                record_id,
                author = author_did,
                "relay advertisement withdrawal refused: only its advertiser or a Manager+ may withdraw it"
            ),
            None => {}
        }
    }

    fn make_eligible(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        url: String,
        row: RelayAdvertisementRow,
    ) {
        self.admitted.entry(verse_id.to_string()).or_default().insert(
            record_id.to_string(),
            AdmittedAdvert {
                author_did: author_did.to_string(),
                url,
                row,
            },
        );
    }

    fn retire(&mut self, verse_id: &str, record_id: &str) -> Option<AdmittedAdvert> {
        let records = self.admitted.get_mut(verse_id)?;
        let advert = records.remove(record_id)?;
        if records.is_empty() {
            self.admitted.remove(verse_id);
        }
        Some(advert)
    }

    /// Recompute the visible set of `verse_id` and emit the difference (withdrawals first).
    fn publish(&mut self, verse_id: &str, now_ms: u64, changes: &mut Vec<AdvertChange>) {
        let next = self.select_visible(verse_id, now_ms);
        let previous = self.visible.remove(verse_id).unwrap_or_default();
        for (record_id, url) in &previous {
            if next.get(record_id) != Some(url) {
                changes.push(AdvertChange::Withdrawn {
                    verse_id: verse_id.to_string(),
                    record_id: record_id.clone(),
                    url: url.clone(),
                });
            }
        }
        for (record_id, url) in &next {
            if previous.get(record_id) != Some(url) {
                changes.push(AdvertChange::Admitted {
                    verse_id: verse_id.to_string(),
                    record_id: record_id.clone(),
                    url: url.clone(),
                });
            }
        }
        if !next.is_empty() {
            self.visible.insert(verse_id.to_string(), next);
        }
    }

    /// Unexpired eligible rows cut to `MAX_ADVERTS_PER_VERSE` by `relay_trust::select_bounded`: signed rows first, compat rows fill what is left.
    fn select_visible(&self, verse_id: &str, now_ms: u64) -> BTreeMap<String, String> {
        let Some(records) = self.admitted.get(verse_id) else {
            return BTreeMap::new();
        };
        let live: Vec<(&String, &AdmittedAdvert)> = records
            .iter()
            .filter(|(_, advert)| !is_expired(&advert.row, now_ms))
            .collect();
        // An unsigned row has no `issued_at_ms` (sorts as 0), so it must not outrank signed rows.
        let (signed, unsigned): (Vec<RelayAdvertisementRow>, Vec<RelayAdvertisementRow>) = live
            .iter()
            .map(|(_, advert)| advert.row.clone())
            .partition(|row| row.sig.is_some());
        let (mut kept, _held_back) = select_bounded(signed, MAX_ADVERTS_PER_VERSE);
        let room = MAX_ADVERTS_PER_VERSE.saturating_sub(kept.len());
        let (compat_kept, _compat_held_back) = select_bounded(unsigned, room);
        kept.extend(compat_kept);
        // The sort is stable, so identical rows keep record-id order: take the first unused match.
        let mut used = vec![false; live.len()];
        let mut visible = BTreeMap::new();
        for row in &kept {
            let found = live
                .iter()
                .enumerate()
                .position(|(index, (_, advert))| !used[index] && advert.row == *row);
            if let Some(index) = found {
                used[index] = true;
                let (record_id, advert) = live[index];
                visible.insert(record_id.clone(), advert.url.clone());
            }
        }
        visible
    }

    /// `true` when `record_id` is eligible and unexpired yet cut by the per-verse bound.
    fn is_held_over_bound(&self, verse_id: &str, record_id: &str, now_ms: u64) -> bool {
        let eligible_live = self
            .admitted
            .get(verse_id)
            .and_then(|records| records.get(record_id))
            .is_some_and(|advert| !is_expired(&advert.row, now_ms));
        let visible = self
            .visible
            .get(verse_id)
            .is_some_and(|records| records.contains_key(record_id));
        eligible_live && !visible
    }

    fn is_live(&self, verse_id: &str, record_id: &str, now_ms: u64) -> bool {
        self.admitted
            .get(verse_id)
            .and_then(|records| records.get(record_id))
            .is_some_and(|advert| !is_expired(&advert.row, now_ms))
    }

    fn admitted_author(&self, verse_id: &str, record_id: &str) -> Option<&str> {
        self.admitted
            .get(verse_id)
            .and_then(|records| records.get(record_id))
            .map(|advert| advert.author_did.as_str())
    }

    /// Hold `entry` under both caps; evictions are counted, never logged per row (a flood is not a log flood).
    fn quarantine(&mut self, mut entry: QuarantinedAdvert) {
        self.remove_pending(&entry.verse_id, &entry.author_did, &entry.record_id);
        let held = self
            .quarantine
            .get(&entry.verse_id)
            .and_then(|authors| authors.get(&entry.author_did))
            .map_or(0, VecDeque::len);
        if held >= RELAY_QUARANTINE_PER_AUTHOR_CAP {
            // At its own cap an author only ever displaces its own oldest row.
            self.pop_oldest(&entry.verse_id, &entry.author_did);
            self.quarantine_author_evicted += 1;
            self.quarantine_evicted += 1;
        } else if self.quarantine_len >= RELAY_QUARANTINE_CAP {
            self.evict_from_largest_holder();
        }
        entry.arrival = self.quarantine_arrivals;
        self.quarantine_arrivals += 1;
        self.quarantine
            .entry(entry.verse_id.clone())
            .or_default()
            .entry(entry.author_did.clone())
            .or_default()
            .push_back(entry);
        self.quarantine_len += 1;
    }

    /// Global cap: evict the oldest row of the (verse, author) holding the most rows (ties: oldest row).
    fn evict_from_largest_holder(&mut self) {
        let victim = self
            .quarantine
            .iter()
            .flat_map(|(verse_id, authors)| {
                authors.iter().filter_map(move |(author_did, pending)| {
                    pending.front().map(|oldest| {
                        (pending.len(), std::cmp::Reverse(oldest.arrival), verse_id, author_did)
                    })
                })
            })
            .max()
            .map(|(_, _, verse_id, author_did)| (verse_id.clone(), author_did.clone()));
        if let Some((verse_id, author_did)) = victim {
            self.pop_oldest(&verse_id, &author_did);
            self.quarantine_evicted += 1;
        }
    }

    fn pop_oldest(&mut self, verse_id: &str, author_did: &str) {
        let Some(authors) = self.quarantine.get_mut(verse_id) else {
            return;
        };
        let Some(pending) = authors.get_mut(author_did) else {
            return;
        };
        if pending.pop_front().is_some() {
            self.quarantine_len -= 1;
        }
        if pending.is_empty() {
            authors.remove(author_did);
        }
        if authors.is_empty() {
            self.quarantine.remove(verse_id);
        }
    }

    /// Drop `author_did`'s pending row for `record_id` in `verse_id`, if any.
    fn remove_pending(&mut self, verse_id: &str, author_did: &str, record_id: &str) {
        let Some(authors) = self.quarantine.get_mut(verse_id) else {
            return;
        };
        let Some(pending) = authors.get_mut(author_did) else {
            return;
        };
        let before = pending.len();
        pending.retain(|entry| entry.record_id != record_id);
        self.quarantine_len -= before - pending.len();
        if pending.is_empty() {
            authors.remove(author_did);
        }
        if authors.is_empty() {
            self.quarantine.remove(verse_id);
        }
    }

    /// Drop every author's pending row for `record_id` in `verse_id` (a Manager+ withdrawal).
    fn drop_quarantined_record(&mut self, verse_id: &str, record_id: &str) {
        let Some(authors) = self.quarantine.get_mut(verse_id) else {
            return;
        };
        let mut removed = 0;
        authors.retain(|_, pending| {
            let before = pending.len();
            pending.retain(|entry| entry.record_id != record_id);
            removed += before - pending.len();
            !pending.is_empty()
        });
        if authors.is_empty() {
            self.quarantine.remove(verse_id);
        }
        self.quarantine_len -= removed;
    }
}

/// Verse-level policy scope (`VERSE#<verse_id>`).
fn verse_scope(verse_id: &str) -> String {
    fe_policy::build_scope(verse_id, None, None)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn is_expired(row: &RelayAdvertisementRow, now_ms: u64) -> bool {
    row.expires_at_ms.is_some_and(|expires| expires <= now_ms)
}

/// Lifetime rules for an unsigned (compat) row: a stated expiry must be in the future and within the TTL cap.
fn check_unsigned_lifetime(row: &RelayAdvertisementRow, now_ms: u64) -> Result<(), AdvertRefusal> {
    let Some(expires) = row.expires_at_ms else {
        return Ok(());
    };
    let start = row.issued_at_ms.unwrap_or(now_ms);
    if expires.saturating_sub(start) > MAX_ADVERT_TTL_MS {
        return Err(AdvertRefusal::TtlTooLong);
    }
    if expires <= now_ms {
        return Err(AdvertRefusal::Expired);
    }
    Ok(())
}

/// Signature (against the author DID's own key), verse binding, lifetime and signed-mode checks.
fn check_authenticity(
    row: &RelayAdvertisementRow,
    verse_id: &str,
    author_did: &str,
    now_ms: u64,
    signed_mode: RelayAdvertSignedMode,
) -> Result<(), &'static str> {
    if row.verse_id.as_deref().is_some_and(|bound| bound != verse_id) {
        return Err(AdvertRefusal::VerseMismatch.name());
    }
    // A future issue time would stretch the 30-day lifetime cap past its intent.
    if row
        .issued_at_ms
        .is_some_and(|issued| issued > now_ms.saturating_add(ADVERT_ISSUED_SKEW_MS))
    {
        return Err("issued_in_future");
    }
    if row.sig.is_some() {
        let key = fe_identity::did_key::public_key_from_did_key(author_did)
            .map_err(|_| AdvertRefusal::AuthorMismatch.name())?;
        return verify_row_at(row, &key, now_ms, Some(verse_id)).map_err(AdvertRefusal::name);
    }
    if signed_mode == RelayAdvertSignedMode::Require {
        return Err("unsigned");
    }
    check_unsigned_lifetime(row, now_ms).map_err(AdvertRefusal::name)
}

/// Version, attribution, URL and authenticity checks; refusals warn and are never quarantined.
fn judge_structure(
    verse_id: &str,
    record_id: &str,
    author_did: &str,
    data: &[u8],
    now_ms: u64,
    signed_mode: RelayAdvertSignedMode,
) -> Structural {
    let row = match parse_row(data) {
        RowParse::Row(row) => row,
        RowParse::UnknownVersion(version) => {
            tracing::warn!(
                verse_id,
                record_id,
                version,
                supported = RELAY_ADVERT_ROW_VERSION,
                "relay advertisement skipped: unknown row version"
            );
            return Structural::Refused;
        }
        RowParse::Malformed(reason) => {
            tracing::warn!(verse_id, record_id, %reason, "relay advertisement skipped: malformed row");
            return Structural::Refused;
        }
    };
    if row.advertised_by != author_did {
        tracing::warn!(
            verse_id,
            record_id,
            author = author_did,
            advertised_by = %row.advertised_by,
            "relay advertisement refused: advertised_by is not the signing author"
        );
        return Structural::Refused;
    }
    let url = match validate_advertised_relay_url(&row.url) {
        Ok(url) => url.to_string(),
        Err(error) => {
            tracing::warn!(verse_id, record_id, %error, "relay advertisement refused: invalid relay url");
            return Structural::Refused;
        }
    };
    if let Err(refusal) = check_authenticity(&row, verse_id, author_did, now_ms, signed_mode) {
        tracing::warn!(
            verse_id,
            record_id,
            author = author_did,
            refusal,
            "relay advertisement refused"
        );
        return Structural::Refused;
    }
    Structural::Valid { url, row }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const VERSE: &str = "verse-a";
    const OWNER: &str = "did:key:owner";
    const MANAGER: &str = "did:key:manager";
    const VIEWER: &str = "did:key:viewer";

    /// Roles at `VERSE#verse-a`; anything else resolves to `None`.
    struct Roles(RefCell<HashMap<String, RoleLevel>>);

    impl Roles {
        fn new(grants: &[(&str, RoleLevel)]) -> Self {
            Self(RefCell::new(
                grants
                    .iter()
                    .map(|(did, role)| (did.to_string(), *role))
                    .collect(),
            ))
        }
        fn set(&self, did: &str, role: RoleLevel) {
            self.0.borrow_mut().insert(did.to_string(), role);
        }
        fn role_of(&self) -> impl Fn(&str, &str) -> RoleLevel + '_ {
            move |did: &str, scope: &str| {
                if scope != verse_scope(VERSE) {
                    return RoleLevel::None;
                }
                self.0.borrow().get(did).copied().unwrap_or(RoleLevel::None)
            }
        }
    }

    fn row(url: &str, advertised_by: &str) -> Vec<u8> {
        format!(r#"{{"v":1,"url":"{url}","advertised_by":"{advertised_by}"}}"#).into_bytes()
    }

    fn admitted(record_id: &str, url: &str) -> AdvertChange {
        AdvertChange::Admitted {
            verse_id: VERSE.into(),
            record_id: record_id.into(),
            url: url.into(),
        }
    }

    fn withdrawn(record_id: &str, url: &str) -> AdvertChange {
        AdvertChange::Withdrawn {
            verse_id: VERSE.into(),
            record_id: record_id.into(),
            url: url.into(),
        }
    }

    /// A signed v1 row by `signer`, valid for a minute from `now`, after `mutate` (applied before signing).
    fn signed_row(
        signer: &fe_identity::keypair::NodeKeypair,
        url: &str,
        now: u64,
        mutate: impl Fn(&mut RelayAdvertisementRow),
    ) -> RelayAdvertisementRow {
        let mut row = RelayAdvertisementRow {
            v: RELAY_ADVERT_ROW_VERSION,
            url: url.to_string(),
            advertised_by: signer.to_did_key(),
            verse_id: Some(VERSE.to_string()),
            issued_at_ms: Some(now - 1_000),
            expires_at_ms: Some(now + 60_000),
            ..Default::default()
        };
        mutate(&mut row);
        let mut message = fe_runtime::relay_advertisement::RELAY_ADVERT_SIGNING_DOMAIN.to_vec();
        message.extend_from_slice(&fe_runtime::relay_advertisement::canonical_body(&row));
        row.sig = Some(hex::encode(signer.sign(&message).to_bytes()));
        row
    }

    fn wire(row: &RelayAdvertisementRow) -> Vec<u8> {
        serde_json::to_vec(row).expect("advert row serializes")
    }

    #[test]
    fn relay_advertisement_book_journey() {
        let roles = Roles::new(&[
            (OWNER, RoleLevel::Owner),
            (MANAGER, RoleLevel::Manager),
            (VIEWER, RoleLevel::Viewer),
        ]);
        let mut book = RelayAdvertisementBook::with_signed_mode(RelayAdvertSignedMode::Compat);

        // Owner advertises: admitted with the validator's normalized URL.
        let changes = book.observe(VERSE, "r-owner", OWNER, &row("https://eu.relay.example", OWNER), false, roles.role_of());
        assert_eq!(changes, vec![admitted("r-owner", "https://eu.relay.example/")]);

        // Viewer advertises: quarantined, no change, not dropped.
        let changes = book.observe(VERSE, "r-viewer", VIEWER, &row("https://viewer.example", VIEWER), false, roles.role_of());
        assert!(changes.is_empty());
        assert_eq!(book.quarantine_len(), 1);

        // Forgery: a Viewer signs a row naming the Owner as advertiser (AC-10 second half).
        let changes = book.observe(VERSE, "r-forged", VIEWER, &row("https://forged.example", OWNER), false, roles.role_of());
        assert!(changes.is_empty());
        assert_eq!(book.quarantine_len(), 1, "a forgery is refused, never quarantined");

        // A Viewer cannot withdraw the Owner's advertisement by tombstoning its record.
        assert!(book.observe(VERSE, "r-owner", VIEWER, &[], true, roles.role_of()).is_empty());

        // Manager advertises, is demoted to Editor, re-judge withdraws it (AC-10 third half).
        let changes = book.observe(VERSE, "r-manager", MANAGER, &row("https://mgr.example", MANAGER), false, roles.role_of());
        assert_eq!(changes, vec![admitted("r-manager", "https://mgr.example/")]);
        roles.set(MANAGER, RoleLevel::Editor);
        assert_eq!(book.rejudge(MANAGER, roles.role_of()), vec![withdrawn("r-manager", "https://mgr.example/")]);
        assert_eq!(book.quarantine_len(), 2);

        // Viewer promoted to Manager: its quarantined advertisement is admitted.
        roles.set(VIEWER, RoleLevel::Manager);
        assert_eq!(book.rejudge(VIEWER, roles.role_of()), vec![admitted("r-viewer", "https://viewer.example/")]);
        assert_eq!(book.quarantine_len(), 1);

        // Owner tombstones its row: withdrawn.
        let changes = book.observe(VERSE, "r-owner", OWNER, &[], true, roles.role_of());
        assert_eq!(changes, vec![withdrawn("r-owner", "https://eu.relay.example/")]);

        // Unknown row version: skipped, nothing admitted (AC-11 second half).
        let v2 = br#"{"v":2,"url":"https://v2.example","advertised_by":"did:key:owner"}"#;
        assert!(book.observe(VERSE, "r-v2", OWNER, v2, false, roles.role_of()).is_empty());

        // Extra unknown field: admitted normally (AC-11 first half).
        let extra = br#"{"v":1,"url":"https://extra.example","advertised_by":"did:key:owner","expires_at":9}"#;
        assert_eq!(
            book.observe(VERSE, "r-extra", OWNER, extra, false, roles.role_of()),
            vec![admitted("r-extra", "https://extra.example/")]
        );

        // A `sig` that does not verify is refused even in compat mode.
        let bad_sig = br#"{"v":1,"url":"https://sig.example","advertised_by":"did:key:owner","sig":"abc"}"#;
        assert!(book.observe(VERSE, "r-bad-sig", OWNER, bad_sig, false, roles.role_of()).is_empty());

        let urls: Vec<String> = book.urls_by_verse()[VERSE].iter().cloned().collect();
        assert_eq!(urls, vec!["https://extra.example/".to_string(), "https://viewer.example/".to_string()]);
        assert_eq!(book.admitted(VERSE).len(), 2);

        // Tombstone memo: a Manager's withdrawal that arrives before the original row still wins.
        let memo_signer = fe_identity::keypair::NodeKeypair::from_bytes(&[7u8; 32]).expect("seed key");
        let memo_did = memo_signer.to_did_key();
        roles.set(&memo_did, RoleLevel::Manager);
        let memo_now = unix_now_ms();
        let mut memo_book = RelayAdvertisementBook::with_signed_mode(RelayAdvertSignedMode::Compat);
        assert!(memo_book.observe_at(VERSE, "r-x", MANAGER, &[], true, roles.role_of(), memo_now).is_empty());
        assert_eq!(memo_book.tombstone_len(), 1);
        let older = signed_row(&memo_signer, "https://memo.example.org", memo_now, |_| {});
        assert!(memo_book.observe_at(VERSE, "r-x", &memo_did, &wire(&older), false, roles.role_of(), memo_now).is_empty());
        assert!(memo_book.urls_by_verse().is_empty() && memo_book.admitted(VERSE).is_empty(), "the late older row stays withdrawn");
        assert_eq!(memo_book.quarantine_len(), 0, "a memo-refused row is never quarantined");
        // A row issued after the memo's receipt is a legitimate re-advertisement and clears the memo.
        let later_now = memo_now + 10_000;
        let newer = signed_row(&memo_signer, "https://memo.example.org", later_now, |_| {});
        assert_eq!(
            memo_book.observe_at(VERSE, "r-x", &memo_did, &wire(&newer), false, roles.role_of(), later_now),
            vec![admitted("r-x", "https://memo.example.org/")]
        );
        assert_eq!(memo_book.tombstone_len(), 0);
        // A Viewer's tombstone leaves no memo, so the following row is admitted.
        assert!(memo_book.observe_at(VERSE, "r-y", VIEWER, &[], true, roles.role_of(), later_now).is_empty());
        assert_eq!(memo_book.tombstone_len(), 0);
        let viewer_case = signed_row(&memo_signer, "https://memo-y.example.org", later_now, |_| {});
        assert_eq!(
            memo_book.observe_at(VERSE, "r-y", &memo_did, &wire(&viewer_case), false, roles.role_of(), later_now),
            vec![admitted("r-y", "https://memo-y.example.org/")]
        );
        // The memo is bounded per verse, oldest evicted first, and the replica closing drops it.
        for index in 0..=RELAY_TOMBSTONE_PER_VERSE_CAP {
            memo_book.observe_at(VERSE, &format!("r-gone-{index}"), MANAGER, &[], true, roles.role_of(), later_now);
        }
        assert_eq!(memo_book.tombstone_len(), RELAY_TOMBSTONE_PER_VERSE_CAP);
        assert_eq!(memo_book.tombstone_evicted(), 1);
        memo_book.close_verse(VERSE);
        assert_eq!(memo_book.tombstone_len(), 0);

        // Closing the replica withdraws everything it contributed.
        assert_eq!(book.close_verse(VERSE).len(), 2);
        assert!(book.urls_by_verse().is_empty());
        assert_eq!(book.quarantine_len(), 0);

        // Flood: a Manager's advert that arrived before its role statement survives non-Manager authors
        // flooding past both caps, and is admitted once the role arrives (delivery-order independence).
        const LATE: &str = "did:key:late-manager";
        for _ in 0..2 {
            let resent = book.observe(VERSE, "r-late", LATE, &row("https://late.example", LATE), false, roles.role_of());
            assert!(resent.is_empty());
        }
        assert_eq!(book.quarantine_len(), 1, "a re-sent row replaces its pending copy");
        let flooders = RELAY_QUARANTINE_CAP / RELAY_QUARANTINE_PER_AUTHOR_CAP + 2;
        for author in 0..flooders {
            let flooder = format!("did:key:flooder-{author}");
            for index in 0..=RELAY_QUARANTINE_PER_AUTHOR_CAP {
                let url = format!("https://flood-{author}-{index}.example");
                let changes = book.observe(VERSE, &format!("r-flood-{index}"), &flooder, &row(&url, &flooder), false, roles.role_of());
                assert!(changes.is_empty());
            }
        }
        assert_eq!(book.quarantine_len(), RELAY_QUARANTINE_CAP);
        assert_eq!(book.quarantine_author_evicted(), flooders as u64);
        assert!(book.quarantine_evicted() > book.quarantine_author_evicted(), "the global cap evicted too");
        roles.set(LATE, RoleLevel::Manager);
        assert_eq!(book.rejudge(LATE, roles.role_of()), vec![admitted("r-late", "https://late.example/")]);
        assert_eq!(book.quarantine_len(), RELAY_QUARANTINE_CAP - 1);

        // NET-22, require mode: only rows signed by the author's own key, bound to this verse,
        // unexpired, within the 30-day TTL and on the strict URL policy are admitted.
        let signer = fe_identity::keypair::NodeKeypair::from_bytes(&[9u8; 32]).expect("seed key");
        let signer_did = signer.to_did_key();
        roles.set(&signer_did, RoleLevel::Manager);
        let now = unix_now_ms();
        let mut strict = RelayAdvertisementBook::with_signed_mode(RelayAdvertSignedMode::Require);
        let observe = |book: &mut RelayAdvertisementBook, record: &str, data: &[u8]| {
            book.observe_at(VERSE, record, &signer_did, data, false, roles.role_of(), now)
        };

        assert!(observe(&mut strict, "r-unsigned", &row("https://plain.example.org", &signer_did)).is_empty());
        let mut tampered = signed_row(&signer, "https://relay-0.example.org", now, |_| {});
        tampered.url = "https://evil.example.org".to_string();
        let refused_rows = [
            ("tampered", tampered),
            ("other verse", signed_row(&signer, "https://relay-0.example.org", now, |row| row.verse_id = Some("verse-b".into()))),
            ("ttl over 30 d", signed_row(&signer, "https://relay-0.example.org", now, |row| row.expires_at_ms = Some(now + MAX_ADVERT_TTL_MS))),
            ("expired", signed_row(&signer, "https://relay-0.example.org", now, |row| row.expires_at_ms = Some(now - 1))),
            ("port policy", signed_row(&signer, "https://relay-0.example.org:8443", now, |_| {})),
            ("issued in the future", signed_row(&signer, "https://relay-0.example.org", now, |row| {
                row.issued_at_ms = Some(now + 2 * ADVERT_ISSUED_SKEW_MS);
                row.expires_at_ms = Some(now + 3 * ADVERT_ISSUED_SKEW_MS);
            })),
        ];
        for (case, refused) in refused_rows {
            assert!(observe(&mut strict, "r-refused", &wire(&refused)).is_empty(), "{case} must be refused");
        }
        assert_eq!(strict.quarantine_len(), 0, "refused rows are never quarantined");

        // The per-verse bound admits the four oldest-issued adverts; the fifth is held back.
        for index in 1..=MAX_ADVERTS_PER_VERSE + 1 {
            let url = format!("https://relay-{index}.example.org");
            let advert = signed_row(&signer, &url, now, |row| row.issued_at_ms = Some(now - 10_000 + index as u64));
            let changes = observe(&mut strict, &format!("r-signed-{index}"), &wire(&advert));
            if index <= MAX_ADVERTS_PER_VERSE {
                assert_eq!(changes, vec![admitted(&format!("r-signed-{index}"), &format!("{url}/"))]);
            } else {
                assert!(changes.is_empty(), "the fifth advert is refused while four are live");
            }
        }
        // Withdrawing one frees a slot for the held-back advert.
        let changes = strict.observe_at(VERSE, "r-signed-1", &signer_did, &[], true, roles.role_of(), now);
        assert_eq!(
            changes,
            vec![
                withdrawn("r-signed-1", "https://relay-1.example.org/"),
                admitted("r-signed-5", "https://relay-5.example.org/"),
            ]
        );

        // The expiry timer withdraws every advert once its signed lifetime ends.
        let expired = strict.expire(now + 120_000);
        assert_eq!(expired.len(), MAX_ADVERTS_PER_VERSE);
        assert!(expired.iter().all(|change| matches!(change, AdvertChange::Withdrawn { .. })));
        assert!(strict.urls_by_verse().is_empty());
    }

    #[test]
    fn relay_advertisement_admission_table() {
        // (author role, row bytes, admitted, quarantined)
        let cases: [(RoleLevel, Vec<u8>, bool, bool); 8] = [
            (RoleLevel::Owner, row("https://r.example", OWNER), true, false),
            (RoleLevel::Manager, row("https://r.example", OWNER), true, false),
            (RoleLevel::Editor, row("https://r.example", OWNER), false, true),
            (RoleLevel::Viewer, row("https://r.example", OWNER), false, true),
            (RoleLevel::None, row("https://r.example", OWNER), false, true),
            (RoleLevel::Owner, row("ftp://r.example", OWNER), false, false),
            (RoleLevel::Owner, row("https://r.example", MANAGER), false, false),
            (RoleLevel::Owner, b"{not json".to_vec(), false, false),
        ];
        for (role, data, expect_admitted, expect_quarantined) in cases {
            let roles = Roles::new(&[(OWNER, role)]);
            let mut book = RelayAdvertisementBook::with_signed_mode(RelayAdvertSignedMode::Compat);
            let changes = book.observe(VERSE, "r1", OWNER, &data, false, roles.role_of());
            assert_eq!(!changes.is_empty(), expect_admitted, "role {role:?}, row {:?}", String::from_utf8_lossy(&data));
            assert_eq!(book.quarantine_len() == 1, expect_quarantined, "role {role:?}");
        }
    }
}
