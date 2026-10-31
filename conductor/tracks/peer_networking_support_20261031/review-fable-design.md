---
type: review
gate: F0
model: fable
created: 2026-10-31
track: peer_networking_support_20261031
title: "Gate F0: design and threat-model review of peer networking support"
scope: spec.md, plan.md, metadata.json of this track; cross-track changes to relay_operability_20261031; seam table in conductor/open-core.md
sources_verified: iroh-1.3.0, iroh-relay-1.3.0, iroh-gossip-0.101.0, iroh-docs-0.101.0 (cargo registry), fe-sync, fe-canonical-log, SECURITY.md at 8ee4a96 plus uncommitted edits
stance: adversarial (refute, then fix in place)
---

# Gate F0 — design review record

**VERDICT: PASS-WITH-FIXES.** Wave 1 may start once `relay_operability_20261031` closes. 16 findings (4 high, 7 medium, 5 low), all fixed in the spec, plan and metadata. One new open decision (OD-13) needs a user ruling; it does not block W1 because every slice implements it behind a toggle whose reversal is a constant.

## 1. What was verified against source

Every iroh claim in spec §2.1 was re-read in the registry sources. Line numbers are as of the versions named above.

| Claim | Source | Result |
|---|---|---|
| `on_connect` returns `Access::{Allow,Deny}`; `on_disconnect` once per admitted connection | `iroh-relay/src/server.rs:285-303`, `:350-357`, `OnDisconnectGuard :376-425` | Confirmed. The draft cited `:350` for `on_disconnect`; that line is the `Access` enum. Corrected. |
| EndpointId is proven before `on_connect` | `server.rs:222-232`; `server/http_server.rs:869-876`; `protos/handshake.rs:11-20` (TLS exported keying material or server challenge, domain-separated) | Confirmed. |
| `on_connect` runs **before** `Clients::register` | `http_server.rs:876-908` | Confirmed; this is the admit/revoke race (F0-1). |
| Embedded host can evict: `relay_service()` → `clients()` → `disconnect(id, None)` | `server.rs:949`; `http_server.rs:953`; `server/clients.rs:172-197` | Confirmed. `None` shuts down the active connection and every inactive duplicate; shutdown is asynchronous; `unregister` (`:109-170`) drops the guard and sends peer-gone notices. `disconnect` returns `false` if nothing is registered. `Clients` has no iteration API. |
| `accept_conn_limit`/`accept_conn_burst` unimplemented | `server.rs:486-499` | Confirmed ("Not currently implemented, setting this has no effect"). |
| Plain HTTP listener serves only the captive-portal probe when TLS is on | `server.rs:465-467`, `:804-812` | Confirmed. |
| `CaTlsConfig::custom_server_cert_verifier` / `with_extra_roots` / `insecure_skip_verify`; crate warns about wrong callbacks | `iroh-relay/src/tls.rs:141`, `:151`, `:94`, warning at `:115-118` | Confirmed. The default mode is embedded WebPKI (`:176-190`). |
| `Builder::ca_tls_config` is a bind-time, endpoint-wide setting | `iroh/src/endpoint.rs:724` | Confirmed (builder method; the relay client receives `tls_config` via `RelayConnectionOptions`, `actor.rs:1349-1356`). |
| One home relay per endpoint, chosen from net-report `preferred_relay` | `iroh/src/socket/transports/relay/actor.rs:1232-1257` (`on_network_change`, single `my_relay`) | Confirmed. |
| iroh dials any relay URL in a remote's address, map or not | `actor.rs:1279-1320` (`active_relay_handle_for_endpoint`), `:1322-1348` (`active_relay_handle` → `start_active_relay`, `auth_token` looked up from the map, `None` when absent) | Confirmed. |
| Non-home relay connections close when idle | `actor.rs:67-68` (`RELAY_INACTIVE_CLEANUP_TIME` = 60 s) | Confirmed. |
| Relay map contributes only `auth_token` and home candidacy | `actor.rs:1344-1348`; `iroh-relay/src/relay_map.rs:232-267` | Confirmed. |
| Portmapping on by default; **mapped address appears in `watch_addr`** | `iroh/src/portmapper.rs:20-38`; `iroh/src/socket.rs:1811-1827` (`update_direct_addresses` inserts `DirectAddrType::Portmapped`); `endpoint.rs:1289-1302` (`watch_addr` chains `ip_addrs()`) | Confirmed. The draft marked this unverified. |
| `net_report` behind `unstable-net-report`; `Report` fields; `bound_sockets`; `clear_ip_transports`; `insert_relay`/`remove_relay`; `remote_info` | `endpoint.rs:1456-1457`, `:1466`, `:510`, `:1000`, `:1014`, `:1644`; `net_report/report.rs:18-37` | Confirmed. |
| iroh-docs rows are signed; bad signature rejected | `iroh-docs/src/sync.rs:700` (`SignedEntry`), `:636`/`:689` (`BadSignature`) | Confirmed. |
| Gossip `Message` is unsigned; `delivered_from` is not the author | `iroh-gossip/src/api.rs:363-371` | Confirmed verbatim. |
| Relay datagrams carry src/dst endpoint ids in the clear | `iroh-relay/src/protos/relay.rs:75-79`, `:185-189` | Confirmed. |
| Standalone binary access modes lack eviction | `iroh-relay/src/main.rs:160-176`, `:217-229` | Confirmed (Everyone / Allowlist / Denylist / Http / shared_token). |
| FE side: `peer_did_key`, `access_snapshot` folds `open_to_all`, `BlobAccessGate`, `add_peer_addr`, `set_settings_relay_spec`, `RelayAccessToken`, `sign_domain`/`verify_domain`, `fe-oplog-v1\0`, F23 direct check, `__peers` self-declaration by author id | `fe-sync/src/{sharding.rs,docs_engine.rs,endpoint.rs,relay_config.rs,distributed_query.rs}`, `fe-canonical-log/src/signing.rs:13,57,62,71` | Confirmed. Line numbers in `sharding.rs` are volatile (uncommitted edits); symbols hold. |

## 2. Findings

Severity: **H** = would have shipped an exploitable or availability-breaking design; **M** = weakens a stated guarantee or leaves a race; **L** = precision, honesty or test-shape.

| ID | Sev | Threat | Finding | Fix applied |
|---|---|---|---|---|
| F0-1 | H | T24 (new), T10 | **Admit/revoke race was not closed.** The draft's "generation re-check after admitting" runs inside `on_connect`, but `Clients::register` runs *after* `on_connect` returns (`http_server.rs:876-908`). A revoke landing in that window calls `disconnect`, which finds nothing (`clients.rs:181-184` returns `false`), and the connection then registers and lives until it drops on its own. | §4.3 "Revocation (convergent)": the gate keeps its own `endpoint_id → {connection_id}` table from the two callbacks (`Clients` cannot be iterated), stamps admissions with the snapshot generation, and a 250 ms reconciler re-issues `disconnect` for every marked DID until `on_disconnect` has fired for each tracked entry. AC-5 now injects the revoke inside the admit path. OD-12 updated. Plan P3 updated. |
| F0-2 | H | T22 (new) | **Bootstrap deadlock in OD-4.** Admission required explicit Viewer+ **∩ declared `__peers`**. A relay-only newcomer (the CGNAT user the relay exists for) cannot publish its `__peers` row until it syncs, and cannot sync without the relay. The intersection added no security: role statements are already explicit Owner/Manager acts and replicate without the newcomer. | OD-4 ruled "explicit Viewer+ only". §4.3 Admitted set rewritten; AC-4 asserts a fresh Viewer with no `__peers` row is admitted; metadata tripwire and plan P3 updated. |
| F0-3 | H | T23 (new), T15 | **Dial-back request could fan out.** The request was a gossip frame; without a named prober every Viewer+ peer would probe the same URL, turning one request into a verse-sized reflection toward the target. Desktops as probers also put an SSRF surface on every member. | §4.2: the `reach_probe` frame names exactly one `prober_did`; all others drop it; probers are fe-relay by default and desktops only by explicit opt-in (`AppSettings.act_as_prober`, default off, plan P11). |
| F0-4 | H | T21 (new), T5, T18 | **Pin-by-hostname could hijack a Configured relay (OD-3 phase).** A hostile Manager advertising `https://<operator-relay-host>/` with an attacker SPKI would, with a hostname-keyed pin set, let an on-path or DNS attacker impersonate the operator's own relay to every member. | §4.4 Token isolation and OD-3: pins are never installed for a host equal to a Configured or Default relay's host; pins are keyed by exact `ServerName`; the verifier contract delegates `verify_tls12_signature`/`verify_tls13_signature`/`supported_verify_schemes` to WebPKI (a verifier that returns success from those accepts any handshake). AC-6 extended. |
| F0-5 | M | T11, hazard B | **AC-9 undecided; "one call onboards every peer" was not true under DialOnly.** iroh keeps one latency-chosen home relay (`actor.rs:1232-1257`). Under DialOnly nobody's home moves, so a verse operator's relay only ever carries peers who configured it by hand. Pin-only HomeEligible is safe but makes advertisement a discovery feature, not an onboarding one. | AC-9 decided as the documented warning path (R1 denies C `not_member`; A's Configured relays stay; diagnostics name the uncovered verse; A → C still works via C's home). Added **OD-13 coverage rule**: auto-HomeEligible only while *every* verse this node is in advertises the URL (then hazard B cannot occur and every verse's Manager vouched the operator), with a Settings toggle. AC-17 added. **Needs a user decision**, recorded in metadata. |
| F0-6 | M | T11 | **Default (n0) relays were lumped under Configured.** If they are never removed, a covered relay competes with n0's fleet on latency and may never become home, so OD-13 would buy nothing. | §4.4 Fallback: three tiers `Configured` / `Default` / `Advertised`. Explicit Configured is never touched; `Default` is demoted only while a coverage-eligible relay is connected as home, and re-inserted on coverage break, health failure, or `home_relay_status()` ≠ `Connected` for 30 s. New risk bullet in §7; metadata tripwire; plan P2/P6 updated. |
| F0-7 | M | T25 (new), T14 | **Replay LRU was per verse (4096).** A hostile member can flood the LRU with its own frames, evicting honest authors' entries and reopening a replay window inside `max_age` for their frames. | §4.5 item 6: LRU is per (verse, author), 256 entries; an attacker evicts only its own entries. Plan P1 updated. |
| F0-8 | M | T14 | **HLC high-water mark as a strict monotone drops reordered frames.** Plumtree hops reorder; a legitimately late frame would be rejected as a replay. | §4.5 item 6: the HWM is a window floor (`hlc < hwm − max_age` drops); the LRU dedupes inside the window. Restart gap inside `max_age` recorded for F2. |
| F0-9 | M | T15 | **Prober accepted a target with mixed A/AAAA records.** Checking only the chosen address lets a happy-eyeballs fallback reach a private record. Hostname verification during IP-pinned connect was unspecified. | §4.2 FR-2b: every resolved address must be public unicast (added NAT64, 6to4, Teredo to the reject list); connect via resolver override while verifying the original hostname. Plan P4 updated. |
| F0-10 | M | T15 | **`DialbackRecord` did not bind the requester and allowed self-attestation.** A desktop could run its own prober and "prove" itself; a record for a URL could be reused by anyone. | §4.2: record is `{prober_did, requester_did, url, at_ms, ok}` under `fe-dialback-v1\0`; the REST layer accepts only `ok`, fresh, `requester_did` = advertiser, `prober_did` ≠ advertiser and a declared Viewer+ or fe-relay. |
| F0-11 | M | T2, T19 | **Connection counting and the circuit breaker were underspecified.** A denied connection gets no `on_disconnect`, so counting in `on_connect` unconditionally leaks counts upward until caps lock everyone out. "Pauses accepting" has no iroh-relay API. | §4.3: counts increment only on `Allow`; desktop breaker closes the listener for a cool-down, fe-relay keeps serving and logs. Reconnect-storm cost (one TLS handshake per retry) stated as a firewall concern. |
| F0-12 | L | T20 (new), T6 | **Cross-verse forwarding on a shared relay was unstated.** iroh-relay forwards between any two admitted clients; a V1 member can push `client_rx`-bounded ciphertext at a V2 member. | §4.3 "Cross-verse forwarding" paragraph; T20 accepted and documented; guidance: one relay process per verse set where isolation matters. |
| F0-13 | L | T1 | **`FE_RELAY_SERVE_VERSES` with a non-hosted ULID would silently deny all** (no role view, fail-closed). `hosted` silently extends to future verses. | §4.1 env table: a non-hosted ULID is a startup error; `hosted` logs a WARN about future verses. Plan P3 test name updated. |
| F0-14 | L | T13 | **Legacy-codec collision with the `FEG1` magic was unaddressed.** If the legacy encoder can emit a leading `FEG1`, an honest legacy frame could be dropped; more importantly the rule had no stated behaviour for that byte pattern. | §4.5 mixed-version: such bytes are treated as a signed-frame attempt and dropped on failure; P9 confirms against the current encoder. |
| F0-15 | L | — | **Spec claimed the portmapped address in `watch_addr` was unverified**, and that a mapping might count toward reachability. | Verified (`socket.rs:1811-1827`); §2.1 and §4.2 updated; a `Portmapped`-only public address classifies as `BehindNat`. |
| F0-16 | L | — | Citation precision: `on_disconnect` line, `ClientRequest::endpoint_id` range, `OnDisconnectGuard` range, plain-HTTP listener semantics, handshake timeout, the fact that `client_rx` bounds sender only. | §2.1 corrected and extended. |

## 3. Focus items (a)–(h), as ruled

- **(a) Revocation and eviction.** Chain confirmed. `Clients::disconnect(id, None)` tears down every connection of the endpoint; `on_disconnect` fires once per admitted connection via the guard even when setup fails after admission. The draft's one-shot generation check could miss (F0-1); replaced by convergent revocation. Reconnect storms: each retry costs the relay one TLS handshake before `on_connect`; the gate cannot reduce that and the spec says so.
- **(b) Cert pinning (OD-3 phase).** The verifier is endpoint-global and built once at bind (`endpoint.rs:724`); the contract written into OD-3 keeps a *live* pin set so the verifier object is never rebuilt, delegates everything for unpinned names to `WebPkiServerVerifier`, always delegates the signature methods, accepts pin-or-WebPKI for pinned names, and never pins a Configured or Default host (F0-4). `insecure_skip_verify` stays test-only.
- **(c) Home relay per endpoint.** Confirmed single `preferred_relay`/`my_relay`. DialOnly is the right default and holds; the Configured fallback is never removed; AC-9 decided; coverage rule added as OD-13 with Default demotion as its necessary companion (F0-5, F0-6).
- **(d) Dial-back.** Amplification is bounded by the 1 KiB body cap, 5 s timeout and 10/min global cap; port scanning is bounded by `RelayUrlPolicy` (443 or allowlist) and public-only targets; SSRF defeated by all-addresses-public plus resolver-override connect; fan-out removed (F0-3, F0-9, F0-10).
- **(e) Gossip signing.** Domain `fe-gossip-v1\0` is distinct from `fe-oplog-v1\0` and from iroh's handshake domain, and the preimage starts with the domain, so cross-protocol reuse of the shared identity key is not exploitable. Replay guard made per-author and window-based (F0-7, F0-8). Bad signature never falls back to legacy; `FEG1` collision clause added (F0-14). Key rotation = new DID; revoked DIDs are refused by the role gate at read time; device subkeys deferred to frame v2 (OD-6). Mixed-version residual (unlatched legacy authors during `compat`) is accepted until the `require` flip; the verse-level `require` setting lets an Owner close it early.
- **(f) Members-only `AccessControl`.** Keyed on the handshake-proven endpoint id, mapped to the DID by `peer_did_key`. `open_to_all`/`default_access` never consulted (tripwire). Unconverged or missing role view → deny (fail closed). `__peers` intersection removed (F0-2). Sybil members are bounded by caps and `client_rx` and by the honesty of grantors (T9, accepted).
- **(g) Rate and connection limits.** The spec honestly states that `accept_conn_limit` is unimplemented and that TLS plus handshake run before `on_connect`; floods go to the OS, firewall or reverse proxy. Counting fixed to `Allow` only; breaker semantics fixed (F0-11).
- **(h) IP and metadata privacy.** A relay operator sees client IPs, src/dst endpoint ids, sizes and timing; payloads stay end-to-end encrypted. Clients learn the operator's IP or hostname. Disclosed in the consent dialog and advertisement UI; IPs never logged by default; diagnostics redact to address family unless an Owner opts in. Cross-verse forwarding on a shared relay added as T20 (F0-12).

## 4. Cross-track changes applied

To `relay_operability_20261031` (spec, plan, metadata): OD-6 default AutoAdopt → DialOnly; versioned `__relays` row (`v`, unknown fields ignored, unknown `v` skipped, new AC-11); role admission at read time on the inbound-apply path with quarantine and re-evaluation on demotion, `SignedEntry` author = `advertised_by`; cross-verse refcount and never `remove_relay` on a configured URL; advertised relays never carry `RelayAccessToken` (tripwire); FR-7 risk text corrected to gossip-only; one `validate_advertised_relay_url` hook; OD-4 and the no-embedded-server non-goal point to this track. AC-8 and S7's flow tests reworded to match.

To `conductor/open-core.md`: a "Peer networking" row and seams **S-RELAY-SERVE** and **S-GOSSIP-SIGN**; **S-RELAY-ADVERTISE** reworded to the hardened contract.

## 5. Residual risks accepted

| Risk | Why accepted |
|---|---|
| Relay metadata exposure: client IPs, who talks to whom, when, how much (T6-T8) | Inherent to any relay; payloads stay end-to-end; disclosed and bounded by DialOnly, the peer-address filter and local deny. Hiding metadata is a non-goal. |
| Handshake CPU is spent before `on_connect` (T1, T2) | Upstream; `accept_conn_limit` is unimplemented. Mitigation is the firewall or a reverse proxy, documented. |
| Cross-verse forwarding on a relay serving several verses (T20) | iroh-relay has no per-verse partition; bounded by `client_rx` and membership; one process per verse set where isolation matters. |
| Unlatched legacy authors can be impersonated during `compat` (T13) | This is today's S-1 residual, shrinking with every latched author; the `require` flip and the verse-level setting close it. |
| In-memory replay guard across a restart (within `max_age` only) | Bounded duplicate work for `compute`; F2 re-examines persistence. |
| Sybil members (T9) | Role grants are explicit Owner/Manager acts; caps and `client_rx` bound the damage. |
| OD-13 demotes n0's fleet while a covered relay is home | Re-insert on three independent triggers (coverage, health loop, `home_relay_status()` 30 s); explicit Configured relays never touched; F1 checks the re-insert path cannot stay silent. |
| Dial-back reveals "host:443 speaks TLS" to a Viewer+ requester | Public hosts only, 443 or allowlist, 1 probe/min/requester; this is information any Internet host already exposes. |

## 6. Needs a user decision

- **OD-13 coverage auto-home.** Recommended: on, with the Settings toggle. The alternative (pin-only HomeEligible) is safe but means an advertised relay never becomes a member's home without a per-peer action. Reversal is a constant change; W1 proceeds either way.

## 7. Tripwires added for F1-F3

- Admission is explicit Viewer+ only; never gated on a `__peers` row.
- Revocation retries `Clients::disconnect` until `on_disconnect` fires for every tracked connection.
- Pins keyed by exact `ServerName`; never for Configured or Default hosts; signature methods always delegated to WebPKI.
- `Default` relays absent only while a coverage-eligible relay is connected as home; explicit Configured never demoted.
- `reach_probe` names one prober; desktops probe only by opt-in; every resolved address public.
- Replay LRU is per (verse, author).
