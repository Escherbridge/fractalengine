//! Read-time, role-gated `__relays` advertisement book (FR-7); see `fe-sync/src/AGENTS.md` §relay-advertise.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use fe_policy::RoleLevel;
use fe_runtime::relay_advertisement::{parse_row, RelayAdvertisementRow, RowParse};

pub use fe_runtime::relay_advertisement::{
    validate_advertised_relay_url, RELAY_ADVERT_ROW_VERSION, RELAY_ADVERT_TABLE,
};

/// Bound on quarantined advertisements across all verses (same posture as `__shards`).
pub const RELAY_QUARANTINE_CAP: usize = 1024;

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

/// One admitted advertisement; `url` is the validator's normalized form.
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

/// Per-verse admitted advertisements keyed by record id, plus a per-author bounded quarantine.
#[derive(Debug, Default)]
pub struct RelayAdvertisementBook {
    admitted: BTreeMap<String, BTreeMap<String, AdmittedAdvert>>,
    /// verse id → author DID → pending adverts, oldest first, one per record id.
    quarantine: BTreeMap<String, BTreeMap<String, VecDeque<QuarantinedAdvert>>>,
    quarantine_len: usize,
    quarantine_arrivals: u64,
    quarantine_evicted: u64,
    quarantine_author_evicted: u64,
}

/// Outcome of the structural checks every row passes before the role gate.
enum Structural {
    Valid { url: String, row: RelayAdvertisementRow },
    Refused,
}

impl RelayAdvertisementBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Judge one inbound `__relays` entry; `author_did` is the `SignedEntry` author, `role_of(did, scope)` the fabric's `role_at`.
    pub fn observe(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        data: &[u8],
        is_tombstone: bool,
        role_of: impl Fn(&str, &str) -> RoleLevel,
    ) -> Vec<AdvertChange> {
        let scope = verse_scope(verse_id);
        let mut changes = Vec::new();

        if is_tombstone || data.is_empty() {
            self.observe_withdrawal(verse_id, record_id, author_did, &scope, &role_of, &mut changes);
            return changes;
        }

        // The author's own earlier pending row for this record is superseded by this one.
        self.remove_pending(verse_id, author_did, record_id);

        let (url, row) = match judge_structure(verse_id, record_id, author_did, data) {
            Structural::Valid { url, row } => (url, row),
            Structural::Refused => {
                // A refused rewrite by the same author retires that author's admission; others' stay.
                if self.admitted_author(verse_id, record_id) == Some(author_did) {
                    self.withdraw(verse_id, record_id, &mut changes);
                }
                return changes;
            }
        };

        if role_of(author_did, &scope).is_at_least(RELAY_ADVERTISER_FLOOR) {
            self.admit(verse_id, record_id, author_did, url, row, &mut changes);
        } else {
            if self.admitted_author(verse_id, record_id) == Some(author_did) {
                self.withdraw(verse_id, record_id, &mut changes);
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
        changes
    }

    /// Re-judge after `did`'s role changed: demoted admissions are withdrawn and quarantined, promoted quarantine admitted.
    pub fn rejudge(
        &mut self,
        did: &str,
        role_of: impl Fn(&str, &str) -> RoleLevel,
    ) -> Vec<AdvertChange> {
        let mut changes = Vec::new();

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
            if let Some(advert) = self.withdraw(&verse_id, &record_id, &mut changes) {
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
                self.admit(&verse_id, &record_id, &author_did, url, row, &mut changes);
            }
        }
        changes
    }

    /// Withdraw every admission and quarantined row of a verse whose replica closed.
    pub fn close_verse(&mut self, verse_id: &str) -> Vec<AdvertChange> {
        if let Some(authors) = self.quarantine.remove(verse_id) {
            self.quarantine_len -= authors.values().map(VecDeque::len).sum::<usize>();
        }
        let Some(records) = self.admitted.remove(verse_id) else {
            return Vec::new();
        };
        records
            .into_iter()
            .map(|(record_id, advert)| AdvertChange::Withdrawn {
                verse_id: verse_id.to_string(),
                record_id,
                url: advert.url,
            })
            .collect()
    }

    /// Normalized admitted relay URLs per verse.
    pub fn urls_by_verse(&self) -> BTreeMap<String, BTreeSet<String>> {
        self.admitted
            .iter()
            .map(|(verse_id, records)| {
                let urls = records.values().map(|advert| advert.url.clone()).collect();
                (verse_id.clone(), urls)
            })
            .collect()
    }

    /// Admitted rows of one verse, in record-id order.
    pub fn admitted(&self, verse_id: &str) -> Vec<&RelayAdvertisementRow> {
        self.admitted
            .get(verse_id)
            .map(|records| records.values().map(|advert| &advert.row).collect())
            .unwrap_or_default()
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

    fn observe_withdrawal(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        scope: &str,
        role_of: &impl Fn(&str, &str) -> RoleLevel,
        changes: &mut Vec<AdvertChange>,
    ) {
        let author_is_manager = role_of(author_did, scope).is_at_least(RELAY_ADVERTISER_FLOOR);
        if author_is_manager {
            self.drop_quarantined_record(verse_id, record_id);
        } else {
            self.remove_pending(verse_id, author_did, record_id);
        }
        let may_withdraw = self
            .admitted_author(verse_id, record_id)
            .map(|admitted_by| admitted_by == author_did || author_is_manager);
        match may_withdraw {
            Some(true) => {
                self.withdraw(verse_id, record_id, changes);
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

    fn admit(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_did: &str,
        url: String,
        row: RelayAdvertisementRow,
        changes: &mut Vec<AdvertChange>,
    ) {
        let records = self.admitted.entry(verse_id.to_string()).or_default();
        let previous = records.insert(
            record_id.to_string(),
            AdmittedAdvert {
                author_did: author_did.to_string(),
                url: url.clone(),
                row,
            },
        );
        match previous {
            Some(previous) if previous.url == url => {}
            Some(previous) => {
                changes.push(AdvertChange::Withdrawn {
                    verse_id: verse_id.to_string(),
                    record_id: record_id.to_string(),
                    url: previous.url,
                });
                changes.push(AdvertChange::Admitted {
                    verse_id: verse_id.to_string(),
                    record_id: record_id.to_string(),
                    url,
                });
            }
            None => changes.push(AdvertChange::Admitted {
                verse_id: verse_id.to_string(),
                record_id: record_id.to_string(),
                url,
            }),
        }
    }

    fn withdraw(
        &mut self,
        verse_id: &str,
        record_id: &str,
        changes: &mut Vec<AdvertChange>,
    ) -> Option<AdmittedAdvert> {
        let records = self.admitted.get_mut(verse_id)?;
        let advert = records.remove(record_id)?;
        if records.is_empty() {
            self.admitted.remove(verse_id);
        }
        changes.push(AdvertChange::Withdrawn {
            verse_id: verse_id.to_string(),
            record_id: record_id.to_string(),
            url: advert.url.clone(),
        });
        Some(advert)
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

/// Version, attribution and URL checks; refusals warn and are never quarantined.
fn judge_structure(verse_id: &str, record_id: &str, author_did: &str, data: &[u8]) -> Structural {
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
    match validate_advertised_relay_url(&row.url) {
        Ok(url) => Structural::Valid {
            url: url.to_string(),
            row,
        },
        Err(error) => {
            tracing::warn!(verse_id, record_id, %error, "relay advertisement refused: invalid relay url");
            Structural::Refused
        }
    }
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

    #[test]
    fn relay_advertisement_book_journey() {
        let roles = Roles::new(&[
            (OWNER, RoleLevel::Owner),
            (MANAGER, RoleLevel::Manager),
            (VIEWER, RoleLevel::Viewer),
        ]);
        let mut book = RelayAdvertisementBook::new();

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
        let extra = br#"{"v":1,"url":"https://extra.example","advertised_by":"did:key:owner","sig":"abc","expires_at":9}"#;
        assert_eq!(
            book.observe(VERSE, "r-extra", OWNER, extra, false, roles.role_of()),
            vec![admitted("r-extra", "https://extra.example/")]
        );

        let urls: Vec<String> = book.urls_by_verse()[VERSE].iter().cloned().collect();
        assert_eq!(urls, vec!["https://extra.example/".to_string(), "https://viewer.example/".to_string()]);
        assert_eq!(book.admitted(VERSE).len(), 2);

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
            let mut book = RelayAdvertisementBook::new();
            let changes = book.observe(VERSE, "r1", OWNER, &data, false, roles.role_of());
            assert_eq!(!changes.is_empty(), expect_admitted, "role {role:?}, row {:?}", String::from_utf8_lossy(&data));
            assert_eq!(book.quarantine_len() == 1, expect_quarantined, "role {role:?}");
        }
    }
}
