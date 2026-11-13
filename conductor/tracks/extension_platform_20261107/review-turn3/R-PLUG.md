---
type: review
track: extension_platform_20261107
gate: R-PLUG
turn: 3
reviews: [X-NET, X-BLOB, I-PLUGIN]
security_review: mandatory (fresh context, prompted to refute)
built: false
---

# R-PLUG — fe-plugin / fe-sync gate (turn 3)

Scope: shared egress tokens (B10d), checked resolver on outbound clients (R-NET-30), scene `blob.source`
fallback bounds and verification (R-LIFE-22/23), fe-plugin crate wiring and deny.toml. Code-first: nothing was
compiled or run (user directive); every verdict below is from reading HEAD plus this gate's edits.

Verdict: **PASS-WITH-FIXES** — 5 fixed, 2 compile breaks handed off, 1 store-side observation handed off.

## Findings

- R-PLUG-01 FIXED (security, steer 2026-11-13) — `fe-plugin/src/registry_client.rs` built its reqwest client
  without `.no_proxy()`: an `HTTP(S)_PROXY` would take the connection and `CheckedResolver` would never see the
  target. Now `.no_proxy()` and `.referer(false)`, like the egress clients.
- R-PLUG-02 FIXED (security, steer 2026-11-13) — the redirect policy ran only `literal_refusal` on the hop's host
  text, so a public registry could redirect to `http://localhost:…` or `http://127.0.0.1:…` (both admitted as
  loopback names) and downgrade https to http. Redirects now follow only within the first request's origin
  (scheme, host, port), as `fe_hexon::remote::same_origin_redirects` does; a cross-origin 3xx is not followed and
  the read fails on its status.
- R-PLUG-03 FIXED (security) — a plaintext `http://` base to a public name was accepted, so the bearer token
  travelled in clear. `host_refusal` now requires `https`, or `http` only to `127.0.0.1` / `::1` / `localhost`;
  an unparseable base is refused the same way. `connect::is_loopback` is `pub(crate)` and re-exported from
  `net` for this. Test table `registry_hosts_follow_the_address_policy` gained the http-public, ftp and bad-url rows.
- R-PLUG-04 FIXED (resources) — `fe-runtime/src/bevy_blob_reader.rs::with_deadline` spawned one OS thread per
  pending source read that slept the whole 35 s even after the answer arrived; a burst of misses parks one thread
  each. Now one shared `fe-blob-deadline` timer thread wakes every registered task at its deadline (per-read
  thread only if that thread cannot start), and a task re-registers when its waker changes. The KTX2/fallback
  journey gained an unpumped-source leg (pending source ends at the deadline).
- R-PLUG-05 FIXED (correctness) — X-BLOB's `blob_source_serves_scene_bytes_only_when_exact` used
  `application/octet-stream`, which `verify_scene_asset_bytes` refuses (`unsupported scene asset MIME`), so the
  "exact bytes are served" leg would have panicked on `unwrap`. It now serves a glTF 2 JSON document under
  `model/gltf+json` and adds a leg proving the intake check runs on hash-true source bytes (JSON under the GLB
  MIME is refused).
- R-PLUG-06 HANDED-OFF (C-EXT, owns `fractalengine/src/extension_host/**` and `plugin_bridge.rs`) — **compile
  break:** `fractalengine/src/extension_host/net_sources.rs:99` builds `NetSources { roles, audit, credentials,
  runtime }` without the new `shared_egress` field. Correct value is the activator's handle, not `Default`:
  in `plugin_bridge.rs` construct `let activator = BindingActivator::new(ticks.clock());` before `build_net`,
  pass `activator.egress_buckets()` into `build_net(.., shared_egress: SharedEgressBuckets)` (both call sites,
  ~4074 and ~4083), and move `activator` into `BindingWorker` (~4245). With `Default` the build compiles but
  bound connectors never share a budget (B10d silently off).
- R-PLUG-07 HANDED-OFF (orchestrator; I-API/R-API owned it this turn) — **compile break:**
  `fractalengine-relay/src/main.rs:297` `BevyHandles { .. }` lacks the new `blob_fallback` field; add
  `blob_fallback: Default::default(),` (the relay has no plugin host binding sources: peers only).
- R-PLUG-08 HANDED-OFF (R-STORE / R-Z) — `fe_hexon::remote::store_client_builder` documents "callers add their own
  checked resolver" but `RemoteRegistryClient` (remote.rs ~100) and the store client build without any
  `dns_resolver`, so a configured registry name that resolves to the LAN is dialled. Same-origin redirects and
  `.no_proxy()` are in place, and registry URLs are operator configuration ("tileset installs trust the operator
  registry"), so this is a residual, not a bypass; fe-hexon cannot depend on fe-plugin, so the fix is an injected
  resolver or a local copy of `is_public_unicast`.
- R-PLUG-09 KEPT — shared egress (`take_egress_token`): a joined instance draws only from its installation's
  `(installation_id, endpoint)` bucket; the per-instance bucket is created but untouched (harmless). The shared
  bucket's rate comes from the first drawer, and every member runs the same manifest, so the rate is the same.
  `leave` drops a group's buckets with its last member. Lock order is net-state → shared, never the reverse.
- R-PLUG-10 KEPT — HTTP egress client (`net/egress.rs::build_client`): `.no_proxy()`, `CheckedResolver`,
  `https_only` unless loopback, redirects only within the declared base path, `http1_only`; guest-set credential
  and host-owned headers are refused on requests and stripped from responses (`response_headers`). Verified.
- R-PLUG-11 KEPT — `CheckedResolver`: resolve once, refuse the whole answer if any address is not admitted,
  loopback names reach loopback only; reqwest connects to exactly those addresses (no rebinding window).
- R-PLUG-12 KEPT — scene `blob.source` path: asked only after every provider missed (or none known), only for an
  admitted transfer permit (`gate.classify` and `admit_job` run first), with `expected_size = permit.size_bytes`;
  the slot checks the 16 MiB cap, exact size and BLAKE3, then `verify_scene_asset_bytes` re-checks size, hash, MIME
  container and the KTX2 guard, and `finish_fetch` re-admits and verifies again before storing. A verse closing
  mid-ask is ended by the job's outer `select!` on the stop signal (no inner select needed). Prefetch retries ask
  the source again per retry; bounded by `PREFETCH_RETRIES` x 35 s inside the 120 s `JOB_TIMEOUT`.
- R-PLUG-13 KEPT — X-BLOB added a new test function instead of extending a journey; the `fe-sync` crate-wide R8
  batch line reaches it (`conductor/test-batches.md`), so it stays selectable.
- R-PLUG-14 KEPT — I-PLUGIN crate wiring: `pub mod admin`; target-gated `windows-sys 0.61` features cover every
  import in `admin/pipe_windows.rs` (Foundation: CloseHandle/LocalFree/errors/HANDLE; Security: EqualSid,
  GetTokenInformation, TOKEN_*; Security_Authorization: SID/SDDL conversions; System_Pipes: client/server PID;
  System_Threading: OpenProcess/OpenProcessToken/GetCurrentProcess); `libc` on Unix for `geteuid`; tokio `net`
  and `time` come from the workspace features. deny.toml needs no new entry (both crates' licences are allowed,
  both already in Cargo.lock). The admin channel's own security is R-ADMIN's gate.
- R-PLUG-15 KEPT — WIT `on-deactivate` doc matches `tick.rs` (OD-E1: in-flight commit settles, staged writes
  discarded for role-lost/policy-denied/revoked/degraded, lease revoked after the call returns).
- R-PLUG-16 DEFERRED (trigger: R-NET-18 reopened) — R-NET-18 stays deferred as X-NET recorded.
