# `fe.portal.v1` IPC contract

**Status:** draft, author-only, 2026-11-08. Task T2.1 of `portal_surface_20261107`; built by S2.
**Written against assumption A-1.** EXT-PLATFORM's integration-bus contract (`docs/extensions/bus.md`) had not posted its milestone when this was written. Every bus dependency below cites the sibling spec's draft requirements (R13–R15, R21–R24) and is marked **[A-n]** (§11). When `bus.md` lands, S2's grill re-verifies each marked line; a difference is resolved in favour of the sibling.
**Versioning:** `fe.portal.v1` is a public contract that bundle authors implement. It is never changed in place.

The design in one sentence: **the host authorizes the sender before it reads a byte of the message, judges every action as the local identity that admitted the bundle, and is one more client of the sibling's broker, never a second data path.**

## 1. Transport

1.1. **Page → host.** `window.ipc.postMessage(string)`, received by a native `ICoreWebView2::add_WebMessageReceived` handler, never wry's `with_ipc_handler` (SP-9: wry builds the request with `Request::builder().uri(url)...unwrap()`, and a `Source` over 65 534 bytes panics inside a COM callback and aborts the process). The handler is registered the way `qa_navigation.rs:139-185` registers its navigation guard and reads `Source` capped at 8 192 bytes and the body capped at 65 536 bytes; anything over either cap is dropped as `dropped_origin` before any parse. wry still defines `window.ipc` on every document (`webview2/mod.rs:884-889`). The bounded `Source` is the authorization input. Because `window.ipc` exists on every page, external sites included, authorization goes by sender, never by whether the API is present.
1.2. **Host → page on Windows.** `ICoreWebView2::PostWebMessageAsJson` to the top-level document, issued only while §2 checks 1–2 hold at send time; SP-3 proves that a post issued before a navigation never reaches the next document (`protocol.md` §10). The page API listens on `window.chrome.webview` and drops any envelope whose `n` is not its own document token. There is no `eval` of variable code, honouring "All JS↔Rust calls via a versioned typed command enum (typed IPC) — no raw eval" (`conductor/tech-stack.md:139`).
1.3. **Host → page elsewhere** (OD-2; blocked in wave 1 by OD-10): a fixed-template `evaluate_script` whose only variable is a `serde_json` literal, the fixture-surface approach (`backends/fixture_surface.rs:510-526`).
1.4. **Document generation and token.**
  - The host keeps one `generation` counter per webview. It bumps on every admitted top-frame navigation (same-origin reload, back and forward included), and on Close, hide, leave, revoke, unload and destroy. It never bumps on reposition or resize. The fixture capture clock does bump on `reposition` (`backends/tauri.rs:289-290`), so it MUST NOT be reused: that would cut IPC on every window resize.
  - When `/__fe/portal-v1.js` is served to an Admitted mount, the protocol handler mints the generation's **document token**: 128 random bits as 32 lowercase hex characters, one per generation, embedded as a string literal in the fixed script template.
  - A message is current only when its `n` equals the token of the current generation. After a reload starts, the old document can keep running script for a moment and posts with the same origin; its old token is what tells it apart. Without the token, the generation check could not distinguish a stale document from the current one.
1.5. **Top-level only.** Replies and deliveries reach the top-level document, so the page API is supported there only. A same-origin frame's calls never resolve.

## 2. Authorization order

Cheap checks come first. Nothing is decoded before checks 1–3 pass.

| # | Check | On failure |
|---|---|---|
| 1 | **Sender.** `origin::parse(uri)` (`protocol.md` §1.5) yields a label that is a currently mounted **Admitted** mount. | dropped, no reply; counter `dropped_origin` |
| 2 | **Current.** The portal is visible, and the body starts byte-for-byte with `{"v":1,"n":"<token>","id":` where `<token>` is the mount's current document token (constant-time compare). | dropped, no reply; counter `dropped_origin` |
| 3 | **Budget.** The body is at most 65 536 bytes, the decimal `id` that follows the prefix is at most 10 digits, and the page's message bucket allows it (§8). | reply `invalid` or `rate_limited` with `re: <id>` |
| 4 | **Decode.** `{` + the body after the verified prefix and `id`, decoded strictly as `PageMessage` (§3.5). | reply `invalid` |
| 5 | **Capability.** The mount's binding grants the request: declared in the manifest and permitted to the installing identity (§6.5). | reply `capability_denied` |
| 6 | **At use.** The broker or DB check passes, judged as the installing identity at write time. | reply `role_denied`, `not_found`, `policy_denied`, `rate_limited`, `revoked`, `unavailable`, `backend` or `unknown_outcome` |

- An external site, a static bundle, a foreign frame, a hidden portal and a stale document all stop at 1–2 and learn nothing: no reply, no timing difference in the reply path, and no audit row, so an external page cannot fill the audit log (§9).
- The fixed prefix makes checks 2 and 3 string operations. The page API always emits `v`, `n` and `id` first and in that order (`JSON.stringify` keeps insertion order).
- **Why decode the remainder.** serde's `deny_unknown_fields` does not combine with `#[serde(flatten)]`, so an envelope struct that flattens the message enum would silently accept unknown keys. Decoding only the part after the verified prefix lets the strict, internally tagged enum stand alone.

## 3. Messages

3.1. **Page → host envelope:** `{"v":1,"n":"<token>","id":<u32>,"t":"<type>", …fields}`. `id` is unique within the document and increases.

| `t` | Fields | Reply `data` |
|---|---|---|
| `hello` | none | `{api, capabilities, context, limits}` (§4.3) |
| `subscribe` | `stream`, `params` | `{sub}`, then deliveries |
| `unsubscribe` | `sub` | `{}` |
| `act` | `op: "set_property"`, `node_id`, `key`, `value` | `{node_id, key}`, after the DB result |
| `invoke` | `package`, `command`, `args` | `{result}` |
| `emit` | `stream`, `data` | `{}` |
| `intent` | `intent: "focus_node" \| "select_node"` with `node_id`; or `intent: "tour_nodes"` with `node_ids` (1-32), optional `speed_mps` (0.5-500, default 20), optional `dwell_s` (0-30, default 2) | `{}` |

3.2. **Reply:** `{"v":1,"n":"<token>","re":<id>,"ok":true,"data":{…}}` or `{"v":1,"n":"<token>","re":<id>,"ok":false,"error":{"code":"<code>","message":"<text>"}}`.
3.3. **Delivery:** `{"v":1,"n":"<token>","sub":<u32>,"seq":<u64>,"kind":"snapshot"|"delta"|"resync_required"|"ended","data":{…}}`, plus `"dropped":<u64>` when the host dropped events for this subscription since the last delivery. `seq` starts at 0 with the first delivery and increases by exactly 1. `ended` carries `{reason}` (`bus.md` §5 reasons, plus `revoked` and the binding change of §5.6) and is the subscription's last delivery; the page may subscribe again.
3.4. **Message text** is short and fixed per code. It never echoes page input.
3.5. **Rust shape** (S2 may rename):

```rust
#[derive(serde::Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
enum PageMessage {
    Hello {}, // an empty struct variant, so deny_unknown_fields covers it too
    Subscribe { stream: String, params: serde_json::Map<String, serde_json::Value> },
    Unsubscribe { sub: u32 },
    Act { op: ActOp, node_id: String, key: String, value: serde_json::Value },
    Invoke { package: String, command: String, args: serde_json::Value },
    Emit { stream: String, data: serde_json::Value },
    Intent { intent: UiIntent, node_id: String },
}
```

## 4. Page API

4.1. `/__fe/portal-v1.js` is served only to Admitted mounts (`protocol.md` §3.4) and defines a frozen `window.fe`:

```js
fe.api                                            // "fe.portal.v1"
fe.hello()                                        // Promise<Hello>
fe.subscribe(stream, params, onDelivery)          // Promise<{ id, unsubscribe(): Promise<void> }>
fe.act({ op: "set_property", node_id, key, value }) // Promise<{ node_id, key }>
fe.invoke({ package, command, args })             // Promise<any>  (the command's result)
fe.emit(stream, data)                             // Promise<void>
fe.intent({ focus_node: id } | { select_node: id } | { tour_nodes: [id, ...], speed_mps?, dwell_s? }) // Promise<void>
```

4.2. **Semantics.**
  - Every promise resolves with the reply's `data` and rejects with a `FePortalError` `{name: "FePortalError", code, message}`; codes are §7.1.
  - The page API enforces the envelope size and message rate locally and rejects with the same codes, so a well-behaved page never reaches the host's silent drops.
  - A request with no reply within 10 s rejects with `timeout`. Dropped messages get no reply by design, so this is how a page notices it was cut off.
  - `onDelivery` receives `{kind, seq, data, dropped}`.
  - On a static mount the script answers `404`, so `window.fe` is `undefined`. Bundles test `if (!window.fe)` and degrade to static behaviour.
4.3. **`hello` data.**
  - `api`: `"fe.portal.v1"`.
  - `capabilities`: the granted set as strings — `subscribe:node`, `subscribe:readings`, `subscribe:query/<name>`, `subscribe:<package_id>/<stream>`, `act:set_property`, `invoke:<package_id>/<command>`, `emit:<stream>`, `intent:focus_node`, `intent:select_node`, `intent:tour_nodes`. Granted means declared and currently permitted to the installing identity (§6.5). It is informational: every call is re-checked at use.
  - `context`: `{package_id, content_hash, version, publisher_did, repository_node_id, petal_id}`; `petal_id` is the binding petal, the repository node's petal.
  - `limits`: the §8 numbers.

## 5. Streams and delivery sources

The page never polls REST on a timer: every `GET /api/v1/hierarchy` also rebuilt the GUI tree under polling (QA-0125). `node` and `readings` are push-only; `query` follows the sibling's cadence.

5.1. **`node`** consumes `host/node-changes` [A-1], "the scene change feed that the `/ws` subscription carries" (sibling R13).
  - `params`: `{petal_id?, node_id?}`. A `petal_id` other than the binding petal, or a `node_id` outside it, answers `not_found`, exactly as REST does (§existence-oracle).
  - First delivery `snapshot`, in parts (`bus.md` §2.1): `{part, last, nodes}`, at most 500 nodes (`NodeDto` plus `properties`) per part, repeated until a part carries `last: true`. Then `delta`: `{changes}`, the `SceneChange` list exactly as `/ws` sends it, so one parser serves both.
  - Broadcast lag or a full queue delivers `resync_required` and then a fresh snapshot **on the same subscription** (`bus.md` §2.1 "Loss"); pending coalesced deltas are discarded. The page clears its view and waits for that snapshot; it does not subscribe again (a second subscription would count against the 16).
5.2. **`readings`** consumes `host/readings` [A-1], "rows accepted by the readings door, both local and replicated" (sibling R13).
  - `params`: `{node_id?, metric?}`, confined to readings anchored in the binding petal.
  - Deltas only (`bus.md` §2.2): no snapshot is delivered. Each `delta` carries `{readings: [...]}` (key `readings`, not `rows`; at most 1 000 per event, each with `reading_id, node_id, metric, value, units, recorded_at, recorded_at_ms, source_did, origin`). History comes from a declared query, not from this stream.
5.3. **`query`** consumes `host/query` [A-1]: one guarded `SELECT` run through fe-api's query guard as the installing identity (sibling R13).
  - `params`: `{name, args}`. `name` is a declared query; `args` supplies every declared parameter with a value of its declared type, bound through the query `vars` map (`QueryRequest.vars`, `fe-api/src/types.rs:278-284`). A `node_id` argument outside the binding petal answers `not_found`. Undeclared SQL is impossible: the page only ever names a query.
  - Deliveries are `snapshot` only, with `{data: [rows], row_count, truncated}` (`bus.md` §2.3). Over a cap the rows are cut at the boundary and `truncated` is true; the subscription continues. The host sends one only when the result digest differs from the last one delivered.
  - Cadence is the sibling's OD-3 answer (`bus.md` §2.3, normative): a query re-runs **only when something in its scope changed** (a write to a table it reads, in the binding petal), with a coalescing floor of at least 1 s between runs and no fixed interval, so an idle portal adds no reads. A query over a table with no change feed is refused at subscribe (`unavailable`). Each run is charged, together with REST, to the identity's `QUERY_RATE_PER_SEC` of 10 (`fe-api/src/limits.rs:18`). REST's caps apply, 10 000 rows and 8 MiB (`:4,6`); a run over a cap delivers the cut snapshot with `truncated: true`.
5.4. **Bus streams** (`hooks`): `stream` is `<package_id>/<stream>` and MUST be a declared hook. The host subscribes through the broker as a client of caller kind `portal` [A-1, sibling R14-R15]. Deliveries are `delta` only, and their `data` is a **wrapper** holding the events, not a bare event (§7.4). Broker drop-oldest counts arrive as `dropped`.
5.5. **Coalescing.** At most one delivery per subscription per 250 ms (4 Hz).
  - `node` deltas coalesce per node id, last state wins, so the last delivery always equals the final state. R9's "100 changes in 1 s produce at most 5 deliveries" follows from 4 Hz plus one leading delivery.
  - `readings` deltas concatenate up to 1 000 readings; beyond that the oldest rows are dropped and counted in `dropped`.
  - Bus events concatenate up to the broker's subscriber queue bound, dropping the oldest (sibling R15).
5.6. **Lifetime.** A subscription belongs to one document generation and ends at any bump (§1.4). In wave 1 no bundle document lives on hidden: Close, hide and leave all unload (`protocol.md` §2.5-2.6). The spec's "pauses while hidden" is therefore realised as "ends at hide, and the reopened document subscribes again".

## 6. Actions, judged as the installing identity

6.1. **`act` — `set_property`.**
  1. `key` matches a declared `commands.set_property.keys` pattern, and is not reserved (`node_kind`, `portal.*`, README §5.1), else `capability_denied`. Both checks run before anything reaches the DB.
  2. `node_id` is in the binding petal, else `not_found`.
  3. `value` is at most 16 KiB of JSON, else `invalid`; the `set_property` bucket allows it, else `rate_limited`.
  4. The write rides the sibling's extension write path as a one-op commit with host-stamped provenance `{package_id, content_hash}` [A-13; sibling R5, R22, R24]. The DB re-checks the installing identity's role (Editor or above at the petal), the capability and the stored ancestry at use (`fractalengine/src/AGENTS.md` plugin scene bridge), and refuses with `role_denied`.
  5. The reply waits for the DB result. A transport timeout answers `unknown_outcome` and is never retried (fe-plugin `src/AGENTS.md` §Scene host).
  - Replicas judge the resulting row by its author's role when written (QA-0131). Revocation never withdraws rows (OD-E1, user decision 2026-11-08 22:00): it stops the bundle after in-flight writes complete, and rows already written through its actions stay, exactly as an extension's do (`admission.md` §4.7, §5 step 3).
6.2. **`invoke`.** The target is a declared `commands.invoke` entry, else `capability_denied`. `args` is at most 16 KiB. The broker call carries the caller `{kind: "portal", did: <installing identity>, package_id, content_hash, role_ceiling}`; `role_ceiling` is the installing identity's role at the bundle's scope and never above the viewer's token (steer 2026-11-08 23:40). A portal is a local caller, so the bus answers it with local codes (`bus.md` §4): no active instance at the petal is `unavailable`, a command not exposed or a role below its floor is `capability_denied`. The reply is `{result}`.
6.3. **`emit`.** `stream` is a declared `streams` id, `data` is at most its `max_event_bytes`, and the rate is at most its `max_rate_hz` (else `rate_limited`). The host publishes it as `<package_id>/<id>`; only packages that declared it under `consume`, were consented and satisfy `consumers` receive it (sibling R14).
6.4. **`intent`.** The intent is declared, and `node_id` is in the binding petal. The host turns it into the existing fe-ui focus or select action. It is local UI only, never a write, and it is not audited. `focus_node` flies the camera to the node, `select_node` selects it through the normal selection path, and `tour_nodes` moves the camera through the nodes in order (a whole tour costs one intent token). An id that is unknown, hidden or outside the binding petal is dropped by the host with a log line, never an error to the page, and a tour skips such ids. Tour options out of range are `invalid`, not clamped. Any user camera input, another focus request or a petal change cancels a running tour. A tour is local to this peer and is never replicated. `tour_nodes` is its own capability, so a bundle that declares only `focus_node` cannot tour.
6.5. **The binding.** At mount, the host mints one binding per mount from the governance admission record plus the validated manifest: `{mount_id, package_id, content_hash, publisher_did, installing_did, role_ceiling, petal_id, repository_node_id, declared}`; `role_ceiling` is the bus `Caller.role_ceiling` of §6.2, and a broker client reported above the viewer's token is closed unused. This follows the `PluginAdmission` pattern, a "host-minted capability binding; never deserialized from plugin request data" (`fe-runtime/src/plugin_host.rs:6`). Nothing in it ever comes from page data. The installing identity is the local identity that admitted the bundle; each desktop peer has one. The binding dies with the mount.

## 7. Errors and counters

7.1. **Codes.**

| Code | Meaning | Sibling `host-error` | Counter (§7.3) |
|---|---|---|---|
| `invalid` | over size, bad envelope, failed strict decode, wrong argument type | `invalid-input` | `invalid` |
| `rate_limited` | a bucket is empty, or too many pending replies | `rate-limited` | `rate_limited` |
| `too_many_subscriptions` | a 17th live subscription | `budget-exceeded` | `too_many_subscriptions` |
| `capability_denied` | not declared, a reserved key, or not granted (an undeclared `subscribe` stream included) | `capability-denied` | `refused_capability` |
| `policy_denied` | the verse policy refuses it (sibling R18) | `policy-denied` | `refused_capability` |
| `role_denied` | the DB refused the write at use | (DB refusal) | `refused_role` |
| `not_found` | out of scope or unknown, the same answer REST gives | uniform not-found | `refused_role` |
| `revoked` | the mount was revoked while the call was in flight | `revoked` | `refused_capability` |
| `unavailable` | the broker or the target extension is not running | `not-available` | `failed` |
| `backend` | a broker or DB failure | `backend` | `failed` |
| `unknown_outcome` | a write timed out; never retried | `unknown-outcome` | `failed` |
| `timeout` | page side only: no reply in 10 s | none | none |

7.2. Wire codes are snake_case; the sibling's WIT `host-error` is kebab-case. The table is the one mapping.
7.3. **Counters** are host state, per mount and in total, exposed by the fixed `ipc-stats` QA operation (`protocol.md` §9): `accepted`, `refused_capability`, `refused_role`, `rate_limited`, `dropped_origin`, `invalid`, `failed`, `too_many_subscriptions`, `live_subscriptions` and `deliveries`. The first six are the spec's outcome classes (spec §6.7). `deliveries` counts §3.3 subscription deliveries (any `kind`), never §3.2 replies. The flat counters of an `ipc-stats` reply are process totals (a drop from a foreign origin counts only there); `mount` holds the last counted mount's.

7.4. **Stream deliveries, as the page receives them** (from code; the path after each statement is its source).
  - Envelope: `{v, n, sub, seq, kind, data}` plus `dropped` when above 0 (`ipc.rs` `deliver`). The page API hands `onDelivery` `{kind, seq, data, dropped}` with `dropped` defaulting to 0 (`portal-v1.js`, delivery handler). `seq` counts this page subscription's deliveries from 0; it is the bridge's own counter, never the bus's.
  - **Foreign extension stream** (`<package_id>/<stream>`): `kind` is always `delta`, and `data` is `{"events": [<event>, ...]}`, where each `<event>` is the producer's published JSON **bare**, exactly as the extension published it (no `source`, `at_ms` or per-event `seq`). Events keep publish order, at most 256 per delivery (`MAX_EVENTS_PER_DELIVERY`); beyond that the oldest are dropped and counted in `dropped` (`ipc.rs` `StreamKind::Bus` in `merge` and `take_delta`; `limits.rs`). A window with no event delivers nothing. There is no snapshot and no `resync_required` for a foreign stream: its bus queue overflows by drop-oldest (`fe-plugin/src/bus/broker.rs`, queue setup `(_, false) => (EXTERNAL_QUEUE, Overflow::DropOldest)`), so loss shows only in `dropped`. A `host/query` delta uses the same `{"events": [...]}` wrapper; `node` uses `{changes}` and `readings` uses `{readings}`.
  - **`resync_required`**: `data` is `{}` (`fe-plugin/src/bus/broker.rs`, `resync`). Only `node` subscriptions produce it. The bridge discards its pending deltas, counts them in `dropped`, and delivers it (not while the subscribe reply is still pending); a fresh `snapshot` follows on the same subscription.
  - **`ended`**: `data` is `{"reason": <string>}` (`broker.rs`, `end`), the subscription's last delivery; the page API forgets the callback after it. Bus reasons: `unsubscribed`, `producer-inactive`, `consent-revoked`, `role-lost`, `instance-stopped`, `policy-denied`, `revoked` (`fe-plugin/src/bus/mod.rs`, `EndReason`); the bridge adds `role-changed` when the viewer ceiling moves (`ipc.rs`, rebind). While a `subscribe` has not yet replied, an `ended` rejects it instead: `revoked` or `consent-revoked` gives `revoked`, `policy-denied` gives `policy_denied`, `role-lost` gives `role_denied`, anything else `unavailable` (`ipc.rs`, `end_error`).
  - A foreign event that is not JSON text reaches the page as `null` (`fractalengine/src/portal_host/broker.rs`, `page_event`).
7.5. **What a page receives for each `invoke` outcome.** Every failure is `{ok: false, error: {code, message}}` and the page API rejects with `FePortalError(code, message)`. The message is fixed per code (`ipc.rs`, `IpcError::message`): the guest's own reason, the bus's detail text and the verse policy's reason are never sent to the page. Success is `{ok: true, data: {result}}` and the promise resolves with `result`.

| Outcome | Code | Message | Source |
|---|---|---|---|
| Not a declared `commands.invoke` target, or not granted | `capability_denied` | "This bundle may not do that" | `ipc.rs` `invoke` |
| Bus refuses: command not exposed (a portal is a local caller) | `capability_denied` | same | `portal_host/broker.rs` `bus_error`; `fe-plugin/src/bus/broker.rs` `client_call_command` |
| **Host role refusal** (the installing identity's role is below the command's floor) | `capability_denied` | same | bus check 4 (`reach_command`) answers `CapabilityDenied` to a local caller |
| Schema-invalid `args` (over 16 KiB, or failing the command's `args_schema`) | `invalid` | "The request is malformed" | `ipc.rs` `invoke` (size); bus check 7, `BusRefusal::InvalidInput` |
| Rate limited: invoke bucket, per-caller bucket, per-command bucket, or a full command queue | `rate_limited` | "Too many requests" | `ipc.rs` `invoke`; bus checks 6 and 8 (`admit_call`) |
| The installing identity cannot see the petal at all (`Reach::Hidden`) | `not_found` | "Not found" | bus check 3 (`reach_command`) |
| No active instance for the package at the petal | `unavailable` | "The host service is not running" | `bus_error`, `NotAvailable` |
| **Guest refusal** (`command-error::refused`) | `policy_denied` | "The verse policy refuses this" | `portal_host/broker.rs` `command_reply` |
| Guest `invalid-input` | `invalid` | "The request is malformed" | `command_reply` |
| Guest `failed` | `backend` | "The host failed" | `command_reply` |
| A host-side error answering the call (`HostError`) | `capability_denied`, `policy_denied`, `invalid`, `rate_limited` (budget exceeded too), `revoked`, `unavailable`, `backend` or `unknown_outcome` | per code | `broker.rs` `host_error` |
| No answer: the bus ticket closed, or 15 s passed (`REPLY_DEADLINE`) | `unknown_outcome` | "The outcome is unknown; do not retry" | `broker.rs` `Forward::settle`; `ipc.rs` `poll_calls` |
| Page side: no reply in 10 s (shorter than the host's 15 s) | `timeout` | "No reply from the host" | `portal-v1.js` `REPLY_TIMEOUT_MS` |

  - **Retryable.** The wire carries no retry flag; the only text that says so is `unknown_outcome` ("do not retry"; §6.1 step 5 "never retried"), and an `invoke` may have run the command, so do not repeat it. By cause, not encoded anywhere in the host: `rate_limited` and `unavailable` are transient (retry later, once the bucket refills or the extension runs); `timeout` is ambiguous like `unknown_outcome` (the host may still answer, and a late reply is discarded); `invalid`, `capability_denied`, `policy_denied`, `role_denied`, `not_found` and `revoked` repeat identically until something outside the page changes; `backend` is the guest's or host's own failure and may or may not recur.
  - **Finding: shared codes.** A guest refusal and a verse-policy denial are both `policy_denied` with one message, so a page cannot tell "the extension said no" from "the verse policy said no". Schema-invalid args and a guest `invalid-input` are both `invalid`, and `HostError::BudgetExceeded` folds into `rate_limited`. A host role refusal is `capability_denied`, indistinguishable from "not declared" or "not exposed"; only a write (`act`) reports `role_denied`. Wire behaviour is unchanged here.

## 8. Budgets (OD-8)

Buckets are token buckets: capacity = burst, refilled continuously at rate.

| Budget | Value |
|---|---|
| Envelope size | 64 KiB |
| Messages per page | 50/s, burst 50 |
| Pending replies per page | 64 |
| Live subscriptions | 16 |
| Deliveries per subscription | 4 Hz (one per 250 ms) |
| `set_property` | 2/s, burst 5 |
| `invoke` | 5/s, burst 5 |
| `emit` | per stream, its `max_rate_hz` (at most 10) |
| `intent` | 2/s, burst 4 |
| `act` value, `invoke` args | 16 KiB each |
| `query` re-runs | charged to the identity's `QUERY_RATE_PER_SEC` (10) together with REST |
| Audited check-5 refusals | 2/s, burst 5 (§9) |

- **`invoke` passes three buckets in series**, all token buckets (`fe-plugin/src/bus/rate.rs`, `TokenBucket`: refilled continuously, each call costs one token). (1) The bridge's per-mount bucket above: 5/s, burst 5 (`limits.rs`, `INVOKE_RATE`); a call that finds it empty never reaches the bus. (2) The bus's per-caller bucket: 10/s, burst 10, keyed per portal package (`CALLER_COMMAND_RATE_HZ`, `CallerKey::Portal`). (3) The command's own bucket, per `(package/command, caller key)`: **a token bucket, not a minimum gap**, with rate `min(rate_hz, 10)` (`MAX_COMMAND_RATE_HZ`) and burst `ceil(rate_hz)`, at least 1. So `rate_hz: 2` admits 2 calls back to back and then one per 500 ms on average, and `rate_hz: 0.5` admits one call, then one per 2 s (`fe-plugin/src/bus/broker.rs`, `admit_call` check 6, `TokenBucket::new`). Each answers `rate_limited` (§7.5).
- **`emit`** takes a bridge bucket per stream, rate `min(max_rate_hz, 10)` and burst `ceil(rate)` (`limits.rs`, `TokenBucket::for_stream`), then the bus's own per-`(petal, package, stream)` bucket at the same ceiling (`PORTAL_STREAM_RATE_HZ`); either answers `rate_limited`.
- R10's "20 calls in 1 s → at most 7 accepted" is the burst of 5 plus 2 refilled.
- The `set_property` budget is a quarter of the measured single-peer DB-thread ceiling of about 8 writes/s (commit `b66dfae7`), so a runaway bundle cannot starve the editor.

## 9. Audit

Audit goes through the sibling governance's seam (R21, A-5), attributed to `(package_id, content_hash)` and the acting DID. There is no portal-private log.

| Event | Audited |
|---|---|
| Admission, revocation | yes, by governance |
| `subscribe` reaching check 5 or 6, any outcome | one entry (sibling R15 audits subscriptions) |
| `act` reaching check 5 or 6, any outcome | one entry with the key and a value digest |
| `invoke` | one entry, written by the broker (R15) with caller kind `portal` |
| `emit`, deliveries, `intent`, `hello` | no; data is audited once it becomes rows, and intents are local UI |
| Drops at checks 1–3 | counters only |
| Refusals at check 5 beyond 2/s (burst 5) per page | counters only (`limits::AUDIT_REFUSAL_RATE`), so an admitted page cannot flood the audit log with denials |

## 10. Bus mapping

| Portal operation | Integration bus (sibling R13–R15) |
|---|---|
| `subscribe node` | consume `host/node-changes` |
| `subscribe readings` | consume `host/readings` |
| `subscribe query/<name>` | consume `host/query` with the declared SQL |
| `subscribe <package_id>/<stream>` | consume a foreign extension stream (a declared hook) |
| `emit` | publish `<bundle package_id>/<stream>` |
| `invoke` | call `<package_id>/<command>` with caller kind `portal` |
| `act set_property` | the extension write path, one-op commit with provenance |
| (host-side, not the page) | consume `host/admission` to unmount within one frame (`admission.md` §5) |

## 11. Assumptions

- **A-1. Bus shape** (plan §0). A broker on the plugin worker, callable from the GUI process without blocking over a channel, offering `host/node-changes`, `host/readings` and `host/query` plus declared extension streams, commands and hooks, with caller kinds including `portal`. *Low before S2 builds, high after.*
- **A-13. One-op portal writes.** The sibling's extension write path accepts a single-op commit from a portal client outside any tick, with the same DB re-checks and host-stamped provenance as an extension's tick commit. Its batching (sibling R5) is a tick concept a portal does not have. *Medium.* If declined, `act` falls back to the plugin scene bridge's typed DB command with provenance added there.
- **A-14. A bundle is an installation without an instance.** Governance represents an admitted bundle as an installation of kind `web-bundle` (sibling R11 status vocabulary) that never ticks, so the broker identifies the portal client by that installation and audit rows carry its id. *Medium.*
- **OD-3 (answered, `bus.md` §2.3).** `host/query` re-runs only on a scope change, with a floor of at least 1 s; the bus delivers a snapshot only when the result's digest changed, so the page never sees an unchanged result (§5.3).

## 12. Test-table seeds (S2)

- **R8:** one well-formed `subscribe` from an external site, a static bundle, a foreign frame, a hidden portal and a stale document (old token) each yields no reply and `dropped_origin` + 1; the control row (admitted, visible, current) gets a reply. Mutation check: with check 1 removed, the external and static rows go red.
- **R9:** an idle admitted bundle with `node` and `readings` subscriptions adds zero REST or DB reads over 60 s; 100 transform changes in 1 s yield at most 5 deliveries and the last equals the final state; an out-of-scope `node_id` answers `not_found`.
- **R10:** a declared `set_property` on an Editor peer persists; after demotion to Viewer the same call answers `role_denied` and no row exists; an undeclared key answers `capability_denied` with no DB command sent; 20 calls in 1 s → at most 7 accepted; each accepted call has one audit row with `(package_id, content_hash)`.
- **R11:** an extension consuming a bundle's declared stream receives its `emit`s; an undeclared `emit` or `invoke` answers `capability_denied`; a declared hook's events reach the page.
