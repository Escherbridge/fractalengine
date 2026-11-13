---
type: Review
date: 2026-11-13
track: portal_surface_20261107
gate: R-POR (turn 3: kill switch, bound opens, resolver/broker remainders, registry install)
reviewer: R-POR (fresh context, opus, adversarial security review-and-fix, code-first: nothing built or run)
reviewed: P-KILL 602838f2, P-BOUND c7c2f9d8, P-RES 9bd2132e, P-STORE 53ee9485
verdict: PASS-WITH-FIXES (5 FIXED, 4 KEPT, 3 HANDED-OFF to I-MAIN)
resource: ./plan.md
---

# R-POR: turn-3 portal gate

One verdict line per finding. FIXED landed in an R-POR-owned file. KEPT gives the reason. HANDED-OFF names the owner.

## Kill switch (P-KILL)

- **R-POR-T3-01 FIXED (High, fail-open).** The kill switch's disengage wrote the pre-engage value into the shared Disable all flag (`restore_to`, a bridge `Local`). Two cases cleared an operator's Disable all: a Disable all set between engage and disengage, and a disengage with no earlier engage (`unwrap_or(false)`). Both left portals admitted while extensions stayed disabled. The fix gives `PortalGovernance` its own `kill_switch` flag (`resolver.rs::set_kill_switch`). `host_disabled()` now reads `disabled_all || kill_switch`, so engaging takes the same host-disabled check that Disable all uses (mount verdict, broker bind re-check, `admitted_set`). Disengaging clears only the kill switch. The bridge (`qa_portal_bridge.rs::kill_switch`) no longer keeps a restore value. The resolver journey now has engage, Disable all, and disengage legs.
- **R-POR-T3-02 FIXED (Low).** `fixture_surface.rs::receipt_is_current` accepted any receipt marked `mounted:false` without comparing it to a mount. It now accepts that only for `PortalIpcStats` with an empty URL.
- **R-POR-T3-03 KEPT.** Engaging does not unload an open mount in the same frame. The host's re-judge tick (`portal_host/mod.rs::tick`, every `REJUDGE_EVERY` while a bundle is open) sends `ListAccepted`, and the listener then revokes the mount with `KillSwitch`. `serve` runs in PostUpdate, so writing a signal there would gain at most one frame. The broker and the mount verdict refuse at once.
- **R-POR-T3-04 KEPT.** Gating is unchanged and sound. The fe-api handler runs the QA gate and the Owner check twice (before and after the native round trip), checks that the credential is current, and checks the receipt scope. The bridge checks again for an Owner at the active scope, `view.covers`, and that the request is still current. No open portal is needed, by design.

## Bound opens (P-BOUND)

- **R-POR-T3-05 KEPT.** `BrowserCommand::OpenBoundBundle { petal_id, package_id }` carries no URL, path or hash. Its anchor `binding:<petal>:<package>` cannot be named by a page (`is_node_id` refuses `:`). The broker refuses a bound bind whose bytes name another package. The offer's click opens through `UiAction::OpenBundle` after selecting the repository node (R-POR-11). The `petal_portal.rs` hunks in c7c2f9d8 change whitespace only.
- **R-POR-T3-06 HANDED-OFF (I-MAIN, build-breaking).** `fe-webview/src/plugin.rs::dispatch_commands` matches `BrowserCommand` exhaustively and has no `OpenBoundBundle` arm. Add `OpenBoundBundle { petal_id, package_id } => host.open_target(BundleTarget::Bound { .. })`, with Unavailable keyed by `bundle_anchor()` when there is no host. Also make `report_bundles_unavailable` and `portal_host/broker.rs::track_portal_scopes` handle it (use `bundle_anchor()`).

## Resolver and broker (P-RES)

- **R-POR-T3-07 FIXED (Medium, generation never moved).** P-RES added `GovernanceSource::generation()` with a default of `None`. The `ExtensionGovernance` impl never overrode it, so `changed_since` always returned false even though X-GOV now has a change counter (`extension_host/governance.rs::generation`). The impl now returns `Some(ExtensionGovernance::generation(self))`.
- **R-POR-T3-08 HANDED-OFF (I-MAIN / R-BIN, `portal_host/mod.rs`).** Make the re-judge tick wake on `resolver.view().changed_since(&mut seen)` (or `ExtensionGovernance::subscribe()`) and keep the 500 ms timer only as a fallback. P-RES's task to stop relying on the 500 ms tick needs this host-side half.
- **R-POR-T3-09 FIXED (Medium, fail-open seam).** `BusPortalBroker::open` skipped the admission re-check whenever no governance was attached, and the new `from_parts` built exactly that kind of broker. `open` now refuses with `Unavailable` when governance is missing. `from_parts(parts, scopes, governance)` now requires the view. Nothing called it yet, so no caller breaks. The broker journey runs governed and asserts that an ungoverned broker binds nothing.
- **R-POR-T3-10 KEPT.** IPC role ceilings are unchanged and intact. The ceiling is the minimum of the bus role, fe-ui's resolved local role (Viewer while pending) and the viewer token. Reads re-check Viewer at use, and no `Caller` literal is built here. `admitted_set()` checks the kill list against `content_hash` only, because a record does not carry archive or member digests. The mount verdict and the broker's bind re-check test every digest, so a pair named only by its archive digest still never mounts or binds. Admission stays per peer, through local consent records only.

## Registry install (P-STORE)

- **R-POR-T3-11 FIXED (Low, resource).** `install.rs::inspect_bytes` wrote the whole download to a scratch file before the verifier checked the cap. It now refuses an over-cap download before any disk write.
- **R-POR-T3-12 HANDED-OFF (I-MAIN).** `portal_host/mod.rs` does not declare `mod install;` yet, so `install_from_registry` (the pinned S-HOST seam) is not compiled. The `configure(InstallContext { blobs, kill_list })` call at startup is also missing. Until it is added, the seam refuses with `NotConfigured` (fail closed). Verified: install checks the seal, kind, id, version and content hash against the listing, and the kill list under every digest. It stores only the blob. It starts nothing, creates no node and records no consent.
