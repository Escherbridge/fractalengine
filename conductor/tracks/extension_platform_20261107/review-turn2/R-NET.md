---
type: review
track: extension_platform_20261107
title: R-NET — extension network cluster security review, swarm turn 2
resource: ../metadata.json
---

# R-NET — network cluster (turn 2, over N1, N3, N4, N5, F-DEVADAPT, B6, I-PLUGIN)

This is a security review-and-fix, code first: nothing was built or run. Each finding has one verdict line:

- **FIXED**: edited here.
- **KEPT**: checked and left as is, with the reason.
- **HANDED-OFF**: the owner is named.
- **DEFERRED**: the trigger is named.

The refutation targets come from the slice task. Two steers are binding: the credential rule (2026-11-08 20:20)
and the slot and SSRF fixes (2026-11-09 01:50, items 1-2).

Files in scope:

- `fe-plugin/src/net/**`
- `fe-plugin/src/wasm/component/net_bridge.rs`
- fe-api `extensions.rs`, `bound_routes.rs` and J6
- `fractalengine/src/extension_host/net_sources.rs`
- the device adapter
- `fe-plugin/Cargo.toml` and `deny.toml`

## Credentials (in to a guest, out from a guest, at rest)

- R-NET-01 FIXED (HIGH, steer 01:50 item 1): credential slots were not scoped to a package.
  - **Defect:** `CredentialStore::get(slot)` was keyed by the manifest-declared slot name alone, so any extension declaring `vault-token` read another extension's secret.
  - **Fix:** the trait is now `get(package_id, slot)`, with no unscoped fallback. `Net::credential` takes the instance's own package id at all three call sites: egress `prepare_http`, `ws_dial::run` and `mqtt_session::run`.
  - **Store:** `FileCredentialStore` is now `{package_id: {slot: ..}}`.
  - **Journey row:** "another package's slot of the same name".
- R-NET-02 KEPT, the credential header strip on ingress, in three layers. No credential header reaches a guest:
  - the fe-api ingress core drops `is_credential_header`;
  - `Net::forwarded_headers` keeps only declared names minus that list, so the smuggled manifest row holds;
  - `net_bridge::frame_to_wit` drops them again.
  - Websocket and MQTT frames carry no headers, and the websocket upgrade forwards none.
  - The bound alias (B6) reaches the same core, so it strips too.
  - fe-api reads the bearer token only from `Authorization` (`auth.rs`), so the forwarded query string is not a credential path.
- R-NET-03 KEPT, the credential header strip on egress. A guest header that is a credential or host-owned (`authorization`, `cookie`, `x-fe-*`, `host`, `proxy-*`, `sec-websocket-*`...) refuses the whole call. `response_headers` drops `set-cookie`, a reflected `authorization` and `x-fe-*` before `net_bridge::response_to_wit` filters again. reqwest is built without a cookie store.
- R-NET-04 FIXED (LOW): `mqtt::Packet` derived `Debug` would print the CONNECT password bytes.
  - **Fix:** a manual `Debug` redacts the password and prints only payload sizes.
  - **Journey row:** "MQTT CONNECT never prints the password".
- R-NET-05 FIXED (LOW): `FileCredentialStore::set_slot` wrote the secret with default permissions (0644 on Unix).
  - **Fix:** the file is now created `0600`, `fsync`ed, then renamed over the store.
  - **Windows:** the file inherits the per-user data directory's ACL.
- R-NET-06 KEPT, no credential leak through logs, status or replication.
  - **Logs:** the store logs only that a parse failed, never serde's text, which could echo content.
  - **Status:** endpoint state says `credential-missing`, never a value.
  - **Debug output:** `Credential`'s `Debug` redacts the secret, and request header values are `set_sensitive`.
  - **Replication:** the store is a local file outside the DB plane.
- R-NET-07 KEPT, by design: a declared host that reflects request headers (httpbin-style `/headers`) hands the guest its own slot's secret. The guest picks the path, so no host-side rule can tell a reflection apart. It is documented as an operator control in `net/AGENTS.md` §credentials.
- R-NET-08 FIXED (doc): `net_sources.rs` pointed at `fractalengine/src/AGENTS.md §extension-net`, which does not exist. The credential-store decision (file, format, scope, permissions, fail-closed on the old flat shape) now lives in `fe-plugin/src/net/AGENTS.md` §credential-store.

## Existence before role checks

- R-NET-09 KEPT, the ingress order. Nothing reveals existence before the role check:
  - **`Net::admit_http`:** instance, endpoint, role floor (audited), then sub-path all answer the same not-found. Only then do 429/405/413/415 apply.
  - **`open_websocket`:** applies the floor before `max_connections`.
  - **The binary's `ingress_gate`:** checks the capped floor before `Net`.
  - **fe-api `gate()`:** rate-limits before any lookup.
  - **The bound alias:** checks petal scope, then Viewer, then `effective()`, then the route. It shows 503 `binding-unresolved` only to Viewer+, which is OQ-B7 by design.
- R-NET-10 KEPT: `Net` never sees the token's `max_role`. `plugin_bridge::ingress_gate` caps the DID's role by it at accept time (G1). The per-frame floor re-check in `Net::websocket_frame` is uncapped, but it gives the same verdict, because the ceiling is fixed for the connection's life and already passed the floor. This is documented in `net/AGENTS.md` (N4 seam).

## Egress escape (dot segments, encodings, redirects, proxies, addresses)

- R-NET-11 FIXED (HIGH, steer 01:50 item 2, the R-NET part): SSRF and DNS rebinding.
  - **Defect:** dial-outs called `TcpStream::connect((host, port))`, and egress used reqwest's default resolver. A name shown at consent could therefore reach `169.254.169.254`, a LAN admin page or the host's own API, and a second lookup could rebind it.
  - **Fix:** new `net/address.rs`:
    - `is_public_unicast` is an allow-list: global unicast only, IPv4-mapped judged as IPv4, 6to4 by its embedded IPv4, and NAT64, Teredo, ULA, documentation and multicast refused.
    - `admits` lets the declared loopback names reach loopback only.
    - IP literals are refused at registration (`connect::target`, `egress::build_client`).
    - `resolve_checked` resolves once, refuses the whole answer on one bad address, and dial-outs connect to exactly those addresses.
    - egress uses `dns_resolver(CheckedResolver)`.
  - **Journey rows:** the 36-row predicate table, `admits` rows, literal dial targets, `build_client` literals (metadata, private, mapped loopback, NAT64), and `resolve_checked` on literals.
- R-NET-12 FIXED (LOW), egress containment:
  - **Defect:** `..%3b/` (a decoding proxy in front of a servlet turns it into `..;/`) and the overlong UTF-8 dot `%c0%ae` passed `endpoint_url`.
  - **Fix:** `%3b`, `%c0` and `%c1` are refused. `within_base` now also refuses a `..;` segment, which closes redirect targets too.
  - **Journey rows:** three path rows and one redirect row.
- R-NET-13 KEPT, redirects and proxies:
  - at most 5 redirects, each `within_base` (same scheme, host and port, no userinfo, under the base path);
  - `no_proxy()`, `referer(false)`, and `https_only` unless the base is loopback;
  - timeouts clamped to 0.1-30 s;
  - the response cap counted per chunk, never cut silently.
- R-NET-14 FIXED (LOW): the egress client could negotiate h2 with a manifest-named server, which reaches RUSTSEC-2026-0258 (h2 unbounded empty DATA frames, ignored in `deny.toml`). The client is now `http1_only()`, and the `deny.toml` comment records why that id stays off this path.

## Caps before buffering (MQTT, websocket)

- R-NET-15 KEPT, the MQTT cap. `mqtt::decode` reports `Oversized { total }` from the fixed header alone, and `Reader` skips those bytes as they stream past. The buffer stays at most `max_packet + READ_CHUNK`, even when the broker announces 256 MiB. Publishers read at most 1 KiB packets. The journey covers the flood and the hang-up.
- R-NET-16 FIXED (MEDIUM): websocket dial-outs buffered up to 4× `max_frame_bytes` (at most 4 MiB) before the declared cap applied.
  - **Fix:** the transport limit is now the declared cap (never below 125 bytes for control frames). tungstenite refuses the frame from its header. `Error::Capacity` counts `ingress_dropped_size`, drops the connection and redials with backoff.
- R-NET-17 FIXED (MEDIUM): websocket accept on fe-api set only `max_message_size`. tungstenite checks the frame size before buffering, but the default is 16 MiB, so one frame could buffer 16 MiB before the 1 MiB message check refused it. `max_frame_size(MAX_INGRESS_BODY_BYTES)` is now set as well.
- R-NET-18 DEFERRED: an accept-mode frame between the endpoint's `max_frame_bytes` and the 1 MiB host ceiling is still buffered (at most 1 MiB), then dropped and counted by `Net`.
  - **Why deferred:** tightening it needs `max_frame_bytes` on fe-api's `WsHostSession`, a seam the binary (`plugin_bridge.rs`) constructs.
  - **Trigger:** the next change to the `ExtensionHost` websocket seam.

## Dial-out lifecycle and generations

- R-NET-19 KEPT, no generation race. The checks were:
  - every task state write goes through `with_generation`;
  - a stale `dial_opened` returns `None`, and the task closes its socket;
  - `unregister_instance` flips each `watch` and drops the senders, so `stop.changed()` errors and stops the task;
  - `until_stopped` races connect, TLS, the handshake, CONNACK/SUBACK and the backoff;
  - egress completions are dropped for a stale generation;
  - a re-registration gets a fresh generation and dial ids.
- R-NET-20 FIXED (MEDIUM): an admitted accept-mode upgrade that never completed leaked its connection.
  - **Defect:** if the client dropped before hyper's upgrade, or the upgrade failed, nothing called `websocket_closed`. The connection kept its `max_connections` slot until the instance stopped, and the guest heard `opened` with no `closed`. One editor could exhaust an endpoint's slots this way.
  - **Fix:** the `ClosedOnDrop` guard rides the `on_upgrade` callback and reports the close on every path. The existing rejection path also uses it.
- R-NET-21 FIXED (LOW): the remote rate gate was charged twice per package-mount request and three times per bound-alias request (handler, core, then alias core), so legitimate callers were throttled early.
  - **Fix:** handlers gate once, then call the ungated `admitted_ingress`, `admitted_websocket` and `admitted_bound_ingress` cores.
  - **Unchanged:** the public cores still gate once each, so the J6 budget arithmetic is unchanged.

## Plaintext beyond loopback

- R-NET-22 KEPT, extended:
  - plaintext `ws://`, `mqtt://` and `http://` are allowed only to exactly `127.0.0.1`, `::1` or `localhost` (`connect::target`, `build_client`, `https_only`);
  - those names may now resolve only to loopback (R-NET-11), so a tampered `localhost` cannot carry plaintext off the host;
  - an MQTT password goes in plaintext only to loopback.

## Bridge, device adapter, dependencies

- R-NET-23 KEPT `net_bridge.rs`:
  - every WIT `ingress.*` and `egress.*` import goes through `Net`, keyed by the host-side `Binding` (`broker.rs` passes `self.binding`, never guest input);
  - refusals map through `wit_host_error`;
  - the unit test covers both header directions.
  - The doc comments now say an empty reply is fe-api's 204, not 202.
- R-NET-24 KEPT, the device adapter (`extension.json`, `src/**`, `fixtures/**`):
  - `forward_headers` names only `x-device-id`;
  - every dial-out is `wss://` or `mqtts://` to a hostname;
  - device ids are validated topic-safe (`valid_device_id`) before they enter an actuation topic, so `+`, `#` and `/` cannot widen the publish past the declared filter;
  - `fe-sdk/schema/examples/device-adapter.extension.json` is byte-identical (`cmp`).
- R-NET-25 KEPT, new dependency licences:
  - tokio-rustls 0.26, tokio-tungstenite and tungstenite 0.29, futures-util 0.3 and semver 1 are MIT or Apache-2.0;
  - webpki-roots 1 is CDLA-Permissive-2.0, already allowed;
  - `[sources]` stays crates.io only.
- R-NET-26 KEPT, advisories for the locked versions:
  - rustls-webpki 0.103.13 is patched for RUSTSEC-2026-0049, 0098, 0099 and 0104;
  - tungstenite 0.29 is past RUSTSEC-2023-0065;
  - no tokio-rustls or webpki-roots id applies.
- R-NET-27 FIXED (triage record): RUSTSEC-2026-0285 (rustls 0.23.40) was re-triaged for the fe-plugin client path. A peer can send should-be-encrypted TLS 1.3 handshake messages in plaintext, but the transcript stays authenticated (C:L, I:N, A:N), so no certificate, SNI or credential check is bypassed. The ignore stays until 0.23.45 resolves without new crates at L1.

## Handed off (files R-NET does not own)

- R-NET-28 HANDED-OFF (orchestrator or R-X; R-REG has closed) `fractalengine/src/extension_host/sources.rs`.
  - **Gap:** its `is_internal` is a deny-list. It misses 2002::/16 (embedded IPv4), 2001::/23 (Teredo), 64:ff9b:1::/48, 2001:db8::/32, 3fff::/20, 192.0.2/24, 198.51.100/24, 203.0.113/24 and 192.88.99/24.
  - **Change:** replace `is_internal(ip)` with `!fe_plugin::net::is_public_unicast(ip)`, keeping the `allow_local` rule, so there is one shared predicate as the steer requires.
- R-NET-29 HANDED-OFF (orchestrator; no turn-2 owner) `docs/extensions/manifest-v2.md`.
  - **§auth:** slots are scoped per package; `get(package_id, slot)`; no cross-package fallback.
  - **"Loopback" paragraph:** every other host must resolve only to global-unicast addresses (allow-list in `fe-plugin/src/net/AGENTS.md` §addresses), and non-public IP literals are refused.
- R-NET-30 HANDED-OFF (R-X) to the other outbound clients: `fe-plugin/src/registry_client.rs`, `fe-plugin/src/lifecycle.rs` (`fetch_revocation_list`) and `fractalengine/src/extension_host/revocation_pull.rs`. They use plain reqwest clients against operator-configured URLs. Decide whether they adopt `CheckedResolver` semantics via `is_public_unicast`, with the same loopback rule.

## Deferred

- R-NET-31 DEFERRED, a consequence of R-NET-11:
  - **Effect:** a private-network target (`mqtts://192.168.1.10`, or `broker.lan` resolving to 10.x) is now refused. A site reaches a LAN broker or device through a loopback bridge on the host, or by pushing to the ingress mount.
  - **Trigger:** the user decides whether an operator may allow a LAN target per installation. That would be new local-acceptance scope.
