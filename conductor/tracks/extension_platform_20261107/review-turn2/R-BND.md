---
type: review
track: extension_platform_20261107
title: R-BND — binding authority and DB plane security review, swarm turn 2
resource: ../metadata.json
---

# R-BND — binding authority and DB plane (turn 2, over B0 `fced8f58`, B1 `2cf07b7e`, B2 `a13d28c2`, B8 `00ce0291`, F-E4 `981b6c09`, I-DB `5ebcec29`)

Security review-and-fix, code-first (nothing built or run). One verdict line per finding:
**FIXED** (edited here), **KEPT** (checked, left as is, reason given), **HANDED-OFF** (owner named),
**DEFERRED** (trigger named). Refutation targets come from the slice task. Rationale for the fixes:
fe-database `src/AGENTS.md` §extension-binding and §plugin-host-flush, fe-plugin `src/bindings/AGENTS.md`
§resolver.

## A binding written or admitted without Manager+ at its scope

- R-BND-01 KEPT `fe-database/src/handlers/extension_binding.rs` `put_binding`: a remote caller needs Viewer+ at the record's own scope (else the uniform 404) and Manager+ (else an audited 403). The local DID, which authors the row, always needs Manager+ too. A delegated token of this node (`sub` = node DID) skips only the caller half, and fe-api first demands a Manager+ ceiling and token-scope coverage.
- R-BND-02 KEPT inbound (`replicated_row.rs` `confine_binding` → `role_gate` `Floor::ManageScope`): the attested author is gated at the binding's own scope NOW. The doc key must equal the payload's canonical re-encoding, the payload author must be the attested author, and the scope must be located in this verse exactly as named. A sub-threshold author is `Deferred(AuthorRole)` on the existing worklist.
- R-BND-03 KEPT admissibility on read: `judge_versions` (DB) and the resolver's `effective_binding` both AND the write-time half (`role_grants::authorized_at_write` over `fe_policy::materialize_at`) with the current half (`EffectiveRoles::authority`). `default_access` is only ever `viewer`/`none` (replicated_row's manifest check), so it can never lend Manager.

## Backdating after demotion; a re-grant reviving a withdrawn version

- R-BND-04 KEPT a demoted author backdating into its old window fails the current half. After a re-grant it fails the write-time half, because the continuity field (`manager_since`) restarts at the re-grant. A far-future clock is Denied at admission (`admit_scene_hlc`, five-minute bound), so no version can pin last-writer-wins.
- R-BND-05 KEPT a re-grant does not revive anything, including through a re-granted grantor in the chain: `materialize_at` re-runs the fixpoint at the version's HLC over each statement's continuity. `extension_binding_test` covers the old version and a backdated one.
- R-BND-06 FIXED `fe-database/src/AGENTS.md` §extension-binding said withdrawn versions "become admissible again when the role returns", the opposite of bindings.md §4.5 and the code. The paragraph now states the continuity rule. Two Latin-1 `§` bytes (§property-schema, §node-history) made the file invalid UTF-8; both are repaired.

## Resolver fall-through and nearer-scope misses

- R-BND-07 KEPT no fall-through: `resolve` returns at the first scope with an effective row. An unresolved `extension` row answers `Unresolved` and never consults an outer row. `inherit` and inadmissible versions look outward. Stored terrain is an implicit petal builtin only when no effective petal row exists (OQ-B6).
- R-BND-08 KEPT rows carry canonical scope strings. The local write requires a `scope_string` round trip plus `scope_is_known` ancestry, and inbound admission denies a scope that differs from the stored hierarchy, so a petal row always matches `resolution_scopes`. A petal moved to another fractal would orphan its petal-scope rows, exactly like role statements; no re-parenting path exists.
- R-BND-09 FIXED `fe-plugin/src/bindings/resolver.rs` row 6: any `kill_override` record passed a revoked build on any host. It now also needs `is_verse_owner_host` (§5.7, "on the Owner's own host"). The table journey gains the non-Owner row.
- R-BND-10 HANDED-OFF I-BIN / R-LIFE (`fractalengine/src/plugin_bridge.rs` `local_facts`, `fe-plugin/src/bindings/activation.rs`): row 10 reads `EffectiveRoles::role_at`, which has no `default_access` fallback. A local DID that relies on the verse's `viewer` default resolves `role-denied` (fail-closed). NEEDS a default-access field on `LocalFacts` or `BindingSnapshot`, set by the binary and used by row 10.

## The effective view and host state

- R-BND-11 KEPT `fe-api/src/bindings.rs` strips `resolution` (state and offer) for every caller except the node DID under an Owner token. A delegated token is remote (journey-pinned).
- R-BND-12 FIXED `fe-api/src/bindings.rs` `effective_after_gate`: the host publishes every point bound anywhere in the verse (activator `points_of` is verse-wide), so a petal Viewer learned the hook ids and keys bound in sibling petals and fractals. The new `decided_points` drops every point that nothing at this petal's chain decides, and shows a nearer builtin (stored terrain) as the deciding builtin. `bindings_api_test` is extended.
- R-BND-13 HANDED-OFF R-GOV (`fractalengine/src/plugin_bridge.rs` `published_petals`, `PublishedPoint::native_terrain`, `BindingView::need_at`): `winner` is taken with `find_map(effective_binding)`, which ignores the implicit stored-terrain builtin. `native_terrain()` then overwrites the resolution, and `need_at` serves the outer row's config. A fractal or verse binding to `org.fractalengine.terrain` therefore beats a petal's own stored terrain column, which breaks OQ-B6. NEEDS: skip the native-terrain override, and set `winner` to `None`, when `resolved.detail.implicit_terrain`.

## The whole-flush rule, provenance and the deferred lane

- R-BND-14 KEPT `fe-database/src/handlers/plugin_host.rs` `flush`. A reading anchored on a node the same batch deletes is refused before the door and the commit. Unknown or foreign anchors fail the door before the commit. A readings-only flush re-checks the Editor floor and the pinned ancestry. A failure after a committed batch is `unknown-outcome`, never a retry instruction.
- R-BND-15 FIXED `fe-database/tests/db_test.rs` J3 never exercised the delete-anchor refusal (G1-C8). It now sends that flush, expects `invalid-input`, and the next flush moves the node, which proves it survived.
- R-BND-16 FIXED `plugin_host.rs` `commit`: a created node's `baseline` journal fact had no `payload.ext`. Only later facts were stamped, so a plain create left no `{package_id, content_hash}` at all. `stamp_provenance` now stamps every fact.
- R-BND-17 HANDED-OFF orchestrator (pinned seam) and R-LIFE (`fe-plugin/src/bindings/decode.rs`, `plugin_bridge.rs`): `ingest.decode` readings cannot be host-stamped with `payload_id` and `source_class: decoder` (bindings.md §6). `PluginCommit.readings` is `Vec<StagedReading>`, which carries neither, and `door_readings` sets `payload_id: None` and the grant's class. `DecodeStamp` goes to `handle.call_hook(request, stamp)`, which no fe-plugin type defines yet. NEEDS a steer: a stamp carrier on `PluginCommit` (one flush per decode call) or a dedicated command.
- R-BND-18 KEPT the deferred lane: transform-only nodes go through `DeferredPublications` exactly like `UpdateNodeTransform` (QA-0161). Created, deleted and property-changed nodes publish at once, and scene changes emit at once.
- R-BND-19 KEPT reading provenance: `insert_readings_with_provenance` takes the local DID as source, the grant's class (never guest-settable) and `admission.package()`.
- R-BND-20 FIXED `fe-plugin-test/src/mock_host.rs` `SetTransform` moved path and stamp geometry that the DB host refuses. The mock now refuses the same, so plugin tests behave like the host.

## Arms against the pinned seams

- R-BND-21 KEPT `fe-runtime/src/messages.rs` and `fe-database/src/lib.rs` land every pinned name:
  - `PluginCommit { admission, ops, readings, reply: Sender<PluginResult> }`;
  - `PluginRead` with the typed reply;
  - the six governance commands;
  - `PutBinding`, `ListBindings` and `LoadBindingSnapshot` with `BindingRefusal`;
  - `AcceptedPayload` with the six pinned fields plus `asset_ids` (additive, documented in fe-runtime `src/AGENTS.md`).
- R-BND-22 FIXED `fe-runtime/src/extension_binding.rs`: B0's parity test pinned a literal list. It now asserts that the served ids minus `portal.open` equal `fe_sdk::manifest_v2::KNOWN_HOOKS`.
- R-BND-23 KEPT `fe-plugin/src/lib.rs` lacked `pub mod bindings` and `pub mod net`, and `bindings/mod.rs` was missing, at review start. I-PLUGIN, running concurrently, has since added both lines and the file, so they are its to land.
- R-BND-24 HANDED-OFF orchestrator / R-UI owner: Keep binding (§4.5) exists only as fe-database `keep_binding` and `kept_template`. No `DbCommand`, `ExtensionOp` or Bindings-view action reaches it. Lowest-cost path: the view sends `PutBinding` built from `kept_template`.
- R-BND-25 KEPT `AppendExtensionAudit` refuses `kill-list-override` but accepts `consent` and `bind` entries, which hydrate consents and acceptances at startup. Legitimate consent recording uses this command, and only host code can send a `DbCommand`. DEFERRED trigger: any path that lets guest- or bus-supplied JSON reach `AppendExtensionAudit`. At that point, give consent and bind their own commands.

## Other checks

- R-BND-26 KEPT step 3 of the write path skips the "excludes its kind" check, because the DB plane has no manifest. Resolution row 7 enforces kinds per build.
- R-BND-27 KEPT a token that covers a scope, held by a DID with no role there, gets 422 for a malformed body versus 404 for a well-formed one. fe-api §existence-oracle treats token coverage as the existence gate.
- R-BND-28 KEPT each refused remote write appends one audit row (`refuse_role`), bounded by the host's `admit_remote` rate gate.
- R-BND-29 KEPT the local dump/load path (`admin` `KnownTable::ExtensionBinding`) stores rows without attestation, but judging on read still decides. It is the trusted operator's path.
- R-BND-30 HANDED-OFF R-HOOKS (`fe-sdk/src/manifest_v2.rs` `KEYED_HOOKS`): it omits `petal.activate`, which the catalogue and bindings.md §6 key by name, so a `provides` entry for `petal.activate` cannot narrow `keys`.
