---
type: reference
title: fe-plugin/src/net — network brokers (ingress, egress)
track: extension_platform_20261107
slice: E5 (P1.5); hardened by N1 (turn 2)
---

# fe-plugin/src/net — network brokers

Spec R8 (ingress) and R9 (egress); manifest fields in `docs/extensions/manifest-v2.md` §ingress/§egress/§auth.
A guest never holds a socket: every connection is the host's, every frame is checked here, and every
delivery reaches the guest through `NetHooks` on the plugin worker.

| File | Owns |
|---|---|
| `mod.rs` | `Net` (state, registration, `dispatch`), credential policy (`is_credential_header`, `is_host_owned_header`, `CredentialStore`), host ceilings, WIT-shaped types (`IngressFrame`, `IngressReply`, `WsFrame`), endpoint states and counters, backoff, `until_stopped`, base64. |
| `ingress.rs` | The HTTP/WS accept mount fe-api routes into, the guest's `ingress.send`/`close`, frame plumbing for dial-outs. |
| `egress.rs` | `egress.http-request` (reqwest) and `egress.mqtt-publish`; URL containment, header policy both ways, redirect policy. |
| `address.rs` | `is_public_unicast` (the one SSRF allow-list, also for registry reads), `resolve_checked`, reqwest's `CheckedResolver`. |
| `connect.rs` | TCP to checked addresses + rustls (webpki roots, ring provider) for dial-outs; plaintext only to loopback. |
| `ws_dial.rs` | The websocket dial-out task. |
| `mqtt.rs` | MQTT 3.1.1 codec + topic-filter matching (pure, table-tested). |
| `mqtt_session.rs` | One MQTT client session per declared broker endpoint (subscribe for ingress, publish for egress). |
| `tests.rs` | One journey, one row per rule below, on a manifest smuggled past validation (batch R11 `net::tests::`). |

## §credentials — the host strips, whatever the manifest says (P2P-QA steer 2026-11-08 20:20)

`is_credential_header` (one list in fe-sdk, shared with the validator and fe-api) is the denylist in code:
`authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key` and every `x-fe-*` header (the
host's own authentication, e.g. `x-fe-join-lease`). It applies in both directions:

- **In to a guest:** HTTP frames keep only declared `forward_headers` *minus* this list
  (`ingress::forwarded_headers`); websocket and MQTT frames carry no headers at all.
- **Out from a guest:** a request that names one of them, or a host-owned header (`is_host_owned_header`:
  framing, hop-by-hop, any `proxy-*`, any `sec-websocket-*`), is refused whole (`egress::request_headers`).
- **Back in from a server:** `egress::response_headers` drops the same two lists, so a `set-cookie` or a
  reflected `authorization` never reaches the guest.

The schema rule (manifest-v2.md V16) is the second layer. The first holds even for a manifest smuggled past
validation, which is what `tests.rs` declares. fe-api reads the bearer token only from the `Authorization`
header, so the forwarded query string is not a credential path; an in-band websocket auth message (fe-api's own
`/ws` style) must be consumed by fe-api and never passed to `websocket_frame`.

Endpoint credentials (`auth.credential` slots) come from the injected `CredentialStore`, are fetched at each
connect or request (so an operator filling a slot needs no restart), are marked `HeaderValue::set_sensitive`,
and never appear in a frame, a status reason or the replicated store (`Credential`'s `Debug` redacts the
secret, and so does `mqtt::Packet`'s for the CONNECT password). An empty slot, an empty secret, or one that does
not fit the scheme (`basic` and `mqtt-password` need a username) keeps the endpoint at `credential-missing`. A
`header`-scheme slot may not name a host-owned header (`host`, `content-length`, `sec-websocket-key`...); naming
`x-api-key` is the normal case: that is the host sending its own operator's secret to the declared host.

**Slots are scoped per package** (P2P-QA steer 2026-11-09 01:50, fixed by R-NET): `CredentialStore::get(package_id,
slot)`, and `Net` always passes the instance's own package id. Two manifests declaring the same slot name read two
different secrets, and there is no fallback to an unscoped slot. An update keeps its package id, so the operator's
slots survive it; a build from a different publisher under the same id is a fresh install with fresh consent
(R-GOV), which is where the operator sees that change.

**Residual, by design:** the guest chooses the path and body of a credentialed request, so a declared host that
echoes request headers back (an httpbin-style `/headers`) hands the guest its own slot's secret. The control is
the operator's: fill a slot only for a host that does not reflect it.

## §credential-store — the binary's `FileCredentialStore` (fractalengine `extension_host/net_sources.rs`)

manifest-v2.md §auth names no store, so N5 chose a local file and R-NET scoped it:
`<extension data dir>/extension_credentials.json`, shaped `{ "<package_id>": { "<slot>": { "username"?, "secret" } } }`.

- **Local only:** it is never replicated, never read by fe-api, and never sent to a guest.
- **Re-read per connect or request:** over 64 KiB or unparseable, every slot reads as empty. Only the fact of
  failure is logged, because serde's error text can echo file content.
- **Written atomically:** `set_slot` writes a temp file, then renames it over the store. The file is created
  `0600` on Unix; on Windows it inherits the per-user data directory's ACL.
- **Format change:** the turn-1 flat `{ "<slot>": ... }` shape never shipped (the build gate was closed). Such a file
  now fails to parse and reads as empty, so it fails closed.

## §ingress — the uniform not-found, then the R8 table

`accept_http` order (fe-api has already authenticated the bearer token and called `Bus::admit_remote`):
1. instance bound at the petal with a declared `http-server` endpoint → else `NotFound`;
2. the caller's role at the petal ≥ `role_floor` (default editor) → else `NotFound`, **audited**;
3. a sub-path only when `sub_paths: true` → else `NotFound`;
4. per-caller-DID rate → 429; method → 405; `max_body_bytes` → 413; media type (parameters stripped) → 415;
5. a full inbox (256) → 429. The receiver resolves with the guest's reply (`None`: fe-api answers 204, no
   body). fe-api waits `INGRESS_REPLY_TIMEOUT_MS` and answers 504 on timeout; a dropped sender means the
   instance stopped (fe-api answers 502 `failed`; the caller was already authorised, so this reveals nothing).

Steps 1-3 never reveal existence: a caller below the floor gets `NotFound` even for a request that would
otherwise be 405, 413 or 415. Once a caller is authorised, 405/413/415 are fine to show. A disabled
installation is simply not registered, so it answers step 1's not-found.

**Decision (E5 question 5, default kept by N1): undeclared or disabled ingress is not audited.** Only the role
floor (step 2) writes a denial: devices hammering an undeclared or disabled endpoint would otherwise flood the
canonical op log, and the uniform not-found already tells them nothing. Counters still record refusals
(`ingress_refused` for floor, sub-path, method and media refusals; `ingress_dropped_*` for rate, size, queue).

Per-caller buckets are kept per `(endpoint, caller DID)`. Past 1,024 tracked callers, buckets idle (and full)
for a minute are swept before a new one is added, so a large membership cannot grow the map without bound.

Websocket accept (`open_websocket`) applies steps 1-2 plus `max_connections` (429). Frames go through
`websocket_frame`: over `max_frame_bytes` or over `rate_hz` (per connection) they are dropped and counted,
never an error to the client. **The floor is re-checked on every frame**: a caller demoted after the upgrade
is sent a 1008 close, removed, audited like a step-2 refusal, and the guest hears `closed` once (the writer
task's later `websocket_closed` is a no-op). A reply to a websocket frame returns on the same connection
(text for `text/*` and `application/json`). `Net::close` never puts a reserved close code (1004-1006, 1015,
outside 1000-4999) on the wire; it sends 1000 instead, and cuts the reason to 123 bytes.

## §dial-outs — targets, tasks, generations and backoff

`register_instance` mounts passive endpoints and spawns one task per `websocket-dial`, `mqtt` ingress and
`mqtt` egress endpoint on the injected runtime (no runtime → `not-available`). **The target is checked at
registration, whatever the runtime.** The endpoint becomes `not-available` with the reason, and no task is spawned,
for any of:

- a plaintext URL beyond loopback;
- userinfo or a query;
- a non-public IP literal (§addresses);
- the wrong scheme for the transport;
- a malformed or empty MQTT topic filter.

So nothing redials a URL the host will always refuse. A hostname is checked at each connect instead, because its
addresses are only known then.

Each task carries the instance's **generation**. Every state change goes through `with_generation`, so a task
that outlives a re-registration can never write into the new instance (`dial_opened` returns `None` and the
task closes its socket). Unregistering flips each task's `watch` stop switch and drops the outbound senders.
`until_stopped` races every slow phase (TCP connect, TLS, the websocket handshake, MQTT CONNACK/SUBACK and
the backoff sleep) against that switch, so a stopped instance's tasks end at once instead of after a
20-second connect. Sends are bounded by `CONNECT_TIMEOUT`, so a stalled server cannot hold a task forever.

A dial-out's declared `max_frame_bytes` is also its transport limit (never below 125 bytes, so a close or ping
still reads). tungstenite refuses an over-cap frame from its header before buffering the payload; the host counts
it as `ingress_dropped_size`, drops the connection, and redials with backoff. The turn-1 code buffered up to 4×
the cap first. A websocket dial-out pings every 30 s and drops the connection if nothing at all (not even a pong) arrived
for two periods; MQTT uses its own keep-alive (PINGREQ every 15 s, two unanswered drop it). Redials use
jittered exponential backoff (`backoff_ms`, `RandomState` as the jitter source: no `rand` dependency) inside
`reconnect_ms` (default 1-60 s; clamped to 0.25-600 s, max never below min); a successful session resets the
attempt counter.

## §mqtt — why a small in-tree codec instead of a client crate

The host needs MQTT 3.1.1 with QoS 0/1, clean sessions, one subscription set or one publish queue, and a
**hard cap enforced before buffering**. `decode` reports `Oversized { total }` from the fixed header alone and
the session's `Reader` skips those bytes as they stream past, so a broker cannot make the host allocate more
than `max_payload_bytes + PACKET_OVERHEAD + READ_CHUNK`, even by announcing the protocol maximum (256 MiB).
Writing that against a third-party client's internals would have cost more than the codec, and it keeps
`deny.toml` and the dependency graph unchanged (every crate this directory uses is already in Cargo.lock). Not
supported, by design: QoS 2, will messages, persistent sessions, retransmission after reconnect. An oversized
QoS 1 PUBLISH is skipped without a PUBACK; with clean sessions the broker does not redeliver it.

`Reader::next` is cancel-safe inside `tokio::select!`: its only await is the socket read, and the bytes are
appended before the next await, so dropping the future loses nothing. `egress.mqtt-publish` queues (WIT:
"queue a publish"): while the broker is down, up to `OUTBOUND_QUEUE` publishes wait for the next session, and
the next one is refused `rate-limited`.

## §egress — the guest names an endpoint, never a host

`endpoint_url` joins the declared base with the guest's path and refuses anything that could leave it:
- dot segments, including a servlet-style `..;` matrix segment;
- `%2e`, `%2f`, `%5c`, `%00`, and `%25` (double encoding, so a server that decodes twice cannot be walked out);
- `%3b`, because a decoding proxy in front of a servlet container turns `..%3b/` into `..;/`;
- `%c0` and `%c1`, the overlong UTF-8 lead bytes behind `%c0%ae` (a dot to legacy decoders);
- `//` at the start, `://`, `?`, `#`, `@`, controls and whitespace;
- more than 2,048 path bytes, more than 64 query pairs, or more than 8,192 URL bytes.

The joined URL must keep the base's scheme, host, port and path prefix, carry no userinfo, and have no `..;`
segment (`within_base`). Redirects follow the same rule, at most 5. Each client uses:
- `no_proxy()`, so the host connects only to the declared host and port;
- `referer(false)`;
- `https_only` unless the base is loopback;
- `http1_only()`, so a manifest-named server never negotiates h2 (RUSTSEC-2026-0258 stays off this path);
- the `CheckedResolver` (§addresses).

An over-cap response is `budget-exceeded`, counted as the bytes arrive (after any decompression), never silently
cut. A connect failure is `backend`; a timeout after sending is `unknown-outcome` (the request may have landed).

Undeclared endpoints, methods and topics are `capability-denied` and audited (R9 AC). MQTT publishes may not
exceed the declared QoS, use a wildcard topic, or set `retain` unless declared. Every refusal of a live
instance increments `egress_refused`.

## §addresses — global unicast only, or the declared loopback (P2P-QA steer 2026-11-09 01:50, SSRF)

Consent shows the declared host. Without this rule, a public-looking name could still resolve to
`169.254.169.254`, a LAN admin page or the host's own API. One predicate, `is_public_unicast` (exported; the
registry sources' `fractalengine/src/extension_host/sources.rs` `is_internal` is its negation, R-X), is an **allow-list**:

- **IPv4:** every address except 0/8, 10/8, 100.64/10, 127/8, 169.254/16, 172.16/12, 192.0.0/24, 192.0.2/24,
  192.88.99/24, 192.168/16, 198.18/15, 198.51.100/24, 203.0.113/24 and 224/3 (multicast, reserved, broadcast).
- **IPv6:** only 2000::/3, minus:
  - 2001::/23 (Teredo, ORCHID, benchmarking);
  - 2001:db8::/32 and 3fff::/20 (documentation);
  - 2002::/16 whose embedded IPv4 is not public.

  Everything outside 2000::/3 is refused by construction: ::/96, ::1, NAT64 (64:ff9b::/96, 64:ff9b:1::/48), ULA,
  link-local and site-local, and multicast.
- **IPv4-mapped IPv6** is judged as its IPv4 address.

`admits(host, ip)` adds the one exception the manifest contract already has. A host declared as exactly
`127.0.0.1`, `::1` or `localhost` may reach loopback, and **only** loopback. Any other name may never reach
loopback, even when DNS answers 127.0.0.1.

Where the rule is enforced:

- **IP literals** are refused at registration (`connect::target`, `egress::build_client`), with no redial loop.
- **Names** are resolved once per connect by `resolve_checked`. A single refused address refuses the whole answer.
  Dial-outs then connect to exactly those addresses. Egress gets the same guarantee through reqwest's
  `dns_resolver(CheckedResolver)`; reqwest never resolves an IP literal, so `build_client` checks those itself.
  Either way, nothing performs a second lookup that DNS could rebind. TLS still verifies against the declared name.

**Consequence:** a private-network broker or device (`mqtts://192.168.1.10`, or `broker.lan` resolving to
10.x) is refused. A site reaches one through a loopback bridge on the host, or by pushing to the host's ingress
mount. Allowing a LAN target per installation would be new operator-acceptance scope (see the R-NET user question).

## §host-ceilings — the schema's numbers, re-applied at runtime

`ExtensionManifest::validate` (V1-V18) does not run the JSON Schema, and a manifest can reach the host
without either (a smuggled or hand-edited package). So the host clamps every number the schema bounds,
at the point of use:

| Field | Host rule |
|---|---|
| `rate_hz` (every endpoint) | ≤ 100 Hz; HTTP egress ≤ 10 Hz; non-finite or ≤ 0 never refills |
| `max_body_bytes`, `max_frame_bytes`, `max_payload_bytes` | ≤ 1 MiB (`HOST_SIZE_CEILING`) |
| `max_connections` | ≤ 256 |
| `max_request_bytes`, `max_response_bytes` | ≤ 8 MiB (defaults 1 MiB) |
| `timeout_ms` | 100-30,000 ms (default 10,000) |
| `reconnect_ms` | 250-600,000 ms (default 1,000-60,000) |

## Seams for other slices

- **N3** (`wasm/component/net_bridge.rs`) implements `NetHooks` over the component and routes WIT
  `ingress.send`/`close` to `Net::send`/`close` and `egress.*` to `http_request`/`mqtt_publish`.
- **N4** (fe-api `extensions.rs`) mounts `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{endpoint}[/{rest}]`:
  - body limit `INGRESS_BODY_CEILING`;
  - `Bus::admit_remote(did)` first, charged once per request;
  - then `accept_http` / `open_websocket`, mapping `IngressRefusal::http_status`.

  A WS writer task drains `WsSession::outbound` and honours `WsOutbound::Close` (the host's 1008 and 1001 closes).
  A drop guard reports `websocket_closed`, so an admitted upgrade that never completes frees its
  `max_connections` slot. The axum socket's frame and message limits are both `INGRESS_BODY_CEILING`.

  fe-api does not depend on fe-plugin, so the binary adapts its `IngressCall` to `HttpIngressRequest`. That
  adapter (`plugin_bridge.rs` `ingress_gate`) applies the floor to the DID's role capped by the token's
  `max_role` (G1); `Net` itself sees no ceiling. That is sound because the ceiling is fixed for the connection's
  life, so `Net`'s per-frame floor re-check gives the same verdict as a capped one.
- **B6** (fe-api `bound_routes.rs`) reaches the same `accept_http` core, so the strip and the R8 order hold for
  the bound alias too.
- **N5** (`fractalengine/src/extension_host/net_sources.rs`) builds
  `Net::new(clock, NetSources { roles, audit, credentials, runtime: Some(Handle::current()) })` over the
  package-scoped `FileCredentialStore` (§credential-store).
- **I-PLUGIN** declares `pub mod net` in `lib.rs`, registers/unregisters instances alongside the bus, and calls
  `dispatch` per instance. Under OD-E1 (no withdrawal) the worker drains the instance's current commit first;
  `unregister_instance` is the stop step and drops queued frames (their HTTP callers see 404). The
  dependencies `fe-plugin/Cargo.toml` needs for this directory are listed in the N1 result (NEEDS).
- **F-DEVADAPT** (device adapter) and **P6** (loopback MQTT broker, loopback HTTP sink) exercise the dial-outs live.

## §shared-egress — one budget per installation (B10d)

`NetSources.shared_egress` is the binding activator's `SharedEgressBuckets` (the binary passes the same handle to
both). HTTP and MQTT egress draw through `bindings::egress::take_egress_token`: an instance a binding keeps uses its
installation's bucket for the endpoint, any other instance its own per-instance bucket. `Default` joins nothing, so
a host without bindings behaves as before.

## §registry-addresses — R-NET-30

`registry_client::RegistryClient` uses the same `CheckedResolver` (exported from `net`) so a registry name resolves
once and connects to exactly the checked addresses. IP-literal base URLs and every redirect hop go through
`literal_refusal`; the documented loopback rule applies (`127.0.0.1`, `::1`, `localhost` reach loopback only, any
other host global unicast only). R-NET-18 stays deferred.
