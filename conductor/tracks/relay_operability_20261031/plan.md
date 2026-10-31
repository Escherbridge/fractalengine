---
type: plan
track: relay_operability_20261031
title: "Implementation Plan: relay operability (metrics, usage events, verse-advertised relays)"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: f55ca21
partitions_confidence: hypothesis
---

# Implementation plan: relay_operability_20261031

> Citations computed @`f55ca21`. Not yet sliced — run `/slice` against this plan once the user
> ratifies OD-1..OD-9 (or accepts the recommended defaults) to get `verified` partitions before
> launching agents. `fe-api/src/lib.rs` carries another session's in-flight edit; re-grep its
> symbols (don't trust line numbers) before any slice touches it.

## 0. Assumptions

Proceeding on every OD's recommended default (spec §6) unless the user overrides first:
D-RO-1..9 map 1:1 to OD-1..9. Reversal costs are recorded there; none of them block starting.

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** No slice runs cargo build/test/clippy itself; the
  orchestrator runs ONE serial gate per wave (`-p fe-runtime -p fe-api -p fe-sync`, then the two
  binaries), and the full workspace sweep once at track end.
- **No business wording anywhere in this track's code, docs, or comments.** `UsageEvent` and
  `UsageSink` describe a generic telemetry seam (`conductor/open-core.md` §The rule). The check is
  mechanical: `grep -rniE "invoice|pric(e|ing)|payment"` over every file S1-S8 touch must return
  nothing.
- **route_class is always the matched template, never an expanded path.** Every test that builds
  a `UsageEvent` or a metric must assert the template string, not a path containing an id
  (spec tripwire; see Risks).
- **`RelayAdvertisePolicy` is local state only.** No slice may add a path that replicates it or
  lets a verse doc override it. Its default is `DialOnly`; only `Pinned` calls `insert_relay`,
  always with `auth_token: None`; and reconciliation never calls `remove_relay` on a configured
  URL (spec FR-7, OD-6, OD-10).
- **Docs convention:** terse doc comments in code; rationale in the directory `AGENTS.md` section
  named in each slice.
- **Tests model real flows** (flow names below, through public seams — axum `TestServer`/`oneshot`
  for `fe-api`, a real two-`SyncEndpoint` harness for `fe-sync`, faking only the process edge: the
  system clock for `at_unix_ms`, and the JSON-lines file's filesystem). No tautologies.
- **No read-only reviewers:** the review slice (S9) fixes its own findings.

## 2. Phase / slice table

| Phase | Slice | Wave | Model | Task | Owns (files) | Depends |
|---|---|---|---|---|---|---|
| FR-1 | **S1** | W1 | sonnet | `UsageEvent`/`UsageSink`/`NoopUsageSink`/`JsonLinesUsageSink` (async-trait, append-only file behind a `tokio::sync::Mutex`) | `fe-runtime/src/usage.rs` (new), `fe-runtime/src/lib.rs` (`pub mod usage;`), `fe-runtime/src/AGENTS.md` (§usage) | none |
| FR-3 | **S5** | W1 | sonnet | `FE_LOG_FORMAT=json` toggle wrapping `tracing_subscriber::fmt()` vs `.json()` in both binaries' `main()` | `fractalengine-relay/src/main.rs`, `fractalengine/src/main.rs` | none |
| FR-7 | **S6** | W1 | sonnet | `__relays/{relay_id}` doc table + versioned `RelayAdvertisement` row type (`v: 1`, unknown fields ignored, unknown `v` skipped) + role-gated admission (Owner/Manager, reusing `resolve_role`) enforced **at read time on the inbound-apply path** with quarantine and re-evaluation on advertiser demotion + `SignedEntry` author must equal `advertised_by` + one `pub fn validate_advertised_relay_url` wrapping the existing `RelayConfig::parse` path (the single hook the next phase extends) | `fe-sync/src/relay_advertise.rs` (new), `fe-sync/src/sharding.rs` (table constant + admission hook point), `fe-sync/src/lib.rs`, `fe-sync/src/AGENTS.md` (§relay-advertise) | none |
| **gate W1** | orchestrator | n/a | n/a | Serial build + `-p fe-runtime -p fe-sync` tests once | n/a | S1, S5, S6 |
| FR-1 | **S2** | W2 | sonnet | Usage-emit middleware (`axum::middleware::from_fn_with_state`, mirrors `mcp_concurrency_limit`'s shape, `fe-api/src/server.rs:443-452`); `ApiState`/`ApiConfig` gain `usage_sink: Arc<dyn UsageSink>`; `usage_sink_from_env()` follows the `elevated_query_allowed_from_env` idiom | `fe-api/src/usage_middleware.rs` (new), `fe-api/src/server.rs`, `fe-api/src/lib.rs` (re-grep symbols first — in-flight edit), `fe-api/AGENTS.md` (§usage-metering) | S1 |
| FR-2 | **S3** | W2 | sonnet | `/metrics` on its own listener: `spawn_metrics_listener(bind_addr)` (axum app, OpenMetrics text exposition), in-memory counters fed by the same per-request hook S2 installs; `ApiConfig` gains `metrics_bind: Option<String>` + `metrics_bind_from_env()` | `fe-api/src/metrics.rs` (new), `fe-api/src/lib.rs` | S1 |
| FR-7 | **S7** | W2 | sonnet | Sync-thread reconciliation: watch `__relays` the same way `§inbound-apply` watches `__peers`/replicated rows; `RelayAdvertisePolicy::from_env()` (`FE_SYNC_RELAY_ADVERTISE_POLICY`, default **`DialOnly`**: URL listed + dial-allowlisted, **no** `insert_relay`); `Pinned(urls)` → `endpoint.insert_relay(url, RelayConfig { auth_token: None, .. })`; `Deny` ignores all; removal refcounted across verses, and `remove_relay` is **never** called for a configured URL (`FE_SYNC_RELAY`, settings, Default fleet); advertised relays never carry `RelayAccessToken` (tripwire); an advertised token-gated relay the peer cannot authenticate to is surfaced via `RelayHealth`/`warn!` | `fe-sync/src/sync_thread.rs`, `fe-sync/src/relay_config.rs` (policy type), `fe-sync/src/AGENTS.md` (§relay-advertise, extended) | S6 |
| FR-7 | **S6b** | W2 | sonnet | REST write path: `POST /api/v1/verses/{verse_id}/relays` + `DELETE .../relays/{relay_id}`, Owner/Manager role-checked (same idiom as the sim-control routes, `fe-api/src/server.rs:406-414`); builds the `__relays` row, adds it to the blob store and forwards its `content_hash` as `SyncCommand::WriteRowEntry` (no new sync command) | `fe-api/src/relay_advertise.rs` (new), `fe-api/src/server.rs`, `fe-api/AGENTS.md` (§relay-advertise) | S6 |
| **gate W2** | orchestrator | n/a | n/a | Serial build + `-p fe-api -p fe-sync` tests once | n/a | S2, S3, S7, S6b |
| integration | **S4** | W3 | sonnet | Wire both binaries' `ApiConfig` construction to the new `usage_sink`/`metrics_bind` fields, env-derived exactly like `upload_quota_bytes_from_env`'s call site | `fractalengine-relay/src/main.rs`, `fractalengine/src/main.rs` | S2, S3 |
| **gate W3** | orchestrator | n/a | n/a | Serial workspace build + both binaries' tests once | n/a | S4 |
| docs | **S8** | W4 | haiku | `docs/relay-operations.md`: backup/restore runbook, self-hosted `iroh-relay` compose recipe + `FE_SYNC_RELAY` pointer, reverse-proxy TLS recipe, consolidated env-var table (`FE_API_METRICS_BIND`, `FE_API_USAGE_LOG`, `FE_LOG_FORMAT`, `FE_SYNC_RELAY_ADVERTISE_POLICY`, plus the existing `FE_BIND_ADDR`/`FE_DB_PATH`/`FE_CORS_ORIGINS`/`FE_SYNC_RELAY` from `docker/README.md`) | `docs/relay-operations.md` (new), `docker/compose.dev.yml` (add the `iroh-relay` service), `docker/README.md` (point at the new doc) | S4, S7 |
| review | **S9** | W5 | opus | Review-and-fix (fresh context, `/code-review high`; `/security-review` on the role-gated `__relays` write path — it is an auth surface). Checks: no business wording leaked (mechanical grep, §1), `route_class` never an expanded path anywhere, metrics listener truly absent when unset, `RelayAdvertisePolicy` never replicated, D-23's "stays open" framing preserved (no accidental default-relay-host pick) | `conductor/tracks/relay_operability_20261031/review.md` (new) + fix-only edits to any S1-S8 file | S8 |
| close | n/a | n/a | n/a | Full workspace `cargo test` + `clippy --all-targets -D warnings` once; `conductor/tracks.md` row; tick this plan's boxes | n/a | S9 |

## 3. Flow tests per slice

Each name is the test to write; each asserts an observable outcome through a public seam.

**S1 (`usage.rs`, pure/async, no network)**
- `noop_sink_records_without_allocating_a_string_per_field` (a bound on cost, not a tautology:
  constructs N events, asserts no panic/leak under a loop count, not "it returns Ok").
- `json_lines_sink_appends_one_parseable_line_per_event_and_survives_reopen`.
- `json_lines_sink_is_safe_under_concurrent_record_calls` (two tasks writing concurrently; file
  has exactly N lines, none interleaved/corrupted).

**S2 (`usage_middleware.rs`, axum integration — real router, no mocked HTTP)**
- `every_authenticated_request_emits_exactly_one_usage_event_with_the_matched_route_template`
  (asserts the string equals the route *template*, e.g. `/api/v1/nodes/{node_id}`, never the
  concrete id — the tripwire test).
- `public_route_emits_a_usage_event_with_caller_did_none`.
- `usage_sink_defaults_to_noop_when_fe_api_usage_log_unset`.

**S3 (`metrics.rs`)**
- `metrics_bind_unset_never_opens_a_second_listener` (AC-1 — assert no bind attempt occurs, e.g.
  via a sentinel the spawn fn would otherwise call).
- `metrics_endpoint_reports_request_count_and_status_for_a_real_request` (AC-2 — drive a request
  through the main router, then scrape `/metrics` on the second listener, assert the counter
  incremented for that route's template + status).

**S5 (`main.rs`, both binaries — tested via a shared pure helper, not full binary spawn)**
- `log_format_env_selects_json_vs_text_formatter` (table test over a pure selector function,
  same pattern as `derive_blob_dir_from_db_path_table`).

**S6 (`relay_advertise.rs`, `sharding.rs`)**
- `owner_or_manager_can_advertise_a_relay_url` / `viewer_cannot_advertise_a_relay_url` (AC-10 —
  table-driven over `RoleLevel`).
- `malformed_relay_url_is_rejected_at_write_time_not_dial_time` (reuses `RelayConfig::parse`'s
  validator).
- `relay_advertisement_removal_tombstones_the_row` (mirrors `__peers` tombstone handling).
- `forged_write_row_entry_bypassing_the_rest_route_is_still_rejected_at_the_admission_gate`
  (defense in depth, AC-10's second half).
- `advertisement_from_a_later_demoted_manager_is_withdrawn_on_apply` (read-time admission,
  AC-10's third half).
- `row_with_unknown_extra_field_applies_and_row_with_unknown_version_is_skipped` (AC-11).

**S6b (`relay_advertise.rs` in fe-api, axum integration)**
- `owner_token_can_post_and_delete_a_relay_advertisement` (AC-8, the REST half).
- `viewer_or_editor_token_gets_403_and_writes_no_row` (AC-10, the REST half).

**S7 (`sync_thread.rs`, two-`SyncEndpoint` harness — real flow, no mocked gossip)**
- `dial_only_default_lists_the_url_without_calling_insert_relay_or_changing_home` (AC-8, first
  half).
- `pinned_policy_calls_insert_relay_without_auth_token_and_remove_relay_at_refcount_zero` (AC-8,
  second half; asserts `auth_token == None` on the inserted `RelayConfig`).
- `withdrawal_never_calls_remove_relay_for_a_configured_url_and_respects_cross_verse_refcount`.
- `deny_policy_never_calls_insert_relay_for_any_advertised_url` (AC-9).
- `pinned_policy_adopts_only_the_pinned_urls_even_when_others_are_advertised`.

**S4 (both binaries, smoke-level — construct `ApiConfig` and assert the fields, not a full process
spawn)**
- `relay_binary_wires_usage_sink_and_metrics_bind_from_env_into_apiconfig`.
- `gui_binary_wires_the_same_two_fields_from_env_into_apiconfig`.

**S8 (docs — no automated test; verified by the review slice reading the doc against the actual
env var names/defaults each prior slice implemented)**

**S9 (review)**
- Re-run AC-1 through AC-10 as a checklist against the actual implementation, not the spec text.
- `cargo deny` advisory/license check unaffected (no new heavy dependency introduced by `/metrics`
  — OD-8's v1 scope deliberately avoids pulling in a full `prometheus` crate; hand-rolled counters
  are enough for request/error/duration).

## 4. Review-and-fix phase (S9)

Fresh context, `/code-review high` across every file S1-S8 touched. `/security-review` is
mandatory on `fe-sync/src/relay_advertise.rs` and the `sharding.rs` admission hook (an auth
surface: a bug here lets a non-Owner/Manager peer redirect other peers' relay traffic). Record one
verdict line per FR here once done; a phase with no verdict is unreviewed, not done.

- FR-1/2 verdict: _(pending)_
- FR-3 verdict: _(pending)_
- FR-4/5/6 (docs) verdict: _(pending)_
- FR-7 verdict: _(pending)_

## 5. Cross-track sequencing

`fe-runtime/src/{lib.rs,AGENTS.md}`, `fe-api/src/{server.rs,lib.rs}`, `fe-api/AGENTS.md`,
`fractalengine/src/main.rs` and `fractalengine-relay/src/main.rs` are also owned by
`agent_input_surface_20261031` and/or `plugin_host_revival_20261031`. Fixed order for those files:
agent_input_surface -> this track -> plugin_host_revival; no two tracks' owning slices run
concurrently, and the later one re-greps symbols after the earlier one's commit.
`polish_lane_20261031` (in flight) also owns `fe-api/AGENTS.md`, `fe-runtime/src/AGENTS.md` and
`fe-sync/src/{sharding.rs,AGENTS.md}`; S1/S2/S6/S7 wait for its owning waves to close.

## 6. Task checklist

- [ ] S1: `UsageEvent`/`UsageSink` + no-op and JSON-lines sinks (fe-runtime)
- [ ] S5: `FE_LOG_FORMAT` toggle in both binaries
- [ ] S6: `__relays` table + role-gated admission + URL validation (fe-sync)
- [ ] S2: usage-emit middleware + `usage_sink_from_env()` (fe-api)
- [ ] S3: opt-in `/metrics` listener (fe-api)
- [ ] S7: sync-thread `insert_relay`/`remove_relay` reconciliation + `RelayAdvertisePolicy`
- [ ] S6b: `POST`/`DELETE /api/v1/verses/{verse_id}/relays` route
- [ ] S4: wire `usage_sink`/`metrics_bind` through both binaries
- [ ] S8: `docs/relay-operations.md` + compose `iroh-relay` service
- [ ] S9: review-and-fix with recorded verdicts; full sweep; `conductor/tracks.md` row

## 7. Docs + retro (folds into S8/S9)

- Update `fe-runtime/src/AGENTS.md`, `fe-api/AGENTS.md`, `fe-sync/src/AGENTS.md` with the new
  §usage, §usage-metering, §relay-advertise sections (one-line doc-comment pointers in code, the
  rationale lives here per the directory-docs convention).
- `conductor/tracks.md` row + this plan's `- [ ]` boxes ticked.
- Retro note: whether OD-1..OD-9 held up as recommended, or the user overrode any — record the
  reversal actually taken if so.
