---
type: Review
date: 2026-11-09
track: portal_surface_20261107
gate: R-P5 (swarm turn 2 fold) — R-POR, R-POR-IPC and the final cross-track gate R-X
reviewer: R-X (fresh context, opus, read-mostly, fix-only; nothing built or run)
verdict: PASS-WITH-FIXES (not merge-ready: three hand-offs land only with or after the merge)
resource: ./plan.md
---

# R-P5: turn-2 review fold for the portal surface

Swarm turn 2 ran two opus gates over this track. Their verdict lines stay in `review-turn2/R-POR.md` and
`review-turn2/R-POR-IPC.md`; this record folds them and adds what the final cross-track gate R-X checked from both
trees (this tree at `4323367a`, the sibling `track/extension-platform` at `d7bd9f18`). R-X's full verdict list is in the
sibling's `conductor/tracks/extension_platform_20261107/review.md` §Turn 2; the lines that concern this track are
repeated below.

## Gate fold

| Gate | Over | Lines | FIXED | KEPT | HANDED-OFF | DEFERRED | Verdict |
|---|---|---|---|---|---|---|---|
| R-POR | P-DOC, P-QA, P-API, P-S0C, P-UI, B10f, I-POR-UI, I-POR-SIM, I-POR-CRATES | 34 | 17 | 8 | 7 | 2 | PASS-WITH-FIXES |
| R-POR-IPC (security) | P-WV, P-BROKER, P-RESOLVE, I-POR-BIN, I-POR-CRATES, B10f | 23 | 8 | 11 | 1 | 3 | PASS-WITH-FIXES |
| **this track** | | **57** | **25** | **19** | **8** | **5** | |

What the gates closed:

- **A replicated row never mounts a bundle.** B10f's `OpenBoundBundle` path wrote `BrowserCommand::OpenBundle` behind fe-ui, with no chrome and outside the re-judge tick; it is gone, and fe-ui's action path is the only writer (R-POR-IPC-01).
- **Admission keys.** Bind-time re-check over every digest plus the consented capability digest (R-POR-IPC-02); member digests reach the kill list in fe-api's summary and export re-check (R-POR-13) and in the binary's local admission source, which now answers `cannot_admit` (R-POR-IPC-06).
- **Contract fidelity.** A portal is a local caller, so a below-floor invoke is `capability_denied` (R-POR-IPC-03, R-POR-23); one audit entry per refusal (R-POR-IPC-04); a user Revoke is `disabled / consent-revoked`, not a revocation-list event (R-POR-IPC-05); ipc-v1 §6.1 now says rows stay (R-POR-20, OD-E1).
- **A dead primary flow.** Admit… and Revoke in the Repository section carry their node, so they work while the inspector is hidden by an open portal (R-POR-01); Admit is offered only where it can succeed (R-POR-03).
- **QA lane.** Fixtures subscribe with valid params, and the probe emits the paths the scenarios assert (R-POR-27, R-POR-29); the killswitch route is mounted (R-POR-17).

Still open from the two gates, with the owner after R-X:

| Line | What | Owner and trigger |
|---|---|---|
| R-POR-IPC-20 (High) | A runtime Disable all never reaches portals: `administer` (sibling `plugin_bridge.rs`) only flips the plugin manager, and `Bus::feed_admission` has no caller in either tree. | Lands **with the merge**, before `ready-to-merge` is accepted: `DisableAll`/`EnableAll` call the portal resolver view's `set_host_disabled` (R-X-33). |
| R-POR-18 | `fe-api/src/qa_portal.rs` `portal_killswitch` is mounted but not written, so fe-api does not compile. | Orchestrator (steer 02:15), refine L2 (R-X-20). |
| R-POR-08 (rest) | Replace (`bundle_asset_pick`) needs a bundle-asset picker in fe-ui. The admission and summary drains landed in I-POR-BIN `627ac021`. | P-UI follow-up; `fractalengine/src/AGENTS.md` §portal-host names the trigger. |
| R-POR-11 | `show_offer` (the `portal.open` Open) has no caller. | Turn 3, with R-X-04 below. |
| R-POR-31 | `ipc-stats` answers only while a bundle is mounted. | No owner (`fe-webview/src/backends/fixture_surface.rs`); turn 3. |
| R-POR-IPC-21, -22, -23; R-POR-32 | Role-lost streams until close; same-origin navigation token overlap (SP-3/SP-5); nothing compiled; probe paths the host does not serve. | DEFERRED with their triggers (feed_admission caller, spikes, refine, host routes). |

## R-X lines for this track

- R-X-01 KEPT `Caller.role_ceiling` end to end: the bus opens every portal client with `Some(ceiling)`; this tree's broker computes `ceiling = min(bus role view, resolved local role, viewer token)`; the bridge closes a client above the viewer cap.
- R-X-02 KEPT every call this tree makes into the sibling's bus (`open_portal_client`, `client_*`, `take_client_events`, `install_portal_writes`, `PortalGrants`, `PortalWriteSink::commit`, `AuditEntry`, `BusRefusal`) and its one-op `DbCommand::PluginCommit { admission, ops, readings: Vec::new(), reply }` match the sibling's API and the pinned I-DB seam.
- R-X-03 KEPT `portal_host/resolver.rs` `GovernanceSource for ExtensionGovernance` and `portal_host/mod.rs` (`record_consent`, `kill_list_override`, `audit_sink`, `LoadExtensionGovernance`) match the sibling's governance. **This tree's binary compiles only on the merged tree.**
- R-X-04 HANDED-OFF (turn 3) `portal.open` never resolves: the sibling's resolver applies the extension rows to a web bundle (`not-installed`, or `not-provided` since a bundle has no `provides`), so B10f's Open never shows. Fix: a `portal.open` branch after rows 1-2 that reads this tree's Admitted records; no Install or Update offer for a bundle. Fails closed today.
- R-X-05 KEPT A-11: the sibling schema's `web-bundle` branch forbids every README §1.8 key plus `provides`; the `$ref` to `manifest-v1.schema.json` is added once the merge puts both schemas in one tree.
- R-X-06 KEPT A-10: the sibling's consent dialog renders the `portal` section and records against the portal capability digest; this tree's Admit drain records through governance.
- R-X-07 KEPT kill-list matching by hash under any id in this tree's path (archive digest before parse, content hash, every member digest, bind-time and export re-checks) and in the extension path; the Owner override is per (verse, content hash), the Owner's own host, audited.
- R-X-08 KEPT OD-E1 in admission.md §5 step 3 and ipc-v1.md §6.1 (rows stay). HANDED-OFF (orchestrator, no turn-2 owner): this track's `spec.md:236` still says "the same revocation withdraws them"; it should read "revocation stops the bundle after in-flight writes; the rows stay (OD-E1)". `conductor/open-core.md` here is the pre-amendment S-PLUGIN row; the merge takes the sibling's amended row (this tree never edited it).
- R-X-09 KEPT leak discipline: zero denylisted terms on the added lines of `ffd76c4f..4323367a` (33 commits, 121 files), on added paths, and in this record.
- R-X-10 KEPT `git merge-tree` (read-only): this branch over `main` is clean; over (main + the sibling) it has 9 union conflicts: `fractalengine/src/main.rs` (keep both blob-store clones), `tools/qa/sim/scenario.py` (keep both op and probe lists, then regenerate `scenario.schema.json` with `scenario.py schema`), `conductor/test-batches.md` (one R9 fe-api line: keep both `--test` sets), and five appended directory docs (`fe-api/AGENTS.md`, `fe-api/src/AGENTS.md`, `fe-format/src/AGENTS.md`, `fe-runtime/src/AGENTS.md`, `fe-ui/src/AGENTS.md`: keep both sections). **Merge order: the sibling first, then this branch rebased onto it**, then refine L3 (`Cargo.lock`, `cargo deny`).
- R-X-26 FIXED test selection: no batch reached the binary's `portal_host::` tests (broker, resolver; R-POR-IPC-23's NEEDS). R6 now runs `cargo test -p fractalengine --bin fractalengine -- portal_host::`, and the lookup table maps `fractalengine/src/portal_host/**` to R6.
- R-X-33 HANDED-OFF (High) = R-POR-IPC-20 above.
- R-X-35 DEFERRED (user, turn-2 plan §10 Q11) OD-7 against the extension update rule: this track admits an exact pair, so a same-publisher build with the same capabilities drops to the static sandbox until re-admitted, while an extension keeps consent across builds with an equal capability digest. admission.md §6 row 1 already expects governance to carry the record over; the cheapest alignment carries an Admitted record to the new hash when the publisher and the capability digest are equal.

## Verdict

PASS-WITH-FIXES for turn 2. The track is code-complete as drafts but cannot build alone: it needs the sibling's bus,
governance and `PluginCommit`, the missing `portal_killswitch` handler, and the merge-time Disable-all wiring.
