---
type: Review
track: hexon_entitlements_20261031
critic: C-HEXON
round: 4
date: 2026-11-13
---

# C-HEXON completeness: hexon_entitlements_20261031 (round 4)

Every spec requirement (FR-1..FR-8, NFR-1..NFR-3, AC-1..AC-7) and plan task checked against code at HEAD. No cargo was run. "Complete" means the code and its test exist; tests have not been run (user-gated).

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 registry protocol v1 (header + `/health`) | complete | `REGISTRY_PROTOCOL` / header `fe-hexon-registry/src/routes.rs:27-29`, stamped on every response incl. token refusals `:138,143-147`, `/health` field `:179`; tests `fe-hexon-registry/tests/registry_test.rs:66,84` |
| FR-2 `PackageStore` + `FsPackageStore` | complete | `fe-hexon-registry/src/store.rs:51,89` |
| FR-3 default-deny | complete | `fe-hexon-registry/src/config.rs:16,36,49`, open warning `src/main.rs:20`; test `registry_test.rs:1008-1020` |
| FR-4/FR-5 verifier, token, wrap scheme, LocalIssuer | complete | `fe-format/src/entitlement.rs:337,366` (wrap/unwrap), `:414,435` (seal/open), `:456` claims, `:477` keyring, `:541` verifier trait, `:673` LocalIssuer; feature gate `fe-format/Cargo.toml:17`; table journey `entitlement.rs:891` |
| FR-6 client-side enforcement | complete | `fe-hexon/src/entitlement_gate.rs` (NotRequired short-circuit `:121,146`); every in-process load goes through `HexonRegistry::gate_for`/`load_entry` `fe-hexon/src/registry.rs:443,472`; REST asset route `fe-api/src/hexon.rs:144,566`, 403 `entitlement_required` `:595-604`, nosniff/attachment `:579`; tileset members gated `fe-terrain/src/tiles/hexon_source.rs:441-460`, status `fe-terrain/src/tiles/entitlement.rs:94` |
| FR-7 Hexon Manager row states + plain link | partial | Installed map cards: chip per state `fe-ui/src/dialogs/hexon_manager.rs:294-299,333`, hardened `web_url` `:307`, `hyperlink_to` with per-row name `:339-340`; access fed from `fractalengine/src/terrain_bridge.rs:512-523`. Not done: the Available tab rows carry no state (`render_available_tab` `hexon_manager.rs:593-684`, no licence field on the advertised DTO) |
| FR-8 format spec 1.1.0 | complete | `docs/hexon-format-spec.md:3,644-654` |
| NFR-1 no payment code | complete | R-HEXON-3 F16 scan; UI says only "Licence details" (`hexon_manager.rs:339`) |
| NFR-2 backward compat | complete | gate never consults the verifier for unlicensed/Free/Attribution (R-HEXON-3 F11, `registry.rs:442`); `registry.rs:910-923` free leg |
| NFR-3 registry stays dumb | complete | `fe-hexon-registry/Cargo.toml` has no fe-hexon or entitlement dependency |
| AC-1 | complete | `fe-hexon/src/registry.rs:910-923` |
| AC-2 | complete (tilesets), partial (UI wording) | preview loads, other entries `EntitlementRequired` `registry.rs:930`, `fe-api/tests/api_integration.rs:988-989`; Manager shows "Preview" for `InstalledPreview` |
| AC-3 / AC-4 | complete | `fe-format/src/entitlement.rs:891`; gate journey `fe-hexon/src/entitlement_gate.rs:258` |
| AC-5 / AC-6 / AC-7 | complete | `registry_test.rs:1008-1020`, existing route suite on `FsPackageStore`, `registry_test.rs:66,84` |
| Plan S1, S3, S5, S6 | complete | as above; S6 verdict `review-turn4/R-HEXON-3.md` (PASS-WITH-FIXES) |
| Plan S2 | complete (code) | the plan note "no `X-Hexon-Registry-Protocol` header" is stale: the header is the lowercase constant at `routes.rs:29` |
| Plan S4 | partial | see FR-7 (Available tab) |
| Plan S7 docs consolidation | partial | root pointer `AGENTS.md:380` and `fe-hexon/src/AGENTS.md:239` exist; the pointer target `fe-terrain/src/AGENTS.md` §tileset-entitlement does not exist (also cited by `fe-terrain/src/tiles/entitlement.rs:1`); `fe-ui/src/dialogs/AGENTS.md` has no licence chip/link note; `conductor/tracks.md:100` row still `[ ]` |

## Excluded by the scoper (not gaps)

- blocked-user: Gate W1, Gate W2, Gate W3 (orchestrator), Full sweep (orchestrator, once), Retro + archive.

## Gate hand-offs (did each reach its owner?)

- F5 NOT landed: `fe-terrain/src/tiles/composite.rs:134` still warns "hexon tile read failed" for `EntitlementRequired`.
- F6, F7, F8: DEFERRED with triggers (no paid tileset published, no streamed licensed tileset, no in-app token source); still accurate at HEAD.
- F9 NOT landed and breaks the fe-ui test build: `fe-ui/src/terrain_map/mod.rs:288` and `fe-ui/src/verse_manager/db_results/terrain.rs:280` build `InstalledTilesetDto` without `access` / `verification_url` (`fe-ui/src/terrain_map/dto.rs:55,57`).
- SEC-INBOX: no entry names a file of this track's gate; entries 5, 6, 8 and 9 are snapshot ones (hexon_unification completeness file).

## HANDED-OFF (owner = next round)

- HANDED-OFF (round 5, owner of the two fe-ui test fixtures): add `access: TilesetAccess::Installed, verification_url: None` at `terrain_map/mod.rs:288` and `db_results/terrain.rs:280` (R-HEXON-3 F9; compile blocker for `cargo test -p fe-ui`).
- HANDED-OFF (round 5, fe-terrain tiles owner): downcast `EntitlementRequired` in `composite.rs:134,161` and `registry.rs:244` and treat it as a silent miss (R-HEXON-3 F5).
- HANDED-OFF (round 5, I-DOC-CORE): write `fe-terrain/src/AGENTS.md` §tileset-entitlement (member-path granularity, gate before I/O, preview vs locked).
- HANDED-OFF (round 5, I-DOC-UI): add a `fe-ui/src/dialogs/AGENTS.md` §hexon-manager note on the access chip and the never-fetched licence link.
- HANDED-OFF (round 5, UI owner of `hexon_manager.rs` + peer advert DTO): Available-tab licence state (FR-7) once peer adverts carry licence terms, or record that it is out of scope.
- HANDED-OFF (round 5, T-HEXON / I-CONDUCTOR): tick S2, drop the stale S2/S4 notes, update `conductor/tracks.md:100`.

## User-question defaults (turn4-plan.md §9) vs code

- Q13 (tileset entry = archive member path): matches. `EntitlementRequired.member` is the member path (`fe-terrain/src/tiles/entitlement.rs:19-20`), and the source refuses per member (`hexon_source.rs:441-460`).
- Q14 (REST licensed entry -> 403 `entitlement_required`): matches (`fe-api/src/hexon.rs:122,595-604`; journey asserts it `fe-api/tests/api_integration.rs:988-989`).

Verdict: PARTIAL. FR-1..FR-6 and FR-8 are complete in code. FR-7 lacks Available-tab states, S7 docs have a dangling pointer, and R-HEXON-3 F5/F9 did not land (F9 blocks the fe-ui test build).
