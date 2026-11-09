---
type: Specification
date: 2026-11-08
status: draft
title: Portal bundle UX - repository inspector section, chrome states, flows, names, keys
track: portal_surface_20261107
resource: ../../../conductor/tracks/portal_surface_20261107/spec.md
---

# Portal bundle UX (T3.1)

Design contract for the Repository inspector section and the host-drawn portal chrome. It applies the shared UI/UX guidance: one clear purpose per surface, one primary action, no helper paragraphs, state carried by the badge text, stable accessible names, keyboard and assistive access preserved. It adds no new layout: the native overlay keeps the measured remaining rect (`fe-ui/src/portal/mod.rs`).

Build slice S3 implements it. The accessible names in section 5 are the locators that QA journeys PJ1-PJ9 (`tools/qa/portal/journeys.md`) use; a rename here is a contract change.

## 1. Principles

- **Host-drawn only.** Every trust and admission indicator is drawn by egui outside the webview. Page content never supplies or alters it.
- **Badge text is the explanation.** No sentences under controls. A reason is a short suffix on the badge ("Blocked - too large").
- **One primary action per state.** Secondary actions appear only when allowed: absent, not disabled, unless the user could act on them after a condition changes (then disabled with a hover reason).
- **Reuse.** Consent is the extension governance dialog (`Admit...` opens it); this track draws no second consent surface. Existing toolbar controls (Back, Forward, Reload, Open externally, Close) are unchanged.
- **No data-loss paths.** Replace and Revoke never delete the bundle blob or the node row; they change the reference or the local admission record.

## 2. Repository inspector section

Shown in the right inspector when the selected node has `node_kind = repository` with a bundle reference. Section title: "Repository". Rows, top to bottom, label left and value right, one line each:

| Row | Value | Notes |
|---|---|---|
| Bundle | bundle title from the portal manifest | truncated, hover shows the full title |
| Publisher | short DID plus trust-root name | "unsigned" when there is no publisher signature |
| Version | version and short content hash (first 8 hex) | hover shows the full hash |
| Status | admission badge (section 3) | same text as the toolbar badge |
| Access | declared capabilities as a plain list | one per line, e.g. "Read node changes", "Set properties"; "None" when empty |

Actions, in one row beneath:

- **Open** (primary, always present).
- **Admit...** only in Static sandbox when the local role may admit. Opens the governance consent dialog.
- **Revoke on this peer** only when Admitted. Withdraws the local admission; an open portal goes to Revoked.
- **Replace bundle** only for Editor or above. It is a property write (`portal.bundle_asset`) through the existing asset picker; the node keeps its identity. Viewers do not see it.

If the node is a repository node whose reference is set but bytes are not yet local, Status reads "Waiting for bundle bytes" and Open stays enabled so the user sees the Waiting state in the portal.

## 3. Chrome state table

The toolbar shows the badge below the action buttons and above the hostname line. Source of truth: the pure chrome model in `fe-ui/src/portal/chrome.rs`, derived from the governance installation status plus portal-only conditions (integrity, size, platform, bytes present).

| State | Rendered | IPC | Badge (accessible name) | Primary action | Secondary |
|---|---|---|---|---|---|
| Admitted | yes | yes | "Admitted - <publisher>" | Close | Revoke on this peer |
| Static sandbox | yes | no | "Static - not admitted on this peer" | Admit... (when allowed) else Close | Close |
| Blocked | no | no | "Blocked - <reason>" | Close | none |
| Revoked | no (unloaded) | no | "Revoked - <reason>" | Close | Admit... (only after the cause is lifted) |
| Waiting for bytes | no | no | "Waiting for bundle bytes" | Close | none |
| Unavailable | no | no | "Embedded browsing unavailable" | Close | none |

The spec writes an em dash; the accessible name uses " - " so locators are plain ASCII. The visible badge may render the em dash.

- **Blocked reasons** (exact suffix text): "integrity failure", "malformed manifest", "too large", "verse policy", "unsupported platform".
- **Revoked reasons**: "kill switch", "revocation list", "admission withdrawn".
- **Visual distinction.** Admitted uses the accent colour; Static sandbox the neutral dim colour with an outline; Blocked and Revoked the error colour. State is always also in the text, never colour alone.
- **Bundle origins.** The in-page trust bar stays (it must not be removed) and reads "Portal bundle" on bundle origins instead of "External Website". The hostname line shows the origin label, not a URL, and "Open externally" is hidden for bundle origins (a bundle origin is not an external URL).

## 4. Flows

### 4.1 Open
1. The user selects a repository node and presses **Open**.
2. The host mounts the bundle under its current admission state; the portal panel opens with the badge.
3. Arrival of a replicated row never opens anything. Only this explicit action does.

Admitted and Static sandbox render. Blocked, Waiting and Unavailable show only the badge and Close; the webview area stays empty.

### 4.2 Admit
1. In Static sandbox, **Admit...** opens the governance consent dialog (capabilities are listed there).
2. Accept: the badge becomes "Admitted - <publisher>" and the document reloads with the bridge. Decline or Esc: nothing changes.

### 4.3 Revoke (user)
**Revoke on this peer** asks no confirmation (it is reversible with Admit...) and withdraws admission. An open portal unloads and the badge reads "Revoked - admission withdrawn".

### 4.4 Revoke (system)
Kill switch, revocation list or policy change: within one frame the portal unloads and the badge reads "Revoked - <reason>". No prompt; the badge is the notification. Re-opening still renders nothing until the cause is lifted and the bundle is re-admitted.

### 4.5 Replace
1. An Editor or above presses **Replace bundle** and picks a bundle asset.
2. The reference property is written; the section refreshes Version, Publisher and Status.
3. Update policy: same publisher with no capability increase stays Admitted; any capability increase drops to Static sandbox until re-consent; a different publisher is a new origin.
4. An open portal for the old content hash reloads under the new state.

### 4.6 Close
**Close** unloads the bundle (no hidden popup keeps scripts alive) and returns to the previous panel. Node state is unchanged.

## 5. Accessible names (locators)

Stable, ASCII, unique per control. Set with the existing `.named(ui, ...)` helper.

| Control | Name |
|---|---|
| Repository section header | "Repository" |
| Open button | "Open bundle" |
| Admit button | "Admit bundle" |
| Revoke button | "Revoke bundle on this peer" |
| Replace button | "Replace bundle" |
| Section status row | "Bundle status: <badge text>" |
| Section publisher row | "Bundle publisher: <value>" |
| Section version row | "Bundle version: <value>" |
| Section access list | "Bundle access" |
| Toolbar badge | the badge text from section 3, e.g. "Admitted - <publisher>" |
| Toolbar Close | "Close portal" (existing) |
| Toolbar Back, Forward, Reload | existing names, unchanged |

The badge is a labelled, non-interactive element so assistive technology reads state changes. When the state changes while the portal is open, the badge name changes in place; there is no separate toast.

## 6. Keyboard paths

- Tab order in the section: Open, then Admit... / Revoke / Replace (those present), then the next inspector section.
- Enter or Space activates the focused button. No action is mouse-only.
- Portal toolbar Tab order: Back, Forward, Reload, Open externally (non-bundle origins only), Close, then the badge, which is focusable for reading.
- Esc closes the consent dialog without effect. While the page has focus, Esc goes to the page; the host Close button is the keyboard route out.
- Existing editor shortcuts are not shadowed by the section.

## 7. States to verify (build and QA)

- **Waiting:** bytes absent shows "Waiting for bundle bytes" and resolves after the prefetch re-arm without user action (PJ6).
- **Empty:** a bundle with no declared capabilities shows "None".
- **Error:** every Blocked and Revoked reason renders its suffix (table-driven test over the chrome model).
- **Disabled:** Admit... shows disabled with a hover reason when the role cannot admit but admission is otherwise possible.
- **Responsive:** rows wrap at the minimum toolbar width (260 px) with no horizontal scroll; the badge truncates with hover for the full text.
- **Permissions:** Viewer sees no Replace; roles that cannot admit see no Admit...; Revoke appears only when a local admission record exists.

## 8. Out of scope

Charting, dashboard building, report layout and any in-host editor for bundle contents: the bundle page does its own presentation. No new consent surface.
