---
type: review
track: extension_platform_20261107
title: R-LIFE — plugin runtime security review, swarm turn 2
resource: ../metadata.json
---

# R-LIFE — plugin runtime (turn 2, over F-TICK `c092a1f2`, F-BUS `f3b85f76`, B4 `17a51b4f`, B10b `0a21f698`, B10d `e95ed719`, I-PLUGIN `03b581e3`)

Security review-and-fix, code-first (nothing built or run). The two mandatory security points are the portal write
path and the OD-E1 drain; the steer of 2026-11-09 02:25 (decode cap) is binding here. One verdict line per finding:
**FIXED** (edited here), **KEPT** (checked, left as is, reason given), **HANDED-OFF** (owner named),
**DEFERRED** (trigger named). Rationale for the fixes: fe-plugin `src/AGENTS.md` §update and §hook-calls,
`src/bus/AGENTS.md` §broker and §portal-clients.

## A stopped or revoked instance writing after its drain

- R-LIFE-01 KEPT `tick.rs` `TickScheduler::stop`: the in-flight commit settles first (bounded 1 s), then `on-deactivate`; `lease_gone` reasons (revoked, policy-denied, role-lost, degraded) discard the window with no final flush. `TickHost::stop` leaves the seat before draining and unbinds after it. J1 covers a revoke mid-flush (commits +1, `flush_in_flight` false, nothing new submitted).
- R-LIFE-02 KEPT a flush that outlives the 1 s wait settles as `unknown`, and the lease is then revoked. It still cannot land late: `fe-database/src/handlers/plugin_host.rs` `flush` checks `admission.require_active()` before committing, so a commit the DB reaches after the revoke is refused whole.
- R-LIFE-03 KEPT the guest runs `on-deactivate(revoked)` once more under a live lease. Its scene and reading stages are discarded, and the seat has already left (no bus or network). Its only other effect is the host-local `kv` (never replicated), and the WIT tells it why it stops.
- R-LIFE-04 KEPT app exit and terminal reset revoke every lease (`PluginContexts::shutdown`) before the worker's `stop_all(host-stopping)`. That final flush is therefore refused at the DB: fail closed, nothing lands after the exit signal. The cost is the last window of a clean exit.
- R-LIFE-05 KEPT the portal write path (`bus/broker.rs` `admit_write`) has no lease. A revoke stops the client at once (`feed_admission` matches the hash under any id), and a write already sent settles and its row stays (OD-E1). The bus journey covers it.

## A lease outliving revoke

- R-LIFE-06 KEPT every governed stop goes through `PluginManager::stop_instance` → `TickHost::stop` → `PluginContexts::unbind`. The degrade path unbinds in `TickHost::poll`. A binding start that races a kill list is refused at `admit_staged` (row 5) and its staged context is discarded. The binary's `LiveInstances` serves only entries whose lease holds.
- R-LIFE-07 FIXED `bindings/blob_source.rs` `HookCaller` contract: it now says a call is refused once the instance's lease is revoked, not only while it is degraded (OD-E1: a stopped instance takes no new call). The implementation is the binary's (not wired yet, R-LIFE-21).

## Update rollback and two live builds

- R-LIFE-08 KEPT `TickHost::update` runs under the host lock. On a refusal, the new build leaves the shared key and its staged context is discarded (lease revoked) before the old build rejoins. On success, the old lease is revoked before the new context is admitted, and `claim_key` refuses a second admitted context per `(petal, package)`. No path leaves two admitted builds. J1 asserts the leases on both branches.
- R-LIFE-09 FIXED `lifecycle.rs` `update_instance` refused nothing when the running build under that id belonged to another installation or package. A caller that picked the wrong id would swap a different package into the seat on one consent check. It now refuses `launch-mismatch` and discards the staged context. The lifecycle journey asserts the refusal, the dead lease and the untouched running build.
- R-LIFE-10 KEPT a publisher change on update is the ops drain's routing (steer 01:50, fixed by R-GOV `913876cc`). `ConsentRecord` holds no publisher (src/AGENTS.md §update).

## Bus seats after uninstall

- R-LIFE-11 KEPT `TickHost::retire` leaves the seat, unbinds, then calls `Bus::forget` and `Net::forget` (G1-C10). `uninstall_instance` covers every id ever started for the installation, stopped ones included. `BindingActivator::forget_installation` drops the egress membership, refusals and `on-install` memory. The `retired` map is keyed by installation and cleared on re-register.
- R-LIFE-12 FIXED `bus/rate.rs` `TokenBucket::is_idle` read the clock that its own refill had just moved. Under the 10 s sweep, no bucket was ever 60 s untouched, so `remote_gate`, caller, command, portal-stream and write buckets were never pruned. A stream of fresh remote DIDs grew them without bound. Only `try_take` now ages a bucket (`touched_ms`); the bus journey sweeps one at 10 s steps.

## Portal publish and set_property: ceiling and rates

- R-LIFE-13 KEPT `client_set_property` checks, in order: a local binding, a declared non-reserved key, provenance, shape, three shared budgets (caller, package burst 5, identity burst 8), and the role capped by the ceiling, Editor or above. The DB re-checks the role and ancestry at use. `client_call_command` judges the capped role (check 4). `client_publish` has a rate keyed per `(petal, package, stream)`, so a reopened client cannot reset it.
- R-LIFE-14 FIXED `bus/broker.rs` `plan_client`: a portal's host sources (node-changes, readings, query, admission) checked neither the binding's locality nor its capped role. Only the stream branch judged the role. A foreign-DID binding, or one capped below Viewer, could read the mount petal's nodes. Both now get the uniform not-found or `capability-denied`.
- R-LIFE-15 KEPT a portal stream event carries no caller role. Events are data, and an extension that consumes a portal stream acts under its own grants. Invoke and write are where the ceiling binds (bus.md §portal-clients).

## Query rows

- R-LIFE-16 KEPT `host/node-changes` snapshots stop at `REST_ROW_CAP` with `truncated: true`. `host/query` snapshots are cut at 10 000 rows and at 1 MiB (guests) or 8 MiB (others). `scene.query-nodes` is bounded at 200 rows and 64 KiB in `context.rs`. The bus journey covers the node cap.

## Binding instances: caps and idle stop

- R-LIFE-17 FIXED `bindings/activation.rs`: eager starts bypassed the per-installation cap (`!plan.eager &&`). A Manager binding `petal.activate` at a verse with N petals started N instances on every accepting peer. bindings.md §5.6 says "at most 64 binding-started instances per installation per host". Planning now holds every start past the cap at `capacity` (eager or lazy), `start` re-checks for all, and eager instances are still never idle-stopped or evicted. The journey runs a cap of 2 with three eager petals.
- R-LIFE-18 FIXED the same file: a stop re-triggered only its own verse, but the cap is per installation. A capacity wait in another verse slept until an unrelated trigger. `wake_capacity_waits` now marks every verse with a `capacity` point.
- R-LIFE-19 KEPT idle stop: `sweep_idle` stops lazy binding instances whose petal is not active once `last_use_ms` is 10 min old. Calls (`need`) and leaving a petal reset the clock. The journey covers stop and the freed cap.

## Decode, egress and blob bytes without BLAKE3 and size

- R-LIFE-20 FIXED steer 2026-11-09 02:25. `bindings/decode.rs` summed bytes after `payload_files` had loaded them. The order now lives in fe-plugin (`read_within_cap`): `DecodeHost::declared_files` returns each row's size, type and BLAKE3 with no blob read, and the sum is checked against 1 MiB first (`TooLarge`). Each file is then read through `read_bounded(file, size + 1)` and kept only when its length and BLAKE3 match (`Mismatch`). The decode journey adds an oversized payload with zero reads, a lying row and foreign bytes. The binary's `DecodePort` must adopt the split: HANDED-OFF (NEEDS in the result, owner R-X or refine).
- R-LIFE-21 KEPT `blob.mirror` reads at most `min(size, 8 MiB) + 1` and checks the hash and size before any guest sees a byte. `blob.source` and `sync.tileset-source` answers pass `verify_fallback_bytes` in the service, again in `BlobFallbackSlot`, and the tileset chunk re-runs `verify_chunk`. The blob reader stores only verified bytes. HANDED-OFF to I-BIN: no binary wiring of `blob_source_channel` or `blob_mirror_channel` exists yet, so these hooks never fire today (fail closed: peers only).
- R-LIFE-22 DEFERRED `blob.source` for scene blobs on a networked host: `BlobAssetReader` asks the source only when no `on_miss` is wired, and `handle_fetch_blob` → `SceneAssetJobs::fetch` never asks it after a peer miss. Trigger: when I-BIN wires the slot, thread it into `fe-sync/src/scene_assets.rs` after the provider loop, as `fetch_chunk` does.
- R-LIFE-23 DEFERRED `BlobFallbackSlot::source_blob` awaits the worker's answer with no timer, by design (executor-agnostic). A slot that is installed but never pumped would park a Bevy IO task. Trigger: the same I-BIN wiring must pump `BlobSourceService` on every binding-worker turn, or add the reader-side bound.

## Hook calls on the DB thread

- R-LIFE-24 KEPT `fe-database/src/handlers/device_payload.rs` fires the accepted-payload sink after the durable insert, only on the accepting path. Idempotent retries return earlier, and replicated rows never reach it. The binary's sink is a non-blocking `unbounded` send with a 256-job backlog cap. Decode reads and guest calls run on the binding worker (`db_rows_within` waits there, never on the DB thread). `MirrorQueue::after_commit` is `try_send`.

## PHR #18 (AOT cache)

- R-LIFE-25 DEFERRED (scope flag §9) `wasm/aot.rs` still `unsafe`-deserializes any `{hash}.cwasm` in the cache directory with no provenance check, so a write to that directory is code execution. The re-check narrows it: 2.0 components compile in memory (`ComponentHost::compile`, the binary's only path, `plugin_bridge.rs:1094`), and only the retired 1.0 core path (`install_plugin`, `activate_with_bytes`) reads the cache. Trigger: before the 1.0 path is re-enabled for third parties, or when `compile_cached` gets a caller.
