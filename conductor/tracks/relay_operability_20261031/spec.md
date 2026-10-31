---
type: spec
title: Relay operability: metrics, usage events, verse-advertised relays, TLS/backup/self-host docs
track: relay_operability_20261031
tags: [feature, relay, fe-api, fe-sync, fe-relay, operability, open-core, relay_operability_20261031]
created: 2026-10-31
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: user grill 2026-10-31 (open-core alignment)
depends_on: []
related: [iroh_1_0_upgrade_20260711]
---

# Specification: Relay operability

**Track ID:** `relay_operability_20261031` · **Type:** feature · **Priority:** P1
**Crates:** `fe-api` (middleware, metrics, HTTP surface), `fe-runtime` (shared trait), `fe-sync` (relay config, verse-advertised relays), `fractalengine` / `fractalengine-relay` (binary wiring), `docker/`
**Citations computed at:** `f55ca21`. Symbol names are stable; `fe-api/src/lib.rs` has an uncommitted in-flight edit from another session (screenshot/operator_gate work) — treat its line numbers as volatile, cite symbols.

## 1. User vision (2026-10-31, open-core alignment)

Per `conductor/open-core.md`, anything a self-hoster needs to run, verify or extend FractalEngine
is open. This track gives a self-hoster's relay first-class operability — metrics, usage
accounting, backups, TLS, and relays that a verse can advertise instead of hand-configuring every
peer. It is framed entirely for self-hosters; the same seams (S-RELAY, S-RELAY-ADVERTISE) are
what any downstream operator tooling builds on, and this repo never names or depends on such
tooling.

## 2. Problem & evidence

**Unobservable.** `fractalengine-relay/src/main.rs:366` reads `FE_BIND_ADDR` (default
`0.0.0.0:8765`) straight into `fe_api::spawn_api_thread`; `fe-api/src/server.rs`'s
`build_router` (`:79-438`) exposes only `/api/v1/health` (always 200, `:112`) and `/ready`
(DB ping, `ready_handler`, `:459-500`) — no `/metrics` route, no usage-accounting hook anywhere in `fe-api/src/*.rs`.

**Plain-text logs.** Both binaries call
`tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init()`
(`fractalengine-relay/src/main.rs:46-48`) — no JSON option for a Loki/CloudWatch/journald pipeline.

**No TLS, no reverse-proxy recipe.** `docker/Dockerfile.relay` exposes 8765 as plain HTTP
(`EXPOSE 8765`, `:14`), `docker/README.md`'s compose example maps `8765:8765` with no TLS layer (`:29-56`), and
`SECURITY.md`'s scope line (`:91-96`) lists the gateway as in-scope attack surface with no TLS
story.

**No backup/restore doc.** `docker/README.md`'s "Data Persistence" section (`:76-78`) says only
"mount a named volume" — no snapshot/restore procedure for the SurrealKV store at `/data`.

**Every peer is pinned to n0's public relay fleet unless `FE_SYNC_RELAY` is set by hand.**
`fe-sync/src/AGENTS.md` §relay-health (line 575) documents `RelayConfig::from_env()` precedence
(`FE_SYNC_RELAY` → app-settings → `Default` = n0's v1 fleet) and flags that those relays "are
shared and rate-limited" (:583-586) — production "should still configure their own relay."
§relay-health's "Self-hosting iroh-relay 1.x" subsection (line 688) documents the developer-facing
facts (the `iroh-relay` binary's `server` feature, `--dev` being test-only, TLS, an
`access.shared_token` TOML, and the `FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` bearer token peers
attach via `RelayAccessToken`), but there is no operator-facing compose service or runbook, and no
mechanism for a verse to hand peers its own relay URL at all.

**Hole-punching does not remove the relay's role.** Per a verified read of iroh 1.3's source
(peer research session, `portmapper.rs` / `PortmapperConfig::Enabled` default), iroh already
attempts UPnP/PCP/NAT-PMP mapping by default and upgrades most connections to direct. That is why
this track is about the relay's *coordination/fallback* role, not a reason to skip it: symmetric
NAT, CGNAT, and locked-down networks cannot hole-punch, and iroh's `home_relay_status()` watcher
(already consumed by §relay-health's `RelayHealth`, line 650-655) exists exactly because the
relay stays load-bearing for those peers.

## 3. Scope / Non-goals

**In scope (public, self-hoster-facing):**
- Opt-in OpenMetrics `/metrics` (own listener, off by default).
- `UsageEvent` + `UsageSink` trait with a no-op default and a JSON-lines file reference sink,
  emitted from `fe-api` middleware for every request.
- Structured JSON logs (opt-in, default stays human-readable to avoid breaking existing operators).
- A SurrealKV backup/restore runbook for the `/data` volume.
- A self-hosted `iroh-relay` 1.x recipe (compose service + `FE_SYNC_RELAY` pointed at it) so
  operators are not stuck on n0's rate-limited public fleet.
- A documented reverse-proxy TLS recipe (nginx/Caddy terminating TLS in front of 8765).
- **S-RELAY-ADVERTISE**: a verse can advertise relay URLs in its own doc; peers add/remove them
  at runtime via iroh 1.3's `Endpoint::insert_relay`/`remove_relay` (verified present in the
  `iroh-1.3.0` source, `endpoint.rs`, by the same research session). Write-gated to Owner/Manager;
  each peer keeps a **local** adopt/pin/deny policy so a verse cannot force a peer onto a relay
  it refuses.
- A consolidated environment-variable config reference across `fe-api`, `fe-sync`, `fe-relay`.

**Non-goals (outside this repository or out of scope entirely):**
- Aggregating, metering or acting on `UsageEvent`s beyond recording them — the trait and its
  no-op/JSONL sink are all this track ships; what an operator does with the records is the
  operator's concern (`conductor/open-core.md` §The rule).
- Multiple isolated deployments inside one relay process — this track adds no such concept to
  `fe-relay`; isolation stays one process per deployment.
- An in-process TLS listener in v1 (OD-2 recommends the reverse-proxy recipe first).
- **No all-peer relay mesh.** Serving as an iroh relay needs a public, TLS-reachable address —
  practically a domain — so relay-serving stays an opt-in recipe here, never something every
  desktop peer does. The embedded, members-only `iroh-relay` server inside `fe-relay` (and the
  opt-in relay-capable desktop mode) is the next phase and belongs to
  `peer_networking_support_20261031`, which consumes this track's S-RELAY-ADVERTISE seam.
- GUI surface for the S-RELAY-ADVERTISE pin/deny policy (env-var only in v1, mirrors how
  `RelayConfig`'s app-settings source was added only after an env-only baseline — §relay-health,
  line 606-622).

## 4. Design

### FR-1 — `UsageEvent` + `UsageSink` (seam **S-RELAY**)

New `fe-runtime/src/usage.rs` (fe-runtime already hosts the one other transport-agnostic trait of
this shape, `BlobTransport`, in `fe-runtime/src/blob_transport.rs`):

```rust
pub struct UsageEvent {
    pub caller_did: Option<String>,   // None for unauthenticated (public) routes
    pub scope: Option<String>,        // the token's ApiClaims.scope ("VERSE#v1", ...); None if unauthenticated
    pub route_class: String,          // axum MatchedPath template, e.g. "/api/v1/petals/{petal_id}/nodes"
    pub method: String,
    pub status: u16,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
    pub at_unix_ms: u64,
}

#[async_trait::async_trait]
pub trait UsageSink: Send + Sync {
    async fn record(&self, event: UsageEvent);
}

pub struct NoopUsageSink; // default
pub struct JsonLinesUsageSink { /* append-only file handle behind a tokio Mutex */ }
```

`route_class` is the **matched route template**, never the expanded path (tripwire). The only
identifiers a usage row carries are `caller_did` and the token's `scope` string (the seam's
"caller DID, scope" fields, `conductor/open-core.md` S-RELAY); neither is ever a `/metrics` label,
so a petal/node ULID from a request path never reaches a row or a label. A new `usage_middleware.rs` (mirrors
the existing `mcp_concurrency_limit` middleware, `fe-api/src/server.rs:443-452`) wraps every
route, reads `ApiClaims` from the request extensions the auth middleware already inserts, and
calls `state.usage_sink.record(event)` after `next.run(req).await` returns. `ApiState`/`ApiConfig`
gain a `usage_sink: Arc<dyn UsageSink>` field; a new `usage_sink_from_env()` in
`fe-api/src/lib.rs` follows the exact idiom already established by
`elevated_query_allowed_from_env()`/`upload_quota_bytes_from_env()`: `FE_API_USAGE_LOG=<path>`
selects `JsonLinesUsageSink`, unset means `NoopUsageSink` — matching the `OperatorGate` idiom's
"opt-in, read once at startup" posture (`fe-api/src/operator_gate.rs`).

### FR-2 — Opt-in OpenMetrics `/metrics` (seam **S-RELAY**)

A **separate listener**, never a route merged into the authenticated router (OD-1): unset
`FE_API_METRICS_BIND` means no second `TcpListener` is ever bound — stronger than an auth-gated
route on the main port. When set (recommended: `127.0.0.1:9765`, loopback-only), a new
`fe-api/src/metrics.rs` spawns a tiny axum app exposing `GET /metrics` in OpenMetrics text:
request/error counts labelled `{route_class, method, status_class}` plus a duration histogram —
fed by the same per-request hook as FR-1's `UsageEvent` (one measurement point, two consumers).
OD-8: P2P/sync gauges (peer count, `RelayHealth`) are a follow-on once FR-7 lands, not v1.

### FR-3 — Structured JSON logs

`FE_LOG_FORMAT=json` switches `tracing_subscriber::fmt()` to `.json()` in both
`fractalengine-relay/src/main.rs:46-48` and the GUI binary's equivalent init; unset keeps today's
human-readable format so no existing operator's log pipeline breaks silently.

### FR-4 — Backup/restore runbook

A procedure for the SurrealKV data dir (`FE_DB_PATH`, default `/data/fractalengine.db`,
`docker/Dockerfile.relay:15`) and the co-located blob dir (`derive_blob_dir_from_db_path`,
`fractalengine-relay/src/main.rs:31-36`): stop-the-world file copy (SurrealKV has no server-side
snapshot command), `FE_SHUTDOWN_AFTER_SECS`/SIGTERM for a clean quiesce before copying (`:702-720`,
`:670-700`), and the restore side (stop, replace `/data`, start). Lives in the new
`docs/relay-operations.md` (OD-9).

### FR-5 — Self-hosted `iroh-relay` recipe

A `docker/compose.dev.yml` service running `n0computer/iroh-relay` (the same digest-pinned image
already proven against a real `SyncEndpoint` in `fe-sync/tests/relay_assisted_test.rs`, per
`conductor/tracks/iroh_1_0_upgrade_20260711/plan.md` ~line 128-135) plus `FE_SYNC_RELAY=<url>`
pointing peers at it. The operator doc lifts §relay-health's "Self-hosting iroh-relay 1.x"
subsection (line 688: non-`--dev` invocation, TLS, `access.shared_token`, and the peer-side
`FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN`) into an operator runbook rather than restating it.
D-23 ("which host is the production default") stays open — this documents **self-hosting one**,
not a hosted default.

### FR-6 — Documented TLS recipe

A reverse-proxy recipe (nginx/Caddy) terminating TLS and forwarding to `127.0.0.1:8765`
(`docker/README.md` / `docs/relay-operations.md`). OD-2 recommends this over an in-process
rustls listener for v1 — mature cert tooling (ACME via Caddy/certbot) over a new `fe-api` TLS
dependency.

### FR-7 — **S-RELAY-ADVERTISE**: verse-advertised relays

New doc table `__relays/{relay_id}` in `fe-sync` (ULID row id, same shape as the existing
`__peers` table, `PEER_DECL_TABLE`/`PeerDeclaration` in `fe-sync/src/sharding.rs:42,194-201`):
`{v: 1, url, advertised_by, note}`. The row is **versioned**: `v` is required, readers ignore
unknown fields (no `deny_unknown_fields`), and a reader that meets a `v` it does not know skips
the row with a `warn!`. This lets the next phase (`peer_networking_support_20261031` FR-4) add its
signature, expiry and dial-back fields without a migration. Unlike `__peers`' self-declaration
check (`:1658-1663`, "rows must be self-declared"), admission here is **role**-gated — only
Owner/Manager at the verse scope may advertise (OD-5), reusing the same
`resolve_role`/`fe_policy::role_grants` machinery the distributed-query gate already calls
(`:49-57`), never a new ad-hoc check. The role check runs **at read time on the inbound-apply
path** on every replica, not only at write time: a row whose `advertised_by` does not hold
Manager+ at apply time is quarantined (same 1024-row cap as `__shards`), and when an advertiser
is later demoted every live advertisement in its name is re-evaluated and withdrawn. The doc
entry's `SignedEntry` author must equal `advertised_by`, so a row cannot be written in another
member's name. URLs are validated by **one** function, `validate_advertised_relay_url` in
`fe-sync/src/relay_advertise.rs`, which wraps the same `iroh::RelayUrl` parser `RelayConfig::parse`
already uses (§relay-health, line 593-596) and is the single hook the next phase extends with its
stricter policy (`https` only, no userinfo/query/fragment, no private IP literal). The REST route
and the admission gate both call it — malformed advertisements are rejected at write time *and*
at apply time, never at dial time.

The sync thread watches `__relays` the same way it watches `__peers` and inbound replicated rows
(§inbound-apply pump shape, line 384-399) and feeds a **local** `RelayAdvertisePolicy` (env
`FE_SYNC_RELAY_ADVERTISE_POLICY`, OD-6). Its default is **`DialOnly`**: an admitted advertisement
makes the URL known to the peer (it is shown in the UI and allowlisted for dialing) but calls
**no** `insert_relay`, so it can never become this endpoint's home relay. iroh dials any relay
URL that appears in a remote peer's address regardless of the relay map (verified in iroh 1.3.0
`src/socket/transports/relay/actor.rs`, `active_relay_handle`/`start_active_relay`), so DialOnly
still lets a peer reach members whose home that relay is. `Pinned(urls)` calls
`endpoint.insert_relay(url, RelayConfig { auth_token: None, .. })` for the named URLs only;
`Deny` ignores every advertisement. Removal is **refcounted across verses**: the same URL advertised
in two verses is removed only when its last advertisement is withdrawn, and a URL that is also a
*configured* relay (`FE_SYNC_RELAY`, app settings, or the Default fleet) is **never** passed to
`remove_relay` by this reconciliation. All of this state is **local only, never replicated**
(tripwire) — a verse's Owner/Manager cannot force an unwilling peer onto a relay it has not
agreed to trust, and cannot knock a peer's own relay out of its map.

**Write path.** `repl.write_row(PEER_DECL_TABLE, ...)` (`sync_thread.rs:2051`) is the precedent
for a non-SurrealDB doc table, but `__peers` rows are published directly by the sync thread, not
through a command. `__relays` rows instead reuse the generic `SyncCommand::WriteRowEntry`
(`fe-sync/src/messages.rs`; handler `handle_write_row_entry`, dispatched at `sync_thread.rs:970`;
`:3646` and `:7014` are tests writing `verse` rows through it) with `table: "__relays"` — no new
`SyncCommand` variant. Its payload is a blob-store `content_hash`, not inline bytes, so the caller
first adds the serialized row to the blob store (the same shape the verse-manifest write uses).
The missing piece is a *caller*: a minimal REST route,
`POST /api/v1/verses/{verse_id}/relays` (advertise) and `DELETE .../relays/{relay_id}`
(withdraw), role-checked Owner/Manager by the same idiom as the sim-control routes
(`fe-api/src/server.rs:406-414`), which builds the row, stores it as a blob and forwards its
hash as a `WriteRowEntry` — one real, auditable write path for operator onboarding flows (and any
future GUI).

**Access-gated relays (OD-10).** A production relay normally restricts access
(`access.shared_token`, §relay-health line 688), and peers present
`FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` only to `Custom` relays over `https://` or loopback
(`RelayConfig::to_relay_mode_with_token`). A `__relays` row is replicated in the clear, so it
**never** carries a credential. **An advertised relay never carries `RelayAccessToken` either
(tripwire, checked at S7 and S9):** iroh reads `auth_token` per URL from the relay map
(iroh 1.3.0 `actor.rs`, `start_active_relay`), the configured token is applied to configured
entries at bind, and every `insert_relay` this track performs passes `auth_token: None`. When an
advertised URL equals a configured one, the configured entry wins and is left untouched. So a
token-gated relay must be *configured*, not merely advertised; an advertised token-gated relay is
simply unusable by peers that lack its token and must be surfaced by `RelayHealth`/`warn!`, never
silently adopted. The credential-free way to admit exactly a verse's members is the members-only
access control of `peer_networking_support_20261031` FR-3, which identifies each client by the
endpoint id it proves in the relay handshake; with it, no shared token exists to distribute.
Per-relay credential distribution stays out of scope.

**Attribution note (corrected at the next phase's F0 review).** `__relays` rows ride the ordinary
doc-replication path, and iroh-docs rows are author-signed (`SignedEntry`,
iroh-docs 0.101.0 `src/sync.rs`; a bad signature is rejected on insert). The row's author is
therefore wire-attributed on every replica, regardless of how many hops or replica holders it
crossed, and the read-time role check above runs on that attributed author. The claim-based
residual that §transport-admission-control (line 1293-1332) documents applies to **gossip frames
only** (plumtree hops cannot be wire-verified, item 2, line 1316-1332); it does not apply to
`__relays` rows, and `peer_networking_support_20261031` FR-5 closes it for gossip by signing
every frame. Routing a QUIC connection *through* an iroh relay **server** is unaffected either way.

## 5. Acceptance criteria (observable outcomes via real flows)

- AC-1: starting `fe-relay` with `FE_API_METRICS_BIND` unset never binds a second port (verified
  by a flow test asserting the listener count, not just reading the config value back).
- AC-2: `FE_API_METRICS_BIND=127.0.0.1:9765` + a request to an existing authenticated route
  produces a `GET 127.0.0.1:9765/metrics` response containing that route's class and status in
  OpenMetrics text form, within one request cycle.
- AC-3: every request through `build_router`, authenticated or public, produces exactly one
  `UsageEvent` with the no-op sink costing no observable allocation on the hot path when
  `FE_API_USAGE_LOG` is unset.
- AC-4: `FE_API_USAGE_LOG=<path>` appends one JSON line per request to that file, parseable as
  the `UsageEvent` shape, surviving a process restart (append, not truncate).
- AC-5: `FE_LOG_FORMAT=json` produces one JSON object per log line on stdout; unset reproduces
  today's human-readable format byte-for-byte (regression guard).
- AC-6: following the backup runbook against a running relay (quiesce → copy → restart) round-trips
  the full hierarchy — a node created before backup is present after restore from the backup copy.
- AC-7: a peer configured with `FE_SYNC_RELAY` pointed at the self-hosted `iroh-relay` compose
  service completes a relay-assisted sync with no reachable direct path (same proof shape as
  `fe-sync/tests/relay_assisted_test.rs`, now against the *documented* recipe rather than a test
  harness container).
- AC-8: an Owner calls `POST /api/v1/verses/{verse_id}/relays`; a second peer with the default
  `FE_SYNC_RELAY_ADVERTISE_POLICY=DialOnly` observes the URL become known (listed, dial-allowlisted)
  with **no** `insert_relay` call and no change to its home relay; the same peer with
  `Pinned(<that url>)` observes `endpoint.insert_relay` fire for that URL, with `auth_token == None`,
  without restarting; `DELETE .../relays/{relay_id}` triggers `remove_relay` for a pinned URL whose
  refcount reaches zero, and never for a URL that is also configured.
- AC-9: a peer with `FE_SYNC_RELAY_ADVERTISE_POLICY=Deny` never calls `insert_relay` for any
  advertised URL, even after the row replicates to it.
- AC-10: a Viewer/Editor-level token's call to `POST /api/v1/verses/{verse_id}/relays` is
  rejected (403) before any row is written, and a direct `WriteRowEntry` forgery attempt is
  rejected at the sync-thread admission gate too (defense in depth, same shape as the existing
  role-grant tests) — never reaching `insert_relay`. A row written by a Manager who is later
  demoted to Editor is withdrawn on every replica when the demotion applies.
- AC-11: a `__relays` row carrying an unknown extra field is applied normally; a row with an
  unknown `v` is skipped with a warning and nothing is inserted.

## 6. Open decisions

- **OD-1** metrics bind: separate listener, default off (recommended). *Reversal: low* — an
  additive route could be layered on later.
- **OD-2** TLS: documented reverse-proxy recipe only for v1 (recommended). *Reversal: medium* —
  an in-process rustls listener is a new dependency, added later without removing the recipe.
- **OD-3** `JsonLinesUsageSink` as the one reference impl (recommended, matches the task brief).
  *Reversal: low* — one trait impl behind `Arc<dyn UsageSink>`.
- **OD-4** self-hosted relay as a compose recipe in this track (recommended). The embedded
  `iroh-relay` `server` feature is not a rejected alternative but the next phase:
  `peer_networking_support_20261031` FR-1/FR-3 adds it to `fe-relay` as a members-only, opt-in
  server. *Reversal: n/a* — the two are sequential, and the recipe remains valid alongside.
- **OD-5** S-RELAY-ADVERTISE write gate: role-based, Owner/Manager (recommended), enforced at
  write time and again at read time on every replica. *Reversal: low* — one `resolve_role`
  call; the required level is a one-line change.
- **OD-6** local adopt policy default: **`DialOnly`** (deny-until-pinned for home eligibility).
  Changed from `AutoAdopt` at the next phase's F0 review: iroh keeps **one** home relay per
  endpoint, chosen by latency from the relay map, so inserting every advertised URL makes any
  verse's relay eligible to become the peer's home for *all* its verses. A members-only relay
  for verse V1 as home would cut the peer off from relayed peers it shares only V2 with, and
  would funnel every verse's relayed metadata to V1's operator. `DialOnly` keeps advertised relays
  dialable and visible without that exposure; `Pinned` opts a peer in. *Reversal: low* — a
  constant change, not a data-model change.
- **OD-7** `/metrics` stays unauthenticated on its own loopback-default bind (recommended).
  *Reversal: low* — a bearer-token check can be layered on later.
- **OD-8** `/metrics` v1 content: HTTP request/error/duration only (recommended). *Reversal: low*
  — P2P/sync gauges are additive once FR-7's reconciliation loop exists.
- **OD-9** `docs/relay-operations.md` as a new file (recommended — `docker/README.md` is already
  dense). *Reversal: low* — a doc merge is non-breaking.
- **OD-10** advertised-relay access: advertised relays never receive the configured relay token
  (tripwire); a token-gated relay must be configured, and an advertised one a peer cannot
  authenticate to is surfaced, not adopted (FR-7 "Access-gated relays"). Admitting exactly a
  verse's members without any shared token is the members-only access control of
  `peer_networking_support_20261031` FR-3. *Reversal: medium* — per-relay credential
  distribution would need a key-delivery design that never rides a replicated row, and the
  members-only gate removes the need for one.

## 7. Risks

- **Attribution residual is gossip-only.** `__relays` rows are `SignedEntry`-attributed, so the
  claim-based residual in §transport-admission-control does not reach them (FR-7 attribution
  note). S9 checks that the read-time role gate runs on the signed author and that nothing in this
  track moves an advertisement through gossip.
- **A hostile or compromised Manager can still advertise a relay it controls.** With the
  `DialOnly` default that relay is dialed only to reach peers homed there and never becomes a
  peer's home; expiry, bounds, health checks and the inner signature arrive with
  `peer_networking_support_20261031` FR-4.
- **Metrics/usage cardinality.** If `route_class` regresses to an expanded path (the tripwire
  this spec exists to prevent), label cardinality explodes with every node/petal id ever touched.
  AC-2's flow test must assert the *template* string, never a path containing a ULID.
- **Self-hosted relay documentation drift.** `relay_assisted_test.rs` pins an exact
  `n0computer/iroh-relay` digest for CI; the operator recipe should track the same tag so CI and
  operators do not silently diverge.
- **An advertised access-gated relay is silently unusable** by a peer that lacks its token
  (OD-10). S7 must surface this through `RelayHealth`/a `warn!` rather than adopting a relay the
  peer cannot authenticate to.
- **S-RELAY-ADVERTISE could be mistaken for a trust upgrade.** Advertising a relay is not vouching
  for its operator; the local policy (OD-6) and the no-all-peer-mesh non-goal keep a verse's
  advertisement a convenience default, never an authority grant.

## 8. Dependencies

- `iroh_1_0_upgrade_20260711` (related, not a gate: its iroh 1.3 port is landed and in use; that
  track is still `in_progress` only for its orchestrator sweep and user-gated flag-day rollout.
  D-23, the *production* relay host default, stays open and out of scope here).
- iroh 1.3's `Endpoint::insert_relay`/`remove_relay` and `home_relay_status()` (already consumed
  read-only by `RelayHealth`, now also written to by FR-7).
- `peer_networking_support_20261031` (public, next phase) consumes and hardens S-RELAY-ADVERTISE
  (inner signature, expiry, bounds, health, members-only relay serving). Its W1 starts only after
  this track's S9 verdict; the eight changes it required here (versioned row, read-time admission,
  `DialOnly` default, cross-verse refcount, no token on advertised relays, gossip-only residual,
  one URL validator, this cross-reference) were applied at its F0 review on 2026-10-31.
- Downstream operator tooling may consume `UsageSink` and S-RELAY-ADVERTISE; per
  `conductor/open-core.md` this repo never depends on, names or feature-flags any such consumer.
