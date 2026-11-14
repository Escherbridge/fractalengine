---
type: spec
title: "Peer networking support: embedded members-only relay, signed relay advertisements and gossip, network diagnostics"
track: peer_networking_support_20261031
tags: [feature, networking, p2p, relay, security, fe-sync, fe-relay, fe-api, fe-ui, peer_networking_support_20261031]
created: 2026-10-31
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: "user directive 2026-10-31: networking support feature, secure, Fable-reviewed (from NAT/relay research session findings)"
depends_on: [relay_operability_20261031]
---

# Specification: Peer networking support (security-first, Fable-reviewed)

**Track ID:** `peer_networking_support_20261031` · **Type:** feature · **Priority:** P1
**Crates:** `fe-sync` (relay server, trust, gossip signing, reachability, diagnostics), `fractalengine-relay`, `fractalengine`, `fe-api` (diagnostics route + MCP), `fe-ui` (Network panel, relay-capable toggle).
**Citations computed at:** `8ee4a96`. iroh paths are relative to `~/.cargo/registry/src/index.crates.io-*/` (`iroh-1.3.0`, `iroh-relay-1.3.0`, `iroh-gossip-0.101.0`, `iroh-docs-0.101.0`). Symbol names are stable. Line numbers are not, so re-grep before editing.

## 1. User vision (2026-10-31)

> "Work in this networking support feature securely with a fable reviewed networking system embedded"

- **Networking support.** Peers reach each other on real networks, and verse operators can run their own relays.
- **Securely.** A relay serves members only and is never open. Relay advertisements and gossip frames carry authorship that can be checked.
- **Fable reviewed, embedded.** Four named gates (F0 to F3, §9 and the plan) run on the `fable` tier. Each records a verdict file, and a FAIL blocks the next wave.

Everything here is framed for self-hosters and verse operators.

## 2. Problem & evidence

### 2.1 What iroh 1.3 already does (we do not rebuild it)

- **Hole-punching is automatic.** A connection starts relayed and upgrades to direct. NAT candidates are QUIC frames *inside* the end-to-end connection (`iroh-1.3.0/src/socket/remote_map/remote_state.rs:534,947`). The upstream test `endpoint_two_relay_only_becomes_direct` proves the upgrade (`src/endpoint.rs:2493`).
- **Portmapping (UPnP/PCP/NAT-PMP) is on by default.** `PortmapperConfig::Enabled` is the `Default` (`src/portmapper.rs:20-38`; `Builder::portmapper_config` at `endpoint.rs:797`). FE never overrides it (`fe-sync/src/endpoint.rs:115-121`). It maps the **UDP** socket only. Its external address is folded into the direct-address set as `DirectAddrType::Portmapped` by `update_direct_addresses` (`src/socket.rs:1811-1827`), and `watch_addr()` chains that set after the home relay (`endpoint.rs:1289-1302`), so a mapped address **does** appear in `watch_addr` (verified at F0).
- **The relay set can change at runtime.** `Endpoint::insert_relay`/`remove_relay` exist (`endpoint.rs:1000,1014`). The relay map contributes only each URL's `auth_token` (`src/socket/transports/relay/actor.rs:1342-1346`; `iroh-relay/src/relay_map.rs:245`) and home-relay candidacy. **iroh dials any relay URL that appears in a remote's address, even one outside the map** (`actor.rs:1279-1322`). Non-home relay connections close when idle (`actor.rs:67`).
- **There is one home relay per endpoint, chosen by latency.** It is net-report's `preferred_relay` (`actor.rs:1235-1265`), and peers reach us through it. The choice is **endpoint-global, not per verse**. This is hazard B (T11).
- **The relay server can be embedded** (iroh-relay `server` feature, `Cargo.toml:71-93`):
  - `Server::spawn(ServerConfig)` (`src/server.rs:691`) takes `ServerConfig {relay, quic, metrics_addr}` (`:111-119`). `RelayConfig::new` defaults access to **`AllowAll`** (`:127-160`).
  - `AccessControl::on_connect` returns `Access::{Allow,Deny}` (`:285-293`; `Access` at `:350`). `on_disconnect` (`:302`) is called once for every connection that `on_connect` **admitted**; a denied connection gets no guard and no disconnect call. `OnDisconnectGuard` fires it exactly once, even when setup fails after admission (`:376-425`).
  - `ClientRequest::endpoint_id` is **proven by the handshake before authorization** (`:222-232`; `src/server/http_server.rs:869-876`): the client signs TLS exported keying material or a server challenge, both domain-separated (`src/protos/handshake.rs:11-20,52-54`).
  - **`on_connect` runs before `Clients::register`** (`http_server.rs:876-908`). A connection therefore exists in the gate's view before it exists in the relay's client table. This is the admit/revoke race that FR-3's convergent revocation closes (F0-1).
  - **An embedding host can evict a client:** `Server::relay_service()` (`:949`) → `RelayService::clients()` (`http_server.rs:953`) → `Clients::disconnect(id, None)` (`src/server/clients.rs:172-197`). With `None` it calls `start_shutdown` on the active connection and every inactive duplicate; shutdown is asynchronous, each connection actor unregisters itself, and `unregister` sends peer-gone notifications (`:109-170`). `disconnect` returns `false` when nothing is registered yet. `Clients` exposes no iteration, so an embedder keeps its own connection table. `set_client_rate_limit` works at runtime (`http_server.rs:940`).
  - **The standalone binary cannot evict.** Its access modes are Everyone (the default), Allowlist, Denylist, Http and shared_token (`src/main.rs:160-190`).
- **Limits are partial.** Only `Limits.client_rx` (per-client bytes/s plus burst) works. `accept_conn_limit` and `accept_conn_burst` are documented "Not currently implemented" (`server.rs:486-500`). `on_connect` runs after TLS, the WebSocket upgrade and the handshake, so a connection flood's CPU cost is paid before any check; a handshake timeout bounds slow connections (`http_server.rs:72-78`). `client_rx` limits what a client *sends*; nothing limits what the relay delivers *to* a client.
- **With TLS on, the plain HTTP listener serves only the captive-portal probe** (`server.rs:465-467`, `:804-812`); the relay protocol runs on `https_bind_addr` only.
- **TLS.** The server side has `CertConfig::{LetsEncrypt, Manual}` (`server.rs:527-545`). ACME needs a hostname and a contact (`main.rs:640-655`) and uses TLS-ALPN-01 (`http_server.rs:1095`), so it needs inbound TCP 443. `--dev` is incompatible with LetsEncrypt (`main.rs:708`). On the client side, `Builder::ca_tls_config` (`endpoint.rs:724`) is set once at bind and applies endpoint-wide. It accepts `CaTlsConfig::custom_server_cert_verifier` or `with_extra_roots` (`iroh-relay/src/tls.rs:141,151`).
- **Reachability can be probed, but only for UDP.** `Endpoint::net_report()` (`endpoint.rs:1457`) is behind `#[cfg(feature = "unstable-net-report")]` (`:1456`). Its `Report` carries `udp_v4/v6`, `mapping_varies_by_dest_*`, `global_v4/v6`, `preferred_relay` and `captive_portal` (`src/net_report/report.rs:18-37`). Two more sources are `bound_sockets()` (`:1466`) and `watch_addr()` (`:1289`). **All of these describe the UDP socket. None proves inbound TCP to an HTTPS port.**
- **Test seams.** Three upstream helpers support tests:
  - `test_utils::run_relay_server_with_access` (`src/test_utils.rs:51`, feature `test-utils`, `lib.rs:305`) runs an in-process relay with a custom access control.
  - `Builder::clear_ip_transports()` (`endpoint.rs:510`, used upstream at `:2618`) builds a relay-only endpoint, which faithfully models a peer that cannot hole-punch.
  - `CaTlsConfig::insecure_skip_verify` (`tls.rs:94`) lets test clients accept a self-signed relay.
- **Doc rows are signed; gossip is not.** iroh-docs entries are signed (`SignedEntry`, `iroh-docs-0.101.0/src/sync.rs:698-702`), and a bad signature is rejected on sync insert (`BadSignature`, `:622-636`). Gossip `Message {content, scope, delivered_from}` is **unsigned**: its own docs say `delivered_from` "is not the same as the original author" (`iroh-gossip-0.101.0/src/api.rs:362-371`; `DeliveryScope::is_direct` at `src/proto/plumtree.rs:174-187`).
- **A relay sees routing metadata.** Each datagram carries `dst_endpoint_id` or `remote_endpoint_id` (`iroh-relay/src/protos/relay.rs:177-187`, `:73-77`). Payloads stay ciphertext.

### 2.2 What FractalEngine lacks today

- **No relay server exists.** `fractalengine-relay/Cargo.toml:42-44` has only a `sim-control` feature and no iroh-relay dependency, and `tokio-rustls-acme` is absent from `Cargo.lock`. Operators run third-party relays (n0's v1 fleet) or hand-run containers (`fe-sync/src/AGENTS.md` §relay-health).
- **Advertised relays are unhardened.** The sibling's S-RELAY-ADVERTISE (FR-7) role-gates the *write*. But its default `AutoAdopt` calls `insert_relay` for every advertised URL, which makes each URL eligible to become the endpoint's home relay. Nothing checks expiry, health, set bounds, cross-verse refcounts or protection of configured relays.
- **Gossip attribution is claim-based (residual S-1).** `ComputeEnvelope` claims `from_did` (`fe-sync/src/distributed_query.rs:83-100`). `TilesetAd` claims `publisher_did` and carries `provider_addr` (`sync_thread.rs:2760-2777`). The pump sets `direct` from the delivery scope (`sync_thread.rs:2318-2325`), and only direct deliveries are identity-checked (`distributed_query.rs:1283,1330`). This is residual **S-1** (§transport-admission-control item 2; `SECURITY.md:79-86`). §write-policy already requires a networked transform to be "a signed canonical operation".
- **Peer addresses can steer our endpoint.** `add_peer_addr` (`sync_thread.rs:2244`) accepts any `Relay(url)`. Because iroh dials any such URL, a peer's address can make us open TLS to an arbitrary host and reveal our IP to it (T16).
- **Diagnostics are minimal.** `SyncStatus` holds only `online`, `node_addr`, `peer_count` and `health` (`status.rs:22-31`), and `fe-ui/src/dialogs/peer_debug.rs:9-30` renders just that. There is no network route in `fe-api/src/server.rs` and no MCP tool.
- **The existing access-snapshot precedent is wrong for a relay.** `VerseFabric::access_snapshot` (`sharding.rs:1494-1509`) folds in `open_to_all` (`docs_engine.rs:44-53`). That is correct for blob reads. Reused for a relay, it would turn the relay of a `default_access: viewer` verse into **an open relay**.
- **Plugins cannot do networking.** `network_scope` and `external_http` are declared but unenforced (`fe-plugin/src/capability.rs:35-39`), and the WIT is aspirational (`fe-plugin/src/AGENTS.md:131-134`). The endpoint is private to the sync thread, so networking is native fe-sync work.

## 3. Scope / Non-goals

**In scope:**

- **FR-1:** an embedded iroh-relay server, opt-in at both build time and runtime.
- **FR-2:** a relay-capable mode for desktop peers, gated by a self-test and a dial-back check.
- **FR-3:** members-only access control, with caps, revocation and abuse logging.
- **FR-4:** advertisement hardening: signature, role check at read time, TTL, bounds, health, adoption modes, fallback, token isolation and a peer-address filter.
- **FR-5:** per-message gossip signing, which closes S-1, plus a mixed-version plan.
- **FR-6:** diagnostics in the GUI, the API and MCP.
- The STRIDE threat model, the Fable gates F0 to F3, and flow tests on in-process relays.

**Non-goals:**

- **Plugin networking.** `network_scope` stays inert, and that is documented.
- **A flat all-peer relay mesh.** CGNAT and symmetric-NAT peers never serve.
- **Replacing iroh hole-punching or portmapping.**
- **Hiding metadata** (onion or mix routing). A relay sees who talks to whom, when, how much, and client IPs. We disclose and limit this; we do not hide it.
- **Redefining sibling-owned pieces.** The `__relays` table, its REST route and `RelayAdvertisePolicy` belong to the sibling's FR-7. This track consumes and hardens them (§8).
- **Integrating the standalone iroh-relay binary** beyond a recipe.
- **Mobile.**

## 4. Design

### 4.0 Trust model: what iroh guarantees and what it does not

| Property | iroh 1.3 guarantees? | FE stance |
|---|---|---|
| Payload confidentiality and integrity through a relay | **Yes.** QUIC/TLS 1.3 runs end to end between EndpointIds | A malicious or dead relay costs **availability, never confidentiality or integrity** |
| Peer identity | **Yes.** The EndpointId is the public key, and the DID shares its seed (F20) | Map DID ↔ EndpointId with `replicator::peer_did_key` |
| Client identity *to the relay* | **Yes.** It is proven before `on_connect` | This is the basis for FR-3 |
| Relay authenticity | **Only through TLS** (WebPKI, or a pin under OD-3) | Treat an unverified relay as an attacker relay |
| Metadata: client IPs, src/dst ids, sizes, timing, presence | **No** | Disclose it; limit it with DialOnly plus local deny (FR-4) |
| Gossip authorship beyond one hop | **No** | FR-5 |
| Doc-row authorship | **Yes** (`SignedEntry`) | Put advertisements in the doc, **never** in gossip |

### 4.1 FR-1: Embedded relay server (fe-relay first)

**Placement.** A new `fe-sync/src/relay_server.rs` sits behind a new fe-sync feature, `relay-server`, which enables `iroh-relay/server` (OD-1). `EmbeddedRelay::spawn(RelayServeConfig, RelayMembershipGate)` builds a `ServerConfig` with TLS always on, `limits.client_rx` set, `access` set to the gate, QUIC address discovery (QAD) as configured, and `metrics_addr: None` unless configured.

**Opt-in.** `fractalengine-relay` gains a feature `relay-server = ["fe-sync/relay-server"]`, which is **not** in `default`. At runtime it follows the OperatorGate idiom (`fe-api/src/operator_gate.rs`): only the value `1` opts in, the value is read once at startup, and enabling it logs a loud WARN.

| Env | Default | Notes |
|---|---|---|
| `FE_RELAY_SERVE` | unset (off) | `1` only. Any other value binds no listener at all |
| `FE_RELAY_SERVE_CERT_MODE` | none, so the relay **refuses to start** | `acme` \| `manual`. Never plaintext |
| `FE_RELAY_SERVE_HOSTNAME` / `_ACME_CONTACT` / `_ACME_STAGING` / `_CERT_PATH` / `_KEY_PATH` | none | ACME needs inbound TCP 443. Its cache lives in the data dir |
| `FE_RELAY_SERVE_HTTPS_BIND` / `_HTTP_BIND` | `0.0.0.0:443` / `0.0.0.0:80` | The HTTP side serves only the captive-portal probe |
| `FE_RELAY_SERVE_VERSES` | **required** | A ULID list, or `hosted`. An empty set refuses to start. Every listed verse must be hosted locally, because the gate needs its role view; a non-hosted ULID is a startup error, not a silent deny-all. `hosted` logs a WARN: it extends service to any verse this node hosts later |
| `FE_RELAY_SERVE_MAX_CLIENTS` / `_MAX_CONN_PER_ENDPOINT` | 1024 / 4 (desktop: 64 / 2) | Enforced in the gate, because iroh's own caps are unimplemented |
| `FE_RELAY_SERVE_CLIENT_RX_BPS` / `_BURST` | 2 MiB/s / 4 MiB (desktop: 512 KiB/s / 1 MiB) | iroh `ClientRateLimit` |
| `FE_RELAY_SERVE_QAD` | `1` (desktop: `0`) | The QAD responder has no access control |
| `FE_RELAY_SERVE_METRICS_BIND` / `_LOG_IPS` | unset / unset | Bind metrics to loopback. IPs are logged only when set to `1` |

**Refusal to start.** The relay refuses to start if the feature is off, `FE_RELAY_SERVE` is not `1`, no cert mode is set, the served-verse set is empty, or access would be `AllowAll`. A plaintext listener can only be built from `#[cfg(test)]` builders.

### 4.2 FR-2: Relay-capable desktop peer mode

**Who can serve.** Serving needs a public address, inbound TCP and a TLS certificate. In practice that means a VPS, or a desktop with a public IP or port forward plus a DNS name.

**Preconditions.** All five must hold:

1. The GUI was built with `relay-server` (OD-2: off in v1).
2. The Settings toggle "Serve as a relay for my verses" is on (default off).
3. The user accepted a consent dialog stating: "your public IP/hostname becomes visible to every member of the verses you serve; relayed members' IPs and traffic metadata become visible to you".
4. The self-test passes.
5. A dial-back check passes.

**Self-test.** A pure classifier, `reachability::classify(NetReportSummary, bound_sockets, portmap)`, returns `RelayCapable` only when all of these hold: `has_udp`; `captive_portal != Some(true)`; `mapping_varies_by_dest != Some(true)` (symmetric NAT makes a poor relay); `global_v4` equals a bound socket (no NAT in the way) or a bound `global_v6` exists; and the address is not RFC 1918, 100.64/10 (CGNAT), link-local or ULA. Otherwise it returns `BehindNat`, `SymmetricNat`, `Cgnat`, `CaptivePortal`, `NoUdp` or `Unknown`, each with remediation text.

A `ReachabilityProbe` adapter wraps `net_report` behind `unstable-net-report`, so upstream churn touches one file (OD-10). F0 confirmed that the portmapper's external address appears in `watch_addr` as a `Portmapped` direct address (§2.1). It is a UDP mapping and is never treated as proof of TCP reachability: a public address that is *only* `Portmapped` (no bound socket carries it) classifies as `BehindNat`.

**Dial-back (the AutoNAT pattern).** A *different* node fetches `GET https://<host>:<port>/ping` with full TLS verification. That node is fe-relay by default; a desktop acts as a prober only when its operator opts in (Settings, default off), which keeps the SSRF surface on servers. `/ping` is `RELAY_PROBE_PATH` (`iroh-relay/src/http.rs:15`, served at `server.rs:738`). The request is a signed `reach_probe` frame (FR-5) that names exactly one `prober_did`; every other peer drops it, so one request can never fan out into a probe from every member. The prober answers with a signed `DialbackRecord {prober_did, requester_did, url, at_ms, ok}`, domain `fe-dialback-v1\0`, carried back in a `reach_result` frame addressed to the requester. A peer-run relay's advertisement must carry a record no older than 10 minutes in which `ok` holds, `requester_did` equals the advertiser, `prober_did` differs from the advertiser (no self-attestation) and `prober_did` is fe-relay or a declared Viewer+ of that verse (FR-4, AC-16).

**FR-2b: prober controls.** The prober fetches a URL on another peer's behalf, so it is an SSRF surface. The requester must be a declared Viewer+ peer of a verse the prober shares with it. The URL must be `https` with no userinfo, query or fragment, and must pass `RelayUrlPolicy` (port 443 or an allowlisted port), which bounds any port-scanning use. DNS is resolved once; **every** resolved address must be public unicast (one loopback, private, link-local, CGNAT, multicast, unspecified, IPv4-mapped, NAT64, 6to4 or Teredo record rejects the whole target, so a happy-eyeballs fallback cannot reach a private record); then the prober **connects to one resolved IP** through a resolver override while still verifying the original hostname in TLS, which defeats DNS rebinding without breaking certificate checks. Only the fixed path is requested, by `GET`, with no redirects, a 1 KiB body cap and a 5 s timeout, so the reflected traffic per probe is bounded by a small constant. Rate limits are 1 probe per requester per 60 s and 10 per minute in total per prober.

### 4.3 FR-3: Relay access control (members-only, never open)

**The gate.** `RelayMembershipGate` implements `iroh_relay::server::AccessControl`. The sync thread feeds it an admission snapshot through a OnceLock seam, the same shape as `set_settings_relay_spec`. The gate never borrows the fabric directly. That is the same reason the `BlobAccessGate` precedent exists (`docs_engine.rs:227`).

**Admitted set.** For each served verse, take the owner plus every DID whose *explicit* role statement resolves to at least Viewer at the verse scope. The admitted set is the union over all served verses, computed by a new `VerseFabric::relay_admission_snapshot`. **`default_access` and `open_to_all` are never consulted** (OD-4, tripwire). F0 removed the draft's `∩ declared __peers` intersection: a relay-only newcomer cannot publish its `__peers` row until it has synced, and cannot sync without the relay, so the intersection deadlocked exactly the peers a relay exists for. A role statement is written by the grantor and replicates without the newcomer's help, and it is already an explicit Owner/Manager act, so it adds no Sybil exposure that the intersection removed.

**`on_connect`.** The gate maps the EndpointId to a DID with `replicator::peer_did_key` and denies with one of three reasons: `not_member`, `per_endpoint_cap` or `global_cap`. Live connection counts go up **only when `on_connect` returns `Allow`** (a denied connection never gets a disconnect callback) and down in `on_disconnect`. The gate keeps its own table `endpoint_id → {connection_id}` from those two callbacks, because `Clients` exposes no iteration. While roles have not converged, or a served verse's role view is unavailable, the gate **denies**: the relay affects only availability, and an honest client retries.

**Revocation (convergent).** When a snapshot update removes a DID, the gate calls `relay_service().clients().disconnect(id, None)` for it, and the client's reconnect is then denied at `on_connect`. Because `on_connect` runs before `Clients::register` (§2.1), a revoke that lands between admission and registration finds nothing to disconnect (`disconnect` returns `false`) and a one-shot design would let that connection live. Revocation is therefore convergent: every admission is stamped with the snapshot generation; a revoke bumps the generation and marks the DID; a reconciler re-issues `disconnect` every 250 ms for every marked DID that still has entries in the gate's connection table, until `on_disconnect` has fired for each entry. The target is eviction within 5 s of the revocation being materialized (OD-12). The standalone binary cannot evict, so its recipe documents restarting the relay on revoke.

**Floods.** The `client_rx` limit gives a documented global bandwidth ceiling of clients × rate. Connection floods must be absorbed by the OS firewall or a reverse proxy, because TLS and the handshake run before `on_connect`; a revoked or hostile client that reconnects in a loop costs the relay one TLS handshake per attempt and the gate cannot make that cheaper. A denial circuit breaker trips when denials exceed N per minute for M minutes. On a desktop it **closes the listener** for a cool-down (iroh-relay has no pause API) and warns in the UI; on fe-relay it logs a WARN and keeps serving, because an operator relay must not drop honest members for an attacker's benefit.

**Cross-verse forwarding.** iroh-relay has no per-verse partition: it forwards between any two admitted clients. On a relay serving several verses, a member of V1 can push `client_rx`-bounded ciphertext at a member of V2; the receiver's QUIC stack discards it, but the bytes still arrive. Operators who need isolation run one relay process per verse set (T20).

**Abuse log.** The events are `relay.denied{reason, endpoint_short}`, `relay.cap_hit` and `relay.revoked_disconnect`. Each is rate-limited to one per endpoint per minute. They never carry IPs (unless `_LOG_IPS=1`) and never carry tokens.

### 4.4 FR-4: Advertisement hardening (consumes S-RELAY-ADVERTISE)

**Row fields.** This builds on the sibling's `__relays/{relay_id}` row and route without redefining them. It *adds* a versioned v1 field set: `v`, `verse_id`, `url`, `operator_did`, `advertised_by`, `issued_at_ms`, `expires_at_ms` (at most 30 days after issue), `access` (`members` or `open`), `dialback`, and `cert_spki_sha256?` (OD-3 phase only). The row is signed as `sig = sign_domain(K_advertised_by, "fe-relay-ad-v1\0", canonical_cbor(fields∖sig))`, using `fe-canonical-log/src/signing.rs:57,62` (`verify_strict`, domain-separated; OD-5).

**Read-time admission.** Every replica runs these checks in the inbound-apply path, and re-runs them on every role change:

1. The doc entry's signed author equals `advertised_by`.
2. The inner signature verifies.
3. `role_at(advertised_by, verse_scope)` is at least Manager. An unconverged role view quarantines the row (1024-row cap, as for `__shards`). A demotion withdraws the advertisement.
4. `verse_id` equals the replica's verse.
5. `issued_at ≤ now + 60 s < expires_at`.
6. The URL passes `RelayUrlPolicy`: `https` only, no userinfo/query/fragment, host at most 253 bytes, no private IP literal, and port 443 or an allowlisted port.
7. The set is bounded: at most 4 live advertisements per verse (oldest issued wins among the highest role (churn-resistant); extras are counted) and at most 8 adopted relays per process.

**Adoption modes.** Adoption is local and never replicated. Each URL is `Ignore`, `DialOnly` or `HomeEligible`.

- **`DialOnly` is the default (OD-9, confirmed at F0).** It calls no `insert_relay`. The advertisement only allowlists the URL in the peer-address filter for that verse, and shows it in the UI. iroh still reaches peers whose home relay it is, because iroh dials any URL found in a peer's address (`actor.rs:1322-1348`, verified).
- **`HomeEligible` is reached by a local pin, or automatically under the coverage rule (OD-13).** It calls `insert_relay(url, RelayConfig{auth_token: None})`. The coverage rule: a URL is auto-eligible only while **every** verse this node participates in carries an admitted, healthy advertisement for that same URL. Then every peer this node can talk to is a member of a verse the relay serves, so hazard B cannot occur, and every verse's own Manager has vouched for that operator. Coverage is re-evaluated on every verse join or leave and on every advertisement change; when it breaks, the URL drops back to DialOnly, the Configured set is already in the map, and the UI says why. A Settings toggle ("never adopt a home relay automatically") disables the rule; the manual pin remains.

DialOnly is the default because the home relay is endpoint-global (`actor.rs:1232-1257`: one `preferred_relay`, one `my_relay`). If a members-only relay for verse V1 became our home relay, peers who share only verse V2 with us could not reach us by relay (hazard B). It would also funnel every verse's relayed metadata to V1's operator. A manual pin that does not satisfy coverage shows the hazard-B warning naming the uncovered verses (AC-9).

**Health.** Before a `HomeEligible` insert, and every 5 minutes afterwards, the node probes `/ping` with WebPKI and runs a relay handshake. After 3 consecutive failures it calls `remove_relay` and marks the relay `Unhealthy`. Recovery takes 2 successes, the same hysteresis `RelayHealth` uses. While an auto-eligible relay is the home relay, `home_relay_status()` is also watched: anything but `Connected` for 30 s counts as a failure immediately, without waiting for the 5-minute probe.

**Fallback.** A pure planner refcounts each URL by source: `Configured` (FE_SYNC_RELAY or Settings), `Default` (n0's public fleet, present only in Default mode) or `Advertised(verse)`. It **never** removes a Configured URL, so a Manager who advertises the operator's own relay URL and then deletes the advertisement cannot drop that relay. It removes an Advertised URL only when its refcount is zero. Under `Disabled`, advertisements are inert unless pinned. Under OD-13 the planner may *demote* `Default` URLs (remove them from the map) only while a coverage-eligible advertised relay is healthy and connected as home; otherwise home selection is a latency race between that relay and n0's fleet and the coverage rule buys nothing. It re-inserts them the moment coverage breaks, the health loop fails, or `home_relay_status()` leaves `Connected` for 30 s. Explicit Configured URLs are never demoted. The Configured set always stays as the fallback.

**Token isolation.** `RelayAccessToken` attaches only to Configured relays, because iroh reads `auth_token` per URL from the map (`actor.rs:1344-1348`). Advertised inserts use `auth_token: None`. When a URL is in both sets, the Configured entry wins and is left untouched. The same rule governs OD-3 certificate pins: a pin is installed only for the exact host of an admitted advertisement and never for a host that equals a Configured or Default relay's host; otherwise a hostile Manager could pin an attacker's key under the operator's own relay hostname and impersonate it to every member (T21).

**Peer-address filter.** Before `add_peer_addr`, every `Relay(url)` is stripped and counted unless it is a Configured relay, an n0 default (in Default mode), or a relay advertised in a verse we share with that peer.

### 4.5 FR-5: Per-message gossip signing (closes S-1)

**Frame.** A frame is `b"FEG1"` followed by canonical CBOR `{verse_id, author_did, hlc:{wall_ms,counter}, kind, payload, sig}`, where `kind` is `compute`, `tileset_ad` or `reach_probe`. The signature is `sig = sign_domain(K_node, "fe-gossip-v1\0", cbor(verse_id, author_did, hlc, kind, payload))`.

**Domain separation.** The domain is distinct from `fe-oplog-v1\0` (`signing.rs:79`) and from TLS, so reusing the shared endpoint/DID key across protocols is not exploitable. fe-sync gains a dependency edge to fe-canonical-log. That direction is allowed, because fe-canonical-log only forbids its own *outbound* edges (`fe-canonical-log/src/AGENTS.md:17-27`). The signing key is OD-6; the codec is OD-7.

**Verification.** The pump runs these checks before `handle_gossip_incoming`:

0. Cap the frame size before decoding (the gossip transport's maximum message size is the ceiling).
1. Decode strictly, then re-encode and require byte equality (the §ingress discipline).
2. `verse_id` equals the verse of the topic the frame arrived on.
3. `verify_strict` against the author's did:key.
4. On a direct delivery, `author == did_key(delivered_from)`. This keeps F23 item 2.
5. The inner `from_did` or `publisher_did` equals the author, and `kind` selects the handler (a `compute` frame can never reach the tileset or probe handlers; the signature covers `kind`).
6. **Replay guard:** a maximum age per kind (`compute`: `timeout_ms` plus 30 s; `tileset_ad`: 10 minutes; `reach_probe`/`reach_result`: 60 s); at most 60 s of future clock skew; a per-(verse, author) HLC high-water mark used as a **window floor**, not a strict monotone, because gossip hops reorder frames (a frame is dropped when `hlc < hwm − max_age`); and a per-(verse, author) LRU of 256 `hlc` values that catches duplicates inside the window. The LRU is per author, not per verse, so a hostile member can evict only its own entries and cannot reopen a replay window on an honest author's frames. The guard is in-memory: after a restart, frames inside the max-age window can be replayed once (F2 re-examines this; `compute` replays cost bounded duplicate work and nothing else).

The membership and role gates (items 3 and 6) then run on an **authenticated** identity, including for frames relayed through other peers. The cost is about 135 bytes and one signature verification per frame. Tileset advertisements are batched into one frame per broadcast round.

**Mixed-version rollout.** `FE_SYNC_GOSSIP_SIGNING` is `compat` (the default for one release) or `require` (OD-8).

- Every outbound frame is signed.
- A frame with the magic prefix that fails verification is **dropped. It is never re-parsed as a legacy frame.** If the legacy codec can emit bytes that begin with `FEG1` (P9 confirms this against the current encoder), such bytes are treated as a signed-frame attempt and fall under this rule.
- In `compat`, a legacy frame is admitted under the F23 rules only while its claimed author is **not latched**. An author latches when their doc-signed, self-declared `__peers` row carries `gossip_sig: 1`, or when we have verified any signed frame from them this session. After that, legacy frames in their name are dropped with `FORGED_SENDER_LEGACY`.
- A verse Owner can set `gossip_signing: "required"` in the manifest. This is an Owner-only, pinned setting (§role-view).

The default flips to `require` at a named trigger: the next minor release, or once every declared peer in the hosted verses is latched.

**Keys.** A node's identity is its endpoint key, so rotating it means a new DID. Frames from the old DID stop being admitted once its role statements are revoked, because the role gate runs at read time. Device subkeys (canonical-log `author_key`) would be frame v2 (OD-6). The sim's virtual transport carries byte-identical signed frames.

### 4.6 FR-6: Networking diagnostics

**Model.** A new `fe-sync/src/net_diagnostics.rs` defines `NetworkDiagnostics`. It holds the reachability verdict and its timestamp, portmapper state (mapped address redacted), the home relay and `RelayHealth`, and relay-server state (serving or not, client count, denials per minute, served verses). Per peer it holds `{did_short, verses, kind: Direct{v4|v6} | Relayed{relay_host} | Mixed | Unknown, since_ms}`.

**Source.** `Endpoint::remote_info(id)` (`endpoint.rs:1644`), read through its active addresses (`TransportAddrUsage::Active`, `src/socket/remote_map/remote_state/remote_info.rs:89-95`). FE holds no `Connection` objects: iroh-docs and iroh-gossip own them, and iroh's hook docs warn against holding one (`src/endpoint/hooks.rs:79-102`). A peer with both kinds active is `Mixed`, and one with nothing known is `Unknown`. The kind is **never inferred from config**.

**Downgrade hint.** A peer that stays relayed for more than 10 minutes while both sides report `has_udp` is flagged "relayed while direct expected".

**Surfaces:**

- **Sync thread:** `SyncCommand::GetNetworkDiagnostics` returns `SyncEvent::NetworkDiagnostics`, on demand only. The UI polls at most once every 5 s, and only while the panel is open.
- **GUI:** `peer_debug.rs` becomes a Network panel.
- **API and MCP:** `GET /api/v1/network/diagnostics` and the MCP tool `get_network_diagnostics` are read-only and scope-confined. Peers are filtered to verses where the caller is Viewer+, and IPs are redacted to their address family. Only an Owner with `FE_API_NETWORK_DETAIL=1` sees full addresses (OperatorGate idiom; OD-11).

### 4.7 Threat model (STRIDE)

| ID | Threat | STRIDE | Control | Residual |
|---|---|---|---|---|
| T1 | Open-relay abuse / bandwidth amplification | D,E | FR-3 members-only gate (never `open_to_all`), caps, `client_rx`. The relay forwards only between connected members. QAD is off on desktop | Handshake CPU is spent before `on_connect` |
| T2 | Connection flood against the hosting peer | D | Gate caps, circuit breaker, documented firewall / reverse-proxy limits | `accept_conn_limit` is unimplemented upstream |
| T3 | Rogue advertisement from a compromised or hostile Manager | I,D | FR-4 signature, role check at read time, TTL, bounds, health checks; DialOnly default; local deny | Members who pin the relay share metadata with its operator |
| T4 | Forged advertisement, or one replayed into another verse | S,T | `SignedEntry` plus an inner signature binding verse and expiry; the author must equal `advertised_by` | none known |
| T5 | Relay impersonation | S | WebPKI TLS. Pinning only in the Fable-gated OD-3 phase. Payloads are end to end | A CA compromise exposes metadata only |
| T6 | Metadata exposure (who, when, how much) | I | UI disclosure; DialOnly; diagnostics show the relay per peer | **Accepted and documented** |
| T7 | IP privacy of relay operators | I | Consent dialog; DNS name or VPS recommended; advertisements are readable by members only | Members learn the operator's IP |
| T8 | IP privacy of clients | I | DialOnly, peer-address filter, local deny | A pinned relay sees the client's IP |
| T9 | Sybil members | S,E | Explicit role statements only; per-endpoint cap; per-client rx limit; grants need Owner/Manager | Bounded by the honesty of grantors |
| T10 | Revocation lag on a live session | E | Snapshot update, `Clients::disconnect`, generation re-check | The standalone binary needs a restart |
| T11 | Home-relay capture / cross-verse unreachability (hazard B) | D,I | DialOnly default; `HomeEligible` only by local pin; the Configured fallback is never removed | A pinned home relay excludes non-members |
| T12 | Forced downgrade to relayed | I | NAT candidates are end-to-end QUIC frames, so a relay cannot forge or selectively strip them; downgrade hint | An on-path UDP blocker can still force relaying |
| T13 | Gossip spoofing / mixed-version bypass | S,R | Signing; no legacy fallback after a bad signature; author latch; verse `require` | Unlatched legacy authors during `compat` |
| T14 | Gossip replay | T | HLC high-water mark, LRU, max age, skew bound | none known |
| T15 | Dial-back SSRF / reflection | S,D | FR-2b controls | none known |
| T16 | Relay-URL injection via peer addresses | I | Peer-address filter | none known |
| T17 | Operator token sent to an advertised relay | I | Token only on Configured relays; inserts use `auth_token: None` | none known |
| T18 | Bug in the cert-pinning verifier (OD-3 phase) | S | The endpoint-global verifier delegates to WebPKI for unpinned hosts; dedicated Fable gate | Relay auth and metadata exposed; payloads stay end to end |
| T19 | Flooding the abuse log | D | Rate-limited events, no IPs | none known |
| T20 | Cross-verse traffic through a shared relay | D | iroh-relay forwards between any two admitted clients; `client_rx` bounds the sender; one relay process per verse set where isolation matters | **Accepted and documented** |
| T21 | Pin hijack of a Configured relay hostname (OD-3 phase) | S | Pins are never installed for Configured or Default hosts; pins are keyed by exact `ServerName`; the verifier delegates signature checks to WebPKI | none known |
| T22 | Relay-only newcomer locked out (bootstrap deadlock) | D | Admission is explicit Viewer+ only; no `__peers` intersection (F0) | none known |
| T23 | Dial-back fan-out: one request makes every member probe | D | `reach_probe` names one `prober_did`; others drop it; probers are servers by default | none known |
| T24 | Admit/revoke race (`on_connect` precedes `register`) | E | Convergent revocation: generation stamp, gate-side connection table, 250 ms retry until `on_disconnect` | none known |
| T25 | LRU eviction reopens a replay window | T | Per-(verse, author) LRU; an attacker evicts only its own entries | none known |

## 5. Acceptance criteria (observable outcomes via real flows)

**Relay server and access control (FR-1, FR-3)**

- **AC-1:** If the binary is built without `relay-server`, or `FE_RELAY_SERVE` is anything other than `1`, no relay listener is bound. The test asserts that no port opened, not merely that the config value was read.
- **AC-2:** `FE_RELAY_SERVE=1` with no cert mode, an empty verse set, or `AllowAll` access refuses to start with a named error.
- **AC-3:** Two relay-only members of verse V (both built with `clear_ip_transports`) sync a row through an in-process relay (`run_relay_server_with_access` with `RelayMembershipGate`).
- **AC-4:** A non-member is denied with `not_member`, even when V is `default_access: viewer`. A member over the per-endpoint cap is also denied. A freshly granted Viewer that has never published a `__peers` row is admitted (no bootstrap deadlock).
- **AC-5:** Revoking a connected member's role disconnects its live session within 5 s, and its reconnect is denied, including when the revoke lands between `on_connect` and `Clients::register` (the test injects the revoke inside the gate's admit path and asserts the connection is still torn down).

**Advertisement hardening (FR-4)**

- **AC-6:** These advertisements are rejected at read time, and `insert_relay` is never called for them: a bad inner signature, an author that differs from `advertised_by`, an Editor advertiser, an expired advertisement, a `verse_id` mismatch, an `http://` or private-IP URL, and a fifth advertisement in one verse. An unhealthy relay is never inserted, and an adopted relay that becomes unhealthy is removed. In the OD-3 phase, an advertisement whose `cert_spki_sha256` names a Configured or Default relay's host installs no pin.
- **AC-7:** With default settings and no coverage, a valid advertisement adds nothing to the relay map, and the peer is still reachable through its own home relay. With a local pin, `insert_relay` fires with `auth_token == None`.
- **AC-8:** Withdrawing an advertisement whose URL equals a Configured relay leaves that relay in the map. A URL advertised in two verses survives withdrawal from one of them.
- **AC-9 (hazard B, decided at F0):** Peer A pins a V1-only relay R1 as home, and peer C shares only V2 with A. The test asserts the **documented warning path**: R1 denies C with `not_member`; A's Configured relays are still in A's map; A's diagnostics raise the hazard-B warning naming V2; and A → C still connects through C's home relay. It does not assert that C reaches A unprompted, because iroh 1.3 keeps one home relay per endpoint and C has no admitted route to it.
- **AC-17 (OD-13 coverage rule):** A node that participates only in verses whose admitted, healthy advertisements all name URL R adopts R as `HomeEligible` with no local action, and `Default` relays are demoted only once R is connected as home. Joining a verse that carries no such advertisement drops R back to `DialOnly` and re-inserts the `Default` relays within one reconcile pass; explicit Configured relays are unchanged throughout. With the "never adopt automatically" toggle on, R stays `DialOnly`.
- **AC-10:** A peer address that carries a non-allowlisted `Relay(url)` has that entry stripped, and the endpoint never dials that host.

**Gossip signing (FR-5)**

- **AC-11:** On the real two-endpoint gossip harness, these frames are dropped with named reasons: an unsigned frame claiming a latched author, a frame signed with the wrong key, a frame signed for V1 that arrives on V2's topic, a replayed frame, a stale frame, and a frame whose inner `from_did` differs from its author. A signed frame that crosses several gossip hops from a declared Viewer+ is admitted with authenticated attribution.
- **AC-12:** A `compat` peer accepts direct frames from an unlatched legacy peer. Once that peer declares `gossip_sig: 1`, its legacy frames are refused. Under `require`, every legacy frame is refused.

**Diagnostics and reachability (FR-6, FR-2)**

- **AC-13:** A pair connected over IP transports reports `Direct`. A relay-only pair reports `Relayed{relay_host}`.
- **AC-14:** The API and MCP omit peers in verses the caller cannot read, and redact IPs. An Owner with `FE_API_NETWORK_DETAIL=1` sees full addresses.
- **AC-15:** The classifier maps each table case to the right verdict: CGNAT, symmetric NAT, captive portal, no UDP, and public. The prober refuses private, loopback and DNS-rebinding targets, and refuses redirects.
- **AC-16:** A desktop cannot write an advertisement for its own relay without a dial-back record no older than 10 minutes.

## 6. Open decisions (recommended default · reversal cost)

- **OD-1 Server location.** Recommended: an fe-sync `relay-server` feature module, which has direct access to the VerseFabric snapshots. Alternative: a new crate. *Reversal cost: medium.*
- **OD-2 Desktop relay-capable mode.** Recommended: a GUI build feature that is off in v1, shipping fe-relay first. Alternative: built by default and off at runtime. *Low.*
- **OD-3 TLS.** Recommended: ACME or an operator-supplied cert first. A domain-free self-signed cert, with its SPKI pinned in the signed advertisement, comes later as a separate **Fable-gated** phase. That phase needs a custom verifier (`CaTlsConfig::custom_server_cert_verifier`, `tls.rs:141`), which is endpoint-global and set once at bind (`endpoint.rs:724`), so it must fall back to WebPKI for unpinned hosts. **F0 verifier contract:** the verifier reads a live `Arc<RwLock<PinSet>>` keyed by exact `ServerName` (the sync thread updates it; the verifier object itself is never rebuilt); for an unpinned name it delegates `verify_server_cert`, `verify_tls12_signature`, `verify_tls13_signature` and `supported_verify_schemes` to `WebPkiServerVerifier` unchanged; for a pinned name it accepts when the leaf SPKI SHA-256 matches **or** WebPKI validates, and still delegates the two signature methods (a verifier that returns success from them accepts any handshake, which is the failure the crate's own warning at `tls.rs:118` describes). Pins are never installed for Configured or Default hosts (T21), and the pin set is bounded by the advertisement bounds. `insecure_skip_verify` stays test-only. *Medium.*
- **OD-4 Admission set.** **F0 ruling: explicit Viewer+ only.** The draft's `∩ declared __peers` deadlocked relay-only newcomers (§4.3, T22). Including `open_to_all` is **rejected**. *Low.*
- **OD-5 Inner advertisement signature.** Recommended: sign the row in addition to `SignedEntry`. The inner signature is portable to the REST and ticket paths, and it binds the TTL and the verse. *Low.*
- **OD-6 Gossip signing key.** Recommended: the identity root key now. Alternative: a canonical-log `author_key` device subkey, as frame v2. *Medium.*
- **OD-7 Frame codec.** Recommended: canonical CBOR via `fe-canonical-log::cbor`, with a byte-exact re-encode check. Alternatives: postcard, or JSON with a detached signature. *Medium.*
- **OD-8 Rollout.** Recommended: `compat` for one release, then `require`. Alternative: `require` immediately. *Low.*
- **OD-9 Default adoption mode.** Recommended: DialOnly. This **requires the sibling's OD-6 to change** (§8). Alternative: AutoAdopt into the relay map. *Low.*
- **OD-10 Reachability source.** Recommended: `unstable-net-report` behind an adapter, with iroh pinned to `~1.3`. Alternative: `watch_addr` only. *Low.*
- **OD-11 Diagnostics detail.** Recommended: redacted by default, with full addresses for an Owner who opts in. *Low.*
- **OD-12 Revocation.** Recommended: an event-driven disconnect with a 5 s target, made convergent by a 250 ms retry loop over the gate's own connection table (§4.3). A pure periodic sweep is the alternative. *Low.*
- **OD-13 Coverage auto-home (added at F0).** Recommended: an advertised relay becomes `HomeEligible` automatically only while every verse this node participates in advertises it, and `Default` (n0) relays are demoted only while that relay is connected as home; a Settings toggle disables it. Alternative: pin-only `HomeEligible`, which keeps every advertised relay out of home selection unless a person acts on each peer, so a verse operator's relay never becomes its members' home without per-peer action. **Needs a user decision**, because it changes which relay carries a member's metadata by default. *Reversal cost: low (a planner rule and a toggle).*

## 7. Risks

- **`unstable-net-report` may change.** The adapter and the iroh minor-version pin contain the damage.
- **New dependencies.** `iroh-relay/server` adds `tokio-rustls-acme`, `rcgen`, `tokio-websockets`, `clap`, `toml` and others to `Cargo.lock`. `cargo deny` must pass; F3 checks it.
- **Upstream behavior may shift.** A 1.x minor could change how the home relay is selected, or add multi-home support. AC-7 and AC-9 pin the behavior this track relies on.
- **Gossip signing adds a verify to a hot path.** It costs one signature verification per compute response. F2 benchmarks it.
- **The author latch has a gap.** A fresh peer cannot latch anyone until `__peers` converges. The verse-level `require` setting closes that gap.
- **OD-13 demotes n0's fleet while a covered relay is home.** If that relay fails and every re-insert trigger misses, the node is unreachable by relay until the health loop notices. The 30 s `home_relay_status()` trigger is the mitigation; F1 checks that the re-insert path has no condition under which it stays silent. Explicit Configured relays are never touched.
- **Operators can misconfigure a relay.** The refuse-to-start rules, the consent dialog and the circuit breaker bound the damage.
- **Shared files with the sibling track.** This track shares paths with the sibling (see `shared_writes` in metadata). W1 starts only after `relay_operability_20261031` closes.

## 8. Dependencies

**Hard dependency:** `relay_operability_20261031` (S-RELAY-ADVERTISE, its FR-7). This track needs the following changes there. **All eight were applied to that track's spec, plan and metadata at Gate F0 (2026-10-31)**; the list stays here as the contract this track consumes.

1. **Change the OD-6 default** from AutoAdopt (insert into the relay map) to DialOnly or deny-until-pinned (T11).
2. **Keep the `__relays` row schema versioned.** Unknown fields must be ignored, so FR-4's fields land without a migration.
3. **Admit at read time** on the inbound-apply path, and re-evaluate when an advertiser is demoted. Checking only at write time is not enough.
4. **Refcount relays across verses**, and never call `remove_relay` on a Configured URL.
5. **Never attach `RelayAccessToken` to an advertised relay.** Make this an S7 tripwire.
6. **Correct the FR-7 risk text.** Doc rows are attributed by `SignedEntry`, so the claim-based residual applies **only to gossip**.
7. **Expose a single `validate_advertised_relay_url`** that FR-4 can extend.
8. **Point OD-4 and the non-goal at this track** as the embedded-server phase.

**Related tracks:** `iroh_1_0_upgrade_20260711` (the iroh 1.3 stack), `role_propagation_20261013` (the sync-plane role view), `canonical_data_log_20260808` (the signing primitives), and `plugin_host_revival_20261031` (confirms plugin networking is a non-goal).

**iroh features:** `test-utils` as a fe-sync dev-dependency, and `unstable-net-report` for FR-2.

## 9. Fable review record

**Gate F0** (design and threat-model review of this spec, on the `fable` tier) ran on 2026-10-31, before any code. Its record is [`review-fable-design.md`](./review-fable-design.md). A FAIL would have blocked W1.

**F0 focus items, as ruled:**

- **(a) Revocation.** `Clients::disconnect(id, None)` tears down the active connection and every inactive duplicate, asynchronously, and `on_disconnect` fires once per admitted connection. The one-shot generation check in the draft could miss a revoke that lands between `on_connect` and `Clients::register`; replaced by convergent revocation (§4.3, T24). Reconnect storms stay a firewall concern and are stated as such.
- **(b) The OD-3 pinning verifier.** The verifier contract is now written into OD-3: live pin set keyed by exact `ServerName`, full WebPKI delegation for unpinned names, signature methods always delegated, no pins for Configured or Default hosts (T21).
- **(c) Hazard B.** Confirmed from `actor.rs`: one `preferred_relay` per endpoint, and any relay URL in a peer's address is dialed with whatever `auth_token` the map holds (`None` when absent). DialOnly is sufficient for safety. AC-9 is decided as the documented warning path. The coverage rule (OD-13) is added so a relay that serves every verse a node is in can become home without per-peer action; it needs a user decision.
- **(d) FR-2b.** Tightened: every resolved address must be public, the resolver override keeps hostname verification, probers are servers by default, the request names one prober (no fan-out, T23), and the record binds requester and prober.
- **(e) Portmapped address in `watch_addr`.** Confirmed (`socket.rs:1811-1827`). It is never TCP evidence.
- **(f) The `open_to_all` exclusion.** Holds. The `∩ __peers` intersection was removed because it deadlocked relay-only newcomers (T22).
- **(g) Downgrade paths in `compat`.** The bad-signature-never-legacy rule and the author latch hold. Added the `FEG1`-collision clause for the legacy codec and made the replay LRU per author (T25). Unlatched legacy authors remain the accepted `compat` residual until the `require` flip.

**Verdicts:**

- F0: **PASS-WITH-FIXES** (2026-10-31, `fable`; 4 high, 7 medium, 5 low findings, all fixed in place; see the review file). Residual risks accepted: relay metadata exposure (T6-T8), handshake CPU before `on_connect` (T1/T2), cross-verse forwarding on a shared relay (T20), unlatched legacy authors during `compat` (T13), in-memory replay guard across a restart (within max-age only), Sybil members bounded by caps and grantor honesty (T9). Needs a user decision: OD-13.
- F1: _(pending, after W2; `review-fable-f1.md`)_
- F2: _(pending, after W3; `review-fable-f2.md`)_
- F3: _(pending, pre-merge; `review-fable-f3.md`)_
