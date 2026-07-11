# Petal Portal — Digital Twin Browser Overlay & IoT Interaction

**Track ID:** `petal_portal_20260322`
**Stack:** Rust, Bevy 0.18, wry 0.54
**Status:** Draft

---

## 1. Overview

Petal Portal turns every 3D model in the fractalengine scene into a portal to the web. When a user selects a model, a browser overlay anchors to that model's screen-space projection. The overlay shows an external URL (a live IoT dashboard, store page, control panel, or mini-game) and, for authorized users, a second config tab for editing the model's metadata in place.

The system is built on the existing `WebViewPlugin` / `WryBrowserSurface` infrastructure. This track wires that infrastructure into the scene selection pipeline and adds the metadata, role-gating, trust-bar, and lifecycle management that make it production-ready.

---

## 2. Existing Infrastructure

| Symbol | Location | Role |
|---|---|---|
| `WebViewPlugin` | `fe-network` (feature-gated `webview`) | Bevy plugin; owns wry lifecycle |
| `WryBrowserSurface` | `fe-network` | Concrete `BrowserSurface` backed by wry |
| `BrowserSurface` trait | `fe-network` | `show / hide / navigate / position` |
| `BrowserCommand` | `fe-network` | `Navigate(url) / Close / GetUrl / SwitchTab(BrowserTab)` |
| `BrowserEvent` | `fe-network` | Events emitted back to Bevy |
| `WebViewConfig` | `fe-network` | `{ petal_id, initial_url, role }` |
| `BrowserTab` | `fe-network` | Enum of named tabs |
| `is_url_allowed()` | `fe-network` | Blocks RFC-1918 ranges |
| `TRUST_BAR_JS` | `fe-network` | JS string; not yet injected |

Nothing in this list is modified by this track except where explicitly noted in Section 5.

---

## 3. Goals

1. Every model can carry an `external_url` and a `config_url` in its schema.
2. Selecting a model opens a browser overlay positioned adjacent to the model's screen-space bounding box.
3. The overlay has two tabs:
   - **Tab 1 — External** (`BrowserTab::External`): loads `external_url`; visible to any role that can select models.
   - **Tab 2 — Config** (`BrowserTab::Config`): loads `config_url`; visible only to `Role::Admin`.
4. All URLs pass through `is_url_allowed()` before any navigation occurs.
5. `TRUST_BAR_JS` is injected into every wry `WebView` at construction time.
6. The overlay hides when the model is deselected or ESC is pressed, and repositions when the camera moves.
7. The model inspector panel gains URL editor fields.

---

## 4. Non-Goals

- This track does not implement OAuth or session management inside the webview.
- This track does not build a new renderer; the overlay is a native OS window child or transparent wry surface per existing `WryBrowserSurface` behavior.
- This track does not change the `is_url_allowed()` logic.
- Mobile / WASM targets are out of scope.

---

## 5. What to Build

### 5.1 Model URL Metadata

Extend the model schema (wherever `ModelMetadata` or equivalent is defined) with two optional fields:

```rust
pub struct ModelUrlMeta {
    /// Public-facing URL (IoT dashboard, storefront, game, etc.)
    pub external_url: Option<String>,
    /// Admin config panel URL
    pub config_url: Option<String>,
}
```

`ModelUrlMeta` is attached as a Bevy `Component` on model entities. It serializes to / deserializes from the scene format. Both fields default to `None`.

### 5.2 Browser Overlay Positioning

A new system `position_overlay_system` runs in `PostUpdate` whenever:
- A model entity is marked `Selected`, **or**
- The camera `Transform` has changed while an overlay is open.

The system:
1. Projects the model's world-space AABB corners through the active camera to get a screen-space bounding rect.
2. Anchors the overlay to the right edge of that rect (clamped to viewport bounds).
3. Calls `BrowserSurface::position(x, y, width, height)`.

Overlay default size: `480 x 320` logical pixels, minimum `320 x 240`.

### 5.3 Activation Flow

```
User clicks / selects model
        |
        v
SelectionChanged event fires
        |
        v
petal_portal_activation_system reads ModelUrlMeta
        |
        +- external_url present? --> validate via is_url_allowed()
        |                                   | pass --> BrowserCommand::Navigate(url)
        |                                   +- fail --> log warning, show error tab
        |
        +- Spawn / reuse WryBrowserSurface for this petal_id
        |
        +- BrowserCommand::SwitchTab(BrowserTab::External)
        |
        +- If Role::Admin AND config_url present
                +--> register Config tab (hidden from non-admins)
```

When a previously selected model is deselected (or ESC fires):

```
Deselect / ESC
        |
        v
petal_portal_deactivation_system
        |
        +- BrowserCommand::Close  (or hide if reuse strategy)
        +- Clear ActivePortal resource
```

### 5.4 Two-Tab System with Role-Gated Visibility

`BrowserTab` gains two variants (or maps to existing variants):

| Variant | Shown to | Loads |
|---|---|---|
| `BrowserTab::External` | All roles with select permission | `ModelUrlMeta::external_url` |
| `BrowserTab::Config` | `Role::Admin` only | `ModelUrlMeta::config_url` |

Tab-switching is driven by `BrowserCommand::SwitchTab`. A `TabVisibilityFilter` resource stores the current role and is queried before any `SwitchTab` command is sent. Attempts to switch to `Config` without `Role::Admin` are silently dropped with a `warn!()` log.

### 5.5 Trust Bar JS Injection

`TRUST_BAR_JS` must be evaluated in every wry `WebView` immediately after the page's `DOMContentLoaded` event. Injection point: `WryBrowserSurface::new()` or equivalent builder — pass `TRUST_BAR_JS` as an `init_script` to the wry `WebViewBuilder`. This is a one-line change to the builder call; no new system is required.

### 5.6 Overlay Lifecycle

| Trigger | Action |
|---|---|
| Model selected | `show()` + `navigate()` |
| Model deselected | `hide()` |
| ESC key | `hide()` + clear selection |
| Camera transform changed (overlay visible) | `position()` recalculated |
| Scene unloaded | `Close` command, drop resource |
| `external_url` changed via inspector | `navigate()` with new URL after `is_url_allowed()` |

The `ActivePortal` resource holds `Option<(Entity, WryBrowserSurface)>` for the currently open overlay. At most one overlay is open at a time (single-selection model; multi-select is a future concern).

### 5.7 URL Editor in Model Inspector Panel

The inspector panel gains a collapsible "Portal URLs" section:
- Text field: "External URL" — bound to `ModelUrlMeta::external_url`
- Text field: "Config URL" (visible to `Role::Admin` only) — bound to `ModelUrlMeta::config_url`
- Both fields show a validation indicator (green / red) computed from `is_url_allowed()` on every keystroke.
- A "Save" button commits the change and, if the overlay is currently open for this model, triggers a `BrowserCommand::Navigate` to the new URL.

---

## 6. Acceptance Criteria

### AC-1: Metadata
- [ ] `ModelUrlMeta` component exists and serializes to/from scene format without data loss.
- [ ] Both fields are `None` by default; loading a scene without portal fields does not error.

### AC-2: Activation
- [ ] Selecting a model with a non-empty, allowed `external_url` opens the overlay within one frame of the `SelectionChanged` event.
- [ ] Selecting a model with no `external_url` does not open an overlay.
- [ ] Selecting a model with a disallowed URL (RFC-1918 or otherwise blocked) logs a warning and does not open the overlay.

### AC-3: Tab Visibility
- [ ] A non-admin user sees exactly one tab (External).
- [ ] An admin user sees two tabs (External + Config).
- [ ] Calling `SwitchTab(BrowserTab::Config)` as a non-admin is a no-op with a warning log — no panic, no crash.

### AC-4: Trust Bar
- [ ] `TRUST_BAR_JS` is present in the wry init script on every constructed `WryBrowserSurface`.
- [ ] The injected script runs before any page script (init_script semantics).

### AC-5: Lifecycle
- [ ] Deselecting a model (or pressing ESC) hides the overlay within one frame.
- [ ] Moving the camera while the overlay is open triggers a reposition call.
- [ ] Reposition output is clamped to viewport bounds; the overlay never renders off-screen.
- [ ] Unloading the scene closes the overlay cleanly (no wry handle leaks).

### AC-6: URL Editor
- [ ] Editing the external URL field and saving updates `ModelUrlMeta` and (if overlay is open) navigates to the new URL.
- [ ] Invalid URLs (blocked by `is_url_allowed()`) show a red indicator; Save is disabled.
- [ ] Config URL field is hidden from non-admin users.

---

## 7. Security Considerations

### 7.1 URL Validation
All URLs — whether loaded from scene files, entered in the inspector, or received via any future RPC — must pass `is_url_allowed()` before any navigation. The validation call is synchronous and must occur on the Bevy main thread before `BrowserCommand::Navigate` is dispatched.

### 7.2 RFC-1918 Blocking
`is_url_allowed()` already blocks private IP ranges. This track does not weaken that logic. Attempts to navigate to `192.168.*`, `10.*`, `172.16-31.*`, or `localhost` variants must fail silently (log + no-op).

### 7.3 Trust Bar
`TRUST_BAR_JS` must be injected as an init script, not via `evaluate_script()` after load. Init scripts run in the main world before any page code and cannot be overridden by page content. This prevents portal pages from spoofing the trust UI.

### 7.4 Role Boundary
The `TabVisibilityFilter` check is performed in Bevy system code, not inside the webview. The webview receives only the URL it is authorized to load; it never receives the `config_url` string if the current role is not Admin.

### 7.5 Content Isolation
Each `petal_id` gets its own `WryBrowserSurface` instance. There is no shared storage or session between different portals.

---

## 8. UX Flow Diagrams

### 8.1 Happy Path — External User

```
[3D Scene]
   |
   |  click model
   v
[Selection Highlight]  -->  [Overlay appears, anchored right of model]
                                    |
                                    |  Tab: External (only tab visible)
                                    v
                            [IoT Dashboard / Store Page loads]
                                    |
                             press ESC or click elsewhere
                                    |
                                    v
                            [Overlay hides]  -->  [Model deselected]
```

### 8.2 Happy Path — Admin User

```
[3D Scene]
   |
   |  click model
   v
[Selection Highlight]  -->  [Overlay appears]
                                    |
                              Tab bar: [External] [Config]
                                    |
                              click [Config]
                                    v
                            [Admin config panel loads]
                                    |
                              edit settings, save
                                    |
                              click [External]
                                    v
                            [External URL reloads]
```

### 8.3 Invalid URL Path

```
[Model selected]
   |
   v
is_url_allowed() --> false
   |
   v
warn!("petal_portal: blocked URL for entity {:?}")
   |
   v
No overlay opened. Selection highlight remains.
Inspector shows red indicator on URL field.
```

### 8.4 Camera Move — Reposition

```
[Overlay open]
   |
   |  camera Transform changed
   v
position_overlay_system runs (PostUpdate)
   |
   v
reproject model AABB  -->  new (x, y)  -->  BrowserSurface::position()
   |
   v
Overlay moves to stay anchored to model
```

---

## 9. Open Questions

1. **Multi-select**: If multiple models are selected simultaneously, which portal opens? Current answer: first-selected wins; others are queued (future track).
2. **wry window parenting**: The correct wry API for embedding as a child surface on Windows/macOS needs a spike — `WryBrowserSurface` may already handle this or may need an OS-specific code path.
3. **Overlay size**: Is `480 x 320` the right default? UX review needed before Phase 4 freeze.
4. **Config URL source**: Does `config_url` point to an external service or a locally-served Bevy egui panel? Current assumption: external URL, same `is_url_allowed()` rules apply.

---

## 10. Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `wry` | 0.54 | WebView surface |
| `bevy` | 0.18 | ECS, rendering, input |
| `bevy_egui` | compatible | Inspector panel UI |
| `fe-network` | workspace | `WebViewPlugin`, `BrowserSurface`, `is_url_allowed` |
| `fe-auth` | workspace | Role resolution |
| `serde` | 1 | `ModelUrlMeta` serialization |
