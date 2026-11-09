# Static export for the web (P6, later)

**Status:** draft, author-only, 2026-11-08. Task T6.1 of `portal_surface_20261107`; built later by S8. It realises D-X7's "through the web" phase: a static export of the same bundle, talking to the public REST with a scoped viewer token.

## 1. Scope

- **In:** export one repository node's bundle as a static directory that any web server can serve, reading data only through signed share URLs.
- **Out (non-goals):** hosting, CDN, custom domains, auth federation, accounts, payments, a hosted viewer, and signing or verifying exports.
- An export carries no IPC, no actions, no hooks and no page API.

## 2. Export route

`GET /api/v1/nodes/{node_id}/portal/export.zip`, authenticated, Viewer or above with reach to the node.

- Viewer suffices because the export reveals only what a node reader already sees: the bundle members (through asset reach) and the feed URLs (through the node's properties).
- The node must be a repository node (README §9); otherwise, or when unknown or out of scope, the uniform `404` (§existence-oracle).
- `409` with `error: "bundle_bytes_absent"` when the blob is not local, and `409` with `error: "revoked"` when the pair is kill-listed or revoked on this peer. A bundle that may not render here is not exported to render elsewhere (`admission.md` §4).
- `422` with `error: "integrity"` when the archive fails verification under `VerifyIfSealed`.
- `200`: `application/zip`, `Content-Disposition: attachment; filename="<package_id>-<version>.zip"`, `X-Content-Type-Options: nosniff`.

## 3. Layout

```
<export root>/
  fe-portal.json
  __fe/feeds.json
  …every web/ member, with the web/ prefix removed
```

- **No `__fe/portal-v1.js`** (R17).
- **`fe-portal.json`:** `{schema: "fe-portal-export/1", package_id, version, content_hash, publisher_did, title, entry, csp, feeds_dropped, exported_at}`, where `entry` is export-relative, `publisher_did` is `null` for an unsealed build, `csp` is §5's policy, and `feeds_dropped` names feeds §4.2 left out.
- **`__fe/feeds.json`:** the same shape a static mount serves (`admission.md` §7.3).
- **One page code path.** A static bundle reads `/__fe/feeds.json` and fetches the URLs in it, whether it runs in the in-app static sandbox or from an export. The export MUST therefore be served at the root of its origin; path-prefix hosting is out of scope.

## 4. Data path and CORS

4.1. **Share URLs are the viewer token.** Feeds are the existing "signed scoped URL, NOT an embedded bearer JWT" (`fe-api/AGENTS.md` §share): a scope ceiling fixed at signing, a TTL of at most 24 h, public redemption and a per-token rate limit. This is D-X7's "scoped viewer token" (OD-6). No new credential type exists.
4.2. **Reachable feeds only.** A token redeems only at the fe-api whose `share_signer` minted it (`admission.md` §7.6). The exporter keeps feeds whose origin is `https` and passes `is_url_allowed`, and drops loopback feeds, which no remote browser can reach, listing them in `feeds_dropped`. Feeds for an export are minted on a shared fe-api with a persistent share signer.
4.3. **CORS is the operator's.** The minting fe-api lists the export's origin in `cors_origins` (`fe-api/src/server.rs:30-31`, applied by the `CorsLayer` at `:91-118`). The static-label rule of `admission.md` §8.2 never applies to exports.
4.4. **Expiry.** An expired token answers `410`, and the page shows its expired state. An export never refreshes its feeds; an Editor attaches fresh share URLs and the operator re-exports. With a 24-hour TTL ceiling, an export is a short-lived dashboard unless it is re-exported.
4.5. **Private networks.** An export on a public origin calling an fe-api on a private address meets the browser's private-network rules. That is a hosting concern and out of scope.

## 5. CSP

- `fe-portal.json.csp` carries the policy a static mount of this bundle would get (`protocol.md` §5), with `connect-src` limited to `'self'` plus the kept feed origins.
- The operator SHOULD send it as a `Content-Security-Policy` header. The exporter never rewrites HTML members: that would change the bytes the publisher signed, and it would need an HTML parser.
- **Residual.** Served without that header, an export runs without the host's CSP. It still has no page API and no IPC, so its reach is that of any page on the operator's origin.

## 6. What an export does not carry

- `window.fe`, IPC, actions, hooks, streams and intents. The manifest's `subscriptions`, `commands`, `hooks`, `streams` and `ui_intents` are inert outside the app.
- Admission state, document tokens, or anything local to the exporting peer.
- A verifiable signature. `content_hash` in `fe-portal.json` is informational; a static server cannot check it.

## 7. Acceptance (R17, PJ10)

- The exported directory, served by any static server whose origin is in the minting fe-api's `cors_origins`, renders the fixture dashboard from a share URL.
- An expired share URL answers `410`, and the page shows a visible expired state.
- The export contains no `__fe/portal-v1.js`, and `window.fe` is `undefined`.

## 8. Change against plan T6.2

Plan T6.2 has S8 add a "static-export mode of the page API" to `portal-v1.js`. This contract replaces that with **no page API in any static mode**: in-app static mounts and exports share one data path, `/__fe/feeds.json` plus `fetch`. `portal-v1.js` therefore needs no static mode, and S8 does not edit it; S8 owns the export route and its test rows only.
