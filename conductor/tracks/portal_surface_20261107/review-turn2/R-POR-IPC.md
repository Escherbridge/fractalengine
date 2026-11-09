---
type: Review
date: 2026-11-09
track: portal_surface_20261107
gate: R-POR-IPC (turn 2: portal IPC security)
reviewer: R-POR-IPC (fresh context, opus, adversarial security review-and-fix, code-first: nothing built or run)
reviewed: P-WV, P-BROKER, P-RESOLVE, I-POR-BIN, I-POR-CRATES, B10f (commits ba6cb3d0..627ac021), plus R-POR hand-offs R-POR-09, R-POR-10, R-POR-24 and R-POR-30
verdict: PASS-WITH-FIXES (8 FIXED; runtime Disable all HANDED-OFF to the merge; 3 DEFERRED with triggers)
resource: ./plan.md
---

# R-POR-IPC: turn-2 security review of the portal IPC

This gate tried to refute nine claims:
- no page (external, unadmitted or revoked) reaches the broker or aborts the app;
- `Source` and the body are bounded;
- every bus `Caller` carries a `role_ceiling` at or below the viewer token;
- `set_property` has exactly one write path, and it stamps provenance;
- kill-list matching is by hash under any id;
- a revoke closes the broker client in the same frame;
- storage is cleared only after `NavigationCompleted`;
- a replicated row never mounts a bundle;
- no bytes are served after a revocation.

There is one verdict line per finding:
- **FIXED**: the fix landed in an R-POR-IPC-owned file in this gate;
- **KEPT**: the reason is given;
- **HANDED-OFF**: the owner is named;
- **DEFERRED**: the trigger that reopens it is named.

Severity is in brackets.

## Fixed

- **R-POR-IPC-01 FIXED (High: a replicated binding mounting with no host chrome; closes R-POR-10).**
  - **What was wrong.** B10f's `petal_portal.rs::OpenBoundBundle` and `bound_bundle_open_system` wrote `BrowserCommand::OpenBundle` behind fe-ui's back.
    - A bound bundle opened that way would mount with no host-drawn badge and no Close.
    - It would also mount outside the binary's 500 ms re-judge tick, which runs only while fe-ui's `PortalState` is a bundle (`portal_host/mod.rs::tick`). An accepted kill list would then not unload it until some other signal arrived.
  - **The fix.** Both are removed. Nothing wrote the message; `binding_offer.rs` already maps Open to `UiAction::OpenBundle`. The only writer of `BrowserCommand::OpenBundle` is now fe-ui's action path.
  - **Docs.** `fe-webview/src/AGENTS.md` §portal-lifecycle records the rule.
- **R-POR-IPC-02 FIXED (Medium: bind-time kill-list match narrowed to one digest, and capability digest unchecked).**
  - **What was wrong.** `broker.rs::still_admitted` passed only `[content_hash]` to `kill_list_status` and never compared the record's capability digest. A rebind could bind a build listed by its archive or member digest, or a build whose consent covered another capability set. A rebind happens on a late broker or on a viewer-ceiling change, inside the re-judge tick's 500 ms window.
  - **The fix.** `BindRequest` now carries `archive_digest`, `member_digests` and `capability_digest` (`ipc.rs`, filled from the `Mount`). `still_admitted` matches every digest and requires the consented digest.
  - **Tests.** The broker journey adds rows for an archive-listed build, a member-listed build and a widened digest. `portal_ipc_journey` asserts that the bind carries every key.
- **R-POR-IPC-03 FIXED (Medium: contract; closes R-POR-24).**
  - **What was wrong.** `broker.rs::invoke_error` turned the bus's `CapabilityDenied` into `NotFound`. That is the remote callers' uniform answer, but a portal is a local caller: ipc-v1 §6.2 (after R-POR-23) and bus.md §4 say "not exposed or below its role floor is `capability_denied`".
  - **The fix.** `invoke_error` is removed; `bus_error` passes the code through. The `PortalBroker::invoke` doc in `ipc.rs` now says the same.
  - **Tests.** The broker journey expects `CapabilityDenied` for the undeclared `ping`.
- **R-POR-IPC-04 FIXED (Medium: audit flooding and duplication, ipc-v1 §9 "one entry").**
  - **What was wrong.** `ipc.rs::route` audited every `CapabilityDenied` outcome of subscribe, act and invoke, including those the broker returned. The bus already audits its own refusals (`client_subscribe` denials, `note_refusal`, `admit_write`), so each such refusal was recorded twice. With R-POR-IPC-03, every bus-refused invoke would have been doubled too.
  - **The fix.** Broker results now pass through `broker_refusal`, which sets `broker_refused`. The bridge audits only its own check-5 refusals.
  - **Tests.** `portal_ipc_journey` refuses one declared invoke at the "bus" and asserts that the bridge did not audit it; the undeclared `stop` is still audited.
- **R-POR-IPC-05 FIXED (Medium: a user Revoke shown as a kill-list revocation).**
  - **What was wrong.** `portal_host/mod.rs` posted `Event(Revoked, ConsentRevoked)`. bus.md §2.4 pairs `consent-revoked` with `disabled`; `revoked` belongs to `revocation-list` alone. So a local Revoke unloaded as "Revoked — revocation list", and the mirrored record fell under the list's rule 4 instead of admission.md §2 rule 5.
  - **The fix.** The event is now `Event(Disabled, ConsentRevoked)`, which is per pair and shows "admission withdrawn". `fractalengine/src/AGENTS.md` §portal-host is updated.
  - **Unchanged.** Rows stay (OD-E1), and the client still closes in the same frame.
- **R-POR-IPC-06 FIXED (P0 compile, plus Medium security; closes R-POR-09).**
  - **What was wrong.**
    - R-POR-13 added `AdmissionQuery.member_digests` in fe-api, but `resolver.rs`'s test still built `AdmissionQuery` without it (E0063).
    - `LocalAdmission::admission` dropped member digests (`Vec::new()`), so fe-api's summary and export re-check missed a member-digest listing.
    - It never answered `cannot_admit`.
  - **The fix.**
    - The member digests now reach the kill list.
    - A read, static build answers `cannot_admit` when the scope-free half of may-admit fails (`PortalGovernance::may_admit_on_host`): the host switch is off, Disable all is on, the build is unsealed, or no root sealed it.
    - The archive-only pre-check still carries no reason.
  - **Tests.** The resolver journey adds member-digest, unsealed, untrusted and trusted rows.
- **R-POR-IPC-07 FIXED (Low: page-chosen text unbounded in logs and chrome).**
  - **What was wrong.** `tauri.rs::decide_navigation` echoed a refused external URL whole into `tracing::warn!` and into a `BackendEvent::Error` string. A page could repeat megabyte navigations.
  - **The fix.** `shown_url` caps the echo at 256 bytes on a character boundary.
  - **Tests.** The existing unparseable-URL test adds a 1 MB row.
- **R-POR-IPC-08 FIXED (Low: doc drift).**
  - The `portal_bundle/AGENTS.md` residual "the binary must insert `PortalBundleResolver`" is now true (`portal_host::resolver::install`).
  - §ipc documents the digest-carrying bind and the one-entry audit rule.

## Kept (refutation attempts that failed)

- **R-POR-IPC-09 KEPT: no page reaches the broker or aborts the app.**
  - **The native handler** (`tauri.rs::install_ipc_handler`) reads `Source` within 8 192 UTF-16 units first.
    - `take_bounded` scans at most `max+1` units and frees the CoTaskMem buffer on every path.
    - Only then does it run check 1 on the mount table. The body is read only for the live, unfaulted Admitted origin.
    - No path unwraps, indexes or slices page data: the prefix and id slices end after ASCII bytes, and `Label::parse` short-circuits before indexing.
  - **wry's own handler.** wry 0.54.4 still registers its `WebMessageReceived` handler (`webview2/mod.rs:469`), but it returns before reading `Source` when no IPC closure is set (`:894`). The only other wry `unwrap` is `NewWindowRequested`'s non-null webview (`:709`).
  - **Static, foreign, hidden and stale pages** stop at checks 1 and 2 with no reply. After a revoke, `bridge.shutdown()` clears the inbox and outbox, and the generation bump kills the token.
- **R-POR-IPC-10 KEPT: `Source` and body bounded.** The caps are 8 192 and 65 536 UTF-16 units before conversion, so at most 192 KiB of UTF-8. Check 3 then answers `invalid` above 65 536 bytes. The journey covers an over-cap `Source` and an unread body.
- **R-POR-IPC-11 KEPT: role ceiling.**
  - The only client constructor is `Bus::open_portal_client(installing_did, ceiling, grants)`, and the bus's portal `Caller` always has `Some(ceiling)` (Viewer when absent).
  - `ceiling = min(bus role view, fe-ui resolved local role (Viewer until resolved), viewer token)`.
  - The bridge closes a client reported above the viewer cap and answers `unavailable`.
  - This tree has no `Caller {` literal.
- **R-POR-IPC-12 KEPT: one-op write path with provenance.**
  - `act` passes these checks in order: key grammar and declared pattern, node id and size, the 2/s bucket, the ceiling at Editor or above.
  - Then the bus runs `admit_write` (local, declared key, valid provenance, shared budgets, role capped by the ceiling).
  - Then `PortalCommitSink` requires a live binding for the same package, hash, DID, petal and verse. It sends exactly one `PluginCommit` with no readings, `with_package{package_id, content_hash}`, property scope = the key, `SetProperty` only, and the pinned ancestry.
  - The DB re-judges Editor at use (`authorize_as`).
- **R-POR-IPC-13 KEPT: kill list by hash under any id.**
  - `GovernanceView::kill_listed(&[digests])` takes no package id.
  - `prepare` checks the archive digest before any parse, then the content hash and every member digest.
  - `revocation_for` matches an event by digest whatever package id it names.
  - The override is per (verse, content hash). It only spares a list cause, never a disable.
- **R-POR-IPC-14 KEPT: revoke closes the client in the same frame.**
  - `BundleHost::revoke` calls `bridge.shutdown()` → `broker.close()` → `bus.close_client()` inside `apply_portal_admission`. That system runs `.before(pump_portal_ipc)`, and `AdmissionWriters` and `observe_admission` are ordered before it.
  - Close, Navigate and an integrity fault unmount earlier in the frame, and `pump`'s `sync` unbinds within the same frame (`pump_portal_ipc` runs after `dispatch_commands` and `drain_backend_events`).
  - In-flight writes settle and their rows stay (OD-E1).
- **R-POR-IPC-15 KEPT: storage clear waits for a commit.**
  - `bundle_clear_storage` only queues the clear.
  - `run_storage_clears` runs a label only after a `PageLoadEvent::Finished` whose URL is another origin. In wry 0.54.4 that event is `add_NavigationCompleted` (`webview2/mod.rs:659-665`), with `Source` read at completion.
  - A failed unload leaves `Source` on the bundle origin, so the clear waits.
  - A label mounted again is skipped.
  - Residual: SP-6, unrun.
- **R-POR-IPC-16 KEPT: a replicated row never mounts.**
  - Only fe-ui's `UiAction::OpenBundle` handler (`actions/mod.rs:1344`) writes `BrowserCommand::OpenBundle`.
  - `PortalBundleChanged` → `refresh` reloads only a target whose local Open is still in force and unrevoked. The new build is judged fresh (exact pair).
  - The bound-target path (`resolve_bound`) refuses bytes of another package.
- **R-POR-IPC-17 KEPT: no bytes after revocation.**
  - `unmount_if` runs first, so new requests get 404.
  - A read that raced the revoke gets 410 at the post-read `is_live` check.
  - `portal-v1.js` and `feeds.json` both go through `mounts.lookup`, which requires the live, unfaulted mount.
  - Residual (`AGENTS.md`): a response already past the 410 check is in flight while the document unloads.
- **R-POR-IPC-18 KEPT (I-POR-BIN, closes R-POR-30).**
  - `qa_debug_bridge.rs` uses `ui.portal.is_open()` (`:579`) and emits `"portal": {node_id, state, badge_text, badge_name}` from `chrome_model`.
  - `qa_portal_bridge.rs:49` treats an open bundle portal as open.
  - `qa_web::allows` never sees a bundle URL. `admits` never defers a main-frame bundle URL, and frames naming a bundle are refused.
- **R-POR-IPC-19 KEPT (Low).** wry buffers any custom-protocol request body in full before calling the handler (`prepare_request`). The handler then answers 405 to anything but GET or HEAD. This is upstream behaviour, bounded by what a page can allocate.

## Handed off and deferred

- **R-POR-IPC-20 HANDED-OFF to the merge (sibling I-BIN owner of `fractalengine/src/plugin_bridge.rs`; High).** A runtime Disable all (layer b) does not reach the portal.
  - `PortalGovernance::disabled_all` starts from `FE_EXT_DISABLE_ALL` and otherwise follows only `host/admission` `disable-all` events.
  - Nothing posts those events: `Bus::feed_admission` has no caller in either tree, and `administer`'s `DisableAll`/`EnableAll` only flip the plugin manager.
  - So an admitted bundle keeps IPC, reads and writes, after Disable all until restart. Writes mint a fresh active admission per commit.
  - The fix: `administer` calls `PortalResolverRes.view().set_host_disabled(true/false)` (an `AtomicBool`). The 500 ms re-judge tick then unloads the mount.
  - This is recorded in `fractalengine/src/AGENTS.md` §portal-host "Deferred".
- **R-POR-IPC-21 DEFERRED (trigger: something calls `Bus::feed_admission` with `role-lost` / `policy-denied`; Low).**
  - Reads are judged against the local role at subscribe (`reads_allowed`). Live streams continue after the local identity drops below Viewer at the petal, until the portal closes or rebinds.
  - Writes are unaffected: the DB judges them at use.
  - With events fed, `revocation_for` (same pair, `PolicyDenied` → admission withdrawn) unloads the mount.
- **R-POR-IPC-22 DEFERRED (trigger: spikes SP-3 and SP-5).** A protocol request carries no document identity. During a same-origin navigation, the outgoing document can fetch `portal-v1.js` and receive the incoming generation's token. Both documents are builds of one (publisher, package). This is recorded in `portal_bundle/AGENTS.md` §residuals.
- **R-POR-IPC-23 DEFERRED (trigger: the build gate opens; refine).** Nothing here was compiled. Two things in particular need a build:
  - the CRLF file `fe-webview/src/petal_portal.rs`, whose original line endings were restored by hand;
  - the new `BindRequest` fields, used by `ipc.rs`, the broker tests and the journey's `FakeBroker`.

  Batches:
  - fe-webview `--test portal_ipc_journey` and `--lib` (which reaches `backends::tauri::tests`), both registered at `conductor/test-batches.md:206-207`;
  - the binary's `portal_host::broker::tests` and `portal_host::resolver::tests`. No batch reaches these yet; the result lists a NEEDS for a `cargo test -p fractalengine --bin fractalengine -- portal_host::` row.
