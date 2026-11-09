# fe-plugin/src/bindings — module rationale

Contract: `docs/extensions/bindings.md`. Types, codec and hook catalogue live in `fe-runtime/src/extension_binding.rs`;
the DB plane (write, admission, demotion) in `fe-database/src/handlers/extension_binding.rs`. This directory is the
per-peer decision layer. `mod.rs` is owned by the plugin integration slice.

## §resolver

`resolver.rs` is pure: a `BindingSnapshot { rows, roles, facts, stored_terrain_petals }` in, a `ResolvedPoint` out. No
IO, no clock, no DB; the binary builds the snapshot on each re-resolution trigger (bindings.md §5.6) and caches results.

- **Effective version (§4.4).** `effective_binding` keeps rows whose `written_authorized` flag is set (the DB plane's
  write-time check) and whose author is Manager+ now (`EffectiveRoles::authority`), then takes the newest by HLC,
  tiebreak canonical payload bytes. An effective `inherit` is "no binding".
- **Nearest scope, no fall-through.** Petal, fractal, verse, first scope with an effective row decides. An unresolved
  `extension` row yields `Unresolved` (built-in meanwhile); it never defers to an outer row.
- **Stored terrain (OQ-B6).** For `petal.map`, a petal in `stored_terrain_petals` with no effective petal-scope row is an
  implicit `builtin` at the petal (`detail.implicit_terrain`). An explicit petal row wins over the column (assumption:
  bindings.md says the column beats *inherited* bindings only).
- **Placement (§5.4)** runs before the local checks: `runs_on`, else the author for one-peer hooks, else every peer.
- **Local checks (§5.2).** `first_failure` covers rows 1-4 (host switch, known hook, installed, SemVer), then each
  satisfying build, highest SemVer first, runs rows 5-14 in `check_build`. The first fully passing build resolves; if none
  passes, the highest build's failure is reported. `version_req` is Cargo SemVer (`semver::VersionReq`); an unparsable
  requirement or build version never satisfies. Config validates through `bus::schema::validate_args` (absent schema
  accepts exactly `{}`). `provides.keys` match exact, `prefix*` or `*`.
- **Offers.** `UnresolvedReason::offer()` names the action; `gated_offer` drops it when it would be a dead control:
  Install needs a policy-admitted package and a registry build satisfying `version_req`; Update needs a satisfying
  registry build (providing the point, for `not-provided`); Override only on the verse Owner's host; Edit binding only for
  Manager+ at the binding's scope. `policy-denied` names the refusing policy in `detail.policy_source`.
- **Acceptance** is `LocalFacts.accepted` keyed `(installation_id, verse_id)`; the binary includes verses implied by an
  existing per-petal local bind (§5.3).

Instance caps (§5.6): only lazy hooks resolve `capacity`; an eager hook or a petal that already runs an instance never does.

- **Kill-list override (§5.7, R-BND).** Row 6 passes a revoked build only when `kill_override` is set AND
  `LocalFacts.is_verse_owner_host`: the override is the Owner's, for the Owner's own host. An override record on any
  other host (a changed local identity, a restored store) never lets a kill-listed build resolve.
