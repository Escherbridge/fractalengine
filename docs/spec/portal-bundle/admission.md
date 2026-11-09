# Admission, kill switches and static feeds

**Status:** draft, author-only, 2026-11-08. Task T4.1 of `portal_surface_20261107`; built by S4, with the chrome model in S3.
**Ownership.** Governance belongs to `extension_platform_20261107` (A-2): it stores per-peer admission records keyed by `(package_id, content_hash)`, never replicated, and owns trust roots, consent, per-verse policy, the kill switch and audit. This document defines what the portal **reads** from governance and what it **does** with it. It stores no admission state of its own.

## 1. Inputs

| Input | Source | Read how |
|---|---|---|
| Admission record for the pair: `Admitted`, `Revoked { reason }`, or none | governance [A-2] | a cached view on the GUI thread, refreshed by `host/admission` events |
| Installation status: `installed`, `active`, `disabled`, `degraded`, `revoked` | sibling R11 [A-14] | same view |
| Kill-switch layers (sibling R23): (a) local Disable of the installation, (b) `FE_EXT_DISABLE_ALL=1`, (c) removal from the verse policy, (d) the S-PLUGIN signed revocation list | governance | same view; (d) as governance's accepted-list view |
| May-admit verdict for the local identity: trust root, verse policy, role (sibling R18–R19) | governance [A-15] | same view; drives `can_admit` |
| `host/admission` events (sibling R23) | the bus | consumed by the mount layer, so revocation needs no polling |
| Portal-only conditions: platform supported (OD-10), bytes present locally, size within the cap, validator result, integrity | this track | resolved at Open (`protocol.md` §2.4) |

## 2. States

The **admission record** is governance's. The **chrome state** is derived from all §1 inputs by one pure function, `chrome_state(inputs) -> BundleState` (`protocol.md` §8), which S3's chrome model and S4's mount both call. Its rules apply in this order, and the first that matches wins:

| # | Condition | Chrome state | Rendered | IPC |
|---|---|---|---|---|
| 1 | no embedded browser backend | Unavailable | no | no |
| 2 | platform outside OD-10 | Blocked — unsupported platform | no | no |
| 3 | (b) host-wide disable, or (a) the installation is locally disabled | Revoked — kill switch | no | no |
| 4 | the bundle matches an accepted revocation list entry by hash (d), and no audited verse-Owner override applies (§4.7) | Revoked — revocation list | no | no |
| 5 | the pair's admission record is `Revoked`: user revoke, (c) policy removal, or a role drop below policy | Revoked — admission withdrawn | no | no |
| 6 | the blob is not local | Waiting for bundle bytes | no | no |
| 7 | over the size cap (README §8.4) | Blocked — too large | no | no |
| 8 | integrity failure | Blocked — integrity failure | no | no |
| 9 | any other validator refusal (README §6) | Blocked — malformed manifest | no | no |
| 10 | a verse-policy ceiling forbids rendering it (A-12) | Blocked — verse policy | no | no |
| 11 | an `Admitted` record for this exact pair, a sealed build whose publisher matches the record, and governance still allows it | Admitted — <publisher> | yes | yes |
| 12 | otherwise | Static — not admitted on this peer (`can_admit` from the verdict) | yes | no |

- **Revocation is checked before bytes.** Rules 3–5 precede rules 6–9, so a revoked or kill-listed pair is never opened, verified or parsed, let alone rendered. It also shows Revoked while its bytes are absent: fetching the bytes cannot turn it into a rendering.
- **Admitted requires the exact pair and a seal.** An unsealed build can never reach rule 11 (D-X5). An Admitted record for `h1` says nothing about `h2` except through §6.
- The badge texts, accessible names and per-state actions are `ux.md` §3 and §5. Reason suffixes are exactly: "integrity failure", "malformed manifest", "too large", "verse policy", "unsupported platform", "kill switch", "revocation list", "admission withdrawn".

## 3. Transitions

Only an explicit local **Open** ever mounts. A replicated repository-node row, a prefetched blob, a policy row or a revocation list can move a mounted bundle **out of** a rendered state, or change an unopened record, but never into a rendered state (D-X5: replicated rows never auto-execute).

| From | Event | To | Mount effect |
|---|---|---|---|
| closed | **Open** | `chrome_state(inputs)` | mount on the admitted label for Admitted, the static label for Static; none otherwise |
| Waiting (portal still open on that node) | bytes arrive | `chrome_state(inputs)` | mount as on Open; the user's Open is still in force, so this is not arrival-driven execution |
| Static | **Admit…** accepted in governance's consent dialog | Admitted | unmount the static label, mount the admitted label, load the entry: a different origin, so storage does not carry over (`protocol.md` §1.2) |
| Static | Admit… declined or dismissed | Static | none |
| Admitted | **Revoke on this peer** | Revoked — admission withdrawn | §5 |
| any rendered | kill switch (a) or (b) engaged | Revoked — kill switch | §5 |
| any rendered | a list matching the bundle by hash is accepted (d) | Revoked — revocation list | §5 |
| Admitted | (c) policy removal, or the installing identity's role drops below the policy | Revoked — admission withdrawn | §5 |
| Revoked — kill switch or revocation list | the cause is lifted | Revoked — admission withdrawn if the bundle had been Admitted; otherwise Static on the next Open | none; re-admission is explicit (`ux.md` §4.4) |
| Revoked — admission withdrawn | Admit… accepted (the cause is gone) | Admitted | as Static → Admitted |
| Admitted or Static | the repository node's `portal.bundle_asset` changes | §6 | an open portal reloads under the new state |
| Admitted or Static | top-frame navigation to a declared `navigation` origin | external website | unmount (`protocol.md` §2.6) |
| any mounted | **Close** | closed | unmount and unload (`protocol.md` §2.5) |

Every revocation path withdraws the admission record, so lifting a cause never silently restores IPC: the user re-admits. A bundle that was never admitted has no record to withdraw, so once (b) lifts it opens in the static sandbox again.

## 4. The kill list

This section answers the orchestrator's 2026-11-07 16:45 point and is the portal half of the store-client contract v0's rule "a kill-listed web bundle is never rendered".

4.1. **Never rendered.** A web bundle that matches (§4.2) an entry on a revocation list that governance accepted from a revocation-capable trust root (S-PLUGIN, `conductor/open-core.md:54`; `plugin_host_revival_20261031` spec §4) is refused by default: not rendered as Admitted and **not in the no-IPC static sandbox**, except under the explicit, audited verse-Owner override (§4.7). It is never mounted, so the protocol answers `404` for both its labels, and a bundle already open is unloaded within one frame of the list being accepted (§5).
4.2. **Match by hash, under any id** (steer 2026-11-08 20:45; same rule as `docs/extensions/package-format.md`). An entry matches a bundle when the entry's `content_hash` equals the bundle's `content_hash`, OR the entry names the bundle's archive digest or the digest of any of its members. The entry's `package_id` is informational and never narrows the match, so renaming a killed bundle does not escape the list. `content_hash` is the lowercase-hex BLAKE3 of the canonical manifest with the signature blanked. The portal needs no kind and reads the entry fields it needs from governance's accepted-list view; it adds no field to the list. Governance exposes the match as `RevocationSet::is_revoked_any(&[digests])`; the portal passes the bundle's `content_hash`, archive digest and member digests.
4.3. **Unsealed builds** have `content_hash` = BLAKE3 of the archive (README §2.3), so a list can name one, and matching is the same hash rule.
4.4. **Order.** The list check is §2 rule 4, ahead of reading any byte. It runs at Open and again for the mounted pair whenever governance accepts a new list.
4.5. **No second list client.** The portal never fetches, verifies or caches lists. It reads governance's accepted-list view [A-2]; offline, the last good list applies (plugin_host_revival OD-8).
4.6. **Residual.** The residual narrows to genuine repackaging: new bytes under a new signature (or none), which no hash on the list covers. Only publisher-key revocation catches that. The static sandbox renders unsigned builds, so a killed bundle's members repackaged as a new unsigned build with new bytes escape the hash match, by contract. The repackaged build gets only the static sandbox: no IPC, no actions, and data only through share URLs an Editor attached to the node. A verse that must not render unsigned bundles at all needs a policy ceiling (A-12).
4.7. **Verse-Owner override** (user decision 2026-11-08 22:00; same rule as extensions). A verse Owner may explicitly override a kill-list refusal for that Owner's own peer. The override is local to that peer, is audited by governance (`RecordKillListOverride`), and never applies to other peers or non-Owner roles. Without it the bundle stays refused. On revocation of an open mount, the portal unloads, and rows written through the bundle's IPC actions stay: there is no withdrawal (OD-E1).

## 5. Revoke mid-session

Triggers: a `host/admission` event for the mounted pair carrying any kill-switch layer, a list acceptance, a policy removal or a role drop, or the user's **Revoke on this peer**. The mount layer drains the event in the same Bevy `Update` it arrives in and runs steps 1–5 in that system, in this order:

1. **Remove the mount.** From here the protocol answers `410` to in-flight requests and `404` to new ones; no byte is served after the decision.
2. **Bump the document generation.** The dying document's posts now fail `ipc-v1.md` §2 check 2, and the host issues no further replies or deliveries.
3. **Close every subscription** with the broker. `act` and `invoke` calls not yet handed to the DB or the broker are dropped. A commit the DB already admitted completes with its true receipt: revocation is not rollback and not withdrawal (sibling R23, OD-E1); the rows stay.
4. **Unload** the document with `load_html` under a one-shot unload token (`protocol.md` §2.5, §6.7).
5. **Emit** `BundleState { Revoked(reason) }`; the badge changes in place (`ux.md` §4.4).
6. **Clear storage** for the bundle's labels on Windows (SP-6), asynchronously. It starts on the unload's `NavigationCompleted` (step 4), never before: clearing while the dying document is still loaded would let it rewrite the storage.

Governance withdraws the admission record and audits the revocation itself (sibling R21). The order is the point: the mount goes before anything else so no byte is served after the decision, and the generation bumps before the unload so the dying document cannot act in between. R-P4 checks that steps 1–5 land within one frame of the decision.

## 6. Update policy (OD-7)

A new build reaches a repository node when an Editor writes a new `portal.bundle_asset`, or when a peer replicates that write. "Increase" is README §7.3.

| Current | New build | Result |
|---|---|---|
| Admitted `(P, pkg, h1)` | sealed by `P`, no capability increase | Admitted for `h2` once governance carries the record over [A-2]; the admitted label and its storage are kept |
| Admitted `(P, pkg, h1)` | sealed by `P`, a capability increase | Static sandbox on `h2`'s static label until re-consent; Admit… shows the new capabilities |
| Admitted `(P, pkg, h1)` | sealed by another publisher | Static sandbox; a new admitted label if admitted later; nothing is inherited |
| any | unsealed | Static sandbox |
| Revoked — admission withdrawn for `h1` | any `h2` | Static sandbox; a withdrawal is per pair |
| any | `h2` is kill-listed | Revoked — revocation list |

- An open portal on the old build reloads under the new state (`ux.md` §4.5).
- **Divergence to settle with governance.** Sibling R20 re-prompts on **any** capability-digest change, a decrease included, while OD-7 keeps a decrease Admitted. Governance wins (A-2): if it re-prompts on decreases, a decrease also drops to the static sandbox until consent.

## 7. The static sandbox and its feeds

7.1. **What a static mount gets:** its `web/` members on its static origin, the composed CSP (`protocol.md` §5), and `/__fe/feeds.json`.
7.2. **What it never gets:** `window.fe` (`/__fe/portal-v1.js` answers `404`), any IPC (posts drop at `ipc-v1.md` §2 check 1), actions, hooks, streams, intents, or the admitted origin's storage.
7.3. **`/__fe/feeds.json`** is built at mount from the repository node's `portal.feeds` (README §9.3):

```json
{ "v": 1, "feeds": { "flow": { "url": "http://127.0.0.1:8765/api/v1/shared/<token>", "expires_at": "2026-11-09T18:00:00Z" } } }
```

  `expires_at` is read from the token payload's `exp` without verifying it. It is a display hint; the share route is the authority.
7.4. **Feed admission.** A feed URL MUST be exactly `<origin>/api/v1/shared/<token>`, where `<origin>` is either this peer's own fe-api origin (loopback, as configured; default `http://127.0.0.1:8765`, `fractalengine/src/main.rs:711`) or an `https` origin that passes `is_url_allowed` (a reachable shared fe-api such as a relay). Any other entry is left out of `feeds.json`. The static mount's `connect-src` gains the exact origins of the kept feeds and nothing else.
7.5. **Without feeds**, `connect-src` is `'self'`, so any API fetch is a CSP violation (R12).
7.6. **Redemption is per peer.** A share token verifies only under the `share_signer` of the fe-api that minted it (`fe-api/AGENTS.md` §share, DEC-C9). A feed minted on a desktop peer's loopback API therefore redeems on that peer only; on other peers it answers `401` and the page shows its own error state. Feeds meant for every peer are minted on a shared fe-api reachable from each peer, such as a relay with a persistent `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`. Expiry answers `410` (TTL at most 24 h).
7.7. **Feeds are published data.** A share URL is a bearer credential, so attaching one publishes that feed to everyone who can read the node (README §9.3).

## 8. CORS and private-network analysis for the share route

8.1. **Today** one global `CorsLayer` built from `cors_origins` covers every route (`fe-api/src/server.rs:91-118`), and `GET /api/v1/shared/{token}` is a public route whose signature is the credential (`:129-133`).
8.2. **The rule (S4, wired by SI).** On `GET /api/v1/shared/{token}` and its preflight only, answer `Access-Control-Allow-Origin: <origin>` (plus `Vary: Origin`) when the request `Origin` matches exactly `https://fe-portal.s-[a-z2-7]{26}.localhost`:
  - **static labels only, never `a-` labels.** An admitted bundle's data path is IPC, with budgets and audit; a share-route grant would let it bypass both;
  - **this route only**, methods `GET` and `OPTIONS`;
  - **no `Access-Control-Allow-Credentials`**: the token is the credential.
8.3. **A pattern, not the mount table.** fe-api runs on its own thread, and on a relay in another process, so it has no mount table. The pattern admits only origins that exist solely inside the portal webview: wry intercepts every `https://fe-portal.*` request (`protocol.md` §1.6), so no website can present that `Origin` from a browser. A non-browser client can forge any `Origin`, but the route is public anyway; CORS protects browser reads, not the endpoint.
8.4. **Mixed content.** A static origin is `https`; the local API is `http://127.0.0.1`. Chromium treats loopback as potentially trustworthy, so no mixed-content block is expected, and the composed CSP carries no `upgrade-insecure-requests` that would rewrite it (`protocol.md` §5.4). SP-8 confirms.
8.5. **Private network access.** A static document came from an intercepted response, so which address space Chromium assigns it is unknown. That decides whether a fetch to `127.0.0.1` triggers a private-network preflight (`Access-Control-Request-Private-Network: true`) or a local-network permission request.
  - If a preflight fires, the share route also answers `Access-Control-Allow-Private-Network: true`, for the same static-label origins only.
  - If a permission request fires, the backend's WebView2 permission handler grants it only to a static-label document fetching a feed origin of its own mount, and denies it everywhere else.
  - SP-8 records which one happens.
8.6. **Exports** (P6) do not use this rule. The operator lists the export origin in `cors_origins` (`static-export.md` §4).
8.7. **Fallback.** If SP-8 fails, OD-6's alternative applies: a host-served same-origin proxy at `/__fe/feeds/<name>`, with `feeds.json` carrying those same-origin URLs. Bundle code does not change, because it always reads URLs from `feeds.json` and never builds them.

## 9. Storage clearing and residuals

- On revoke, the host clears the bundle's label storage through SP-6 when that works. `clear_all_browsing_data` (`wry lib.rs:2195`) clears every origin and is never used for this.
- **Without SP-6**, a revoked origin keeps its storage on disk. It is never mounted again until re-admitted, and on re-admission by the same publisher it sees its pre-revocation storage, because the admitted label is per `(publisher, package)`. PJ3 records the residual.
- A static label's storage persists across opens of the same build. It is the same content, so that is harmless.

## 10. Assumptions

- **A-2** (plan §0): governance owns admission records keyed `(package_id, content_hash)` with change notifications, consent, the policy check and a local kill-switch action. *Medium.*
- **A-12** (README §11): no policy ceiling forbids static rendering by default. *Low.*
- **A-14** (`ipc-v1.md` §11): an admitted bundle is an installation of kind `web-bundle` without an instance, so R11 status and R23 (a) apply to it. *Medium.*
- **A-15. A synchronous view.** Governance exposes a cached, synchronous view of records, installation status, the accepted list and the may-admit verdict to the GUI thread, refreshed by `host/admission`, so `chrome_state` never blocks a frame. *Medium.*
- **A-16. Event payload.** `host/admission` events carry `{package_id, content_hash, state, reason}`, with `reason` distinguishing the R23 layers, so the chrome can name the cause. *Low.*

## 11. Test-table seeds (S4, S3)

- **Precedence (S3 chrome model, pure):** one row per §2 rule, including a kill-listed pair whose bytes are absent (Revoked, not Waiting) and an unsealed build with a stale Admitted record (Static).
- **Revocation (S4, extends `fe-webview/tests/portal_ipc_journey.rs`):** a mounted admitted bundle with three live subscriptions and one queued `act`; a list entry matching the bundle's `content_hash` under a different `package_id` is accepted; then the protocol answers `410` or `404`, `live_subscriptions` is 0, the queued `act` is never sent, a post from the old document raises `dropped_origin`, and re-Open renders nothing.
- **Update (S4):** v1 Admitted; v2 with the same capabilities keeps the admitted label; v3 adding a key scope opens Static on a new label.
- **Admission read (S4, extends `fe-api/tests/portal_bundle_test.rs`):** the summary's `admission` follows governance for the local identity and is absent for a remote token.
- **Native:** PJ2, PJ3 and PJ9 (`tools/qa/portal/journeys.md`).
