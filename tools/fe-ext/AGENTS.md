# tools/fe-ext

The extension authoring tool (spec R25) and its Rust guest SDK (R26). Contracts it implements live
in `docs/extensions/` and `fe-plugin/wit/fractal-plugin-2.wit`; this file holds the design notes.

## Commands (src/main.rs)

| Command | Module | Notes |
|---|---|---|
| `new <kind> <name>` | `src/new.rs` | Simulation, device-adapter and bi-connector scaffolds copy the shipped `fe-sdk/schema/examples/*.extension.json` with identity swapped, so they cannot drift from the schema; `general` is a minimal handwritten manifest. The scaffold is validated before anything is written. |
| `build` | `src/main.rs` | `cargo build --target wasm32-wasip2 --target-dir <project>/target` (explicit, so a `CARGO_TARGET_DIR` or a config `target-dir` cannot move the artifact), copy to `dist/component.wasm`, then the host's install admission (`ComponentHost::compile`). |
| `validate` | `src/validate.rs` | `fe_sdk::manifest_v2::parse_and_validate` (the entry REST install shares) plus the 64 KiB header ceiling; component checks when `dist/component.wasm` exists. |
| `pack` | `src/pack.rs` | `PluginHexonBuilder` seals and signs; the result is re-verified with `verify_plugin_package`. |
| `sign` | `src/pack.rs` | Re-seals the same members under another publisher key (a sealed manifest cannot be edited in place). Licensed packages are refused. |
| `countersign` | `src/pack.rs` | Appends `plugin/countersignatures.json` by raw-copying every other zip member, so `content_hash` never moves. Signs `signing_bytes` with `fe_format::sign_manifest` (package-format.md §Signatures: the same bytes and base64 as the publisher). Until fe-hexon's verifier checks entries with `fe_format::verify_manifest` instead of its raw-bytes `signature::verify_manifest`, the re-verify here refuses with `countersignature-mismatch` (handed off by G3). |
| `inspect` | `src/inspect.rs` | Verifies with every signer accepted (trust is the host's decision) and prints the ids, hashes, capability digest, `publisher_did` and the verified `countersigners`. It never prints a "trusted by": under accept-all that would always name the publisher. |
| `run --sandbox` | `src/sandbox.rs` | See §sandbox. |
| `run --lab` | `src/lab.rs` | `--scenario <script.json> [--runs N] [--root DIR] [--out report.json]`: hosts the fleet-sim package as the fe-sim reading schedule (see §lab); `--runs` repeats the session and every fingerprint must equal the first. `--sandbox` and `--lab` are mutually exclusive. |
| `admin <verb>` | `src/admin.rs` | Drives the running app over the LOCAL admin channel only (`fe_plugin::admin::transport`, instance picked by `FE_EXT_ADMIN_INSTANCE`); no token, URL or network fallback. Consent is never answered here: the reply names the pending consent and the GUI confirms it (docs/extensions/admin-channel.md). Exits non-zero when the app refuses or is not running. |
| `index build\|verify` | `src/index.rs` | Writes and re-verifies a registry contract v1 static signed catalogue tree (`StaticIndexSource` end to end, pinned index key, index key never a publisher key). See �index. |
| `keygen` | `src/key.rs` | A key is 64 hex characters (the ed25519 seed). Never overwrites. |

## §sandbox

`run --sandbox` is the real `ComponentHost` and `ExtensionInstance`, not a re-implementation. Around
it: `fe-plugin-test`'s `MockHostEnv` as the scene reader, a `ManualClock`, and a `SandboxSink`
(`StagedCommitSink`) that applies each tick window to a COPY of the mock's nodes and swaps it in only
when every op and reading is valid (the production all-or-nothing flush).

- The tick driver is hand-rolled, not `TickScheduler`: one loop sets the clock to `start_ms + (n-1) * cadence`,
  delivers fixture ingress frames due at tick n, refuels, calls `on-tick`, flushes, and feeds the outcome into the
  next `tick-info.last-commit`. No threads, so a run is a pure function of (manifest, component, fixture).
- The sink assigns node ids `sbx-000001...` instead of using the mock's ULIDs, and the trace sorts nodes by id, so the
  fingerprint is byte-stable. The fingerprint is `blake3(serde_json canonical trace)`; keys sort because
  `serde_json` is built without `preserve_order`. Do not add `preserve_order` to this crate's dependency graph without
  sorting the trace by hand.
- Fixtures (`Fixture` in `src/sandbox.rs`): nodes, ticks, cadence override, source-class pick, recorded ingress
  frames and an fe-sim `FleetModel` (validated, then stored as property `fleet.config` on node `fleet-1`, which is
  what the fleet-sim example's UI looks for). Bus events and egress completions are not fed yet: the broker is
  `NoBroker`, so those calls answer `not-available`.
- Ingress frames pass the host's checks first (`admit_ingress`, production order from fe-plugin `net/ingress.rs`):
  undeclared endpoint or sub-path → 404, method → 405, size → 413 (websocket/mqtt: dropped), media type → 415; only
  the declared `forward_headers` minus credential headers reach the guest; http and websocket-accept frames carry a
  caller DID. A refused frame is recorded in `ingress_replies` with `host: <reason>` and never reaches the guest.
  A transport or mqtt topic the endpoint could never produce is a fixture error. Rates are not modelled. Without
  this a fixture could hand the guest a header or endpoint production never would (steer 2026-11-08 20:20).
- The binding's `source_class` is the class the token writes as (the fixture pick, or the manifest's only class),
  as in production; a manifest that declares a class SET needs `source_class` in the fixture or the bind is refused.
- `--lab` is the separate mode below; `--sandbox` takes `--fixture`, `--lab` takes `--scenario`.

## §lab

`run --lab` (src/lab.rs, R30) calls `lab::run_lab`: `HostedFleetSource` plans the fleet through the real component, `ScenarioSession::start_with_source` (fe-sim, hence the `fe-sim` dependency) ingests it, and the canonical fingerprint is `blake3` of the outcome. The package must cover the whole native schedule or the run errors. Each run needs a fresh session directory under `--root`.

- **Determinism by construction.** The only clock is `LabClockAdapter`: fe-sim's seeded `SimClock` (frozen during
  `plan`) plus the planner's own step offset. The component's WASI wall and monotonic clocks are frozen to it and its
  randomness is seeded from `(installation_id, activation)` (fe-plugin `wasm/component/wasi.rs`), so two runs give
  one fingerprint. Nothing here reads `SystemTime` or `Instant`; keep it that way.
- **Crate edge.** The `fe_plugin::tick::Clock` adapter lives here because fe-sim must not depend on fe-plugin
  (fe-sim's `LabClock` is a plain newtype over its `SimClock`).
- **Row to sensor.** A recorded row maps to the first sensor with the same anchor, metric and units whose cadence
  divides its offset and that is not yet filled at that instant, so sensors sharing an anchor never collapse. A row
  no unfilled sensor can take (a duplicate emission included) is an error, as is a missing one.
- **Scratch.** Without `--root` the sessions go under a pid-named temp directory and each is removed after its run
  (best effort); with `--root` they stay for inspection.

## §guest-sdk (tools/fe-ext/guest)

Its own workspace (`[workspace]` in its Cargo.toml), built for `wasm32-wasip2`, never a member of the host
workspace. `wit_bindgen::generate!` points at the one WIT file, not the `wit/` directory, for the same reason the
host does (`fe-plugin/wit/AGENTS.md` §one-file).

- `Extension` has a default for every hook except `on_tick`; `extension!(Type)` implements the four generated
  `Guest` traits by delegation and calls the export macro. One `extension!` per crate.
- `window`: JSON-valued staged writes and `room()`. `readings_batch::ReadingBatcher`: queues readings and stages only
  what fits the window, requeuing on refusal. `tracking::snap_and_stage`: REST's `check_tracking_and_alert`
  semantics (four metrics, `tracking_alert = "off_route"` only when deviation is strictly over the threshold,
  default 50 m) staged through `txn`; `key_prefix` puts the keys inside the manifest's property scope.
- `hooks`: `Call`/`Output`/`Failure`, `route` and `key_matches` for `hooks.on-hook` (bindings.md §7.3), re-exported as `HookCall`/`HookOutput`/`HookFailure`/`HookFn`. The `extension` world does not export `hooks`; a provider uses `extension-with-hooks` and maps the generated `hook-call` onto `Call::parse`.
- The helpers compile natively (wit-bindgen stubs the imports off-wasm), so their pure parts carry unit tests.

## �index

`index build --packages DIR --base-url URL --key INDEX.key --out TREE` is deterministic (issue time defaults to the newest
package `updated_at`); `index verify TREE|URL --index-key did:key:..` runs the same reader the app uses and lists
`verified <id>@<version>` per package. File trees only here; an http(s) verify needs an injected fetcher
(`run_index_with`). `validate` and `build` also run `check_provides`: a manifest declaring `provides` needs the hooks
export (`missing-export`).

## Cargo.lock policy

`tools/fe-ext` is a workspace member and resolves through the root `Cargo.lock`; it has no lock of its own. The
standalone guest crates (`tools/fe-ext/guest`, `extensions/*`) are excluded from the workspace and carry their own
`Cargo.lock`, generated on their first build and committed with the crate so a pack is reproducible.

## Build wiring

- Root `Cargo.toml` needs `tools/fe-ext` as a member (this crate inherits `edition`/`license` from the workspace, so
  it cannot build otherwise) and `tools/fe-ext/guest` plus the `extensions/*` guest crates in `exclude`.
- Root `.gitignore` ignores `/tools/*` with a `!/tools/qa/` exception; this directory needs its own
  `!/tools/fe-ext/` line or none of it is ever committed (G3 found it untracked on 2026-11-08).
- The scaffold's path dependencies point at this checkout (`env!("CARGO_MANIFEST_DIR")` at compile time).
- `new` writes a fixture with the manifest's first source class when it declares a set, and stamps a key inside the
  manifest's first `<prefix>.*` scope, so `new` → `build` → `run --sandbox` commits on every kind.

## Tests

J8 (`tests/j8_cli.rs`, `cargo test -p fe-ext -- j8`) is the one journey: `new` for every kind, the validation table,
then each shipped example (`extensions/fleet-sim`, `extensions/device-adapter`, `extensions/bi-push`; bi-push asserts zero faults) once its `dist/component.wasm` exists
(two sandbox runs, one fingerprint, identical traces, the fixture outcomes, `pack`, then `inspect` succeeding on the same
content hash `pack` printed), then the scaffold's
pack/countersign/tamper/sandbox half (the examples' legs also run `sign`, `index build` and `index verify`) when `FE_EXT_TEST_COMPONENT` names a built component. The examples carry only
pure unit tests; their sandbox legs moved here so no example crate builds the host stack. Guest-side unit tests run
with `cargo test --manifest-path tools/fe-ext/guest/Cargo.toml`.
