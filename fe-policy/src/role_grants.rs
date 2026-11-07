//! Pure per-grantor role-statement model — see fe-policy/AGENTS.md §role-grants.
//! Algorithm: least fixpoint via Kleene iteration over a monotone operator on
//! a finite lattice (Knaster-Tarski).

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::role_level::RoleLevel;

/// The four roles a statement can grant (`owner` is structurally excluded —
/// nobody can grant Owner; T3). Declared in ascending rank so the derived
/// `Ord` gives `None < Viewer < Editor < Manager`, matching the `max()` used
/// by the materializer's step 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantedRole {
    None,
    Viewer,
    Editor,
    Manager,
}

impl GrantedRole {
    /// Strict parse: only the four wire strings. `"owner"` and anything else
    /// is rejected, never silently mapped the way `RoleLevel::from` maps an
    /// unknown string to `None` (§2.2).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manager" => Some(GrantedRole::Manager),
            "editor" => Some(GrantedRole::Editor),
            "viewer" => Some(GrantedRole::Viewer),
            "none" => Some(GrantedRole::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GrantedRole::Manager => "manager",
            GrantedRole::Editor => "editor",
            GrantedRole::Viewer => "viewer",
            GrantedRole::None => "none",
        }
    }

    pub fn as_role_level(self) -> RoleLevel {
        match self {
            GrantedRole::Manager => RoleLevel::Manager,
            GrantedRole::Editor => RoleLevel::Editor,
            GrantedRole::Viewer => RoleLevel::Viewer,
            GrantedRole::None => RoleLevel::None,
        }
    }
}

impl fmt::Display for GrantedRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A role statement: `grantor_did` grants (or revokes) `role` to `peer_did`
/// at `scope`, stamped with the grantor's local `hlc`. One doc key per
/// `(scope, peer_did, grantor_did)` (§2.1); each grantor's own statements
/// resolve last-writer-wins by `hlc` ([`latest_per_grantor`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleStatement {
    pub scope: String,
    pub peer_did: String,
    pub grantor_did: String,
    pub role: GrantedRole,
    pub revoked: bool,
    pub hlc: u64,
    /// HLC since which this key has granted at least Editor without a break
    /// (`None` = `hlc`) — QA-0131 write-time judging, AGENTS.md §write-time-role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_since: Option<u64>,
    /// HLC since which this key has granted Manager without a break (`None` = `hlc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_since: Option<u64>,
}

impl RoleStatement {
    /// Since when this key has continuously granted at least `level`
    /// (Editor or Manager), or `None` when it does not grant it now.
    pub fn granted_since(&self, level: GrantedRole) -> Option<u64> {
        if self.revoked || self.role < level {
            return None;
        }
        let manager = self.manager_since.unwrap_or(self.hlc);
        match level {
            GrantedRole::Manager => Some(manager),
            _ if self.role == GrantedRole::Manager => {
                Some(self.editor_since.unwrap_or(self.hlc).min(manager))
            }
            _ => Some(self.editor_since.unwrap_or(self.hlc)),
        }
    }

    /// This NEWER version of a key, carrying the continuity of `prior` (the
    /// key's previous live version): a raise or lower that stays at Editor+
    /// (or Manager) keeps its original `since`; a break restarts it.
    pub fn continuing(mut self, prior: Option<&RoleStatement>) -> Self {
        let prior = prior.filter(|prior| prior.hlc < self.hlc);
        let editor = prior
            .and_then(|prior| prior.granted_since(GrantedRole::Editor))
            .filter(|_| self.granted_since(GrantedRole::Editor).is_some());
        let manager = prior
            .and_then(|prior| prior.granted_since(GrantedRole::Manager))
            .filter(|_| self.granted_since(GrantedRole::Manager).is_some());
        self.editor_since = editor;
        self.manager_since = manager;
        self
    }

    /// What this key granted at HLC `at`, as far as its continuity shows: an
    /// Editor+ grant counts from its `since`, anything else from its `hlc`;
    /// `None` when the statement is not known to have applied at `at`.
    fn as_of(&self, at: u64) -> Option<RoleStatement> {
        let Some(editor_since) = self.granted_since(GrantedRole::Editor) else {
            return (self.hlc <= at).then(|| self.clone());
        };
        let role = match self.granted_since(GrantedRole::Manager) {
            Some(since) if since <= at => GrantedRole::Manager,
            _ if editor_since <= at => GrantedRole::Editor,
            _ => return None,
        };
        Some(RoleStatement {
            role,
            ..self.clone()
        })
    }
}

/// The three parsed segments of a `role` table doc key
/// (`role/{scope}/{peer_did}/{grantor_did}`; any `role/` table prefix is
/// stripped by the caller before `parse`). Scopes and did:key strings never
/// contain `/`, so a valid key parses as exactly three non-empty segments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StatementKey {
    pub scope: String,
    pub peer_did: String,
    pub grantor_did: String,
}

impl StatementKey {
    pub fn parse(record_id: &str) -> Result<Self, RejectReason> {
        let parts: Vec<&str> = record_id.split('/').collect();
        if parts.len() != 3 {
            return Err(RejectReason::MalformedKey);
        }
        let (scope, peer_did, grantor_did) = (parts[0], parts[1], parts[2]);
        if scope.is_empty() || peer_did.is_empty() || grantor_did.is_empty() {
            return Err(RejectReason::MalformedKey);
        }
        Ok(StatementKey {
            scope: scope.to_string(),
            peer_did: peer_did.to_string(),
            grantor_did: grantor_did.to_string(),
        })
    }
}

/// The raw wire payload before validation: `role` is still a string (never
/// pre-mapped through `RoleLevel::from`), so [`validate_statement`] can
/// reject `"owner"` and unknown strings outright instead of silently
/// downgrading them to `None` (§2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleStatementPayload {
    pub scope: String,
    pub peer_did: String,
    pub grantor_did: String,
    pub role: String,
    pub revoked: bool,
    pub hlc: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_since: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_since: Option<u64>,
}

/// Why [`validate_statement`] denied a statement. Every variant is a
/// **permanent** deny — none of these are retried (contrast with the
/// Deferred/held path of §2.4, which lives above this pure layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// T1: the key's `grantor_did` does not match the attested entry author.
    GrantorMismatch,
    /// The payload's `scope`/`peer_did`/`grantor_did` disagree with the key.
    PayloadKeyMismatch,
    /// The doc key did not parse as exactly three non-empty `/`-separated segments.
    MalformedKey,
    /// The scope string does not parse as `VERSE#<v>[-FRACTAL#<f>[-PETAL#<p>]]`.
    MalformedScope,
    /// T6: the scope's verse does not match the replica this statement arrived on.
    CrossVerseScope,
    /// T3: `role` is not one of `manager`/`editor`/`viewer`/`none` — this is
    /// also how `"owner"` is rejected, never mapped through `RoleLevel::from`.
    InvalidRole,
    /// T5: `peer_did` names the owner. The owner is never a grant target.
    TargetsOwner,
    /// A continuity field (`editor_since`/`manager_since`) on a statement that
    /// does not grant that level, or later than the statement's own `hlc`.
    InvalidContinuity,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RejectReason::GrantorMismatch => "statement grantor does not match the attested author",
            RejectReason::PayloadKeyMismatch => "payload fields disagree with the statement key",
            RejectReason::MalformedKey => "doc key did not parse as scope/peer_did/grantor_did",
            RejectReason::MalformedScope => "scope string did not parse",
            RejectReason::CrossVerseScope => "scope's verse does not match this replica",
            RejectReason::InvalidRole => "role is not manager, editor, viewer or none",
            RejectReason::TargetsOwner => "the owner can never be a grant target",
            RejectReason::InvalidContinuity => {
                "continuity field does not match the granted role or follows the statement"
            }
        };
        write!(f, "{}", s)
    }
}

impl std::error::Error for RejectReason {}

/// `S` followed by its ancestors up to (and including) the verse scope —
/// `chain(S)` in §2.2. Walks via `crate::scope::parent_scope`, the single
/// scope-chain parser (see AGENTS.md §scope-parser — W5b/OF-3 unified this
/// with what used to be a private duplicate here).
fn scope_chain(scope: &str) -> Vec<String> {
    let mut chain = vec![scope.to_string()];
    let mut current = scope.to_string();
    while let Some(parent) = crate::scope::parent_scope(&current) {
        chain.push(parent.clone());
        current = parent;
    }
    chain
}

/// Checks a statement for permanent validity (§2.2's bulleted list). Any
/// failure here is a **permanent deny** — never retried. The owner-dependent
/// check (target-is-owner) needs the manifest: with `owner_did: None`
/// (pre-manifest), this runs only the owner-independent checks, and the
/// full check must run again when the statement is promoted once the owner
/// is known (§2.4).
pub fn validate_statement(
    replica_verse: &str,
    owner_did: Option<&str>,
    key_parts: &StatementKey,
    author_did: &str,
    payload: &RoleStatementPayload,
) -> Result<RoleStatement, RejectReason> {
    if key_parts.grantor_did != author_did {
        return Err(RejectReason::GrantorMismatch);
    }
    if payload.scope != key_parts.scope
        || payload.peer_did != key_parts.peer_did
        || payload.grantor_did != key_parts.grantor_did
    {
        return Err(RejectReason::PayloadKeyMismatch);
    }

    let scope_parts =
        crate::scope::parse_scope(&key_parts.scope).map_err(|_| RejectReason::MalformedScope)?;
    if scope_parts.verse_id != replica_verse {
        return Err(RejectReason::CrossVerseScope);
    }

    let role = GrantedRole::parse(&payload.role).ok_or(RejectReason::InvalidRole)?;

    if let Some(owner) = owner_did {
        if key_parts.peer_did == owner {
            return Err(RejectReason::TargetsOwner);
        }
    }

    let statement = RoleStatement {
        scope: key_parts.scope.clone(),
        peer_did: key_parts.peer_did.clone(),
        grantor_did: key_parts.grantor_did.clone(),
        role,
        revoked: payload.revoked,
        hlc: payload.hlc,
        editor_since: payload.editor_since,
        manager_since: payload.manager_since,
    };
    for (since, level) in [
        (statement.editor_since, GrantedRole::Editor),
        (statement.manager_since, GrantedRole::Manager),
    ] {
        if since.is_some_and(|since| {
            since > statement.hlc || statement.revoked || statement.role < level
        }) {
            return Err(RejectReason::InvalidContinuity);
        }
    }
    Ok(statement)
}

/// Collapses statements for the same `(scope, peer_did, grantor_did)` key
/// down to the single latest one: last-writer-wins by `hlc`; on a tie, the
/// lexicographically larger canonical payload **bytes** win (deterministic,
/// no hash dependency — critic tick 8 AC-51).
pub fn latest_per_grantor<'a, I>(statements: I) -> Vec<RoleStatement>
where
    I: IntoIterator<Item = &'a RoleStatement>,
{
    let mut by_key: HashMap<(String, String, String), &RoleStatement> = HashMap::new();
    for stmt in statements {
        let key = (
            stmt.scope.clone(),
            stmt.peer_did.clone(),
            stmt.grantor_did.clone(),
        );
        match by_key.get(&key).copied() {
            Some(existing) if !is_newer(stmt, existing) => {}
            _ => {
                by_key.insert(key, stmt);
            }
        }
    }
    by_key.into_values().cloned().collect()
}

fn is_newer(candidate: &RoleStatement, existing: &RoleStatement) -> bool {
    match candidate.hlc.cmp(&existing.hlc) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => canonical_bytes(candidate) > canonical_bytes(existing),
    }
}

/// A deterministic byte encoding of a statement's payload fields, used only
/// for the AC-51 tie-break — not a wire format, and never hashed.
fn canonical_bytes(stmt: &RoleStatement) -> Vec<u8> {
    let mut bytes = format!(
        "{}|{}|{}|{}|{}|{}",
        stmt.scope,
        stmt.peer_did,
        stmt.grantor_did,
        stmt.role.as_str(),
        stmt.revoked,
        stmt.hlc
    );
    if let Some(since) = stmt.editor_since {
        bytes.push_str(&format!("|e{since}"));
    }
    if let Some(since) = stmt.manager_since {
        bytes.push_str(&format!("|m{since}"));
    }
    bytes.into_bytes()
}

/// A materialized cache row: `resolve_role` walks to the most specific of
/// these and that returns exactly `role_at` (§2.2 step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRow {
    pub scope: String,
    pub peer_did: String,
    pub role: RoleLevel,
}

/// Each DID whose authority moved, with the direction — the input to the
/// §2.4 demote/promote partition worklist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedDids {
    pub risen: Vec<String>,
    pub fallen: Vec<String>,
}

/// The materialized result of [`materialize`]: owner pins and blocks, plus
/// the `mgr` map raised by Kleene iteration. Exposes the shared predicates
/// both the DB and sync planes gate on (§2.2, §2.5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectiveRoles {
    owner_did: Option<String>,
    /// Owner grant: an exact pinned value at `(scope, peer_did)`.
    owner_pins: HashMap<(String, String), GrantedRole>,
    /// Owner revoke: the cell is blocked (no explicit value; the peer
    /// inherits from the enclosing scope, skipping this cell's own
    /// Manager grants).
    owner_blocks: HashSet<(String, String)>,
    /// `mgr(S, P)`: the maximum valid Manager grant on each cell.
    mgr: HashMap<(String, String), GrantedRole>,
}

impl EffectiveRoles {
    /// `role_at(P, S)`: `P`'s effective role at scope `S`, or
    /// `RoleLevel::None` (⊥) if nothing grants them one. Callers that gate
    /// content fall back to `default_access` on `None` (§2.2).
    pub fn role_at(&self, peer_did: &str, scope: &str) -> RoleLevel {
        if self.owner_did.as_deref() == Some(peer_did) {
            return RoleLevel::Owner;
        }
        let mut acc: Option<GrantedRole> = None;
        for cell_scope in scope_chain(scope) {
            let key = (cell_scope, peer_did.to_string());
            if let Some(pin) = self.owner_pins.get(&key) {
                acc = Some(acc.map_or(*pin, |a| a.max(*pin)));
                break;
            }
            if self.owner_blocks.contains(&key) {
                continue;
            }
            if let Some(mgr_val) = self.mgr.get(&key) {
                acc = Some(acc.map_or(*mgr_val, |a| a.max(*mgr_val)));
            }
        }
        acc.map_or(RoleLevel::None, GrantedRole::as_role_level)
    }

    /// `authority(G, S) = role_at(G, S)` — named separately because callers
    /// read it as "can G act as Manager+ here", while `role_at` reads as
    /// "what does P effectively hold here". Same computation (§2.2).
    pub fn authority(&self, grantor_did: &str, scope: &str) -> RoleLevel {
        self.role_at(grantor_did, scope)
    }

    /// The one placement predicate both planes use (§2.2 "Admission"): true
    /// for the owner, and otherwise iff the grantor's authority at the
    /// statement's scope is at least Manager under this materialization.
    pub fn admissible(&self, statement: &RoleStatement) -> bool {
        if self.owner_did.as_deref() == Some(statement.grantor_did.as_str()) {
            return true;
        }
        self.authority(&statement.grantor_did, &statement.scope)
            .is_at_least(RoleLevel::Manager)
    }

    /// True if `did` holds any effective role at any scope of the verse —
    /// including a petal-scoped grant that resolves to `None` at the wider
    /// verse scope. Backs the RP-3 enrolment membership floor (AC-44c).
    pub fn has_any_role(&self, did: &str) -> bool {
        if self.owner_did.as_deref() == Some(did) {
            return true;
        }
        self.owner_pins
            .iter()
            .any(|((_, peer), role)| peer == did && *role != GrantedRole::None)
            || self
                .mgr
                .iter()
                .any(|((_, peer), role)| peer == did && *role != GrantedRole::None)
    }

    /// §2.2 step 4's cache projection: a row for `(S, P)` when the cell is
    /// owner-pinned (value: the pin), or when it is not blocked and
    /// `mgr(S,P) > role_at(P, parent(S))` (value: `mgr(S,P)`). The hot-path
    /// `resolve_role` (most-specific row wins) then returns exactly
    /// `role_at` without needing to know about pins, blocks or `mgr` at all.
    pub fn cache_rows(&self) -> Vec<CacheRow> {
        let mut cells: HashSet<(String, String)> = HashSet::new();
        cells.extend(self.owner_pins.keys().cloned());
        cells.extend(self.owner_blocks.iter().cloned());
        cells.extend(self.mgr.keys().cloned());

        let mut rows = Vec::new();
        for (scope, peer) in cells {
            let key = (scope.clone(), peer.clone());
            if let Some(pin) = self.owner_pins.get(&key) {
                rows.push(CacheRow {
                    scope,
                    peer_did: peer,
                    role: pin.as_role_level(),
                });
                continue;
            }
            if self.owner_blocks.contains(&key) {
                continue;
            }
            if let Some(mgr_val) = self.mgr.get(&key) {
                let inherited = match crate::scope::parent_scope(&scope) {
                    Some(parent) => self.role_at(&peer, &parent),
                    None => RoleLevel::None,
                };
                if mgr_val.as_role_level() > inherited {
                    rows.push(CacheRow {
                        scope,
                        peer_did: peer,
                        role: mgr_val.as_role_level(),
                    });
                }
            }
        }
        rows
    }

    /// Reports every DID whose effective role ROSE at any cell (`scope`,
    /// `did`) either snapshot mentions (pins, blocks, Manager grants) in
    /// `risen`, and every DID whose role FELL at any such cell in `fallen`;
    /// an owner change counts the new owner risen and the old one fallen.
    /// Per cell, never "best across scopes" (refine R3): a Manager at
    /// fractal A newly empowered at fractal B must still be risen, or the
    /// §2.5 re-read on empowerment never recovers what it holds at B. A DID
    /// may appear in both lists. The §2.4 worklist seed — a caller re-checks
    /// `admissible` per statement; this only narrows which DIDs to re-check.
    pub fn diff(&self, previous: &EffectiveRoles) -> ChangedDids {
        let mut cells: std::collections::BTreeSet<(&str, &str)> = std::collections::BTreeSet::new();
        for roles in [self, previous] {
            let keyed = roles
                .owner_pins
                .keys()
                .chain(roles.owner_blocks.iter())
                .chain(roles.mgr.keys());
            cells.extend(keyed.map(|(scope, peer)| (scope.as_str(), peer.as_str())));
        }

        let mut risen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut fallen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (scope, did) in cells {
            match self.role_at(did, scope).cmp(&previous.role_at(did, scope)) {
                std::cmp::Ordering::Greater => {
                    risen.insert(did.to_string());
                }
                std::cmp::Ordering::Less => {
                    fallen.insert(did.to_string());
                }
                std::cmp::Ordering::Equal => {}
            }
        }
        if self.owner_did != previous.owner_did {
            if let Some(owner) = &self.owner_did {
                risen.insert(owner.clone());
            }
            if let Some(owner) = &previous.owner_did {
                fallen.insert(owner.clone());
            }
        }
        ChangedDids {
            risen: risen.into_iter().collect(),
            fallen: fallen.into_iter().collect(),
        }
    }
}

/// Computes the least fixpoint of role statements for one verse: owner pins
/// and blocks are fixed first (step 1), then Manager statements are raised
/// by Kleene iteration from `mgr ≡ ⊥` until a pass changes nothing (steps
/// 2-3; bounded at `|statements|+1` passes, with a `tracing::warn!` bug
/// guard if the bound is hit — plan §5 risk 1). See fe-policy/AGENTS.md
/// §role-grants and plan §2.2.
pub fn materialize<'a, I>(owner_did: &str, statements: I) -> EffectiveRoles
where
    I: IntoIterator<Item = &'a RoleStatement>,
{
    let all: Vec<&RoleStatement> = statements.into_iter().collect();
    let mut result = EffectiveRoles {
        owner_did: Some(owner_did.to_string()),
        ..Default::default()
    };

    let mut manager_statements: Vec<&RoleStatement> = Vec::new();
    for stmt in &all {
        if stmt.grantor_did == owner_did {
            let cell = (stmt.scope.clone(), stmt.peer_did.clone());
            if stmt.revoked {
                result.owner_blocks.insert(cell.clone());
                result.owner_pins.remove(&cell);
            } else {
                result.owner_pins.insert(cell.clone(), stmt.role);
                result.owner_blocks.remove(&cell);
            }
        } else {
            manager_statements.push(stmt);
        }
    }

    let max_passes = all.len() + 1;
    let mut reached_fixpoint = false;
    for _ in 0..max_passes {
        let mut changed = false;
        for stmt in &manager_statements {
            if stmt.revoked || stmt.role == GrantedRole::None {
                continue;
            }
            let cell = (stmt.scope.clone(), stmt.peer_did.clone());
            if result.owner_pins.contains_key(&cell) || result.owner_blocks.contains(&cell) {
                continue;
            }
            if !result
                .authority(&stmt.grantor_did, &stmt.scope)
                .is_at_least(RoleLevel::Manager)
            {
                continue;
            }
            let current = result.mgr.get(&cell).copied().unwrap_or(GrantedRole::None);
            let raised = current.max(stmt.role);
            if raised != current {
                result.mgr.insert(cell, raised);
                changed = true;
            }
        }
        if !changed {
            reached_fixpoint = true;
            break;
        }
    }
    if !reached_fixpoint && !manager_statements.is_empty() {
        tracing::warn!(
            passes = max_passes,
            statements = manager_statements.len(),
            "fe_policy::role_grants::materialize did not reach a fixpoint within the bound \
             (|statements|+1 passes) — this is a bug guard, see plan §5 risk 1"
        );
    }

    result
}

/// [`materialize`] as of HLC `at`: each statement counts only from the time
/// its continuity shows it applied ([`RoleStatement::granted_since`]), so a
/// row stamped `at` is judged by the role its author held when it was
/// written (QA-0131). Callers AND it with the current view —
/// AGENTS.md §write-time-role.
pub fn materialize_at<'a, I>(owner_did: &str, statements: I, at: u64) -> EffectiveRoles
where
    I: IntoIterator<Item = &'a RoleStatement>,
{
    let as_of: Vec<RoleStatement> = statements
        .into_iter()
        .filter_map(|statement| statement.as_of(at))
        .collect();
    materialize(owner_did, as_of.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(
        scope: &str,
        peer: &str,
        grantor: &str,
        role: GrantedRole,
        revoked: bool,
        hlc: u64,
    ) -> RoleStatement {
        RoleStatement {
            scope: scope.to_string(),
            peer_did: peer.to_string(),
            grantor_did: grantor.to_string(),
            role,
            revoked,
            hlc,
            editor_since: None,
            manager_since: None,
        }
    }

    /// A deterministic Fisher-Yates shuffle seeded by `seed`, using a small
    /// hand-rolled LCG — no `rand`/`proptest` dependency (critic tick 8,
    /// AC-51: `fe-policy/Cargo.toml` carries neither today).
    fn seeded_shuffle<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
        let mut state = seed
            .wrapping_mul(2_862_933_555_777_941_757)
            .wrapping_add(3_037_000_493);
        let mut indices: Vec<usize> = (0..items.len()).collect();
        for i in (1..indices.len()).rev() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let j = ((state >> 33) as usize) % (i + 1);
            indices.swap(i, j);
        }
        indices.into_iter().map(|i| items[i].clone()).collect()
    }

    const OWNER: &str = "did:key:owner";

    #[test]
    fn statement_key_parses_exactly_three_segments() {
        let key = StatementKey::parse("VERSE#v1-FRACTAL#f1-PETAL#p1/did:key:peer/did:key:grantor")
            .unwrap();
        assert_eq!(key.scope, "VERSE#v1-FRACTAL#f1-PETAL#p1");
        assert_eq!(key.peer_did, "did:key:peer");
        assert_eq!(key.grantor_did, "did:key:grantor");

        assert_eq!(
            StatementKey::parse("too/few"),
            Err(RejectReason::MalformedKey)
        );
        assert_eq!(
            StatementKey::parse("way/too/many/segments"),
            Err(RejectReason::MalformedKey)
        );
    }

    #[test]
    fn latest_per_grantor_breaks_equal_hlc_ties_on_canonical_payload_bytes() {
        let a = stmt(
            "VERSE#v1",
            "did:key:p",
            "did:key:g",
            GrantedRole::Editor,
            false,
            5,
        );
        let b = stmt(
            "VERSE#v1",
            "did:key:p",
            "did:key:g",
            GrantedRole::Viewer,
            false,
            5,
        );
        let forward = latest_per_grantor(&[a.clone(), b.clone()]);
        let reversed = latest_per_grantor(&[b, a]);
        assert_eq!(forward.len(), 1);
        assert_eq!(
            forward, reversed,
            "the tie-break must not depend on arrival order"
        );
    }

    #[test]
    fn a_statement_whose_key_names_another_grantor_is_rejected() {
        let key = StatementKey {
            scope: "VERSE#v1".into(),
            peer_did: "did:key:target".into(),
            grantor_did: "did:key:claimed-grantor".into(),
        };
        let payload = RoleStatementPayload {
            scope: key.scope.clone(),
            peer_did: key.peer_did.clone(),
            grantor_did: key.grantor_did.clone(),
            role: "editor".into(),
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let result = validate_statement("v1", None, &key, "did:key:actual-author", &payload);
        assert_eq!(result, Err(RejectReason::GrantorMismatch));
    }

    #[test]
    fn a_cross_verse_scope_is_rejected() {
        let key = StatementKey {
            scope: "VERSE#other-verse-FRACTAL#f1".into(),
            peer_did: "did:key:target".into(),
            grantor_did: "did:key:grantor".into(),
        };
        let payload = RoleStatementPayload {
            scope: key.scope.clone(),
            peer_did: key.peer_did.clone(),
            grantor_did: key.grantor_did.clone(),
            role: "editor".into(),
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let result = validate_statement("v1", None, &key, "did:key:grantor", &payload);
        assert_eq!(result, Err(RejectReason::CrossVerseScope));
    }

    #[test]
    fn a_manager_cannot_grant_owner_and_an_owner_grant_statement_is_rejected() {
        let key = StatementKey {
            scope: "VERSE#v1".into(),
            peer_did: "did:key:target".into(),
            grantor_did: "did:key:some-manager".into(),
        };
        let payload = RoleStatementPayload {
            scope: key.scope.clone(),
            peer_did: key.peer_did.clone(),
            grantor_did: key.grantor_did.clone(),
            role: "owner".into(),
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let result = validate_statement("v1", Some(OWNER), &key, "did:key:some-manager", &payload);
        assert_eq!(result, Err(RejectReason::InvalidRole));
    }

    #[test]
    fn statements_targeting_the_owner_are_rejected() {
        let key = StatementKey {
            scope: "VERSE#v1".into(),
            peer_did: OWNER.into(),
            grantor_did: "did:key:some-manager".into(),
        };
        let payload = RoleStatementPayload {
            scope: key.scope.clone(),
            peer_did: key.peer_did.clone(),
            grantor_did: key.grantor_did.clone(),
            role: "viewer".into(),
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let result = validate_statement("v1", Some(OWNER), &key, "did:key:some-manager", &payload);
        assert_eq!(result, Err(RejectReason::TargetsOwner));

        // Pre-manifest (owner unknown) the check cannot run yet: held, not denied.
        let held = validate_statement("v1", None, &key, "did:key:some-manager", &payload);
        assert!(
            held.is_ok(),
            "pre-manifest, the owner-dependent check defers rather than permanently denying"
        );
    }

    #[test]
    fn grant_authority_is_the_grantors_own_subtree() {
        // A petal Manager's authority is confined to that petal.
        let petal = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:petal-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            )],
        );
        assert_eq!(
            petal.authority("did:key:petal-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Manager
        );
        assert_eq!(
            petal.authority("did:key:petal-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p2"),
            RoleLevel::None,
            "a sibling petal is out of reach"
        );
        assert_eq!(
            petal.authority("did:key:petal-mgr", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::None,
            "the enclosing fractal is out of reach"
        );
        assert_eq!(
            petal.authority("did:key:petal-mgr", "VERSE#v1"),
            RoleLevel::None,
            "the verse is out of reach"
        );

        // A fractal Manager's authority covers every petal of that fractal, and nowhere else.
        let fractal = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1-FRACTAL#f1",
                "did:key:fractal-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            )],
        );
        assert_eq!(
            fractal.authority("did:key:fractal-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Manager
        );
        assert_eq!(
            fractal.authority("did:key:fractal-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p2"),
            RoleLevel::Manager
        );
        assert_eq!(
            fractal.authority("did:key:fractal-mgr", "VERSE#v1-FRACTAL#f2"),
            RoleLevel::None,
            "a sibling fractal is out of reach"
        );
        assert_eq!(
            fractal.authority("did:key:fractal-mgr", "VERSE#v1"),
            RoleLevel::None,
            "the verse is out of reach"
        );

        // A verse Manager's (or the owner's) authority covers the whole verse.
        let verse = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1",
                "did:key:verse-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            )],
        );
        assert_eq!(
            verse.authority("did:key:verse-mgr", "VERSE#v1"),
            RoleLevel::Manager
        );
        assert_eq!(
            verse.authority("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::Manager
        );
        assert_eq!(
            verse.authority("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Manager
        );
        assert_eq!(
            verse.authority("did:key:verse-mgr", "VERSE#v1-FRACTAL#f2-PETAL#p9"),
            RoleLevel::Manager
        );
    }

    #[test]
    fn an_owner_pin_below_manager_carves_a_petal_out_of_a_verse_managers_authority() {
        let statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:verse-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            // The owner pins the verse Manager down to Viewer specifically in petal p1.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:verse-mgr",
                OWNER,
                GrantedRole::Viewer,
                false,
                2,
            ),
            // The demoted Manager tries to appoint a petal Manager in p1 to undo the carve-out.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:puppet",
                "did:key:verse-mgr",
                GrantedRole::Manager,
                false,
                3,
            ),
        ];
        let result = materialize(OWNER, &statements);
        assert_eq!(
            result.role_at("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Viewer,
            "the owner pin wins over the wider verse grant"
        );
        assert_eq!(
            result.authority("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Viewer,
            "pinned-down authority cannot re-appoint"
        );
        assert_eq!(
            result.role_at("did:key:puppet", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::None,
            "the puppet grant is never admitted: its grantor lacked Manager authority there"
        );
        assert_eq!(
            result.role_at("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::Manager,
            "elsewhere in the verse the demoted manager's authority is untouched"
        );
    }

    #[test]
    fn a_lower_scope_manager_cannot_demote_a_wider_scoped_peer() {
        let statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:verse-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:petal-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                2,
            ),
            // The petal Manager writes an explicit, LOWER statement for the verse Manager
            // in their own petal.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:verse-mgr",
                "did:key:petal-mgr",
                GrantedRole::Viewer,
                false,
                3,
            ),
        ];
        let result = materialize(OWNER, &statements);
        assert_eq!(
            result.role_at("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Manager,
            "Managers only raise: the lower statement has no effect"
        );
        assert!(
            !result
                .cache_rows()
                .iter()
                .any(|r| r.scope == "VERSE#v1-FRACTAL#f1-PETAL#p1"
                    && r.peer_did == "did:key:verse-mgr"),
            "no cache row shadows the inherited Manager role"
        );

        // A Manager-issued `none` statement likewise has no effect.
        let statements_none = vec![
            stmt(
                "VERSE#v1",
                "did:key:verse-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:petal-mgr",
                OWNER,
                GrantedRole::Manager,
                false,
                2,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:verse-mgr",
                "did:key:petal-mgr",
                GrantedRole::None,
                false,
                3,
            ),
        ];
        let result2 = materialize(OWNER, &statements_none);
        assert_eq!(
            result2.role_at("did:key:verse-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
            RoleLevel::Manager
        );
    }

    #[test]
    fn owner_revocation_of_a_manager_voids_their_concurrent_grant_in_every_order() {
        let grant_manager = stmt(
            "VERSE#v1",
            "did:key:m1",
            OWNER,
            GrantedRole::Manager,
            false,
            100,
        );
        let revoke_manager = stmt(
            "VERSE#v1",
            "did:key:m1",
            OWNER,
            GrantedRole::Manager,
            true,
            200,
        );
        let owner_latest = latest_per_grantor(&[grant_manager, revoke_manager]);
        assert_eq!(owner_latest.len(), 1);
        assert!(owner_latest[0].revoked);

        // M concurrently grants X at the fractal, and X appoints a petal Manager under them.
        let m_grants_x = stmt(
            "VERSE#v1-FRACTAL#f1",
            "did:key:x",
            "did:key:m1",
            GrantedRole::Manager,
            false,
            150,
        );
        let x_appoints_petal_manager = stmt(
            "VERSE#v1-FRACTAL#f1-PETAL#p1",
            "did:key:petal-mgr",
            "did:key:x",
            GrantedRole::Manager,
            false,
            160,
        );
        let statements = vec![
            owner_latest[0].clone(),
            m_grants_x,
            x_appoints_petal_manager,
        ];

        for seed in 0..10u64 {
            let shuffled = seeded_shuffle(&statements, seed);
            let result = materialize(OWNER, &shuffled);
            assert_eq!(
                result.role_at("did:key:m1", "VERSE#v1"),
                RoleLevel::None,
                "seed {seed}"
            );
            assert_eq!(
                result.role_at("did:key:x", "VERSE#v1-FRACTAL#f1"),
                RoleLevel::None,
                "seed {seed}: the cascade must void X's grant"
            );
            assert_eq!(
                result.role_at("did:key:petal-mgr", "VERSE#v1-FRACTAL#f1-PETAL#p1"),
                RoleLevel::None,
                "seed {seed}: the cascade reaches the subtree Manager X appointed"
            );
        }
    }

    #[test]
    fn mutual_manager_revocation_has_a_unique_outcome() {
        let statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:m1",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1",
                "did:key:m2",
                OWNER,
                GrantedRole::Manager,
                false,
                2,
            ),
            stmt(
                "VERSE#v1",
                "did:key:m2",
                "did:key:m1",
                GrantedRole::Manager,
                false,
                3,
            ),
            stmt(
                "VERSE#v1",
                "did:key:m1",
                "did:key:m2",
                GrantedRole::Manager,
                false,
                4,
            ),
            // Each revokes the other's grant — their own key only.
            stmt(
                "VERSE#v1",
                "did:key:m2",
                "did:key:m1",
                GrantedRole::Manager,
                true,
                5,
            ),
            stmt(
                "VERSE#v1",
                "did:key:m1",
                "did:key:m2",
                GrantedRole::Manager,
                true,
                6,
            ),
        ];
        let live = latest_per_grantor(&statements);
        let result = materialize(OWNER, &live);
        assert_eq!(
            result.role_at("did:key:m1", "VERSE#v1"),
            RoleLevel::Manager,
            "stands on the owner's own grant, not the retracted peer grant"
        );
        assert_eq!(result.role_at("did:key:m2", "VERSE#v1"), RoleLevel::Manager);
    }

    #[test]
    fn a_far_future_hlc_from_a_manager_cannot_displace_the_owners_statement() {
        // The owner revokes M (an owner-blocked cell).
        let owner_revoke = stmt(
            "VERSE#v1",
            "did:key:m1",
            OWNER,
            GrantedRole::Manager,
            true,
            100,
        );
        // M stamps a far-future HLC on a statement under its OWN grantor key — a different
        // storage row than the owner's. There is no cross-author LWW in this model, so no
        // HLC, however large, lets one grantor's row contest another's.
        let m_self_regrant = stmt(
            "VERSE#v1",
            "did:key:m1",
            "did:key:m1",
            GrantedRole::Manager,
            false,
            u64::MAX,
        );
        let result = materialize(OWNER, &[owner_revoke, m_self_regrant]);
        assert_eq!(
            result.role_at("did:key:m1", "VERSE#v1"),
            RoleLevel::None,
            "validity is authority-based, never HLC-based, so skew cannot win a contest"
        );
    }

    #[test]
    fn an_unrooted_grant_cycle_is_never_effective() {
        let statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:a",
                "did:key:b",
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1",
                "did:key:b",
                "did:key:a",
                GrantedRole::Manager,
                false,
                2,
            ),
        ];
        let result = materialize(OWNER, &statements);
        assert_eq!(result.role_at("did:key:a", "VERSE#v1"), RoleLevel::None);
        assert_eq!(result.role_at("did:key:b", "VERSE#v1"), RoleLevel::None);
    }

    #[test]
    fn a_manager_grant_lapses_when_the_manager_loses_authority_and_returns_when_regranted() {
        let x_grant = stmt(
            "VERSE#v1-FRACTAL#f1",
            "did:key:x",
            "did:key:m1",
            GrantedRole::Editor,
            false,
            2,
        );

        // Phase 1: the owner grants M, M grants X.
        let r1 = materialize(
            OWNER,
            &[
                stmt(
                    "VERSE#v1",
                    "did:key:m1",
                    OWNER,
                    GrantedRole::Manager,
                    false,
                    1,
                ),
                x_grant.clone(),
            ],
        );
        assert_eq!(
            r1.role_at("did:key:x", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::Editor
        );

        // Phase 2: the owner revokes M; X's grant lapses with it, even though X's
        // statement was never re-sent.
        let r2 = materialize(
            OWNER,
            &[
                stmt(
                    "VERSE#v1",
                    "did:key:m1",
                    OWNER,
                    GrantedRole::Manager,
                    true,
                    3,
                ),
                x_grant.clone(),
            ],
        );
        assert_eq!(
            r2.role_at("did:key:x", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::None
        );

        // Phase 3: the owner re-grants M; the same held statement is admissible again.
        let r3 = materialize(
            OWNER,
            &[
                stmt(
                    "VERSE#v1",
                    "did:key:m1",
                    OWNER,
                    GrantedRole::Manager,
                    false,
                    4,
                ),
                x_grant,
            ],
        );
        assert_eq!(
            r3.role_at("did:key:x", "VERSE#v1-FRACTAL#f1"),
            RoleLevel::Editor,
            "re-granting M re-admits the never-resent held statement"
        );

        // QA-0131 write-time judging: a row X stamped while M was revoked (3)
        // stays refused after M's re-grant (4); a row stamped after it is admitted.
        let (x, scope) = ("did:key:x", "VERSE#v1-FRACTAL#f1");
        let m_regrant = stmt("VERSE#v1", "did:key:m1", OWNER, GrantedRole::Manager, false, 4);
        let x_editor = stmt(scope, x, "did:key:m1", GrantedRole::Editor, false, 2);
        let at = |statements: &[RoleStatement], hlc| materialize_at(OWNER, statements, hlc).role_at(x, scope);
        assert_eq!(at(&[m_regrant.clone(), x_editor.clone()], 3), RoleLevel::None);
        assert_eq!(at(&[m_regrant.clone(), x_editor.clone()], 5), RoleLevel::Editor);
        // A raise that stays at Editor+ keeps continuity (X's row at 5 survives
        // X's promotion at 9); a break — revoke, then re-grant — restarts it.
        let x_promoted = stmt(scope, x, "did:key:m1", GrantedRole::Manager, false, 9)
            .continuing(Some(&x_editor));
        assert_eq!((x_promoted.editor_since, x_promoted.manager_since), (Some(2), None));
        assert_eq!(at(&[m_regrant.clone(), x_promoted.clone()], 5), RoleLevel::Editor);
        let x_revoked = stmt(scope, x, "did:key:m1", GrantedRole::Manager, true, 10)
            .continuing(Some(&x_promoted));
        let x_regranted = stmt(scope, x, "did:key:m1", GrantedRole::Editor, false, 11)
            .continuing(Some(&x_revoked));
        assert_eq!((x_revoked.editor_since, x_regranted.editor_since), (None, None));
        assert_eq!(at(&[m_regrant, x_regranted], 5), RoleLevel::None);
        // A continuity field the statement does not back is a permanent deny.
        let key = StatementKey::parse(&format!("{scope}/{x}/did:key:m1")).unwrap();
        let mut payload = RoleStatementPayload {
            scope: scope.into(),
            peer_did: x.into(),
            grantor_did: "did:key:m1".into(),
            role: "viewer".into(),
            revoked: false,
            hlc: 12,
            editor_since: Some(3),
            manager_since: None,
        };
        assert_eq!(
            validate_statement("v1", Some(OWNER), &key, "did:key:m1", &payload),
            Err(RejectReason::InvalidContinuity)
        );
        payload.role = "editor".into();
        assert_eq!(
            validate_statement("v1", Some(OWNER), &key, "did:key:m1", &payload)
                .map(|statement| statement.granted_since(GrantedRole::Editor)),
            Ok(Some(3))
        );
    }

    #[test]
    fn has_any_role_is_true_for_a_petal_scoped_member_even_though_verse_scope_resolves_to_none() {
        let statements = vec![stmt(
            "VERSE#v1-FRACTAL#f1-PETAL#p1",
            "did:key:petal-member",
            OWNER,
            GrantedRole::Viewer,
            false,
            1,
        )];
        let result = materialize(OWNER, &statements);
        assert_eq!(
            result.role_at("did:key:petal-member", "VERSE#v1"),
            RoleLevel::None,
            "no role at the wider verse scope"
        );
        assert!(
            result.has_any_role("did:key:petal-member"),
            "but the DID holds a role somewhere in the verse (RP-3 enrolment floor, AC-44c)"
        );
        assert!(!result.has_any_role("did:key:stranger"));
    }

    #[test]
    fn diff_reports_the_direction_of_authority_change() {
        let before = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1",
                "did:key:m1",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            )],
        );
        let after_revoke = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1",
                "did:key:m1",
                OWNER,
                GrantedRole::Manager,
                true,
                2,
            )],
        );
        let changed = after_revoke.diff(&before);
        assert!(changed.fallen.contains(&"did:key:m1".to_string()));
        assert!(!changed.risen.contains(&"did:key:m1".to_string()));

        let viewer = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1",
                "did:key:m2",
                OWNER,
                GrantedRole::Viewer,
                false,
                1,
            )],
        );
        let promoted = materialize(
            OWNER,
            &[stmt(
                "VERSE#v1",
                "did:key:m2",
                OWNER,
                GrantedRole::Manager,
                false,
                2,
            )],
        );
        let changed2 = promoted.diff(&viewer);
        assert!(changed2.risen.contains(&"did:key:m2".to_string()));
    }

    /// Refine R3: `diff` is per cell. A Manager at fractal A newly made
    /// Manager at fractal B rose at B even though their best role did not
    /// change — the sync plane's §2.5 re-read keys off `risen`, so a
    /// best-across-scopes diff never re-read what they hold at B. An owner
    /// change raises the new owner and drops the old. Mutation check:
    /// compare max-over-scopes again, or drop the owner clause, and the
    /// matching assertion goes red.
    #[test]
    fn diff_reports_a_rise_at_one_scope_even_when_the_best_role_is_unchanged() {
        let at_a = stmt(
            "VERSE#v1-FRACTAL#a",
            "did:key:m",
            OWNER,
            GrantedRole::Manager,
            false,
            1,
        );
        let at_b = stmt(
            "VERSE#v1-FRACTAL#b",
            "did:key:m",
            OWNER,
            GrantedRole::Manager,
            false,
            2,
        );
        let before = materialize(OWNER, std::slice::from_ref(&at_a));
        let after = materialize(OWNER, &[at_a, at_b]);
        let changed = after.diff(&before);
        assert_eq!(changed.risen, vec!["did:key:m".to_string()]);
        assert!(changed.fallen.is_empty());

        let repinned = materialize("did:key:new-owner", std::iter::empty::<&RoleStatement>());
        let changed = repinned.diff(&materialize(OWNER, std::iter::empty::<&RoleStatement>()));
        assert_eq!(changed.risen, vec!["did:key:new-owner".to_string()]);
        assert_eq!(changed.fallen, vec![OWNER.to_string()]);
    }

    #[test]
    fn materialize_is_order_independent() {
        let statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:m-verse",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1",
                "did:key:m-fractal",
                "did:key:m-verse",
                GrantedRole::Manager,
                false,
                2,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:m-petal",
                "did:key:m-fractal",
                GrantedRole::Manager,
                false,
                3,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:editor-x",
                "did:key:m-petal",
                GrantedRole::Editor,
                false,
                4,
            ),
            // The owner pins the verse Manager down in p1 — exercises the pin-cuts-walk path.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:m-verse",
                OWNER,
                GrantedRole::Viewer,
                false,
                5,
            ),
            // A rootless cycle elsewhere in the same verse — must never become effective.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p2",
                "did:key:cyclic-a",
                "did:key:cyclic-b",
                GrantedRole::Manager,
                false,
                6,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p2",
                "did:key:cyclic-b",
                "did:key:cyclic-a",
                GrantedRole::Manager,
                false,
                7,
            ),
        ];
        let baseline = materialize(OWNER, &statements);
        for seed in 0..20u64 {
            let shuffled = seeded_shuffle(&statements, seed);
            let permuted = materialize(OWNER, &shuffled);
            assert_eq!(
                permuted, baseline,
                "seed {seed} produced a different fixpoint"
            );
        }
    }

    #[test]
    fn partitioned_admission_reaches_the_fixpoint_over_everything_received() {
        let all_statements = vec![
            stmt(
                "VERSE#v1",
                "did:key:m-verse",
                OWNER,
                GrantedRole::Manager,
                false,
                1,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1",
                "did:key:m-fractal",
                "did:key:m-verse",
                GrantedRole::Manager,
                false,
                2,
            ),
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:editor-x",
                "did:key:m-fractal",
                GrantedRole::Editor,
                false,
                3,
            ),
            // This grantor is not yet empowered when this statement is first seen in some
            // orderings — it must still land once `did:key:not-yet-manager` is promoted.
            stmt(
                "VERSE#v1-FRACTAL#f1-PETAL#p1",
                "did:key:late-editor",
                "did:key:not-yet-manager",
                GrantedRole::Editor,
                false,
                4,
            ),
            stmt(
                "VERSE#v1",
                "did:key:not-yet-manager",
                OWNER,
                GrantedRole::Manager,
                false,
                5,
            ),
        ];

        for seed in 0..10u64 {
            let order = seeded_shuffle(&all_statements, seed);
            let mut admitted: Vec<RoleStatement> = Vec::new();
            let mut held: Vec<RoleStatement> = Vec::new();

            // Admit-if-admissible against the materialization computed so far.
            for s in &order {
                let current = materialize(OWNER, &admitted);
                if current.admissible(s) {
                    admitted.push(s.clone());
                } else {
                    held.push(s.clone());
                }
            }

            // Re-partition to quiescence: promote any held statement that becomes
            // admissible under the latest materialization, repeating until nothing moves.
            loop {
                let current = materialize(OWNER, &admitted);
                let mut promoted_any = false;
                held.retain(|s| {
                    if current.admissible(s) {
                        admitted.push(s.clone());
                        promoted_any = true;
                        false
                    } else {
                        true
                    }
                });
                if !promoted_any {
                    break;
                }
            }

            let partitioned_result = materialize(OWNER, &admitted);
            let full_result = materialize(OWNER, &all_statements);
            assert_eq!(
                partitioned_result, full_result,
                "seed {seed}: partitioned admission diverged from materializing the whole set"
            );
            for s in &held {
                assert!(
                    !partitioned_result.admissible(s),
                    "seed {seed}: a held statement must stay inadmissible under the final materialization"
                );
            }
            for s in &admitted {
                assert!(
                    partitioned_result.admissible(s),
                    "seed {seed}: an admitted statement must stay admissible"
                );
            }
        }
    }
}
