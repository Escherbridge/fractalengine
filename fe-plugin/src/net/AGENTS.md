---
type: reference
title: fe-plugin/src/net — network brokers (ingress, egress)
track: extension_platform_20261107
slice: E5 (P1.5)
---

# fe-plugin/src/net — network brokers

Spec R8 (ingress) and R9 (egress); manifest fields in `docs/extensions/manifest-v2.md` §ingress/§egress/§auth.
A guest never holds a socket: every connection is the host's, every frame is checked here, and every
delivery reaches the guest through `NetHooks` on the plugin worker.

| File | Owns |
|---|---|
| `mod.rs` | `Net` (state, registration, `dispatch`), credential policy (`is_credential_header`, `CredentialStore`), WIT-shaped types (`IngressFrame`, `IngressReply`, `WsFrame`), endpoint states and counters, backoff, base64. |
| `ingress.rs` | The HTTP/WS accept mount fe-api routes into, the guest's `ingress.send`/`close`, frame plumbing for dial-outs. |
| `egress.rs` | `egress.http-request` (reqwest) and `egress.mqtt-publish`; URL containment, header policy, redirect policy. |
| `connect.rs` | TCP + rustls (webpki roots, ring provider) for dial-outs; plaintext only to loopback. |
| `ws_dial.rs` | The websocket dial-out task. |
| `mqtt.rs` | MQTT 3.1.1 codec + topic-filter matching (pure, table-tested). |
| `mqtt_session.rs` | One MQTT client session per declared broker endpoint (subscribe for ingress, publish for egress). |
| `tests.rs` | Ingress table, credential stripping, websocket limits, egress refusals. |

## §credentials — the host strips, whatever the manifest says (P2P-QA steer 2026-11-08 20:20)

`is_credential_header` is the denylist in code: `authorization`, `proxy-authorization`, `cookie`,
`set-cookie`, `x-api-key` and every `x-fe-*` header (the host's own authentication, e.g. `x-fe-join-lease`).
It applies in both directions: inbound HTTP frames keep only declared `forward_headers` *minus* this list
(`ingress::forwarded_headers`), and a guest's outgoing request that names one of them is refused
(`egress::request_headers`). The schema rule (manifest-v2.md V16) is the second layer; this one holds even for
a manifest smuggled past validation, which is exactly what `tests.rs` declares.

Endpoint credentials (`auth.credential` slots) come from the injected `CredentialStore`, are fetched at each
connect or request (so an operator filling a slot needs no restart), are marked `HeaderValue::set_sensitive`,
and never appear in a frame, a status reason or the replicated store. An empty slot, or one that does not fit
the scheme (`basic` and `mqtt-password` need a username), keeps the endpoint at `credential-missing`.

## §ingress — the uniform not-found, then the R8 table

`accept_http` order (fe-api has already authenticated the bearer token and called `Bus::admit_remote`):
1. instance bound at the petal with a declared `http-server` endpoint → else `NotFound`;
2. the caller's role at the petal ≥ `role_floor` (default editor) → else `NotFound`, **audited** (the only
   audited ingress refusal: undeclared/disabled traffic from devices would otherwise flood the op log);
3. a sub-path only when `sub_paths: true` → else `NotFound`;
4. per-caller-DID rate → 429; method → 405; `max_body_bytes` → 413; media type (parameters stripped) → 415;
5. a full inbox (256) → 429. The receiver resolves with the guest's reply (`None` = 202). fe-api waits
   `INGRESS_REPLY_TIMEOUT_MS` and answers 504 on timeout; a dropped sender means the instance stopped (404).

Steps 1-3 never reveal existence; once a caller is authorised, 405/413/415 are fine to show.
A disabled installation is simply not registered, so it answers step 1's not-found.

Websocket accept (`open_websocket`) applies steps 1-2 plus `max_connections` (429). Frames go through
`websocket_frame`: over `max_frame_bytes` or over `rate_hz` (per connection) they are dropped and counted,
never an error to the client. A reply to a websocket frame returns on the same connection (text for
`text/*` and `application/json`).

## §dial-outs — tasks, generations and backoff

`register_instance` mounts passive endpoints and spawns one task per `websocket-dial`, `mqtt` ingress and
`mqtt` egress endpoint on the injected runtime (no runtime → `not-available`). Each task carries the
instance's **generation**; every state change goes through `with_generation`, so a task that outlives a
re-registration can never write into the new instance. Unregistering flips each task's `watch` stop switch
and drops the outbound senders, so tasks end even mid-backoff. Redials use jittered exponential backoff
(`backoff_ms`, `RandomState` as the jitter source: no `rand` dependency) inside the declared
`reconnect_ms` (default 1-60 s); a successful session resets the attempt counter.

## §mqtt — why a small in-tree codec instead of a client crate

The host needs MQTT 3.1.1 with QoS 0/1, clean sessions, one subscription set or one publish queue, and a
**hard cap enforced before buffering**. `decode` reports `Oversized { total }` from the fixed header alone and
the session's `Reader` skips those bytes as they stream past, so a broker cannot make the host allocate more
than `max_payload_bytes + PACKET_OVERHEAD`. Writing that against a third-party client's internals would
have cost more than the codec, and it keeps `deny.toml` and the dependency graph unchanged (every crate this
directory uses is already in Cargo.lock). Not supported, by design: QoS 2, will messages, persistent
sessions, retransmission after reconnect. An oversized QoS 1 PUBLISH is skipped without a PUBACK; with clean
sessions the broker does not redeliver it.

`Reader::next` is cancel-safe inside `tokio::select!`: its only await is the socket read, and the bytes are
appended before the next await, so dropping the future loses nothing.

## §egress — the guest names an endpoint, never a host

`endpoint_url` joins the declared base with the guest's path and refuses anything that could leave it: dot
segments, encoded `.`/`/`/`\`, `//`, `://`, `?`, `#`, `@`, controls and whitespace; the joined URL must keep
the base's scheme, host, port and path prefix. Redirects follow the same rule (`within_base`, at most 5).
Clients use `no_proxy()` (the host connects only to the declared host and port), `https_only` unless the base
is loopback, `timeout_ms` ≤ 30 s and `max_response_bytes` ≤ 8 MiB; an over-cap response is `budget-exceeded`,
never silently cut. A connect failure is `backend`; a timeout after sending is `unknown-outcome` (the request
may have landed). Undeclared endpoints, methods and topics are `capability-denied` and audited (R9 AC).
MQTT publishes may not exceed the declared QoS or set `retain` unless declared.

## Seams for other slices

- **E1** implements `NetHooks` over the component and routes WIT `ingress.send`/`close` to `Net::send`/`close`
  and `egress.*` to `http_request`/`mqtt_publish`.
- **E6** mounts `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{endpoint}[/{rest}]`: body limit
  `INGRESS_BODY_CEILING`, `Bus::admit_remote(did)` first, then `accept_http` / `open_websocket`, mapping
  `IngressRefusal::http_status`; a WS writer task drains `WsSession::outbound` and reports `websocket_closed`.
- **E7** builds `Net::new(clock, NetSources { roles, audit, credentials, runtime: Some(Handle::current()) })`
  on the plugin worker, registers/unregisters instances alongside the bus, and calls `dispatch` per instance.
- **E15** (device adapter) and **P6** (loopback MQTT broker, loopback HTTP sink) exercise the dial-outs live.
