---
type: Product Guidelines
title: FractalEngine Product Guidelines
tags: [guidelines, ux, terminology, analytics]
timestamp: 2026-07-17T00:00:00Z
resource: ./product.md
---

# Product Guidelines

## Communication Style

**Minimal & silent.** The interface stays out of the way. UI copy, labels, and messages appear only when they serve a direct purpose. No onboarding tours, no tooltips explaining the obvious, no marketing language inside the app. The 3D world is the primary surface — the chrome around it is infrastructure.

Rules:
- Labels use nouns, not verbs where possible (`Petals`, `Paths`, `Roles` — not `Manage your Petals`)
- Status messages are one line maximum
- Success states are silent (the action completed; no confirmation toast needed unless the result is non-obvious)
- Error messages state what happened and what to do — nothing more
- Never use exclamation marks in system copy

---

## Visual Identity

**Functional minimalism.** Every pixel serves a purpose. No decorative gradients, no shadows for atmosphere, no illustration. Panels are flat, borders are thin, spacing is generous.

Rules:
- Dark-first rendering context: the 3D viewport is always dark/immersive; UI panels use dark neutral backgrounds that do not compete with the 3D scene
- Typography is monospace or geometric sans-serif — matching the technical, precise nature of the tool
- Color is used exclusively for state communication:
  - Green / accent: connected, active, permitted
  - Amber: degraded, cached/stale, warning
  - Red: error, denied, revoked
  - No color used purely decoratively
- Icons over text where the meaning is unambiguous; text labels always available as fallback
- No animations except functional transitions (panel slide-in, modal fade) — no looping or ambient motion

---

## Error Handling

**Modal blocking for critical errors.** When something requires user action before proceeding, stop and say so clearly. Do not attempt to hide failures or silently retry without informing the operator.

Tiers:
1. **Silent** — routine operations that succeed (asset loaded, peer connected, role applied): no feedback
2. **Inline indicator** — non-blocking state changes (peer disconnected, cache stale, JWT expiring): small persistent HUD indicator updated in place, no interruption
3. **Dismissible banner** — recoverable issues that don't require immediate action (relay fallback activated, asset fetch retrying): one-line banner, auto-dismisses on resolution
4. **Modal block** — actions that cannot proceed (keypair missing, SurrealDB unwritable, WebView CSP violation, destructive confirmation): modal with clear title, one-sentence explanation, and a single required action button

Error message format:
```
[What happened]: FractalEngine could not write to the OS keychain.
[What to do]: Check OS keychain permissions and restart FractalEngine.
```
Never show stack traces or internal error codes to the operator in production mode. Log them to the local log file silently.

---

## Operator Experience

**Zero-config defaults.** The app should be usable immediately after first launch without reading documentation. Defaults are chosen to be safe and functional:

- Default role for unauthenticated visitors: `Viewer` (read-only)
- Default asset size limit: 256 MB
- Default JWT TTL: 300 seconds
- Default session cache TTL: 60 seconds
- Default max concurrent visitors: unlimited (configurable)

Progressive disclosure pattern:
- Basic controls are always visible: Petal list, connected peers, active sessions
- Advanced settings are one level deep: accessed via a gear/settings affordance, never surfaced by default
- Per-node role overrides, cache management, and rate limits are advanced settings
- Destructive actions (delete Petal, revoke all sessions, clear cache) always require an explicit confirmation step — a second intentional click or typed confirmation, never a single accidental action

---

## Analytics & data-egress surfaces

The GIS panel, Export tab / Copy-for-BI card, rulers/measurement HUD, and
inspector value boxes are the product's primary UX (see
[product.md](./product.md)). Rules for all of them:

1. **Numbers always carry units.** Meters for position/length/width, degrees
   for rotation/bearing. Never render a bare number where a unit exists.
2. **Coordinates: lat/lon at the UI edge; petal-local meters are never
   surfaced.** Users think in lat/lon; the store's petal-local meter frame is
   an implementation detail (the CRS seam — see
   [tech-stack.md](./tech-stack.md)).
3. **Everything pasteable is one-click copyable.** Any query string, API URL,
   SQL, DuckDB snippet, or curl command the user might paste elsewhere gets a
   copy affordance, is shown verbatim in monospace, and is never truncated or
   ellipsized.
4. **Copy is silent-success** (consistent with the error-handling tiers): the
   click copies; no toast unless the copy failed.
5. **Exported artifacts state their scope honestly.** Every export (parquet,
   CSV, GeoJSON) declares its CRS and as-of time, and the UI says what the
   export contains (which petal, which node/path selection, which columns) —
   never imply an export is broader than it is.
6. **Share URLs display scope and expiry before minting.** The user sees what
   a signed URL grants and for how long before it exists.

---

## Security & Trust Communication

Implement all three trust signals — but implement them passively. They must be present without demanding attention.

**Role visibility (passive):** A small persistent chip in the corner of the viewport shows the current peer's role in the active Petal (e.g., `Editor` or `Viewer`). Clicking it expands a compact permissions summary. It never blinks, animates, or draws attention unprompted.

**WebView trust indicator (mandatory):** Whenever an embedded browser portal is open, a non-dismissible bar at the top of the portal shows:
- The domain of the loaded Portal URL
- An `External Website` badge
- A close button

This bar cannot be hidden by the loaded page. It is the primary defense against credential-harvesting UI within the portal.

**Identity transparency (on-demand):** The operator's public key and `did:key` identifier are accessible from the settings panel — not surfaced on the main screen. Peers can inspect their own identity from the same panel. The primary identity surface in daily use is the operator's human-readable name, not the cryptographic identifier.

---

## Naming & Terminology

Always use the canonical naming system consistently. Never use generic substitutes in UI copy:

| Use | Never use |
|---|---|
| Verse | tenant, organization, workspace (top-level scope) |
| Fractal | network, mesh, internet (a federation within a Verse) |
| Petal | world, room, space (a 3D world/space) |
| Node | object, item (a scene entity placed in a Petal) |
| Path | track (in any UI label; internal identifiers unchanged) |
| Map / Map Manager | hexon, tileset (in general UI; "hexon" is reserved for the package format in publish/import contexts) |
| Portal URL | external URL, browser URL, website (the URL field on a portal, everywhere) |
| Owner / Manager / Editor / Viewer | admin, superuser, root, member (the fixed role ladder — see product.md §RBAC) |
| Peer | user, visitor, client |

Historical terms that must not appear in new copy: `Room` and `Model` (entity
tiers from the 2026-03 concept that were never built — the scene entity is a
Node).

---

## Accessibility Baseline

- All 2D UI surfaces target WCAG 2.1 AA contrast ratios
- All interactive 2D elements are keyboard-navigable with visible focus indicators
- Every interactive Node in a Petal has a keyboard-accessible equivalent action
- 3D content accessibility is a documented known gap for v1 — a text-layer description per Node is the v1 mitigation
