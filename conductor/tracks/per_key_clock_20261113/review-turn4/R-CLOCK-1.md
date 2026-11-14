---
type: Review
---

# R-CLOCK-1 — adversarial review and security review of CLOCK-01..08

SEC-INBOX 4: CONFIRMED-FIXED - fe-database/src/role_grants.rs:171,511 - wire windows were trusted after a shape check (fe-policy validate_statement); CLOCK-04's `proven_windows` in `place_statement` (the only inbound entrance) keeps a window only when the window re-derived from the stored prior version contains it; R-CLOCK-1 allowed a lower role inside a proven window (:182) and added the test row. The second clause (never beyond the grantor's authority at that time) holds through `materialize_at`: a window's statement counts only when its grantor is Manager as of `at` (fe-policy/src/role_grants.rs:673). Exactness: see R6 and R7.
SEC-INBOX 6: HANDED-OFF (gate owning fe-database/src/handlers/petal_snapshot.rs and fe-api/src/petal_snapshot.rs) - the clock side is clean: the import writes no role_grant, row_field_version or `_fe_clock` rows, so an imported row cannot carry a winning clock or a grant window; the op_log injection stays with that gate.
R1 FIXED - fe-database/src/handlers/scene_publication/inbound.rs:153 - `rejudge_legacy` judged with `written_while_authorized`, which returns true for anyone WITHOUT a current role (it assumes the current-role gate CLOCK-07 removed), so a never-authorized publisher's legacy node was kept or restored; now `held_at_write(.., Editor, hlc)`.
R2 FIXED - fe-database/src/handlers/scene_publication/inbound.rs:310 - same defect in the v1 (no `_fe_clock`) snapshot precheck; now `held_at_write`, so a v1 row from an author who never held Editor at its clock is Denied.
R3 FIXED - fe-database/src/role_grants.rs:614 - a local revoke of a peer this node holds no write from closed the window at the revoke clock, which admits writes the revoker never saw (contrary to user decision 2); the frontier is now 0, so the window closes empty. Journal updated: fe-database/tests/role_propagation_local_test.rs:326.
R4 FIXED - fe-database/src/role_grants.rs:182 - `proven_windows` required an equal role, so an honest narrowing (an Editor span claimed inside a Manager span this peer derived) was dropped; now `claimed.role <= local.role`.
R5 FIXED - conductor/tracks/per_key_clock_20261113/spec.md:134 - §3.7 said remote statements carry their windows verbatim; it now states the proof rule, the empty-frontier rule and residual risks 5 and 6 (:211, :216).
R6 KEPT (needs a user decision) - fe-policy/src/role_grants.rs:367 - `editor_since` / `manager_since` (QA-0131) are still accepted from the wire and seed a "proven" window at the next break; bounded by the grantor's authority at time (same as residual 3). Re-deriving them locally would refuse every pre-join edit of a live member on a fresh peer.
R7 KEPT (steer 22:55 mandates fail-closed) - fe-database/src/role_grants.rs:171 - proof is local, so a peer that never stored a key's earlier version keeps no windows and refuses rows an older peer admits from inside them; peers disagree only on rows written inside closed windows. Revisit trigger: a QA report of a revoked member's edit missing on a newly joined peer.
R8 HANDED-OFF (CLOCK-10) - fe-database/src/field_clock/rejudge.rs:15 - `rejudge_authors` is `unimplemented!()` and the app is panic=abort; it has no caller today, and it must not be wired before CLOCK-10 lands its body.
R9 KEPT (fenced on read) - fe-database/src/field_clock/publish.rs:191 - the outbound stamp commits in its own transaction after the row commit; an inbound merge fences on the stored versions (inbound.rs NODE_VERSIONS_FENCE, clocked_row clock_fence), so a race reorders the two writes but never loses either.
R10 KEPT (bounded) - fe-database/src/handlers/clocked_row.rs:290 - inadmissible versions are stored for a later re-judge; bounded at 512 keys per author per row and one version per (key, author), the same data the iroh replica already holds.

Verdict: PASS-WITH-FIXES (SEC-INBOX 4 closed; R6 is a user decision, R8 is CLOCK-10's).
