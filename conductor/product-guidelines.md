# Product Guidelines

## Communication Style

**Minimal & silent.** The interface stays out of the way. UI copy, labels, and messages appear only when they serve a direct purpose. No onboarding tours, no tooltips explaining the obvious, no marketing language inside the app. The 3D world is the primary surface — the chrome around it is infrastructure.

Rules:
- Labels use nouns, not verbs where possible (`Node`, `Petals`, `Roles` — not `Manage your Petals`)
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
[What happened]: Node could not write to keychain.
[What to do]: Check OS keychain permissions and restart FractalEngine.
```
Never show stack traces or internal error codes to the operator in production mode. Log them to the Node's internal log file silently.

---

## Operator Experience

**Zero-config defaults.** A Node should be usable immediately after first launch without reading documentation. Defaults are chosen to be safe and functional:

- Default role for unauthenticated visitors: `public` (read-only)
- Default Petal persistence tier: `CACHED`
- Default asset size limit: 256 MB
- Default JWT TTL: 300 seconds
- Default session cache TTL: 60 seconds
- Default max concurrent visitors: unlimited (configurable)

Progressive disclosure pattern:
- Basic controls are always visible: Petal list, connected peers, active sessions
- Advanced settings are one level deep: accessed via a gear/settings affordance, never surfaced by default
- Per-Model role mapping, per-Room access overrides, cache management, and rate limits are advanced settings
- Destructive actions (delete Petal, revoke all sessions, clear cache) always require an explicit confirmation step — a second intentional click or typed confirmation, never a single accidental action

---

## Security & Trust Communication

Implement all three trust signals — but implement them passively. They must be present without demanding attention.

**Role visibility (passive):** A small persistent chip in the corner of the viewport shows the current peer's role in the active Petal (e.g., `member` or `public`). Clicking it expands a compact permissions summary. It never blinks, animates, or draws attention unprompted.

**WebView trust indicator (mandatory):** Whenever the embedded browser overlay is open, a non-dismissible bar at the top of the overlay shows:
- The domain of the loaded URL
- An `External Website` badge
- A close button

This bar cannot be hidden by the loaded page. It is the primary defense against credential-harvesting UI within the browser overlay.

**Identity transparency (on-demand):** The operator's Node public key and `did:key` identifier are accessible from the Node settings panel — not surfaced on the main screen. Peers can inspect their own identity from the same panel. The primary identity surface in daily use is the Node name (human-readable), not the cryptographic identifier.

---

## Naming & Terminology

Always use the fractal naming system consistently. Never use generic substitutes in UI copy:

| Use | Never use |
|---|---|
| Node | server, instance, host |
| Petal | world, room, space (for top-level) |
| Room | area, zone, space (within a Petal) |
| Model | object, asset, item |
| Fractal | network, mesh, internet |
| Admin | owner, superuser, root |
| Peer | user, visitor, client |

---

## Accessibility Baseline

- All 2D UI surfaces target WCAG 2.1 AA contrast ratios
- All interactive 2D elements are keyboard-navigable with visible focus indicators
- Every interactive Model in a Room has a keyboard-accessible equivalent action
- 3D content accessibility is a documented known gap for v1 — a text-layer description per Model is the v1 mitigation
