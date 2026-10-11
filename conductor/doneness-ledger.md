---
type: ledger
title: FractalEngine doneness ledger
updated: 2026-10-10
source: 2026-10-10 three-auditor board reconciliation + consolidation pass
---

# Doneness ledger — 2026-10-10

This is the work queue for the next gap-analysis/execution workflows. The board ([tracks.md](./tracks.md)) says where each track stands; this file says what is left and who can do it. The machine truth for each track stays in `tracks/<id>/metadata.json`.

**How this was built.** Three auditors checked every plan checkbox against the tree (code and git evidence, no cargo) and reclassified every open item. A consolidation pass then spot-verified 18 ticks and 20 classifications. It un-ticked 4 over-claims, each because the task's own acceptance test was missing. It also reclassified 3 canonical-log items, merged 13 duplicates and added 1 newly found gap (P2P-REF-CONFINE).

## Corrections made in consolidation

- Un-ticked the `stamped_asset_nodes` gimbal task. `fe-ui/src/node_manager/gimbal_interaction.rs:267-273` resolves a stamp gimbal hit as `SelectionKind::Node`, so Move still drags a stamp (STAMP-GIMBAL-TRANSLATE-LOCK).
- Un-ticked `mcp_scene_primitives` 2.2 (no REST 201/403/503 tests) and 5.2 (no single chained FR-8 loop). That track goes back from done to in_progress (MCP-TEST-UPLOAD, MCP-TEST-LOOP).
- Un-ticked the `sculpt_earthwork_regions` sculpt-ops task, because its required determinism test is absent (SCULPT-DETERMINISM-TEST).
- Canonical log: the owner approved SPEC-1..8 and Workstream G on 2026-08-08/09, which makes **local** implementation GO. Only network exposure is NO-GO. Three items changed as a result. CANON-BOUNDARY was split: its six bypass counters are already in CDL-W4, and the dual-emit cut-over became CANON-DUAL-EMIT (needs ratification). CANON-SIGS moved from NO_GO to stale, because D-CL24(5) makes legacy rows permanently unsigned; its rotation assembler split out as CDL-ROTATION-ASSEMBLY. CANON-WS stays NO_GO, but now covers only the mount; its pre-mount tests (CDL-R6, CDL-WS-ORACLE) are doable.
- Merged duplicates: FEAPI-DECOMP into REG-DECOMP; FEUI-SETTINGS-PERSIST into PAS-P4B; FEUI-RESIDENCY-LEDGER into PAS-P4; FEUI-INAPP-MINT into EGR-MINT; UXQA-SCALE-AUTHORITY into MSA-P3; TERRAIN-STRATA into SCULPT-STRATA; XP-3.2 into OSS-5.5; XP-1.4 into RCI-2.7; OD-D-53 into OSS-5.6; UXQA-DECISIONS into OD-UX; SIGNED-OP-REPLICATION and TS-SHARDS-GATE into SYNC-SIGNED-PLANE; the ten stale OD-D-* entries into OD-OVERTAKEN.
- Closed in this pass (docs): OSS-TRUTH. README.md now says the plugin host is library-only. SECURITY.md now describes the DB-thread Editor+ gate and its residuals instead of a "permissive" gate. The SQLite line in root AGENTS.md (part of OSS-3.7B) is fixed. The README Status section is corrected: BI egress and IoT export are done, the MCP vocabulary has shipped, the release matrix reflects the macOS removal, and the crate count is 24.

## Per-track doneness

Counts are plan checkboxes (`[x]` versus `[ ]`/`[~]`) where a plan exists. `-` means there is no plan.md and the tasks live in metadata or the spec.

| Track | Status | Done | Open | Remaining / note |
|---|---|---|---|---|
| [mission_prodready_continuation_20261008](./tracks/mission_prodready_continuation_20261008/) | done | 24 | 0 | F9-F19, A20-A33 validated; pushed 1e9be3a; archive-ready |
| [node_lifecycle_addressing_20260725](./tracks/node_lifecycle_addressing_20260725/) | done | 15 | 0 | Spine c6fdf0d/aeba832; residuals T1-TXN, T1-GC |
| [analytics_egress_20260714](./tracks/analytics_egress_20260714/) | done | 21 | 0 | Live DuckDB e2e 49/49; residuals EGR-* |
| [iot_spatial_reporting_20260714](./tracks/iot_spatial_reporting_20260714/) | done | 12 | 1 | 4.2 poll connectors = future work (IOT-4.2) |
| [api_mcp_integration_tests_20260717](./tracks/api_mcp_integration_tests_20260717/) | done | 19 | 1 | Inventory artifact API-INV-1 (non-blocking) |
| [p2p_mycelium_completion_20260701](./tracks/p2p_mycelium_completion_20260701/) | done | - | 1 | Residual MYC-SECRETS |
| [timeseries_fabric_20261009](./tracks/timeseries_fabric_20261009/) | done | - | 0 | Record-only |
| [simulation_lab_20261009](./tracks/simulation_lab_20261009/) | done | - | 0 | Record-only |
| [bi_verification_20261009](./tracks/bi_verification_20261009/) | done | - | 0 | Record-only |
| [contextual_controls_20260725](./tracks/contextual_controls_20260725/) | implemented_pending_inapp_verify | 8 | 1 | Retro + in-app |
| [shell_ux_sidebar_20260725](./tracks/shell_ux_sidebar_20260725/) | implemented_pending_inapp_verify | 7 | 2 | Stale docs, retro/in-app |
| [stamped_asset_nodes_20260725](./tracks/stamped_asset_nodes_20260725/) | implemented_pending_inapp_verify | 9 | 3 | Gimbal translate lock un-ticked; N-9 test |
| [sculpt_earthwork_regions_20260725](./tracks/sculpt_earthwork_regions_20260725/) | implemented_pending_inapp_verify | 9 | 2 | Determinism test un-ticked; no delete producer |
| [pen_curve_tool_20260722](./tracks/pen_curve_tool_20260722/) | implemented_pending_inapp_verify | 31 | 0 | In-app re-verify + archive only |
| [ui_shell_architecture_20260724](./tracks/ui_shell_architecture_20260724/) | implemented_pending_inapp_verify | 23 | 6 | All 6 open are in-app checkpoints |
| [ui_semantics_unification_20260808](./tracks/ui_semantics_unification_20260808/) | implemented_pending_inapp_verify | - | 5 | In-app, rulings record, f32::MAX lint |
| [endpoint_api_surface_20260725](./tracks/endpoint_api_surface_20260725/) | in_progress | 5 | 4 | T5-LOCK, T5-PARITY, T5-TRIGGER, retro |
| [hierarchy_visibility_groups_20260808](./tracks/hierarchy_visibility_groups_20260808/) | in_progress | - | 5 phases | Phases 2-6 doable (VIS-PH2..6) |
| [mcp_scene_primitives_20260716](./tracks/mcp_scene_primitives_20260716/) | in_progress (was done) | 23 | 3 | 2.2/5.2 un-ticked (tests owed); 2.4 REST place |
| [hexon_scale_orchestration_20260712](./tracks/hexon_scale_orchestration_20260712/) | in_progress | 17 | 7 | HS-P5, HS-P6-GRID; callouts on D-11 |
| [auth_policy_pattern_20260710](./tracks/auth_policy_pattern_20260710/) | in_progress | 13 | 3 | AP-3.1, AP-3.4; 3.5 stale |
| [canonical_data_log_20260808](./tracks/canonical_data_log_20260808/) | in_progress | - | 14 tasks | Wave 4 GO (local); network NO-GO |
| [p2p_asset_streaming_20260718](./tracks/p2p_asset_streaming_20260718/) | in_progress | 8 | 24 | PAS-P0..P6 |
| [oss_release_20260717](./tracks/oss_release_20260717/) | in_progress | 17 | 15 | Doable: 1.3, 1.5, 2.4, fe-sdk docs, 4.2, 4.3, P6 |
| [release_ci_20260429](./tracks/release_ci_20260429/) | in_progress | 9 | 1 | v* tag run (user) |
| [cross_platform_desktop_20260429](./tracks/cross_platform_desktop_20260429/) | in_progress | 12 | 2 | ARM64 via tag; Linux --workspace (D-66) |
| [thorns_shields_20260321](./tracks/thorns_shields_20260321/) | in_progress | - | 3 FRs | TS-FR1, TS-FR3 doable; TS-FR2 ratify |
| [ux_qa_review_20260714](./tracks/ux_qa_review_20260714/) | in_progress (meta) | - | 5 | Never done by design |
| [hexon_unification_20260716](./tracks/hexon_unification_20260716/) | pending | 0 | 37 | HU-P1..P6 all doable |
| [map_scale_authority_20260716](./tracks/map_scale_authority_20260716/) | pending | 0 | 21 | MSA-P1..3,5 doable; P4 on D-38 |
| [road_builder_ux_20260716](./tracks/road_builder_ux_20260716/) | pending | 0 | 30 | ROAD-PATH-KIND, ROAD-CORE doable |
| [iroh_1_0_upgrade_20260711](./tracks/iroh_1_0_upgrade_20260711/) | pending | 0 | 1 | IROH-1X; deadline 2026-12-31 |
| [crate_consolidation_r2_20260718](./tracks/crate_consolidation_r2_20260718/) | pending | 0 | 15 | CC-P1 doable; gates need ratification |
| [inspector_settings_20260419](./tracks/inspector_settings_20260419/) | blocked_on_ratification | 11 | 19 | FR-3/FR-4 on D-32/D-33 |
| [outstanding_decisions_20260715](./tracks/outstanding_decisions_20260715/) | blocked_on_ratification | 4 resolved | 68 entries | 10 overtaken, 3 user actions, 55 to ratify |
| [hexon_delta_format_20260710](./tracks/hexon_delta_format_20260710/) | spec_only | 0 | 1 | Foundry candidate |
| [hexon_p2p_bucket_20260710](./tracks/hexon_p2p_bucket_20260710/) | spec_only | 0 | 1 | Foundry candidate |
| [verse_services_20260711](./tracks/verse_services_20260711/) | spec_only | 0 | 1 | Foundry candidate |
| [sso_federation_20260429](./tracks/sso_federation_20260429/) | spec_only | 0 | 6 | Never scheduled |
| [light_box_20260402](./tracks/light_box_20260402/) | off_strategy | 0 | 24 | Zero code |
| [profile_manager_20260419](./tracks/profile_manager_20260419/) | off_strategy | 0 | 31 | Zero code |

## Remaining work by class

| Class | Items |
|---|---|
| AGENT_DOABLE_RATIFIED | 81 |
| NEEDS_RATIFICATION | 47 |
| USER_GATED | 15 |
| NO_GO | 3 |
| OFF_STRATEGY_OR_SPEC_ONLY | 16 |
| STALE_OR_OBSOLETE | 18 |
| **Total** | **180** |

Suggested batches for the next workflows (agent-doable, disjoint files):

- **fe-api quick wins:** MCP-2.4, MCP-TEST-UPLOAD, MCP-TEST-LOOP, FE-GPX-HTTP-EXPORT-PARENT, EGR-DBREADER, SIM-GUI-TOOLS, API-INV-1.
- **Spatial Builder close-out:** SB-EARTHWORK-DELETE, STAMP-GIMBAL-TRANSLATE-LOCK, T5-LOCK, STAMP-N9-ROWCOUNT, SCULPT-DETERMINISM-TEST, SHELL-STALE-DOCS, SB-RETROS.
- **Scale:** MSA-P1, then MSA-P2, MSA-P3 and MSA-P5.
- **Hexon unification:** HU-P1 through HU-P6, run serially.
- **Time-critical:** IROH-1X before 2026-12-31. Decide D-22/D-23 first.
- **Stale sweep:** every STALE_OR_OBSOLETE item with a code or doc acceptance, in one pass.

### AGENT_DOABLE_RATIFIED (81)

Design ratified; an agent can build and test it now without new scope. Items whose evidence says *trigger-gated* are ratified but conditional: build them when the trigger fires or opportunistically.

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **AP-3.1** | Route fe-api auth.rs role/scope helpers through the fe-policy engine | S | auth_policy_pattern_20260710 Task 3.1 | fe-api/src/auth.rs:55-90 has no fe_policy call; fe-api/src/hexon.rs:181 already uses fe_hexon::authz::authorize (916a3ae); spec §D1 amendment ratified 2026-07-11 | require_role_level/require_scope evaluate via fe_policy (RoleLevelPolicy + scope containment); table test: Viewer denied Write, foreign-verse scope denied, in-scope Editor allowed; bi-egress-verify / hexon-install-verify / hexon-serve-verify scripts still pass |
| **AP-3.4** | TokenScopePolicy + OwnershipPolicy with production callers | M | auth_policy_pattern_20260710 Task 3.4 / fe-policy/AGENTS.md:56 | No Rust definition of either type; fe-policy/AGENTS.md:56 lists them as deferred | Table tests: VERSE#v1 token covers PETAL in v1, denies v2; only the publisher DID may uninstall/republish; one production caller each, proven by a test that fails if the policy is removed |
| **CDL-W4** | Workstream G Wave 4 serial integration (compose.rs, lib.rs surface, six record_bypass counters, cross-spec test) | L | canonical_data_log_20260808 G-WAVE-4 (workstream-g-plan.json waves[3]); absorbs the six D-CL20 handlers from CANON-BOUNDARY | Owner approved SPEC-1..8 + Workstream G local implementation 2026-08-08/09; record_bypass only in fe-database/src/migration/boundary.rs; build_router_with_canonical_log only #[cfg(test)] callers. The RUNBOOK 'NO-GO-implementation' header refers to network rollout; metadata network_enablement=prohibited | Per the wave brief: six strictly-additive record_bypass calls (create_verse/fractal/petal, import_gltf, rename_node, duplicate_node) with a test asserting each counter increments and no OpLogEntry is built; composed CandidateVerifier/CheckpointView/ScopeKeyResolver/SealedArtifactStore/CapabilityVerifier; cross_spec_integration.rs green without DB or network; 0 NO_CALLER gates; /security-review verdict recorded in the track |
| **CDL-R1** | Seal VerifiedEnvelopeMeta (#[non_exhaustive] + verifying constructor) | S | RUNBOOK a31c403 §10a residual 1 | fe-canonical-log/src/materialize/traits.rs:38 all-pub struct; blocker was a #[cfg(test)] literal at fe-database/src/canon_log/admission.rs:245 | #[non_exhaustive] with one verifying constructor; compile_fail doctest proves external struct-literal fails; fe-database canon_log tests green |
| **CDL-R2** | Production DeviceEnrolmentView + IssuerAuthorityView implementors | M | fe-canonical-log/src/crypto/AGENTS.md:29; RUNBOOK a31c403 §10a residual 2 | Only test implementors (crypto/key_wrap.rs:805,817); enrolment registry 'Not built. Wave 4 follow-up' | DB-backed views; wrap to unenrolled device refused, non-Manager issuer refused, enrolled+authorized succeeds; AGENTS.md row 'Not built' -> 'Structural' |
| **CDL-R6** | Scope-sensitive test double proving canonical WS capability scope gating | S | RUNBOOK a31c403 §10a residual 6 | MockCapabilityVerifier ignores scope; async-trait absent from fe-api [dev-dependencies] | Out-of-capability subscribe refused, in-scope admitted; test fails if the epoch_scope.contains gate is removed |
| **CDL-WS-ORACLE** | Existence-oracle audit of canonical_ws (pre-mount) | S | mission_prodready_continuation deferred register 2026-10-10 | Trigger 'when routed in server.rs'; /ws/canonical unmounted (mount itself is CANON-WS, NO_GO). Trigger-gated: do before any mount | Missing-vs-foreign scope errors on /ws/canonical byte-identical (test) |
| **CDL-SPEC10** | Draft SPEC-10 observation plane (columnar hexon, observation-batch kind) with errata G3/G6 | L | canonical_data_log_20260808 SPEC-10 + errata G3/G6 | Direction ratified D-CL26/D-CL29 (2026-08-16); tasks not_started; normative status stays owner-gated | Draft spec with conformance-test names, iot_reading graduation path, LaneClass/ObjectClass mapping; submitted for owner approval |
| **CDL-STATS** | Validate manifest statistics against received headers in selective fetch | S | RUNBOOK a31c403 §9 deferred hardening | Trigger-gated: no selective-fetch consumer exists yet | When segment skipping is built: an under-reported HLC range causes fetch, not skip (test) |
| **CC-P1** | Consumer-map and dependency-closure evidence artifacts | S | crate_consolidation_r2_20260718 Phase 1 | User directive 2026-07-18; fe-plugin-test zero consumers; fe-query consumed by fe-api and fe-database | evidence.md with per-crate consumer tables and fe-hexon-registry vs fe-hexon closure sizes; cross-ref task in hexon_unification |
| **HU-P1** | fe-format absorbs fe-hexon types (Script/PetalSnapshot, License/EntryKind parity, HexonKind compat) | M | hexon_unification_20260716 Phase 1 | User directives binding 2026-07-16; fe-format/src/manifest.rs:6 HexonType lacks Script/PetalSnapshot | Serde round-trips for new variants; fe-hexon license fixture parses; every legacy HexonKind string maps; fe-format gains no deps |
| **HU-P2** | Rebuild fe-hexon on fe-format | L | hexon_unification_20260716 Phase 2 | fe-hexon/Cargo.toml:39 fe-format dev-only; manifest.rs:99 crate_id; .fecrate in package.rs/publisher.rs | cargo test -p fe-hexon -p fe-hexon-registry green; in-process publish->search->download by hexon_id; no production crate_id/.fecrate in fe-hexon |
| **HU-P3** | Re-point fe-api hexon routes, real install_crate, delete .fecrate | M | hexon_unification_20260716 Phase 3 | fe-api/src/hexon.rs:246-295 install_crate returns synthetic success | Install-then-list shows petal association; Editor allowed, Viewer denied; axum test returns fe-format fields; no 'fecrate' in production code |
| **HU-P4** | PetalSnapshot export | L | hexon_unification_20260716 Phase 4 | No PetalSnapshot symbol; open questions 1-2 are decide-at-Phase-4 (spec.md:436-447) | Embedded-DB test asserts row counts + strict lamport order in snapshot/oplog.jsonl; Editor 403, Manager gets application/x-hexon+zip |
| **HU-P5** | PetalSnapshot import/restore + round-trip determinism | L | hexon_unification_20260716 Phase 5 | Not started; FR-9 round-trip is the acceptance | export->import->re-export equal rows/op-log order/BLAKE3; duplicate import distinct error; corrupt blob aborts atomically; first post-import write exceeds imported max lamport |
| **HU-P6** | Unification docs + sweep | S | hexon_unification_20260716 Phase 6 | fe-format/src/AGENTS.md has no snapshot layout section | Docs describe one format + snapshot layout; sweep green; grep fecrate/crate_id clean |
| **IROH-1X** | Port fe-sync/fe-network from iroh 0.35 to 1.x before the 2026-12-31 relay EOL | L | iroh_1_0_upgrade_20260711 | Decision §D5-2; Cargo.toml:67-78 pins 0.35 + iroh-quinn-proto 0.13 (BBR); live 0.35 docs stack (f9c609a, 2364600). Timing posture D-22/D-23 still open but the port itself is ratified | Workspace builds on 1.x; harness real-loopback scenarios pass; relay-assisted connection against a self-hosted 1.x relay; BBR still selected; no consumer changes outside fe-sync/fe-network |
| **PAS-P0** | Evidence pack: tile/asset size histogram, double-residency math | S | p2p_asset_streaming_20260718 Phase 0 | No evidence.md; decision record :180 lists relay_data_horizon as open input | p50/p90/p99 per-zoom sizes from installed Zurich/Fuji hexons + gis-tile-etl outputs; decision record updated |
| **PAS-P1V** | Headless run showing relay mode + health | S | p2p_asset_streaming_20260718 Phase 1 verification | Phase-1 code done (320ebfe) but no recorded run | fe-relay with Default/Custom/Disabled logs mode, EOL warning for Default, SyncStatus health; output captured in the track |
| **PAS-P3** | Transport trait + real tileset meta/chunk handlers + resumable downloads | L | p2p_asset_streaming_20260718 Phase 3 | D-73 A4 + D-77 E2 ratified; fe-sync/src/sync_thread.rs:1969-2000 handlers emit ChunkFailed | Two-instance loopback chunked transfer with per-chunk blake3; tampered chunk rejected; TilesetDownloadTracker resumes after restart; in-memory transport impl used by tests |
| **PAS-P4** | Pointer-set residency ledger, distance-ranked residency, lazy HexonTileSource, exit-ring eviction | L | p2p_asset_streaming_20260718 Phase 4 + 2.6; absorbs FEUI-RESIDENCY-LEDGER (fe-ui petal_respawn.rs:95 TODO) | D-74 ratified; fe-ui/src/verse_manager/spawn.rs:34 soft allowance only; batch spawners use distance 0.0 | Ledger resource replaces the boolean gate; nodes beyond render_distance x1.5 despawn/re-enter with hysteresis (moving-camera system test); lazy tile reads with offset index; 2.6 outcome recorded |
| **PAS-P4B** | AppSettings RON persistence, remaining knobs, PetalManifest render-distance override, relay config from settings | M | p2p_asset_streaming_20260718 Phase 4b; fe-ui/src/settings.rs:73 TODO; fe-sync/src/relay_config.rs:91 TODO; absorbs FEUI-SETTINGS-PERSIST | D-78 ratified; serde derives in place; only 2 knobs exposed; relay config env-only | settings.ron survives restart (temp-dir test; corrupt/partial file -> defaults); caps/tile mode/camera knobs editable; PetalManifest.render_distance overrides global (test) |
| **PAS-P5** | PartialHexonFetch (HTTP + P2P), HexonArchive::import split, signed digest integrity | L | p2p_asset_streaming_20260718 Phase 5 | D-75 C1 + D-76 D2 ratified; zero code; coordinate manifest fields with HU-P1/P2 | Shared conformance suite for hosted + P2P partial fetch; chunk-by-seq + meta routes; asset_hash verified at install and publish (mismatch test) |
| **PAS-P6** | Close-out sweep, retro, archive | S | p2p_asset_streaming_20260718 Phase 6 | Plan Phase 6 open; after PAS-P0..P5 | Sweep green; retro written; decision record closed |
| **TS-FR1** | Commit an unwrap/expect census script and rewrite the audit | M | thorns_shields_20260321 FR-1 | docs/unwrap-audit.md STALE; scripts/audit.sh never committed | Script lists production unwrap/expect sites (tests excluded); audit doc replaced with current register + justified panics; STALE marker removed |
| **TS-FR3** | Fuzz targets built and run in CI | M | thorns_shields_20260321 FR-3 | fuzz/targets/{ed25519_verify,jwt_parse}.rs exist; no workflow references fuzz | CI job builds both with cargo-fuzz (nightly), runs each for a bounded time; green on main |
| **OSS-1.3** | cargo deny check licenses in CI | S | oss_release_20260717 1.3 | deny.toml:4-32 correct; no cargo-deny step in build-artifacts.yml | Lint job runs cargo deny check licenses, green on main |
| **OSS-1.5** | Reconcile the placeholder-signature register (20 sites) | S | oss_release_20260717 1.5 / 6.4 | grep '"00".repeat(64)' = 20 sites in 7 fe-database files (verified 2026-10-10) vs 13 in spec FR-2 | Register lists every site with file:line and cites D-CL24(5); re-grep matches |
| **OSS-2.4** | Fix BUILDING.md GHCR pull URL | S | oss_release_20260717 2.4 | BUILDING.md:128 ghcr.io/jadezaher vs release.yml:208 repository_owner | Pull URL matches the image release.yml pushes |
| **OSS-3.7B** | Fill fe-sdk missing-docs gaps | S | oss_release_20260717 3.7b | AGENTS.md SQLite line fixed 2026-10-10 consolidation; fe-sdk part needs a cargo doc/clippy run | cargo doc/clippy on fe-sdk emits no missing_docs warnings |
| **OSS-4.2** | cargo publish --dry-run for fe-sdk and fe-plugin-test | S | oss_release_20260717 4.2 | Not run | Both dry-runs succeed; output recorded in the plan |
| **OSS-4.3** | Relocate root .glb strays, replace 14-byte camera.glb | S | oss_release_20260717 4.3 | camera.glb, duck.glb, info_panel.glb at repo root | No .glb at repo root; references updated and green |
| **OSS-P6** | Pre-push preflight: history secrets scan, decision-register review, final README audit, sig re-grep | M | oss_release_20260717 6.1-6.4 | 6.3 partially re-run 2026-10-10 (README Status + SECURITY.md corrected); origin already pushed so 6.1 is retroactive | Secrets tool + date recorded with zero findings; each Phase-5 item has a dated resolution; README claims re-checked |
| **SYNC-RELAYWATCH** | Continuous relay-health monitoring via home_relay() watcher | M | fe-sync/src/sync_thread.rs:586 TODO; fe-sync/src/AGENTS.md §relay-health | Relay loud-fail ratified FR-1 hardening (D-77); only startup bind + gossip spawn tracked | Command loop selects on the watcher; test drives Healthy->Degraded->Healthy and RelayHealthChanged reaches SyncStatus |
| **SYNC-RESPONDER** | Responder-side filtering to hosted shards | S | fe-sync/src/AGENTS.md (distributed query) | Recorded as an efficiency follow-up, not a correctness fix | Non-host responder skips the DB round trip (test counts zero queries); merge results unchanged |
| **SYNC-NSID-CAP** | Assess whether verse.namespace_id readable via /query is a replica read capability | S | RUNBOOK §9 open question (M4 re-review) | Open question, trigger 'next fe-sync security pass' | Written finding in fe-sync/src/AGENTS.md; if it is a capability, /query denies namespace_id (test) |
| **REG-DECOMP** | Cap per-entry and total zip decompression in the hexon registry and archive reader | S | mission deferred register + security audit 2026-07-31; fe-hexon-registry/src/index.rs:40; absorbs FEAPI-DECOMP (fe-format archive reader) | index.rs:40 Vec::with_capacity(entry.size()) trusts declared size; decompression-bomb DoS finding. Trigger-gated: registry go-live | Reads bounded on declared and actual bytes (take()); zip-bomb fixture refused at publish, entries/asset GET and archive import without allocation blowup |
| **API-INV-1** | Endpoint-family inventory artifact (router vs test coverage) | S | api_mcp_integration_tests_20260717 Phase 4 task 1 | No inventory file; ~66 routes in fe-api/src/server.rs | Every route( in server.rs plus /ws and /mcp listed with covering test or explicit reason; diff against grep shows none missing |
| **MCP-2.4** | REST create_node optional asset_id/rotation/scale | S | mcp_scene_primitives_20260716 Task 2.4 | fe-api/src/types.rs:23 CreateNodeRequest has only petal_id/name/position (verified) | POST /petals/{p}/nodes with asset_id routes through place_asset_core and returns asset_path; omitted fields byte-identical legacy; foreign asset_id same denial as MCP place_asset |
| **MCP-TEST-UPLOAD** | REST multipart asset upload 201/403/503 tests | S | mcp_scene_primitives_20260716 Task 2.2 (un-ticked 2026-10-10) | upload.rs:362 untested over REST; task's own TDD acceptance unmet | Multipart POST via ApiHarness: 201 {asset_id,content_hash,size_bytes} + blob fetchable; 503 with blob_store None; Viewer/foreign denied |
| **MCP-TEST-LOOP** | Single chained MCP full-loop scene test (spec FR-8) | S | mcp_scene_primitives_20260716 Task 5.2 (un-ticked 2026-10-10) | Only piecewise tests exist | upload_asset->place_asset->set_property->update_transform->get_gis_nodes shows moved node->delete_node->get_hierarchy lacks it, all via /mcp tools/call |
| **T5-LOCK** | Reject free-translate of position-locked stamp nodes over REST/MCP | M | endpoint_api_surface_20260725 FR-3 / stamped D-A5 | fe-database/src/handlers/transform.rs has no node_kind=stamp guard. Pair with STAMP-GIMBAL-TRANSLATE-LOCK (UI side) | update_transform with a changed position on node_kind=stamp returns a typed denial and leaves the row unchanged; rotation/scale-only succeeds; tests on both transports |
| **T5-PARITY** | UI-vs-API op-log parity test, earthwork summary on real rows, per-type read_node, retro | S | endpoint_api_surface_20260725 Phase 3 + retro | No parity test; earthwork/summary only in denial table; Q-1..Q-4 ratified 2026-07-25 | API DELETE and UI TombstoneNode produce equal op entries; seeded T3 region returns real cut/fill; per-type payload test for stamp/region/path; retro in plan |
| **HS-P5** | Interactive tape/bearing anchors + area tool | M | hexon_scale_orchestration_20260712 Phase 5 | spec FR-9; D-10 gate met 2026-10-09; only pure math exists | Reducer tests: two anchors -> distance m + bearing deg; polygon -> area m2 at world_scale 1 and 0.01; selectable tool in the right sidebar |
| **HS-P6-GRID** | Adaptive metric world grid | M | hexon_scale_orchestration_20260712 Phase 6 | spec FR-10; no graticule code | Unit tests on height->spacing/subdivision + fade alpha; grid renders 1/2/5x10^n and fades |
| **MSA-P1** | Canonical scale accessor fe_format::scale; fe-terrain delegates | M | map_scale_authority_20260716 Phase 1 | User directive 2026-07-16 binding; no fe-format/src/scale.rs | fe-format scale tests (ported vectors + null/missing/NaN/out-of-bounds) green; fe-terrain scale tests unchanged |
| **MSA-P2** | Map-authoritative placement scale from petal terrain | M | map_scale_authority_20260716 Phase 2 | crud.rs and lib.rs hardcode scale [1,1,1] | Node created/imported on a world_scale 0.01 petal has scale [0.01;3] and matching echo; terrain-less stays [1;3]; existing rows untouched |
| **MSA-P3** | UI units audit through the accessor (single scale authority) + fe-api §units | L | map_scale_authority_20260716 Phase 3; absorbs UXQA-SCALE-AUTHORITY | Two authorities: fe-terrain/src/config.rs:86 effective_world_scale and PetalMapState.world_scale; no fe-api §units | fe-ui has only delegating shims; UI numbers and ruler read one accessor (agreement test for a hexon-scaled petal); clamped world_scale lands in PetalMapState; §units documents world units on the wire |
| **MSA-P5** | Scripted world_scale round-trip scenario | S | map_scale_authority_20260716 Phase 5 | spec FR-6 | SetPetalTerrain 0.01 -> ImportGltf -> read back -> world_to_meters correct; repeated for 1.0 and terrain-less |
| **EGR-DBREADER** | /api/v1/query and share fmt=json channel fallback when db_reader is None | M | fe-api/AGENTS.md §export (F10 follow-up) | rest.rs:1131 and share.rs:260 return 'not available (no db_reader)'; export already uses run_guarded_query_via_state | With db_reader None both return scoped rows via the correlated channel (same TIMEOUT + row cap) or honest 503 under semaphore exhaustion; tests mirror the export fallback tests |
| **T1-TXN** | Cascade tombstone: op append and row UPDATE in one transaction (or replay) | M | fe-database/src/AGENTS.md §lifecycle | Documented: a failed update leaves a replayable pending operation; log-first WAL ratified (D6) | Injected UPDATE failure after commit_operation is rolled back or replayed on next start; no logged-but-unapplied cascade survives (test) |
| **FEAPI-ORACLE-TIMING** | Index petal_id/fractal_id/node_id/verse_id lookups | M | fe-api/AGENTS.md §existence-oracle Residuals | schema.rs defines no index on those four id fields (verified); perf win + timing-channel narrowing | DEFINE INDEX IF NOT EXISTS for the four ids; schema_idempotent asserts them; existence_oracle_test green |
| **FEAPI-ASSET-OWNER** | Owner/petal column on asset written at upload | M | fe-api/AGENTS.md §assets | Design stated; trigger-gated: multi-tenant deploy | CreateAsset writes owner; unreferenced uploads readable by uploader only; owned assets skip the reference scan (tests) |
| **FEAPI-BLOB-GC** | Blob GC (refcount) + per-token upload quota | M | fe-api/AGENTS.md §asset-ingest | Trigger-gated: multi-tenant deploy | Unreferenced blobs past grace deleted by GC (test); over-quota upload gets 413 (test) |
| **FEAPI-GUI-HYDRATE** | Remove GUI's duplicate hydrate/scene-change conversion fns | S | fe-api/AGENTS.md §analytics | Documented as pending mechanical migration | GUI calls fe_api hydrate_entity_store/runtime_scene_change_to_store; duplicates deleted; analytics_scope_test green |
| **FEAPI-ELEV-TIMEOUT** | DB-side TIMEOUT on elevated statements | S | mission retro 2026-10-10 | Only a tokio 10s wrapper; trigger 'before anyone sets FE_API_ELEVATED_ALLOW=1' | Each elevated statement gets TIMEOUT 5s; a long scan errors instead of hanging db_reader |
| **FEAPI-GETTERRAIN-CORR** | GetPetalTerrain reply correlation | S | mission retro 2026-10-10 | Pattern ratified by 9cbdea6 SetPetalTerrain correlation | GetPetalTerrain carries correlation_id; concurrent GUI loads never misroute the tileset GET fallback (test) |
| **FEAPI-TILESET-2MIB** | REST tileset install 2 MiB effective cap | S | mission plan M5 notes | No DefaultBodyLimit on the route; trigger first >2 MiB install | Route limit matches TILESET_ARCHIVE_MAX_BYTES; a 5 MiB archive installs via REST and MCP (test) |
| **RT-CORR-EDGE** | Correlate remaining uncorrelated awaited commands | S | fe-runtime/src/AGENTS.md §api-reply-correlation | Revisit trigger: new timeout-capable caller; remedy pattern ratified | Timed-out caller's late reply never reaches the next waiter (test); reply_kind_mapping green |
| **RT-FSBLOB** | Reconcile the two FsBlobStore path conventions | S | fe-runtime/src/AGENTS.md §property-bridge | Fast-follow before texture registry wiring | One convention; a blob written by fe-runtime is readable by fe-hexon FsBlobStore (test) |
| **TERRAIN-RESIDENCY** | Route proposal-overlay spawns through the residency mesh budget | M | fe-terrain/src/terrain_plugin.rs:1060 TODO | MAX_PROPOSAL_OVERLAYS=4096 interim cap | Bridge resource consulted by render_terrain_proposals; spawns stop at budget (test) |
| **TERRAIN-REGROUND** | Re-ground relative-op proposal ghosts when chunks arrive | S | fe-terrain/src/terrain_plugin.rs:1064 TODO | Same pattern as EarthworkBaked re-bake | Ghost built before coverage is rebuilt once its chunks load (system test) |
| **SIM-GUI-TOOLS** | Hide sim_* MCP tools when no sim-control seam | S | mission plan M3 deferred minors | Static ToolSpec rows fe-api/src/mcp/mod.rs:158-166 | tools/list omits sim_* without the seam; inventory test asserts both configurations |
| **SB-EARTHWORK-DELETE** | UI producer for SculptDeleteRegion | S | sculpt_earthwork_regions_20260725 Phase 3 / ui_semantics PHASE-6 | T3 Q-2 ratified; UiAction::SculptDeleteRegion only matched at actions/mod.rs:1062 (verified no producer) | Region hit -> SculptDeleteRegion, palette proposal -> TerrainProposalDelete (table test); after drain the region record is gone and its node tombstoned |
| **SB-RETROS** | Retros for contextual_controls, shell_ux_sidebar, stamped_asset_nodes, sculpt_earthwork_regions | S | Spatial Builder Wave 0/1 plans final task | No retro.md in any of the four | OKF retro.md each: what shipped with hashes, integration lessons, open residuals |
| **SHELL-STALE-DOCS** | Correct stale fe-ui directory docs (sticky sidebar, Settings section, classifier hit kinds) | S | shell_ux_sidebar Phase 4 docs; fe-ui/src/ui_shell/AGENTS.md; fe-ui/src/dialogs/AGENTS.md | ui_shell/AGENTS.md:49-58 AutoCollapse; dialogs/AGENTS.md:119-142 deleted Settings window; :114 classifier kinds | grep AutoCollapse/SettingsToggle in fe-ui AGENTS.md = 0; docs match left_visibility(policy,user_intent) + Ctrl+B, right-sidebar Settings, current hit kinds |
| **STAMP-N9-ROWCOUNT** | N-9 test: un-selected stamps add zero store rows | S | stamped_asset_nodes_20260725 Phase 2 | Promotion gating tested; row count not asserted | N stamps materialized, one promoted -> exactly 1 new node_kind=stamp row |
| **STAMP-GIMBAL-TRANSLATE-LOCK** | Lock free translate for stamps in the viewport Move gimbal | S | stamped_asset_nodes_20260725 Phase 3 (task un-ticked 2026-10-10) | D-A5 ratified; gimbal_interaction.rs:267-273 resolves a stamp gimbal hit as SelectionKind::Node (verified) | Stamp + Tool::Move: no translate drag begins (resolve_operation table test with SelectionKind::Stamp); rotate/scale still apply as overrides |
| **SCULPT-DETERMINISM-TEST** | Determinism test for sculpt ops | S | sculpt_earthwork_regions_20260725 Phase 2 (task un-ticked 2026-10-10) | Plan requires it; ops are pure but untested for repeatability | Each op applied twice to identical input gives bit-identical heights and identical cut/fill |
| **VIS-PH2** | Visibility filters on remaining pickers + H/Shift+H/Alt+H + Isolate menu + Esc rung | M | hierarchy_visibility_groups_20260808 Phase 2 (D-7/D-8/D-12/D-17) | 18 decisions ratified; context_pick/pointer/sidebar_sync/path_segment_interaction never reference visibility; SoloSet has no UI | Hidden nodes / hidden GPX ribbons cannot be right-clicked, sidebar-selected into the gimbal, segment-picked or pointer-claimed; H/Shift+H/Alt+H; row Isolate/Hide others/Show all; Esc clears isolate first; per-picker tests + ladder test |
| **VIS-PH3** | Persist view.state / view.groups node properties | M | hierarchy_visibility_groups_20260808 Phase 3 (D-2, D-6) | visibility/mod.rs:7-8 session-only | Override survives restart via view.state; NodePropertiesLoaded test |
| **VIS-PH4** | Petal group registry column + authed Set/GetPetalVisibilityGroups + persisted fractal/verse eyes | L | hierarchy_visibility_groups_20260808 Phase 4 (D-1, D-3, D-15) | visibility_groups 0 hits in fe-database/fe-runtime | Viewer denied, Editor allowed; groups round-trip; fractal/verse visible flags persist (integration tests) |
| **VIS-PH5** | Groups right-sidebar section + unified layer registry | M | hierarchy_visibility_groups_20260808 Phase 5 (D-10, D-11) | RevealSection exists (actions/mod.rs:402); GPX/GeoJSON layer checkboxes inert | RevealSection{slug:groups} opens CRUD; terrain layers + groups share one registry; GPX/GeoJSON toggles live; slug + registry tests |
| **VIS-PH6** | REST + MCP visibility-groups endpoints | M | hierarchy_visibility_groups_20260808 Phase 6 | Depends on VIS-PH4; no route exists | Editor+ via fe-policy; uniform denial for foreign petal; integration tests |
| **UISHELL-SILENT-PICK-WARN** | Debounced warning when a path-like click no-ops while tracks load | S | ui_shell_architecture_20260724 Phase 0 FR-2 deferred | ui_ux.md §6 no-silent-failure ratified; needs TrackPickShape check | One warn per petal load for a TrackPickShape click while tracks_pending; never on ordinary clicks (pure-helper test) |
| **ROAD-PATH-KIND** | path_kind designed\|recorded provenance property | S | road_builder_ux_20260716 Phase 2 | Ratified 2026-07-16; PATH_KIND_KEY 0 hits | CreateTrack writes designed, GPX import recorded; gpx_bridge tests green |
| **ROAD-CORE** | Road builder input layer (snap core, Tool::Road, RoadPlace priority, ghost/guides/readout) | L | road_builder_ux_20260716 Phases 1,3,4,5 | Design user-ratified 2026-07-16; prerequisite pointer-ops manager landed a731369; reuse pen bezier authoring for curved mode | Spec AC tests: two clicks -> CreateTrack + 2 AppendPoints; undo removes last segment; snapped endpoint bitwise-equal; snap order vertex>intersection>guide>angle |
| **CROSS-TRACK-ANCHOR-SNAP** | Cross-track anchor snapping for Pen append/drag | S | fe-ui/src/node_manager/AGENTS.md:14-19 | Coordinate-sharing snap ratified in road_builder FR-5; share candidate collection with ROAD-CORE | snap_path_position considers other tracks' anchors within radius and copies coordinates exactly (unit test) |
| **FEUI-QUERY-REPLY-ROUTING** | Correlate GUI RawQuery replies (GIS panel vs inspector Query tab) | S | fe-ui/src/AGENTS.md:260-268 | correlation_id seam exists (DEC-C18) but GUI sends None; db_results/query.rs:18 routes by query_pending | Two in-flight queries each land in their own buffer (interleaved-reply test) |
| **WEBVIEW-IGNORED-ACTIVATION-TESTS** | Un-ignore two portal activation tests in a minimal Bevy App | S | fe-webview/src/petal_portal.rs:456,462 | #[ignore = needs full Bevy app] stubs; no design decision needed | Both tests run un-ignored and pass in cargo test -p fe-webview |
| **FE-GPX-HTTP-EXPORT-PARENT** | HTTP GPX export reads a never-written parent_node_id | S | fractalengine/src/AGENTS.md Residual 3 | fe-api/src/gpx.rs:237 reads n['parent_node_id'] (verified); writers use gpx_track_id | REST GPX export nests gpx_track_id-linked waypoints under their track (integration test) |

### NEEDS_RATIFICATION (47)

Blocked on a user/owner decision. Do not build until the decision is recorded (usually in `tracks/outstanding_decisions_20260715/spec.md` or a `decisions/` record).

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **CDL-R-ACCEPTED** | AeadSuite nonce-reuse capability, fail-closed amplifier, hazardous pub surface, CAS concurrency coverage | M | RUNBOOK a31c403 §10a residuals 3,4,5,7 | Recorded 'open by decision, not by oversight' | Owner decision per residual in decisions/canonical-data-log-20260808.md; accepted fixes land with regression tests |
| **CDL-SPEC9** | SPEC-9 peer N-of-M durability protocol | L | canonical_data_log_20260808 SPEC-9-durability | Availability floor unchosen; placement-index privacy gate | Owner picks the floor; spec with recorded privacy-leakage analysis |
| **CDL-PROV** | Ratify provisional CBOR key numbering | S | fe-canonical-log/src/AGENTS.md §Provisional wire numbering; D-CL24(3) | Provisional keys ship with no interop claim; three key-wrap rows UNRATIFIED | Each row ratified or replaced and moved to normative tables |
| **CDL-FORK** | SPEC-2 §4.6 rotation-fork resolver | M | D-CL24(2) | Deferred: forks retained with both successors inactive | Owner-approved rule, then resolver with both-successor fork tests |
| **CDL-WIT** | Canonical nano-unit s64 types over the plugin WIT | M | D-CL24(1) | Waits on a plugin-facing canonical API; plugin host not wired into any binary | Decision on the API; then WIT s64 fields with a round-trip test |
| **CANON-DUAL-EMIT** | Wire commit_operation_dual_emit into live handlers (SPEC-8 cut-over) | L | fe-database/src/migration/AGENTS.md §boundary (split from CANON-BOUNDARY) | Wave 4 brief forbids calling it; metadata: legacy path must keep working until SPEC-8 dual-emit equivalence is validated — needs an owner go after equivalence evidence | Equivalence evidence accepted by owner; then dual-emit wired per family with shadow-ledger parity tests |
| **CDL-ROTATION-ASSEMBLY** | Assemble/sign author-key-rotation-v1 payloads using the fe-identity provisioning ends | M | fe-identity/src/AGENTS.md §key-provisioning-tracking (split from CANON-SIGS) | SPEC-2 approved but no Workstream G wave brief schedules the rotation-op assembler | Scheduled by the owner; planned rotation end-to-end test (provision successor -> rotation admitted -> predecessor retired) |
| **CC-GATES** | Decide merge gates G-1/G-2/G-3 | S | crate_consolidation_r2_20260718 Phase 2 | No G-1..G-3 entries in conductor/decisions/ | Three dated ACCEPT/DEFER entries |
| **CC-EXEC** | Execute accepted merges + close-out | L | crate_consolidation_r2_20260718 Phases 3-4 | Blocked on CC-GATES | Per accepted gate: crate removed, consumers re-pointed, tests moved green, registry image builds if G-2 |
| **MYC-SECRETS** | Persist relay REST-created verse namespace secrets / invite-over-REST | M | p2p_mycelium_completion FUTURE-OPS; fractalengine-relay/README.md:41 | 'deferred, NOT scheduled'; workaround documented | After decision: replica re-opens after relay restart without env injection (test) |
| **P2P-REF-CONFINE** | Cross-verse referential confinement at the inbound admission gate | M | fe-database/src/handlers/replicated_row.rs:251-259 (new, consolidation 2026-10-10) | Documented known gap: an Editor admitted in verse A can push rows referencing verse B ids; fix 'once the verse-manifest/role bootstrap contract is specified'. Not on any prior ledger | Bootstrap contract decided; then an interior row referencing a foreign verse's petal/fractal is denied (two-peer harness test) |
| **TS-FR2** | Webview DNS-rebinding post-navigation check + oversized-content limit | M | thorns_shields FR-2; docs/webview-threat-model.md:17,21 | Both 'deferred to v2 (known gap)' | Owner pulls into v1 or confirms v2; if v1, rebinding to 127.0.0.1 blocked and oversized content refused (tests) |
| **OSS-5.3** | Sign-or-disclose choice for op-log signatures | S | oss_release_20260717 5.3 (REL-03) | D-CL24(5) makes legacy rows permanently unsigned; disclosure text in place (README, SECURITY.md) | Dated user resolution (recommended: disclosure + canonical-log pointer) |
| **OSS-5.5** | Ratify scoped CI test sweep or run cargo test --workspace on Linux | M | oss_release_20260717 5.5 (D-66); absorbs cross_platform_desktop Task 3.2 (XP-3.2) | build-artifacts.yml:118 tests only fe-ui, fe-terrain, fractalengine; Linux --workspace never run | D-66 dated decision; if widened, ubuntu-latest cargo test --workspace green |
| **SYNC-ACCEPTED-RESIDUALS** | Manifest re-emit on replica open, shard re-planning on churn, gossip join-retry monitor | M | fe-sync/src/AGENTS.md:260-264, :537, :760-763 | Each 'known and accepted', no trigger fired | Owner picks which to build; each gets a race regression test |
| **PLUG-HOST** | WIT bindgen + hosting fe-plugin in the shipped binaries | L | fe-plugin/src/AGENTS.md:132-134 | WIT aspirational; no binary depends on fe-plugin (README now says so); plugin system DORMANT per 2026-07-31 audit | Owner activates; bindgen host with fixture plugin round-trip + sandbox security review |
| **T5-TRIGGER** | Trigger-op endpoint (e.g. re-sculpt a region) | M | endpoint_api_surface_20260725 FR-3 | No trigger vocabulary specified | Ratified op list; endpoint + MCP tool emit the UI's DbCommand with identical op-log entry (test) |
| **HS-P6-CALLOUT** | CAD-style dimension callouts persistence | M | hexon_scale_orchestration Phase 6 | Blocked on D-11 | After D-11: callout round-trips through property store and re-renders on reload |
| **MSA-P4** | Terrain-height snap-on-place | M | map_scale_authority Phase 4 | Overlaps D-38 (Y=0 vs snap) | After D-38: sampler tests; elevated placement lands at surface y; terrain-less keeps y=0 |
| **EGR-MINT** | In-app share-link mint button (fe-ui -> fe-api client seam) | M | D-64; analytics_egress Task 4.2; fe-ui/src/panels/AGENTS.md:102-106; absorbs FEUI-INAPP-MINT | D-64 DEFAULTED to copyable curl; no HTTP client seam in fe-ui | If D-64 flips: button mints and shows URL; failure toasts; fake-client test |
| **EGR-WKB** | WKB local-frame axis order | S | bi_verification Known limitations | Contract change deferred until a GIS consumer forces it | Decided order documented; DuckDB spatial ST_X/ST_Y fixture |
| **EGR-F32** | f32 anchor quantization (~1 m) in latlon egress | M | bi_verification / D-54 | D-54 v1 limit; trigger sub-meter consumer | latlon export round-trips within 1 cm (test) |
| **T1-GC** | Tombstone GC/compaction | L | fe-entity-store/src/AGENTS.md §lifecycle; node_lifecycle Q-4 | Q-4 'unbounded for now'; needs merge-safety proof | Proof accepted; GC past horizon never resurrects (two_peer_tombstone_sync extension) |
| **FEAPI-ELEV-WRITES** | /query/elevated writes bypass the DB thread | M | mission retro 2026-10-10 | Remove vs route through DbCommand | User picks; if route, elevated write appears in node_log and replicates |
| **FEAPI-BUILDER-CRS** | TilesetBuilder stamps crs: None on fresh archives | S | mission retro 2026-10-10 | fe-terrain/src/tiles/builder.rs:266/:376 deliberate; semantic call | Decided CRS stamped; resolve_petal_crs returns it (test) |
| **DB-REVOKE-BCAST** | Broadcast session revocation to peers | M | fe-database/src/AGENTS.md §session-cache | Transport unsettled (libp2p kept per D-71 vs live iroh-docs) | Revocation on peer A evicts on peer B within TTL (harness) |
| **DB-TS-RETENTION** | iot_reading retention/GC | M | fe-database/src/AGENTS.md §replication-mode; D-5/D-7 | Deliberately not wired; D-5/D-7 open | Ratified policy enforced by GC; union-merge does not resurrect (test) |
| **SCULPT-STRATA** | Layered strata + survey-grade volumes | L | sculpt_earthwork_regions Q-3; fe-terrain/src/AGENTS.md §sculpt; absorbs TERRAIN-STRATA | Q-3: single-material now, strata a later phase; no go decision | User decides; new track defines strata model + tolerance |
| **UISEM-RULINGS-RECORD** | Record D-1..D-18 rulings in the unification spec | S | ui_semantics_unification spec §4 | Code cites outcomes; spec has no ruling table | User confirms shipped choices; dated ruling table |
| **UISEM-FMAX-LINT** | CI guard against f32::MAX inside egui .range( | S | ui_semantics_unification spec header | Labelled 'to consider' | If approved: CI step fails on a seeded violation |
| **INSPECTOR-FR3** | Hierarchy inspection (InspectedEntity) | L | inspector_settings Phase 3 | D-32 [USER]; zero code | After D-32 yes: petal/fractal/verse info in Inspector; routing tests |
| **INSPECTOR-FR4** | Access tab RBAC UI | L | inspector_settings Phase 4 | D-32/D-33; zero code | After decisions: role list, Owner edits via authorized DbCommand, Viewer read-only; tests |
| **UXQA-UNDO** | Undo/redo (first: Paths batch undo) | L | ux_qa_review findings [MAJOR] no undo | D-29 unresolved; no undo code | After D-29: Ctrl+Z reverts last Paths batch (PathOp inverse test) |
| **UXQA-MODIFIERS** | Unify Shift/Ctrl/Alt modifier semantics | M | ux_qa_review findings [MINOR] | No ratified convention table | Ratified table in ui_ux.md; shortcut tests assert it |
| **UXQA-ONBOARDING-FRAMING** | Plain-language framing for the hierarchy | M | ux_qa_review findings [POLISH] | User-owned UX track (D-29) | After scoping: first-run tooltips checked against ui_ux.md §9 |
| **FEUI-GLB-BROWSE** | Browse-and-ingest .glb in path-asset picker | M | fe-ui/src/panels/AGENTS.md:125-131 | Gated on D-36 ImportGltf quarantine (D-36 itself is overtaken; the quarantine lift is the remaining call) | After decision: local .glb ingests to blob:// and becomes the stamp asset |
| **FEUI-MANIFEST-SAVE** | PetalManifestSave only logs (silent failure) | M | fe-ui/src/actions/hexon.rs:239 TODO | No persistence target or route; relates to p2p_asset_streaming PetalManifest | Target decided; save persists, reloads on petal entry, failure toasts (DB round-trip test) |
| **FEUI-ANNOTATION-FORM** | Inline per-point annotation form | S | fe-ui/src/AGENTS.md:445-451 | New UI scope (D-29) | If scoped: values persist as gis.annotation.* |
| **WEBVIEW-OVERLAY-POSITION** | Model-anchored portal overlay | M | fe-webview/src/petal_portal.rs:290 TODO | Portal docks today; no decision to anchor | If wanted: overlay tracks projected AABB (projection unit test) |
| **OD-D-22-23** | iroh 1.0 timing + relay posture after EOL (TIME-CRITICAL) | S | outstanding_decisions D-22, D-23 | [USER] open; EOL 2026-12-31 | Dated resolution lines |
| **OD-EGRESS-IOT** | Egress limits, RBAC-on-results owner, IoT storage/ingest/caps, node_log retention, GeoParquet contract | S | outstanding_decisions D-3..D-8, D-54, D-56..D-59 | Unresolved; D-4/D-54 partly overtaken | Dated Resolved lines |
| **OD-SCALE** | CRS placeholder, callout persistence, scale-bar v1, placement Y=0 vs snap | S | outstanding_decisions D-9, D-11, D-60, D-38 | D-11 gates HS-P6-CALLOUT; D-38 gates MSA-P4 | Dated Resolved lines |
| **OD-FOUNDRY** | Foundry open/closed line + delta/asset/service/economics | S | outstanding_decisions D-12, D-13, D-15..D-20 | Blocks foundry-tagged specs | Dated Resolved lines |
| **OD-P2P-AUTH** | Relay WS mutation surface, SSO, BBR dep, bridge drop semantics | S | outstanding_decisions D-24..D-27, D-62, D-63 | Unresolved | Dated Resolved lines |
| **OD-UX** | UX/product surface defaults | S | outstanding_decisions D-28, D-30..D-35, D-37, D-39, D-64, D-65; absorbs UXQA-DECISIONS | Unresolved; spec.md:67-78, checklist :131-152 | Dated Resolved lines; tags flip to [RESOLVED] |
| **OD-TRACK-DISPO** | Deferred-track dispositions | S | outstanding_decisions D-40..D-45, D-47..D-49 | Kill/keep/merge calls unresolved | Dated Resolved lines; dispositioned tracks archived or re-scoped |
| **OD-RELEASE** | Artifact matrix, relay image, CI deviations, FE_KEYSTORE_BACKEND wire-or-drop | S | outstanding_decisions D-51, D-52, D-66, D-67 | Unresolved; macOS clauses moot after D-CI2; D-66 also tracked as OSS-5.5 | Dated Resolved lines |

### USER_GATED (15)

Needs the user's hands: in-app GUI checks, repo settings, pushing tags.

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **OSS-5.4** | Enable GitHub private vulnerability reporting | S | oss_release 5.4 | Repo-settings action | Security tab shows 'Report a vulnerability' |
| **OSS-5.6** | Publishing gate: repo settings, tokens, version tag | S | oss_release 5.6 (D-53); absorbs OD-D-53 | Rolling latest prerelease already publishes (D-CI3) | User records the D-53 resolution |
| **OSS-6.5** | Public visibility decision | S | oss_release 6.5 | Origin visibility unrecorded | User confirms visibility |
| **RCI-2.7** | Push a v* tag and verify the release pipeline incl. Windows ARM64 | S | release_ci Task 2.7; absorbs cross_platform Task 1.4 (XP-1.4) | release.yml triggers on v* only; no v* tag; checklist stale after 6aaeabd | Every release.yml job green (incl. windows-arm64), prerelease with SHA256SUMS, GHCR image pulls/runs; checklist re-derived |
| **XP-3.3** | Binary launch smoke test (Windows x64, Linux) | S | cross_platform Task 3.3 | Needs a display | Window opens, Bevy logs init, Ctrl+C exits cleanly |
| **SB-INAPP-VERIFY** | In-app verification of the 2026-07/08 editor builds | M | Spatial Builder Wave 0/1 + ui_shell (6 checkpoints) + pen_curve + ui_semantics | 2026-07-26 pass PARTIAL FAIL; repaired 631f389/4d0c12d; never re-verified | User confirms context-menu verbs, sticky sidebar, Settings/Maps sections, curve stamps + overrides, brush region with volume, pen handles, terrain tools without crash; findings filed |
| **T5-USER** | Context-menu Copy API / Report yields live data | S | endpoint_api_surface Phase 6 | In-app | Pasted string returns the node's live data |
| **HS-USER** | In-app check of scale bar + measurement tools | S | hexon_scale_orchestration | GUI | Scale bar matches a known distance on Zurich/Fuji; readouts plausible |
| **MSA-USER** | Manual 1:1 vs 1:100 placement check | S | map_scale_authority Phase 5 | Manual plan | Same glb sized correctly; 2 m cube reads 2 m |
| **MISSION-USER** | In-app terrain + scale bar for Zurich/Fuji, Hexon Manager, whole-build GUI smoke | S | mission_prodready retro | User-gated residuals | Outcome recorded in the track |
| **T1-USER** | In-app delete/cascade/promote flows | S | node_lifecycle_addressing | GUI | Parent delete with confirm, stamp promote, both survive restart |
| **INSPECTOR-PH1-MANUAL** | Manual portal URL restart round-trip + blocked-URL feedback | S | inspector_settings Phase 1 | Automated half covered | URL persists across restart; blocked URL rejected; empty clears |
| **FERENDERER-LAYER-FLOOR** | Camera height floor ignores layer-hidden terrain | S | fe-renderer/src/AGENTS.md:96-100 | Accepted 2026-07-18 with trigger 'if it annoys in-app' | Only if reported: hidden layer no longer floors camera (unit test) |
| **OD-D-29** | Hands-on UX-QA pass + ux_polish scoping | M | outstanding_decisions D-29 | User-owned GUI review | User logs findings.md |
| **OD-D-46** | Download-fix in-app verification | S | outstanding_decisions D-46 | Native save dialog | User confirms Download behaviour |

### NO_GO (3)

Explicitly prohibited until the owner lifts the canonical-log network gate.

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **CDL-NET** | Canonical-log network rollout, relay seeding, inbound P2P | L | canonical_data_log network_enablement | network_enablement 'prohibited'; owner approval required | None until the owner lifts the gate |
| **CANON-WS** | Mount /ws/canonical: production AuthorizationView, leaf_certificate_id, epoch-bump source | L | fe-api/src/canonical_ws/AGENTS.md | Mounting exposes the canonical path over the network (owner-gated). Pre-mount tests CDL-R6 / CDL-WS-ORACLE are doable now | Only after the owner approves canonical network exposure |
| **SYNC-SIGNED-PLANE** | Signed sync plane: relayed gossip attribution, __shards Editor+ gate, signed gimbal/editor ops over P2P | L | fe-sync/src/AGENTS.md §transport-admission-control items 2,5; node_manager/AGENTS.md:62-67; timeseries_fabric admission; absorbs SIGNED-OP-REPLICATION + TS-SHARDS-GATE | Not closable without per-op signing, which lives only in the canonical log; adopting it on the sync plane is canonical network rollout (owner-gated) | After owner approval: forged relayed frames and viewer-authored __shards rows rejected (harness tests) |

### OFF_STRATEGY_OR_SPEC_ONLY (16)

Reference only, or deferred by the 2026-07-14 alignment or a recorded condition. No action.

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **HDF-SPEC** | Hexon delta op-log format | L | hexon_delta_format_20260710 | FOUNDRY-CANDIDATE 2026-07-14; premises now delivered by canonical_data_log | None in this repo |
| **HPB-SPEC** | Hexon P2P bucket / 3D visual IPFS | L | hexon_p2p_bucket_20260710 | FOUNDRY-CANDIDATE; FR-5 overlaps PAS-P3 | None in this repo |
| **VS-SPEC** | Verse services accelerator-only plugin class | L | verse_services_20260711 | FOUNDRY-CANDIDATE | None in this repo |
| **IOT-4.2** | Poll-based external IoT connector catalog | L | iot_spatial_reporting Task 4.2 | D-6 DEFAULTED future work | Only if promoted |
| **FEAPI-ORACLE-RESID** | Tileset 'assigned to another petal' + node-wide crate catalog reveal | S | fe-api/AGENTS.md §existence-oracle | Documented inherent | None unless multi-tenant registry promoted |
| **FEAPI-DIRASSET** | Directory-asset returns 501 | L | fe-api/AGENTS.md §assets | Extension point for D-12/D-17 | Only after D-12/D-17 |
| **TERRAIN-OFFSTRAT** | Deferred rendering refinements | M | fe-terrain/src/AGENTS.md, mesh/, splat/ | Weighed and deferred; splat OFF-STRATEGY (D-43) | None unless revived |
| **SIM-RESID** | Sim HLC stamps, bounded(64) drop, step idempotency | S | fe-sim/src/AGENTS.md | Accepted residuals under lab doctrine | None unless a retrying client appears |
| **STAMP-CUSTOM-PIPELINE** | Custom instanced pipeline + criterion bench | M | fe-ui/src/verse_manager/AGENTS.md:312-317 | Conditional; FR-4 met via auto-instancing | Only if profiling shows 10k over budget |
| **SB-OUT-OF-PROGRAM** | Radial menu (D-A9) + live earthwork stack (D-A8) | M | decisions/spatial-builder-program-20260725.md | Out of program | None unless a polish track opens |
| **UISHELL-GUARD-GRANULARITY** | Per-section panic guard / panic=unwind | S | ui_shell_architecture notes (d) | panic=abort ratified 2026-07-25 | None unless reversed |
| **UISHELL-WELCOME-SPLIT** | Welcome vs inspector split | S | fe-ui/src/ui_shell/AGENTS.md:84-88 | Seam only | None unless scoped |
| **LIGHTBOX-ALL** | Lighting rig, presets, persistence, inspector | L | light_box_20260402 | OFF-STRATEGY; zero code | None unless re-prioritized |
| **PROFILE-ALL** | Profile UI + P2P profile sync | L | profile_manager_20260419 | OFF-STRATEGY; zero code | None unless re-prioritized |
| **SSO-ALL** | OIDC SSO | L | sso_federation_20260429 | spec_only, never scheduled | Security-review-gated plan if scheduled |
| **WEBVIEW-SERVO-TAURI-STUBS** | Servo backend body + Tauri command wiring | L | fe-webview/src/backends/servo.rs:16; tauri_commands.rs | No full Tauri shell scheduled | None unless scheduled |

### STALE_OR_OBSOLETE (18)

Superseded or already true. Most are one-line doc/TODO cleanups: batch them into one sweep.

| ID | Title | Size | Source | Evidence | Acceptance |
|---|---|---|---|---|---|
| **AP-3.5** | Causal-DAG membership ops + strong-removal resolver | S | auth_policy_pattern Task 3.5 | Superseded by canonical_data_log SPEC-2/3; D-CL24(5) | Owner closes 3.5 as superseded |
| **CANON-SIGS** | Placeholder op-log signatures on legacy rows | S | fe-database queries.rs etc. (reclassified from NO_GO 2026-10-10) | D-CL24(5): legacy op_log rows are permanently unsigned, no backfill; disclosure is OSS-1.5/OSS-5.3; rotation-op assembly split to CDL-ROTATION-ASSEMBLY | No code; keep disclosure register current (OSS-1.5) |
| **MYC-STALE** | Per-petal namespaces, transform gossip, per-petal topics, tileset P2P | S | p2p_mycelium_completion phases 2-5 | Superseded per-phase (metadata superseded_note) | Optional: IrohPetalReplicator (replicator.rs:1134) removed or documented legacy |
| **SYNC-STALE-TODOS** | Dead NodeTransformed TODO and stale 0700 TODO | S | fe-sync/src/status.rs:141, blob_store.rs:44 | NodeTransformed never emitted (verified); 0o700 code directly below the TODO (verified) | Comments removed/reworded; no behaviour change |
| **NET-STUBS** | Log-only libp2p gossip + iroh-blobs stubs | S | fe-network/src/gossip.rs:1, iroh_blobs.rs | Gossip real in fe-sync; fetch_asset referenced only by a comment (fe-renderer/src/loader.rs:11, verified) | Stubs deleted (keep verify_gossip_message test) or annotated superseded; builds |
| **XP-MACOS** | macOS builds + universal binary | S | cross_platform_desktop FR-2 | Owner removed macOS (6aaeabd) | Spec FR-2 marked descoped |
| **FEAPI-STALE-DOC** | Stale claim: verse/fractal tokens get NO row filter | S | fe-api/AGENTS.md:1101 | Superseded by M4 FROM-substitution (667aa18); line verified | Sentence points at §query-guard |
| **DB-TODO-QUERIES** | TODO permission check in seed-only create_petal | S | fe-database/src/queries.rs:13 | Only caller handlers/seed.rs:182 (verified) | TODO replaced by seed-only note |
| **DB-TODO-SCOPEMEMBERS** | TODO in uncalled get_scope_members | S | fe-database/src/role_manager.rs:172 | No callers (verified) | Dead fn deleted; builds |
| **DB-TODO-TOKENTEST** | Empty #[ignore] token round-trip test | S | fe-database/src/api_token_store.rs:224 | Covered by token_lifecycle_test.rs | Placeholder removed |
| **FEUI-IDENTITY-TODO** | Stale NodeIdentity TODO | S | fe-ui/src/plugin.rs:795 | fe-identity is a fe-ui dependency (Cargo.toml:29, verified) | Commented param deleted |
| **FEUI-TERRAIN-MODE-TODO** | Stale TODOs waiting on ProposalOp | S | fe-ui/src/panels/tool_panel.rs:134, :1016 | ProposalOp exists (terrain_proposal_state.rs:15, verified) | TODOs removed |
| **FEUI-ASSET-DOWNLOAD-TOAST** | Doc says AssetDownloadStatus is unread | S | fe-ui/src/AGENTS.md:182-190 | asset_card.rs:13 reads it (verified) | Doc points at asset_card.rs |
| **FEUI-SAVEURL-SILENT** | SaveUrl/OpenPortal silent no-op fragility | S | fe-ui/src/AGENTS.md:192-204 | Superseded by SaveUrlOutcome/OpenPortalOutcome toasts | Fragility list updated |
| **FE-GPX-RESIDUALS-1-2-4** | parent_node_id column, TrackRouteMap source, GpxImportStatus names | S | fractalengine/src/AGENTS.md Residuals 1,2,4 | Superseded by lifecycle parent_id, gpx_points route property, gpx_ops.rs:25 | Residuals marked superseded with citations |
| **STALE-1ULP** | 1-ULP legacy rows backfill | S | mission plan M3 deferred | None in prod | No action |
| **STALE-F24-RACE** | F24 hydration/drain startup race | S | mission plan M3 deferred #9 | Relay subscribes before DB spawn | No action |
| **OD-OVERTAKEN** | Ten register entries overtaken by later decisions/landed work | S | outstanding_decisions D-1, D-2, D-10, D-14, D-21, D-36, D-50, D-55, D-61, D-68 | Per the spec.md 2026-10-10 audit appendix (mission D1, DEC-C9, D-CI2, 623145f, 9c4da00/770acd3, etc.) | User confirms; each marked [RESOLVED] superseded-by with a date |
