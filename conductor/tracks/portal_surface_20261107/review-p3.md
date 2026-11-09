---
type: Review
date: 2026-11-08
track: portal_surface_20261107
gate: R-P3 (UX)
reviewer: P-S6 (fresh context, opus)
verdict: PASS-WITH-FIXES (blocked on NEEDS P3-01..P3-03)
resource: ./plan.md
---

# R-P3: UX review (`/code-review` + UI-UX guidance)

**Scope.** S3 at `dcfcd388` plus SI's fe-ui wiring in `eabcef17`: `fe-ui/src/portal/{mod,chrome,repository}.rs`, `fe-ui/src/actions/portal.rs`, `fe-ui/src/panels/portal_toolbar.rs`, and the central callers they rely on (`fe-ui/src/actions/mod.rs`, `fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/plugin.rs`). Contract: `docs/spec/portal-bundle/ux.md`; guidance: `C:/Users/atooz/.gemini/UI-UX-guidance.md`.
**Method.** Desk review; nothing built or rendered.

## Plan §4 checks

- P3-C1 PASS one primary action per state: `chrome::chrome_model` follows ux.md §3, and the toolbar skips the model's Close because the fixed Close button is the one Close control.
- P3-C2 PASS no helper text: the badge carries the state; the rows are label/value only.
- P3-C3 PASS after P3-05 stable ASCII accessible names: every ux.md §5 name is present, and the badge and publisher names are now ASCII.
- P3-C4 PASS keyboard: every action is an egui `Button`, so it is focusable and activates on Enter or Space; there is no mouse-only path. The badge is not focusable (P3-10).
- P3-C5 PASS conditional on P3-01 unchanged insets: `compute_portal_rect` is untouched and the bundle chrome lives in the toolbar. Until P3-01 lands, bundles never reach the content-rect branch at all.

## Findings (one verdict per line)

- P3-01 NEEDS (P0, compile and critical UX) In `fe-ui/src/actions/mod.rs` (SI-owned), `UiManager::portal_url` / `portal_hostname` match `PortalState` without the new `Bundle` arm (E0004). `portal_is_open()` also stays false for bundles, so `right_sidebar.rs:324` never shows the portal toolbar and `plugin.rs:1311` never measures the content rect. Back, Forward and Reload are no-ops. The result: a bundle renders with no badge and no Close.
- P3-02 NEEDS (P0, compile) `portal_toolbar.rs:53-54` pushes `UiAction::PortalAdmitBundle` / `UiAction::PortalRevokeBundle`, which `actions/mod.rs` never declares. At runtime they stay unreachable until governance is injected: without it, `can_admit` is false and Admitted cannot be reached.
- P3-03 NEEDS (High, UX) The Repository section has no call site, and nothing produces `UiAction::OpenBundle`, so a bundle cannot be opened from the UI. SI owns the inspector call site. The `RepositoryView` data (summary read `GET /api/v1/portal-bundles/{asset_id}` plus governance) also has no fe-ui read path yet.
- P3-04 FIXED (Medium, trust chrome) Leaving a mounted bundle for a declared origin emitted `Closed`, so fe-ui closed the portal while the native webview kept showing the external page with no host chrome. In `actions/portal.rs`, an allowed `UrlChanged` on a mounted bundle (`label` set) now becomes `PortalState::Open` with website chrome, and the following `Closed` is a no-op. A change before the first mounted state is the previous page's and is ignored. The bundle journey is extended.
- P3-05 FIXED (Medium) `short_did` used "…", so "Admitted - z6Mkha…2doK" and "Bundle publisher: …" broke ux.md §5's "Stable, ASCII". It now uses "...", and the chrome table asserts the name is ASCII.
- P3-06 FIXED (Low) The badge sat below the hostname line, against ux.md §3's "above", and its first appearance moved the native viewport. It is now a fixed-height band (one text row plus one control row) above the hostname line, drawn for every bundle state.
- P3-07 PASS `validate_url` refuses both bundle spellings and the `fe-portal:` scheme, so raw bundle URLs are never persisted, and `PortalState::url()` is empty for bundles.
- P3-08 PASS `apply_bundle_state` ignores events for another node, so a stale open cannot repaint the chrome.
- P3-09 NOTE (Low, contract) ux.md §7 ("Admit… disabled with a hover reason") contradicts §1/§2 ("absent … only when the local role may admit"). The code follows §2. SU/SD should settle the wording.
- P3-10 DEFERRED (Low) ux.md §6 asks for a focusable badge, but egui labels are not focusable. The AccessKit name is set, so screen readers announce the state change in place.
- P3-11 DEFERRED (Low) Revoked offers no "Admit… (after the cause is lifted)": `BundleState` carries no cause-lifted fact. This needs governance's installation status (A-15).
- P3-12 NOTE `chrome_table_matches_ux_contract` restates the badge table, which plan §1 says is proven through ui-tree names. It stays because ux.md §7 asks for exactly this table; drop it if PJ1-PJ9 cover every reason.

## Verdict

PASS-WITH-FIXES for the S3 files. fe-ui does not compile, and a bundle has no chrome, until SI applies P3-01 and P3-02 (exact patch in the S6 return). P3-03 gates any user-reachable Open.
