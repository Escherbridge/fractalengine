# Painted viewport chrome owns its pointer surface

This implements the separate QA38 armed-Circle/Back reproduction. It does not
claim the original pass05 QA-0024 trigger is reconstructed or explained.

viewport.rs registers actual clipped response rectangles for the active-Petal
navigation header (Back and all wrapped breadcrumbs), browser header/body and
visible tool hints. The passive hint Area is registered explicitly because its
noninteractable setting need not make egui report pointer capture. Registration
does not change visuals, names, keyboard actions, layout or navigation callbacks.

gardener_console starts a pending publication; gardener_ui_system finishes it
only after deriving the existing owned scene interaction rectangle. Navigation
during painting invalidates that pass. The next matching complete paint restores
admission. The router supplies the actual current window and existing navigation
revision; no duplicate node selection or navigation state is created.
The editor plugin explicitly installs the renderer's EditorScenePointerRequired
policy and all editor scene/cursor claims call strict editor admission helpers.
Standalone renderer geometry-only region clients do not opt into this policy.

The browser's actual allocated body response is UI-owned, including blank space
within that response. Unallocated space outside its response remains governed by
the normal scene rectangle and floating layers. Browsing has no active Petal
authoring scope. An active Petal instead excludes only its actual header/hint
surfaces, leaving uncovered scene available for normal tools and camera input.

Press priority and existing gesture hold/release/cancel logic stay in the domain
managers. The same shared current-physical-point gate is used for scene claims and
camera input and world-cursor previews; the viewport's secondary-click producer
requires completed geometry plus chrome/layer checks too. There is no synthetic
pre-move, input-origin branch or new action/DB surface.

The existing armed-shape journey now exercises actual rendered Back and ancestor
controls with ordinary egui press/release: completed AccessKit labels identify real
node IDs joined to observed response hit/clip geometry, without private IDs or
hardcoded coordinates. It asserts no authoring action or document change,
normal navigation, cancellation by navigation, then legitimate uncovered-scene
placement through the shared gate. Existing renderer and divider-boundary journeys
exercise completed snapshot, scale and ownership behavior. They remain authored,
not passed, until root's integrated sweep and live verification.
