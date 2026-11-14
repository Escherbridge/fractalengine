---
type: checklist
title: Round 4 QA checklist (public tracks)
updated: 2026-11-14
---

Compiled from 210 of 210 slices (round-4 swarm, now complete; nothing below has been run yet).
Tick a box only after you saw the stated result. Re-running the scribe adds new slices and keeps every ticked line as it is. `(derived)` = the worker left no QA line, so the step was written from the slice summary and files.

## 1. How to run

Three ways to verify. Tests and simulator runs only when you name them (AGENTS.md test policy); never `cargo test --workspace`.

- **In the app.** Stage a build and launch an isolated run with a seeded fixture (`basic` = offline terrain on Petal B, `terrain` = on Petals A and B):
  - `tools\qa\stage.ps1 -Jobs 2` then `tools\qa\launch.ps1 -Fixture basic` (add `-ApiPort N` to pick the loopback API port)
  - drive it with `python tools/qa/fe.py <command>` (screenshots, camera read-back, synthetic input; `--run <run>` binds a specific run); stop with `tools\qa\stop.ps1`
  - details: `tools/qa/AGENTS.md` (§fe-py, "Fixture modes and oracle")
- **API / simulator.** The API listens on the run's loopback port (owner token is read by `fe.py`; use it as a Bearer token for `curl`). Simulator: `python tools/qa/sim/run.py validate` (no app), then `python tools/qa/sim/run.py p2p-core --peers owner,editor --steps 0-3 --allow-stale-stage` (2-peer dry run; full coverage needs owner, editor, viewer, editor2). Scenarios live in `tools/qa/sim/scenarios/`.
- **Test batches.** `tools\qa\test_batches.ps1 -Batches R1,R8` (add `-ListOnly` to print the chunks without running cargo). Batch ids and filters are in `conductor/test-batches.md`; section 6 lists the ones this round touches.
- Kinds used below: `app` = in the GUI, `api` = REST/MCP/env/process, `sim` = simulator scenario, `batch` = named test batch.

## 2. Checks by track

### relay_operability_20261031
- [ ] NET-03 · api · (derived) Start the API with `FE_API_USAGE_LOG=<file>` set, call `GET /api/v1/health` with no token and `GET /api/v1/petals/{petal_id}/gis/nodes` with a token -> the file gains two JSON lines; both show the route template (`{petal_id}`, not the real id), the second shows the caller, the first none. (needs the binary wiring from I-BIN to be in)
- [ ] NET-06 · batch · (derived) Run `tools\qa\test_batches.ps1 -Batches R8` -> the `__relays` book tests pass: a row from a below-Manager author is held back (quarantined), a row whose `advertised_by` is not its signer is refused, a Manager+ row is admitted, demoting that Manager withdraws it.
- [ ] NET-09 · api · (derived) As verse Manager: `POST /api/v1/verses/{verse_id}/relays` with `{"url":"https://relay.example.org/"}` -> 201 `{relay_id,url}`; then `DELETE /api/v1/verses/{verse_id}/relays/{relay_id}` -> 204.
- [ ] NET-09 · api · (derived) Same POST as Viewer -> 403 before any URL check; as Manager with `"url":"relay.example.org"` (no scheme) -> 400; with no node signer configured -> 503.
- [ ] NET-04 · api · (derived) Start with `FE_API_METRICS_BIND=127.0.0.1:9100`, then `curl http://127.0.0.1:9100/metrics` -> OpenMetrics text with `fe_api_request_duration_seconds_bucket`, labelled by route template only.
- [ ] NET-04 · api · (derived) Request 50 different nonexistent URLs, fetch `/metrics` again -> they add at most one `<unmatched>` series (no per-URL growth). With `FE_API_METRICS_BIND` unset nothing listens on 9100, and the normal API port answers `/metrics` with 404.
- [ ] NET-07 · batch · (derived) In the same R8 run, `a_forged_shards_row_from_a_stranger_never_reaches_the_ledger` passes and its `__relays` leg shows the stranger's row not admitted while the owner's row is.
  - not finished: `fe-sync/src/lib.rs` has no `mod relay_advertise;` yet (the slice does not compile until it is added); the WriteRowEntry test leg and closing the book on `CloseVerseReplica` are not done.
- [ ] NET-08 · api · (derived) Two-peer verse where a Manager advertises `https://relay.example.org/`: with `FE_SYNC_RELAY_ADVERTISE_POLICY` unset (dial-only) the advertised relay never enters the node's relay map; with `pinned` it does while advertised and is removed after the tombstone; with `deny` it is never even listed.
- [ ] R-NET-2 · batch · R8 (cargo test -p fe-sync reaches relay_advertise::tests::relay_advertisement_book_journey and relay_reconcile planner test)
- [ ] R-NET-2 · sim · 130 non-Manager peers each write 9 __relays rows, then a Manager advert lands before its role grant -> after the grant the Manager relay is admitted; quarantine never exceeds 1024
- [ ] R-NET-2 · sim · FE_SYNC_RELAY_ADVERTISE_POLICY=pinned:<url> with a member advertising <url> whose host resolves to 10.x/127.x -> warn 'pinned advertised relay not inserted', relay map unchanged

### map_scale_authority_20260716
- [ ] CITY-02 · app · (derived) Launch `-Fixture basic`, open Petal B (map scale 1:100), import a plain GLB, open the new node in the inspector -> scale shows 0.01 / 0.01 / 0.01. Repeat on a petal with no map -> scale 1/1/1.
- [ ] CITY-02 · app · (derived) Create a node by the Add-node action on Petal B -> it is placed at the same 0.01 scale and still has that scale after you restart the run with `launch.ps1 -ResumeRun`.
- [ ] CITY-03 · app · (derived) Open Hexon Manager -> Installed, click "Set petal map" on a tileset, then reopen the petal -> the shown world scale is a value inside the tileset's scale bounds (never 0, NaN or a huge number), and inspector metre readouts for a node on that petal still convert correctly (a 100 m Measure tape reads 100.0 m).

### hexon_unification_20260716
- [ ] HEXON-02 · api · (derived) Build a hexon package, open it -> signature valid and integrity `Sealed`. Flip one asset byte, rename a manifest field, strip the seal, or swap the publisher DID in separate copies -> none of them reports a valid signature (a tampered sealed archive fails to open; an unsealed one opens as unverified).
- [ ] HEXON-03 · api · (derived) Against a dev registry, publish a built hexon with `RemoteRegistryClient::publish` (`POST /api/v1/hexons/publish`, raw bytes), search for it, download it -> found by id and tag, downloaded bytes are identical to what was published.
  - not finished: fe-hexon and fe-api do not compile until HEXON-04..07 rename `crate_id`/`HexonKind` users; `PluginHexonBuilder` still uses the zip 0.6 option type.
- [ ] HEXON-08 · batch · R11 -> petal_snapshot_round_trips_rows_and_oplog_order, empty_petal_snapshot_is_valid_and_lists_every_state_file, petal_snapshot_refuses_tampered_blob_wrong_type_and_descending_oplog pass.
- [ ] HEXON-04 · batch · R11 (`cargo test -p fe-hexon -- --skip ...`) reaches registry::tests::test_install_and_list, test_search_local and authz::tests::* (no new test functions).
- [ ] HEXON-04 · api · publish a hexon, then POST /api/v1/crates/{uri}/install?petal_id=A and petal_id=B as editor -> GET installed lists it for both A and B.
- [ ] HEXON-04 · api · DELETE /api/v1/crates/{uri}/uninstall?petal_id=A -> A no longer lists it, but B still does.
- [ ] HEXON-04 · api · as a viewer, POST /api/v1/crates/{uri}/install?petal_id=A -> refused, and nothing appears under A.
- [ ] HEXON-04 · api · publish hexon_id X with key A, then publish or install X signed by key B -> refused; the install route never accepts petal_id=global.
- [ ] HEXON-06 · api · POST /api/v1/crates/publish (owner, .hexon upload) then POST /api/v1/crates/{uri}/install?petal_id=P as editor -> 200 with hexon_id; GET /api/v1/crates/installed?petal_id=P lists it
- [ ] HEXON-06 · api · same install as a viewer token -> 'insufficient permissions'; install of an unpublished uri -> 'not found â€” publish it first'
- [ ] HEXON-06 · batch · R9 (cargo test -p fe-api --test api_integration filter) once I-API-2 adds the journey
  - not finished: asset_gc test blob_named_by_a_crate_entry_survives_its_asset_row CREATE statement is unaffected (no change made). / grep -i fecrate fe-api/src is now clean.
- [ ] HEXON-11 · batch · R11 fe-hexon filter -> snapshot_blob_stage test passes: missing and mismatched blobs refused, corrupted archive writes no blob, good archive round-trips rows.
- [ ] R-HEXON-1 · batch · R11 `cargo test -p fe-format -- --skip portal_bundle::` covers the HexonType names test and the sealed-snapshot legs; R11 fe-hexon filter covers discover::tests::test_search_by_kind.
- [ ] R-HEXON-1 · batch · R11-STORE `cargo test -p fe-hexon --features remote --test remote_registry_test` -> the publish leg passes once I-BATCH-1 registers the line.
- [ ] HEXON-10 · batch · petal_snapshot_test journey (filter above) -> export, import, refuse, HLC and refusal legs all pass
- [ ] HEXON-10 · api · POST /api/v1/petals/import-snapshot twice with the same archive (after HEXON-12) -> first creates the petal, second is refused as already existing, petal unchanged
- [ ] HEXON-10 · api · import an archive whose node row has elevation "high" -> request fails and GET of that petal_id finds nothing (no partial petal)
- [ ] HEXON-13 · batch · R10 -> petal_snapshot_export_then_import_journey passes including the FR-9 round-trip leg.
- [ ] HEXON-13 · batch · R11-STORE -> publish_accepts_and_indexes passes with the petal_snapshot search/download leg.
  - not finished: the worker reported partial without listing what is left (see its journal entry for HEXON-13)
- [ ] HEXON-12 · api · GET /api/v1/petals/{id}/snapshot.hexon with an Editor token -> 404 'not found or not permitted'. With a Manager token -> 200 application/x-hexon+zip named {id}.snapshot.hexon
- [ ] HEXON-12 · api · POST /api/v1/petals/import-snapshot (multipart field archive) with that file onto the same node as Manager -> 409 error 'refused'. The petal must be unchanged
- [ ] HEXON-12 · api · same import with an Editor token or a Manager token of another fractal -> 404. No new blob files appear under data/blobs
- [ ] HEXON-12 · batch · R9 (export_share_test::petal_snapshot_export_import_authz_journey)
- [ ] R-HEXON-2 · api · POST /api/v1/petals/import-snapshot with a snapshot whose node_id already exists in another petal -> 409 refused 'collide'; the other petal's node is unchanged.
- [ ] R-HEXON-2 · api · import a snapshot whose op-log entry has a scope under another fractal -> 400 refused 'invalid petal snapshot'; no petal row is created.
- [ ] R-HEXON-2 · api · import a valid snapshot exported from another node into an existing parent fractal as Manager -> 200 with petal_id; re-import -> 409 'petal already exists'.
- [ ] R-HEXON-2 · batch · fe-database petal_snapshot_test (pending I-BATCH filter); fe-hexon registry tests via `cargo test -p fe-hexon -- --skip entitlement_client::tests:: --skip entitlement_gate::tests::`.
- T-HEXON · partial, no checks of its own
  - not finished: HANDED-OFF (fe-hexon-registry owner, next round): S2 protocol version via X-Hexon-Registry-Protocol response header and /health protocol_version; not found in fe-hexon-registry/src. / HANDED-OFF (fe-ui owner, next round): S4 installed-preview / entitlement-required / entitled row states in fe-ui/src/dialogs/hexon_manager.rs. / HANDED-OFF (next round, fe-api owner): install_crate routing through the registry (fe-api/src/hexon.rs:309) and the end-to-end HexonBuilder to POST publish test; both unverified. / HANDED-OFF (I-LIBS-1, per R-HEXON-1 R1-09): remove zip 0.6 from fe-hexon/Cargo.toml; HexonPackage task left unticked until this lands. / HANDED-OFF (next round, fe-hexon-registry owner): FR-10 registry serves PetalSnapshot hexons (HEXON-13); no PetalSnapshot reference in fe-hexon-registry/src. / The stale '(AUDIT 2026-10-10: OPEN ...)' text on the fe-hexon manifest task is still in plan.md; the new tick supersedes it, but the note was not rewritten.

### p2p_asset_streaming_20260718
- [ ] NET-11 · api · (derived) Set `FE_MAX_DOWNLOAD_RATE=1000000` and download a ~5 MB tileset from a peer -> takes at least 5 s; unset, `0` or `abc` -> no throttling.
- [ ] R-NET-6 · api · GET /api/v1/network/diagnostics with a token scoped to one verse -> no other verse id appears in the body, and home_relay/relay_host show ipv4-literal for IP relays.
- [ ] R-NET-6 · app · open Peer Debug, click Refresh -> Network block shows; no IP address text; a hazard line shows a verse count, not ids.
- [ ] R-NET-6 · app · Settings > Network, tick 'Serve as a relay for my verses' -> consent sentence with Confirm/Cancel; Cancel leaves it off; disabled with a reason when not built.
- [ ] R-NET-6 · batch · R8 (tileset_transfer_test rate leg: two concurrent capped downloads take at least combined bytes/rate).

### per_key_clock_20261113
- [ ] CLOCK-02 · sim · (derived) Run `p2p-core` with revoke-then-regrant of the editor: an edit stamped before the revoke is kept; an edit stamped between the revoke and the regrant is refused on every peer; after regrant the editor can write again -> all peers show the same node values.
- [ ] CLOCK-02 · batch · (derived) `tools\qa\test_batches.ps1 -Batches R7,R10` -> role-grant window tests pass, including the changed assertion "X's row at 5 after revoke and re-grant is Editor, not None" and the new "at 10 is None" check.
  - not finished: `fe-database` does not compile until the NEEDS (CLOCK-03..) land; any older test that asserts "break restarts, old row refused" must be re-checked (R-CLOCK-1).
- [ ] CLOCK-04 · batch · R7 (`cargo test -p fe-database --test role_propagation_local_test`) reaches the extended journey; the role_grants.rs payload unit test stays in its existing unit filter.
- [ ] CLOCK-04 · app · Access panel, owner grants peer Manager at the verse, then revokes -> the role_grant row for that peer carries a `windows` JSON value with one manager window ending at the revoke.
- [ ] CLOCK-04 · sim · a remote grantor sends a revoke whose payload claims a Manager window older than this peer's stored grant -> the stored row has no such window; rows from that time stay refused.
- [ ] CLOCK-06 · batch · R10 filter `op_log::` reaches a_failing_statement_rolls_back_the_op_log_row_too -> _fe_clock own HLCs per key, the tombstone key, and the rollback leaving no row_field_version rows all pass.
- [ ] CLOCK-06 · sim · two peers, peer A edits a property then a node transform -> A's published node entry has `_fe_clock.own` with properties/<k> at the first edit HLC and transform at the second; `_fe_scene` is unchanged (v1).
- [ ] CLOCK-06 · app · delete a node in the editor with replication on -> the published row's `_fe_clock.own.tombstone.hlc` equals `_fe_scene.hlc`; no publication warning about the field clock appears in the log.
- [ ] CLOCK-05 · batch · R10 filter `handlers::replicated_row::` reaches verse_row_merge_updates_existing, deferred_row_converges_on_replay_after_role_lands and the ts_mode/default_access governance test.
- [ ] CLOCK-05 · sim · owner and Editor rename the same verse at the same HLC on two peers in opposite orders -> both peers show the same name (the larger author DID).
- [ ] CLOCK-05 · sim · Editor sends a clocked ts_mode change, then a verse Manager sends an older clocked ts_mode -> Manager's value shows; Editor's value never appears on any peer.
- [ ] CLOCK-05 · sim · peer whose Editor grant is only held renames a petal -> name does NOT change and no error; the version is stored (row_field_version) for re-judge.
- [ ] CLOCK-08 · sim · create a verse and fractal on peer A, rename the fractal -> peer B's replica shows the new name once the lib.rs publish_entity wiring lands.
- [ ] CLOCK-08 · batch · R1 (created_fractal_publishes_its_own_clocked_name) -> replicated row carries _fe_clock.own.name with local DID and positive hlc; unchanged re-publish emits identical own versions.
- [ ] CLOCK-07 · batch · R10 (op_log:: and handlers::replicated_row:: once registered) -> a_failing_statement_rolls_back_the_op_log_row_too and stale_live_row_cannot_resurrect_locally_tombstoned_node pass.
- [ ] CLOCK-07 · sim · grant B Editor, B edits a node, owner revokes B on peer 1 before B's row reaches peer 2 -> B's pre-revoke edit is present on both peers and survives restart.
- [ ] CLOCK-07 · sim · A and B edit different properties of one node at once while partitioned, then heal -> both properties present on every peer, whatever the delivery order.
- [ ] CLOCK-07 · sim · after B is revoked, B publishes another edit -> peers keep the value B wrote before the revoke; the new value never appears.
- [ ] CLOCK-07 · sim · Editor A deletes a node while B edits its name later -> the node stays deleted on every peer (B's edit never brings it back).
- [ ] CLOCK-07 · app · owner revokes a member in the Access panel -> nodes the member edited while an Editor keep those edits in the viewport and inspector (no rollback).
- [ ] R-CLOCK-1 · batch · R7 (cargo test -p fe-database --lib -- role_grants; --test role_propagation_local_test) covers the proof row and the empty-frontier journal
- [ ] R-CLOCK-1 · app · Access tab, revoke an Editor who has never edited -> role_grant row for that member has no windows column; a late edit by that member is refused
- [ ] R-CLOCK-1 · sim · Editor edits node, Manager revokes after receiving it, a late edit stamped before revoke arrives -> first edit stays, late edit is refused on every peer
- [ ] CLOCK-09 · batch · R1 visibility_groups_test -> published registry petal row carries _fe_clock and peer store converges to the registry.
- [ ] CLOCK-09 · api · PUT /api/v1/petals/{p}/terrain then replicate -> the published petal row carries _fe_clock; a write by a non-Editor is still refused.
- [ ] CLOCK-09 · batch · R10 handlers::verse_settings -> set timeseries settings and default access re-emit the verse manifest with _fe_clock.
  - not finished: Deferred: petal_terrain/render_distance/verse_settings unit-test assertions on key versions were not added (no file access to their test modules beyond owned files; only the visibility_groups journey was extended).
- [ ] CLOCK-11 · batch · R8 (`cargo test -p fe-sync -- --skip docs_engine::tests::a_hash_with_no_tileset_context_is_unaffected_by_the_filter`) reaches the extended journey. The test returns early without UDP.
- [ ] CLOCK-12 · sim · run p2p-core and read step 14p -> clock_owner_key and clock_editor2_key are present on all 4 peers, clock_same agrees on all peers, and the petal reads 'Swarm Fractal/A' with no 'B' anywhere.
- [ ] CLOCK-12 · sim · run p2p-core and read steps 13a and 13b -> N4 after_restart_rest and after_restart_mcp stay equal to 1 on every peer for 40 s after each revoke, and N2's transform agrees on all peers.
- [ ] CLOCK-12 · sim · run p2p-core and read steps 5 and 6 -> N1 position X and N2 position agree on every peer, with no named winner asserted.
- [ ] CLOCK-10 · batch · R10 -> deferred_row_converges_on_replay_after_role_lands passes: the rename is held, a Manager grant makes it the value, a late revoke reverts it to P.
- [ ] CLOCK-10 · sim · p2p-core with a held Editor grant whose Manager grant arrives later -> the Editor's earlier petal rename appears on every peer without a re-send.
- [ ] CLOCK-10 · app · owner revokes an Editor in Access panel after they renamed a fractal -> sidebar name reverts to the pre-edit value; it must NOT keep the revoked edit.
- [ ] CLOCK-10 · app · owner grants Editor to a member whose rename was refused earlier -> the rename appears in the sidebar without restarting the app.
- [ ] R-CLOCK-2 · sim · p2p-core step 13b -> pre_revoke2 on N2 (editor2 wrote it in 13a) stays 1 on every peer for 40 s after revoke2; it must NOT disappear.
- [ ] R-CLOCK-2 · batch · R4 handlers::petal_terrain -> set_petal_terrain_replicates_and_a_peer_converges_on_it passes; the published row carries _fe_clock.own.terrain by owner-did.
- [ ] R-CLOCK-2 · batch · R10 handlers::replicated_row:: -> deferred_row_converges_on_replay_after_role_lands passes (the late revoke demotes the petal name to the legacy floor).
- [ ] R-CLOCK-2 · app · owner revokes an editor in verse Options > Access -> that editor's petal renames made while still Editor stay; renames after the revoke are gone.
- CLOCK-13 · partial, no checks of its own
  - not finished: SEC-INBOX: no inbox entry names field_clock/AGENTS.md or merge.rs in my read; the grant-window steer (entry 4, fe-policy) is outside my owned files.

### road_builder_ux_20260716
- [ ] CITY-08 · app · (derived) Import a GPX file, then draw a track by hand; open each track node's property list -> the imported one has `path_kind = recorded`, the hand-drawn one `path_kind = designed`; both keep their existing gpx_type, gpx_points and name properties.
- [ ] CITY-09 · app · (derived) Once the Road tool is wired (CITY-11/13), place three points on one road -> one track node is created after the first point and the other two points are appended to it in click order (reload the petal and compare).
  - not finished: the ordered-flush test is not written; `fe-ui` does not compile until `path_point_interaction.rs` is fixed.
- [ ] CITY-10 · app · (derived) With edit rights, press L in the viewport -> the toolbar highlights the new Road tool and the viewport hint changes; with Viewer rights, L does nothing.
- [ ] CITY-10 · app · (derived) With Road active, click an existing node and then empty ground -> the selection does not change either way (Road never selects or deselects).
- [ ] CITY-05 · app · (derived) Launch `-Fixture basic`, pick Pen, click on a raised part of Petal B's terrain -> the anchor point sits on the surface (not underground); on a petal with no terrain the anchor is at height 0.
  - not finished: `mod ground_snap;` is not yet declared in `fe-ui/src/node_manager/mod.rs` (CITY-15), so this does not compile yet; a Pen press-drag on a slope may count height in the drag threshold (not checked).
- [ ] CITY-11 · app · Press L for the Road tool in a petal, click twice on terrain in Straight mode -> a track named 'Road 1' appears with 2 points and the Paths tab opens it for editing.
- [ ] CITY-11 · app · continue clicking, then press Backspace -> the last segment's points are removed. Press Escape -> the pending segment clears, a second Escape ends the chain.
- [ ] CITY-11 · app · click within the snap radius of another track's endpoint -> the new point lands exactly on that endpoint. Hold Alt while clicking -> no snap.
- [ ] CITY-11 · app · with a terrain map loaded, click on a hill -> the road point height follows the terrain surface; without terrain it is Y=0. Leave the petal or switch tools -> the chain clears.
- [ ] CITY-11 · app · with Road active, click an existing track ribbon -> it is NOT selected (the Road tool owns the press). Click twice quickly right after the first click -> no duplicate track is created.
- [ ] CITY-11 · batch · R5, after the filter gains `node_manager::road_interaction`.
- [ ] CITY-12 · app · Press L, click a start point then move the cursor over terrain -> an accent ghost line follows with a cursor label like '12.4 m'; after the first segment is committed the label also shows 'Â· total X'.
- [ ] CITY-12 · app · In Curved mode click start then apex -> ghost becomes a live curve; with angle snap on near 45 degrees the label appends the degrees and a tick shows at the cursor.
- [ ] CITY-12 · app · Hover near another track endpoint or guideline -> ring on the vertex and dashed guide lines appear; with no active petal or cursor on the anchor the ghost and label turn red; on a map-less petal the label reads 'N.NN world units'.
- [ ] CITY-12 · batch · R5 (filter node_manager::road_overlay) once registered by I-BATCH.
- [ ] CITY-13 · app · Press L, open Tools panel Options, click Straight/Curved/Freeform -> the selected mode highlights and matches keys 1/2/3.
- [ ] CITY-13 · app · Set Snap radius to 999 and Curve samples to 1 in the Road options -> the fields clamp to 50 and 4.
- [ ] CITY-13 · app · With Road active and no chain, open the panel -> Undo segment and End road are disabled and Chain length reads 0. After placing two clicks, both enable and the length matches the on-canvas readout.
- [ ] CITY-13 · app · Toggle Angle snap off -> the 45/90 buttons grey out. Click End road mid-chain -> the chain closes and the length resets to 0.
- [ ] CITY-13 · batch · R6 (panels::tool_options) -> dispatcher_picks_one_arm_per_tool passes with the road settings assertions.
- [ ] R-CITY-2 · app · Road tool (L), Curved mode, place 3 points then a curve, press Backspace -> every curve point disappears from the track, one save toast at a time, no 'Saving the previous edit' advisory.
- [ ] R-CITY-2 · app · Road tool, draw two segments, Options > Undo segment -> last segment removed from the track and Chain length drops; button disabled before the first segment is saved.
- [ ] R-CITY-2 · app · Road tool, Freeform mode, first press-drag-release on empty ground -> a road is created along the stroke (the first drag is not lost).
- [ ] R-CITY-2 · app · Road tool active, hold A -> camera flies left and the Angle snap checkbox does NOT change.
- [ ] R-CITY-2 · app · Road tool on a petal with a map, hover near another track's end -> the snap ring is about half the snap radius in metres, not huge or tiny.
- [ ] R-CITY-2 · batch · R5 (once the road filters are added), R2 (dispatch), R6 (panels::tool_options).

### peer_networking_support_20261031
- [ ] NET-17 · api · (derived) With the `relay-server` feature, start the embedded relay with `FE_RELAY_SERVE=1` and `FE_RELAY_SERVE_ACCESS=open` -> it refuses to start with a named error; with `FE_RELAY_SERVE_METRICS_BIND=0.0.0.0:9200` -> refuses (`metrics_not_loopback`); with hosted verses unset -> refuses (`hosted_verses_unknown`).
  - not finished: process wiring (NET-24/NET-30) is not in; the three journey tests were never run.
- [ ] NET-17 · app · (derived) With the relay running for one verse: a member peer connects through it; a non-member is denied; removing the member's role disconnects them within about a second, even if they reconnect at the same moment.
- [ ] NET-24 · batch · cargo test -p fractalengine-relay --features relay-server -- relay_serve -> refused_configs_exit_with_named_reason_and_bind_nothing passes (no_cert_mode, unknown_cert_mode, open_access_refused).
- [ ] NET-24 · sim · start fe-relay built with relay-server and FE_RELAY_SERVE=1 but no FE_RELAY_SERVE_CERT_MODE -> process exits non-zero with error naming no_cert_mode and binds no port.
  - not finished: QUIET batch R8: cargo test -p fractalengine-relay -- relay_serve -> relay_binary_binds_no_relay_listener_without_opt_in passes (listener count 0).
- [ ] NET-14 · batch · R8 -> relay_advertisement_rows_and_urls_table passes; signed rows with a tampered url, expired, over-TTL, wrong verse, http, 10.0.0.5 and :8443 are each refused with the named AdvertRefusal.
- [ ] NET-13 · batch · R8 (fe-sync line, or filter gossip_auth::tests::) -> signed_frame_round_trips_byte_exact, tampered_..._named_reason and replayed_or_stale_... pass
- [ ] NET-15 · batch · R8 -> relay_trust_coverage_bound_and_filter_journey passes
- [ ] NET-19 · batch · run R8 -> prober_refuses_private_loopback_mixed_record_rebinding_and_redirect_targets passes (private, loopback, mixed, rebinding, URL-policy rows refused; second probe by same requester rate-limited)
- [ ] NET-19 · sim · have a verse member advertise a relay whose DNS answers one public and one 127.0.0.1 address -> dial-back is refused, no record signed, no connection attempted to either address
- [ ] NET-25 · api · POST /api/v1/verses/{id}/relays with url https://relay.example.org and expires_at_ms 1 hour ahead -> 201, and the __relays row carries sig, verse_id and expires_at_ms.
- [ ] NET-25 · api · the same POST with http://, a private IP literal, no expires_at_ms or an expiry more than 30 days ahead -> 400 each, nothing replicated.
- [ ] NET-25 · api · POST with peer_run=true and no dialback record -> 400; with a fresh third-party dialback record for this DID and URL -> 201.
- [ ] NET-25 · batch · R9 (once registered) reaches the new relay_advertise journey test.
- [ ] NET-23 · batch · R8 -> endpoint_constructs_and_has_node_id passes; a peer address carrying a stray relay URL is stored with only its direct IP
- [ ] NET-23 · sim · add a peer address naming a relay outside FE_SYNC_RELAY -> debug log shows a stripped count without peer IPs and the address book entry holds no relay
  - not finished: RESIDUAL for AGENTS.md: this is an input filter on the address book only, not an egress allowlist; per §relay-routing-contract iroh may still reuse other routes (e.g. a relay learned from a connecting peer or a home-relay path)
- [ ] NET-21 · batch · R8 (cargo test -p fe-sync -- sharding::tests::a_petal_scoped_viewer_is_not_a_verse_wide_blob_viewer) -> passes.
- [ ] NET-21 · sim · host with relay-server on and verse default_access viewer; non-member dials the relay -> denied not_member, must NOT be admitted.
- [ ] NET-21 · sim · owner grants Viewer to a fresh peer that never published __peers -> its relay connection is admitted (no bootstrap deadlock).
- [ ] NET-21 · sim · owner revokes a connected Viewer -> relay evicts it within 5 s and reconnects are denied until re-granted.
- [ ] NET-16 · batch · R8, `cargo test -p fe-sync --lib -- relay_reconcile::tests` (both property tests plus the AC-17 journey)
- [ ] NET-16 · sim · after NET-22, 2 peers in one verse whose Manager advertises public relay R -> R becomes home, n0 leaves the map; join a verse without R -> n0 back in one pass
- [ ] NET-16 · app · after NET-22, turn off Settings auto-home relay while R is home -> R drops to dial-only and the n0 relays return; the configured relay must never be removed
- [ ] NET-20 · batch · run R8 batch -> diagnostics_redact_ips_by_default and remote_info_active_addrs_map_to_direct_relayed_mixed_unknown pass
- [ ] NET-22 · batch · R8 (cargo test -p fe-sync) reaches relay_advertisement_book_journey (signed/require/bound/expiry segment) and resolve_precedence_table (signed-mode parse rows).
- [ ] NET-22 · sim · three verse members, a Manager posts five signed adverts through POST /api/v1/verses/{v}/relays -> peers list only the four oldest; deleting one surfaces the fifth.
- [ ] NET-22 · sim · set FE_SYNC_RELAY_ADVERT_SIGNED=require on one peer and write an unsigned __relays row -> that peer never lists it; a compat peer does.
- [ ] NET-22 · sim · pin an advertised relay (FE_SYNC_RELAY_ADVERTISE_POLICY=pinned:<url>), then stop that relay -> after about 90 s the log says it was removed after failed health checks; a Configured relay is never removed.
- [ ] NET-22 · sim · a node whose home is a pinned advertised relay opens a second verse that does not advertise it -> one warn names that verse; it must not repeat every tick.
- [ ] R-NET-4 · batch · R9 fe-api filter relay_advertise_row_is_v1_signed_with_bounded_expiry_and_dialback -> passes incl. throwaway-prober and loopback/metadata/localhost refusals
- [ ] R-NET-4 · batch · R8 fe-sync filters relay_advertise, a_quarantine_flood_cannot_lose_a_legit_editors_shards_row, resolve_precedence_table -> pass
- [ ] R-NET-4 · api · POST /api/v1/verses/{v}/relays with peer_run=true and a dialback signed by a key with no role in v -> 400 'dialback prober is not a member', no row replicated
- [ ] R-NET-4 · api · POST /api/v1/verses/{v}/relays url=https://169.254.169.254 or https://localhost -> 400 before signing; nothing appears in __relays
- [ ] R-NET-4 · sim · two peers, Manager advertises a relay with expires_at 2 min ahead -> within ~30 s after expiry the relay leaves the other peer's relay map
- [ ] NET-26 · batch · R8 -> relayed_request_falls_back_to_claim_admission and peer_declaration_round_trips_through_the_doc_row pass
- [ ] NET-26 · sim · two upgraded peers on one verse run a distributed query -> answers arrive; debug log shows no 'refused at the signed-frame ingress' for either peer
- [ ] NET-26 · sim · set FE_SYNC_GOSSIP_SIGNING=require on one peer, keep an older unsigned peer -> its queries are refused (LEGACY_REFUSED); signed peers still answer
- [ ] NET-26 · app · publish a tileset on peer A, open the same verse on peer B -> B's Hexon Manager lists A's ad, attributed to A's DID even when relayed
- [ ] NET-28 · batch · run batch R8 -> shutdown_command_terminates_thread passes: GetNetworkDiagnostics returns relay_mode 'disabled' and zero peers, and Stopped still follows.
- [ ] NET-28 · api · once the API wiring exists, call the network diagnostics route with detail unset and then detail=true -> default peer addrs read like 'ipv4:ephemeral', detail shows full socket addresses.
- [ ] NET-28 · sim · connect two peers over a relay only and request diagnostics -> the peer row kind is 'relayed' and node.downgrade_hint is set; with a direct path the hint is absent.
- [ ] R-NET-3 · sim · peer advertises relay R via a Manager advert, then the advert is withdrawn -> peer's stored address no longer lists R (no dial to R)
- [ ] R-NET-3 · api · POST /api/v1/verses/{verse_id}/relays with url https://relay.internal -> 400 refused (no signed row written)
- [ ] R-NET-3 · batch · R8 (fe-sync endpoint, relay_reconcile, relay_trust tests); fe-runtime relay_advertisement::tests:: once I-BATCH adds the filter
- [ ] R-NET-3 · app · run with FE_SYNC_RELAY=disabled in a verse whose Manager advertises a relay -> no relay listed as known or dialled in network diagnostics
- [ ] NET-27 · batch · R8 virtual-transport gossip journey must show the sealed frame delivered unchanged and admitted once the NEEDS test edit lands.
  - not finished: the worker reported partial without listing what is left (see its journal entry for NET-27)
- [ ] NET-29 · batch · R8 -> dialback_responder::tests journey passes: undeclared and sub-Viewer requesters refused, Viewer gets a signed record, second probe in a window rate limited
- [ ] NET-29 · sim · two peers in one verse, one with act_as_prober on; the other calls request_dialback(relay url) -> a signed ok record comes back; with act_as_prober off, or from an undeclared peer, nothing is answered and the call returns None after the timeout
- [ ] NET-31 · app · open Peer Debug dialog and click Refresh -> Network block fills with relay mode, home relay, health, port mapping, reachability, relay server; peer table shows Direct/Relayed/Mixed/Unknown rows and NO IP addresses anywhere
- [ ] NET-31 · app · open Peer Debug before sync is online (no sender) -> Refresh is disabled and 'No network snapshot yet' shows
- [ ] NET-31 · batch · R6 (dialogs::peer_debug) reaches network_panel_renders_fixture_without_ips
- [ ] NET-32 · api · GET /api/v1/network/diagnostics with a viewer token scoped to one verse -> only peers sharing that verse appear, each with only that verse id, addresses like ipv4:ephemeral and no raw IPs.
- [ ] NET-32 · api · same GET with an Owner token while FE_API_NETWORK_DETAIL is unset -> addresses stay redacted; with FE_API_NETWORK_DETAIL=1 -> full ip:port, still only for readable-verse peers.
- [ ] NET-32 · api · GET on a host with no diagnostics bridge wired -> 503; MCP get_network_diagnostics returns the same confined result as REST.
- [ ] NET-32 · batch · R9 (fe-api --lib `network::`) reaches api_diagnostics_hides_peers_from_unreadable_verses_and_redacts_ips.
- [ ] NET-30 · app · Settings sidebar > Network, tick 'Serve as a relay for my verses' -> toggle is disabled with a reason line unless the build has relay-server and the self-test verdict is public.
- [ ] NET-30 · app · with the toggle enabled, tick it -> a consent sentence with Confirm and Cancel appears; Cancel leaves it off, Confirm turns it on and shows 'Applies after restart'.
- [ ] NET-30 · app · edit settings.ron to remove the three fields and relaunch -> 'Use verse relays as home' is on; the other two are off.
- [ ] NET-30 · batch · R6 (settings::, ui_shell::) and R8 relay_serve_bridge cover the new tests.
  - not finished: Tests: saved_settings_survive_a_restart now round-trips the three fields. partial_file_fills_missing_fields_from_defaults checks their defaults and relay_serve_blocker rows. relay_toggle_is_inert_without_feature_and_refuses_without_passing_self_test is a table
- [ ] NET-33 · batch · R8 peer_relay_e2e: run `cargo test -p fe-sync --features relay-server -- peer_relay_e2e` -> all 4 journeys pass: a row crosses the relay, a stranger gets not_member, a revoked member is gone in 5 s or less, only the valid gossip frame is admitted.
- [ ] NET-33 · sim · relay-only pair reaches the in-process relay and diagnostics collect_peer_rows -> kind Relayed with relay_host equal to the relay host and no IP addresses (peer_relay_e2e, first journey).
  - not finished: the worker reported partial without listing what is left (see its journal entry for NET-33)
- [ ] R-NET-5 · batch · R8 - cargo test -p fe-sync -- gossip_auth::tests dialback_responder::tests distributed_query::tests::relayed_request_falls_back_to_claim_admission
- [ ] R-NET-5 · sim · restart one peer mid-run and replay a captured pre-restart signed frame -> refused with 'predates this session'; new frames from all peers still admitted.

### hexon_scale_orchestration_20260712
- [ ] HEXON-19 · app · (derived) Launch `-Fixture basic`, draw a 100 m Measure tape on Petal B and promote it to a dimension callout -> a "100.0 m" label is drawn at the callout in the viewport, moves with the camera, and disappears when the callout is off-screen or behind the camera; the new node is named "Dimension 100.0 m".
- [ ] HEXON-19 · app · (derived) Do the same on a petal with no map -> the label reads in world units (no NaN).

### extension_platform_20261107
- [ ] EXT-08 · batch · run python tools/qa/docs_link_check.py from the repo root -> prints '<n> documents, 0 failures' and exits 0; any dead link or unlinked fence prints file:line
- [ ] EXT-14 · app · launch with FE_PEER_LABEL=A, open a verse then a petal -> title reads '[A] <petal> Â· <verse> â€” FractalEngine'; with no env var the label prefix is absent.
- [ ] EXT-14 · app · navigate back out of the petal and verse -> title degrades to '<verse> â€” FractalEngine' and then 'FractalEngine'.
- [ ] EXT-15 · sim · run scenario fleet-terrain on the terrain fixture -> three Road nodes and gpx_points on every peer, mover0..2 transforms agree on every peer for 120 s, no 504 notice.
- [ ] EXT-10 · batch · R3 -> the ('m', 2) measurements row returns values [11.0, 12.0] (newest two, ascending) with truncated true; ft and uncapped rows unchanged.
- [ ] EXT-10 · app · Inspector > Data > Measurements on a node with more than 1024 readings -> chart ends at the newest reading and the label reads 'Showing the latest 1024 points.'
- [ ] EXT-02 · batch · R11 batch (conductor/test-batches.md) -> blob_mirror_tap test passes: one job for the committed import; legacy, failed, non-blob and missing-file inputs queue none.
- [ ] EXT-02 · app · import a .glb into a petal that has a blob.mirror binding installed -> the mirror guest is called once with that blob; a failed import produces no call.
  - not finished: the worker reported partial without listing what is left (see its journal entry for EXT-02)
- [ ] R-EXT-1 · app · Maps > Install from file with a legacy terrain .hexon -> Error notice 'Could not install <name>.hexon: this is a legacy terrain hexon...'; no full path in the subject
- [ ] R-EXT-1 · batch · R4 `terrain_bridge::` (local install journey and seeding/download journeys must not panic on a missing Time)
- [ ] EXT-11 · batch · run batch R3 -> the measurements leg passes, including the two-row petal readings summary and the foreign-scope refusal
- [ ] EXT-03 · batch · R11 (filter `extension_host::` reaches extension_host::registry_offers::tests::registry_offers_journey_offered_unlisted_killed_refused_cached).
- [ ] EXT-03 · app · bind a hook to a package not installed but listed by a configured source; open Extensions > Bindings, wait ~20 s, Refresh -> row shows an Install action.
- [ ] EXT-03 · app · same binding, but the source lists nothing for the package, or the build is on the kill list -> row shows its state with NO Install/Update button.
- [ ] EXT-12 · app · open Data window on a petal receiving readings, pick Readings tab -> grid (Node, Metric, Value, Units, Age, Per min) refreshes every ~2 s with no click, newest first.
- [ ] EXT-12 · app · tick 'Pause updates' -> rows stop changing; untick -> updates resume.
- [ ] EXT-12 · app · click a node name in the Readings grid -> node is selected and the camera frames it.
- [ ] EXT-12 · app · switch the active petal with the tab open -> old rows clear; with no recent readings the tab shows 'No readings in the last minute.'
- [ ] EXT-12 · batch · R3 (gis:: filter) -> readings route test: own id accepted, foreign id ignored, RequestFailed with own id sets error.
- [ ] EXT-13 · app · GIS panel, Export tab, Copy for BI, click Readings -> a snapshot time shows and Readings Parquet URL, CSV URL and DuckDB rows appear; the URLs end with &as_of_ms=<n>.
- [ ] EXT-13 · app · click Refresh snapshot -> the as_of_ms in the copied URLs increases; the share section shows 'Parquet/CSV links need a petal-scoped token.'
- [ ] EXT-13 · api · GET /api/v1/petals/{p}/export.csv with an iot_reading query and as_of_ms, twice while posting new readings -> identical body and ETag.
- [ ] EXT-13 · api · GET export.parquet with a node query and as_of_ms=1 -> 400 'as_of_ms applies to iot_reading exports'; no rows returned.
- [ ] EXT-13 · batch · R9 export tests and R3 gis:: cover the ordering/cutoff and readings SQL/URL rows.
- [ ] R-EXT-2 · app · Extensions > Browse a source while one configured source is offline -> listing appears without waiting on the offline source
- [ ] R-EXT-2 · app · Bindings view with a binding naming an uninstalled package a source lists -> Install offer shows; kill-listed or unlisted -> no Install control
- [ ] R-EXT-2 · app · Store details > Check licence (entitlement build, pinned issuer) -> Held with an expiry; issuer 403 -> Missing; no issuer pinned -> not-available
- [ ] R-EXT-2 · batch · R11 (`extension_host::` line) reaches registry_offers::tests and blob_mirror_tap::tests; R11 fe-hexon line reaches the package.rs tamper table
- [ ] R-EXT-3 · app · Data window > Readings tab with a node emitting m and ft for one metric -> two rows with correct units, values never mixed
- [ ] R-EXT-3 · app · Export tab > Copy for BI > Readings -> label reads 'Snapshot as of YYYY-MM-DD HH:MM:SS UTC'; Refresh snapshot moves it to now
- [ ] R-EXT-3 · api · GET /api/v1/petals/{p}/export.parquet with readings SQL and same as_of_ms twice while readings arrive -> identical bytes and ETag
- [ ] R-EXT-3 · batch · R3 (entity_property_test petal readings leg, gis::) and R9 (export:: readings_snapshot_is_ordered_and_cut_at_as_of)

### plugin_host_revival_20261031
- [ ] EXT-16 · app · Maps > Install from file, pick a legacy hexon with hexon_type terrain -> one error notice 'Could not install <file>: this is a legacy terrain hexon; re-export it as terrain_tileset', no full path shown
- [ ] EXT-16 · app · Maps > Install from file, pick a tampered sealed .hexon -> an error notice naming the file and the refusal reason; nothing installed
- [ ] EXT-16 · batch · R4 (terrain_bridge:: filter) covers the tampered leg asserting one Error notice
- [ ] EXT-17 · app · open the Data window on a petal with no map (docs/images MANIFEST recipe 'export1') -> the Data window covers the 'world units (no map scale)' label, which does not paint over it.
- [ ] EXT-17 · app · on a petal with a map, orbit the camera -> the scale bar still shows in the viewport corner and a floating window dragged over it hides it.
- [ ] EXT-04 · batch · run the `--features remote` remote_registry_test line once I-BATCH-1 registers it -> the sealed plugin is listed once, header and fetch verify, the renamed copy and the killed hash are refused.
- [ ] EXT-04 · batch · R11 fe-hexon package tests (`test_tampered_package_fails_signature`) -> every tamper row is refused, including the two removed-member rows, and the untouched control verifies.

### polish_lane_20261031
- [ ] UI-01 · app · with a petal whose map is disabled, select a proposal or draw a footprint in the Volume report -> the panel shows 'No map on this petal. Assign one in Maps' and never 'Computing true cut/fill'.
- [ ] UI-01 · app · click 'Open Maps' in the Volume report -> the right sidebar reveals the Maps section.
- [ ] UI-01 · batch · R4 (`cargo test -p fe-terrain --features render`, fe-ui panels::volume_report_panel, fe-renderer terrain_overlay) -> the NoMap and WrongPetal replies each appear once, with the request's correlation id.
- [ ] UI-10 · app · open Settings in the right sidebar and hover the Mesh budget ceiling value -> tooltip reads 'Most 3D objects shown at once; the viewer pauses extra ones'.
- [ ] UI-10 · app · open Maps and click Install from file -> the file dialog filter reads 'Map package'; the Available tab hint reads 'package name or name@version'.
- [ ] UI-10 · batch · R6 filter `ui_shell::` -> user_copy_has_no_dev_jargon passes; re-adding 'MeshInstanceBudget' to the hover text must turn it red.
- [ ] R-UI-1 · app · on a petal with a map, select a proposal right after switching petals -> Volume report shows figures or 'Waiting for this petal's map to load.', never stuck 'Computing'
- [ ] R-UI-1 · app · on a petal with no map, open Volume report on a drawn selection -> 'No map on this petal. Assign one in Maps.' and Open Maps reveals the Maps section
- [ ] R-UI-1 · api · press F12 while an agent calls the screenshot route -> both the F12 file is saved and the API reply arrives (neither dropped)
- [ ] R-UI-1 · batch · R9 (cargo test -p fractalengine --bin fractalengine -- screenshot_bridge) reaches the extended F12 journey
- T-UI · partial, no checks of its own
  - not finished: HANDED-OFF (I-UI-1): R-UI-2 F6 mount render_section_profile_block in fe-ui/src/ui_shell/right_sidebar.rs; R-UI-3-06 NEEDS fe-ui/src/panels/mod.rs:401 call mark_verse_local_only (EXT-18). These block the S10 and EXT-18 ticks. / SEC-INBOX: no entry in turn4-security-inbox.md was checked by this slice (bookkeeping only, no owned code). Not applicable to T-UI.

### agent_vision_20261031
- [ ] UI-11 · app · Press F12 while an agent GET /screenshot is in flight -> both the F12 file in data/screenshots/ and the API reply arrive, one file written, no panic or dropped capture.

### rich_data_history_20261101
- [ ] DATA-01 · app · Inspector > Custom Properties, Add property type geometry_point with value {"type":"Point","coordinates":[1,2]} -> saved, grid type label reads 'object'
- [ ] DATA-01 · app · Add property type datetime with value 'yesterday' -> error naming RFC 3339 shape, draft stays editable, nothing saved
- [ ] DATA-01 · app · Add geometry_polygon with an unclosed ring -> actionable error about closed rings of >=4 positions, nothing saved
- [ ] DATA-01 · app · sign in as Viewer, open Custom Properties -> Add/Edit/Delete disabled, one 'Editor access to this petal is required.' line, Copy still works (after I-UI-1 wiring)
- [ ] DATA-01 · batch · R3 (panels::inspector) once inspector.rs legs land
  - not finished: the worker reported partial without listing what is left (see its journal entry for DATA-01)
- [ ] DATA-03 · app · open Appearance on any tree entry at 1280x720 and at 1024x640 with UI scale 2.0 -> Save, Cancel and Reset to default are visible without scrolling and the body scrolls.
- [ ] DATA-03 · app · in the Appearance dialog, type an invalid hex value -> Save is disabled, the error line is reachable by scrolling the body, and Cancel and Reset stay on screen.
- [ ] DATA-03 · batch · R6 once the dialogs/mod.rs TreeAppearance leg is added.
- [ ] DATA-02 · app · select 2+ nodes, right-click a tree row, choose 'Add selection to group...' with at least one group existing -> per-group Add all selected / Remove all selected buttons appear at once, disabled with 'Reading selected objects', then enable without pressing Refresh.
- [ ] DATA-02 · batch · R3 (`panels::inspector`) once the NEEDS journey leg lands.
  - not finished: Suspect for QA-0111: after a read timeout or error, self.error stays set and the guard `self.error.is_none()` in maintain blocks any auto-retry until the user presses Refresh. Not changed, since mod.rs timeout text says 'Refresh to retry' by design; the orches
- [ ] R-DATA-1 · app · Inspector > Add Property, type geometry_polygon, ring [[0,0],[4,0],[4,3],[0.0,0.0]] -> accepted and listed as object, not refused as unclosed.
- [ ] R-DATA-1 · app · at 1024x640, UI scale 2, open a tree row's Appearance dialog -> Save/Cancel/Reset are visible without scrolling; delete the entry -> the 'no longer exists' reason shows beside them.
- [ ] R-DATA-1 · api · REST rotate/scale a promoted stamp -> GET its properties shows stamp.override.rotation (xyzw) and stamp.override.scale with the new values.
- [ ] R-DATA-1 · batch · R3 (gis::, panels::inspector) and R1 node_lifecycle_test reach the DATA tests; stamp_transform and dialogs tests need the I-BATCH filters first.
- [ ] DATA-06 · batch · no behaviour changed, so no QA lines.
  - not finished: Owned-surface check passed: no unmapped owners and no missing endpoint IDs for object_data, property_grid, property_value, inspector, tree_entry, tree_appearance, node_history or property_schema. Node-history REST routes and the get_node_history tool already h / Census exit is 2, not 0. Remaining unmapped actions: PortalAdmitBundle, PortalRevokeBundle, ReviewSceneAssetCopy, AdoptSceneAssetCopy, CancelSceneAssetReview, ToggleTerrainVisibility. Remaining unmapped controls: fe-ui/src/extensions/*, portal/{binding_offer,r / Missing checkpoint ids belong to other families: 5 F116 MCP tools (list-extensions, extension-command, list-bindings, set-binding, clear-binding) and 11 REST routes (portal-bundles, portal-observe/ipc/action/killswitch, relays). / catalog_source_fingerprint left null as required. Two clean full-coverage passes remain blocked-user.

### portal_surface_20261107
- [ ] EXT-05 · batch · R6 -> sender_gate_drops_every_unauthorized_post_without_a_reply passes; each non-control row shows dropped_origin +1 with no reply, and control gets one ok hello reply.
- [ ] EXT-05 · api · POST /api/v1/debug/portal/ipc-post {"row":"external"} as Owner with a bundle open -> 200, ipc_post.dropped_origin_delta 1, delivered false, replied false.
- [ ] EXT-05 · api · POST /api/v1/debug/portal/ipc-post {"row":"control","extra":1} -> 400 invalid_fixture_operation; the same with a non-Owner token -> 403 qa_gate_refused.
- [ ] EXT-05 · api · POST ipc-post {"row":"control","package_id":"other.pkg"} while qa.portal.actions is open -> 409 portal_fixture_mount_mismatch, and ipc-stats counters must not change.
- [ ] EXT-05 · sim · run.py portal-core step 2 -> ipc.dropped_origin_total 5 after r_stale, delivered_non_control 0, ipc.accepted 1 after r_control (watch the 409 ordering risk above).
- [ ] EXT-07 · sim · cargo run -p fe-webview --example portal_spikes -- SP-5 -> PASS; JSON line records load_html URL x2, host about:blank AllowHostUnload, page about:blank Refuse
- [ ] EXT-07 · sim · portal_spikes -- SP-6 -> PASS; A local/indexed = <none> after clearDataForOrigin, B local/indexed = B
- [ ] EXT-07 · sim · portal_spikes -- SP-8 -> PASS; static s- mount reads the share body, admitted a- mount and fe-spike page do not; preflights and permissions are recorded
- [ ] EXT-07 · sim · portal_spikes -- SP-9 -> runner survives and PASS; wry child exits abnormally with no result line, native child exits 0 with a dropped 70 000+ source
- [ ] EXT-07 · sim · portal_spikes -- SP-7 -> PASS for parse, gate and Windows sender; notes must say non-Windows host needed (OD-10)
- [ ] R-EXT-4 · api · POST /api/v1/debug/portal/ipc-post {row:'unsigned', content_hash:<unsigned build hash>} with any bundle open -> 200, dropped_origin_delta 1, delivered false, replied false (not a 409).
- [ ] R-EXT-4 · api · POST ipc-post {row:'control'} with package_id/content_hash of a bundle that is not open -> 409 portal_fixture_mount_mismatch; nothing is posted.
- [ ] R-EXT-4 · api · POST ipc-post with a body over 256 bytes, an unknown field or a query string -> refused (413/400); a non-Owner token -> 403 qa_gate_refused.
- [ ] R-EXT-4 · batch · portal_ipc_journey (cargo test -p fe-webview --test portal_ipc_journey, conductor/test-batches.md:213) reaches the extended PJ2 row loop.

### terrain_proposal_delta_viz_20261025
- [ ] UI-02 · app · open Terrain tools section with at least one proposal, untick 'Cut/fill colors' -> red/blue drape disappears while the ghost and baked terrain remain; re-tick restores it.
- [ ] UI-02 · app · with no proposals, open Terrain tools -> checkbox is hidden; restart editor -> checkbox is checked again (session-only, no DB write).
- [ ] UI-02 · batch · R4 -> fe-terrain heatmap toggle journeys assert drape entities vanish when HeatmapOverlayToggle.enabled=false.
- [ ] UI-03 · batch · R4 (`cargo test -p fe-terrain --features render -- profile::tests`) -> both profile tests pass, covering the analytic fill, cut, crossing, hole, None and unscaled cases.
- [ ] UI-04 · batch · run the R4 batch with filter section_responder::tests -> round trip returns fill_area ~20 with matching correlation id, a wrong-petal request returns WrongPetal, a no-map request returns NoMap
- [ ] UI-04 · app · once UI-05 lands, draw a section across a Raise proposal in Volume report -> a fill area is shown; on a petal with no map the Maps prompt shows and the plot is never blank
- [ ] UI-05 · app · select a drawn or proposal footprint, open Volume report > Section, drag Angle -> the plot redraws with two curves, red cut and blue fill, and the cut/fill areas change.
- [ ] UI-05 · app · on a petal with no map, open Section -> 'No map on this petal' with an Open Maps button; the plot must NOT be blank.
- [ ] UI-05 · app · click 'Copy section as TSV' -> a toast appears and the clipboard holds a Distance/Existing/Proposed row for every sample.
- [ ] UI-05 · batch · R4 (needs the panels::section_profile_panel filter) runs section_profile_round_trip_draws_both_curves.
  - not finished: the worker reported partial without listing what is left (see its journal entry for UI-05)
- [ ] R-UI-2 · app · Volume report > select a proposal > Section, then change Angle -> the plot redraws along the new chord and the cut/fill areas change
- [ ] R-UI-2 · app · open Section on a petal right after switching to it, before tiles load -> 'No terrain under this section yet.' changes to the plot once tiles arrive, with no click
- [ ] R-UI-2 · app · with Section open, edit the selected proposal's delta -> the plot and areas update; a petal with no map shows only the volume body's Open Maps button, not a second one
- [ ] R-UI-2 · batch · R4 (cargo test -p fe-terrain --features render) reaches section_request_round_trip_answers_fill_and_unavailable and profile tests

### inspector_settings_20260419
- [ ] UI-07 · app · Entity Settings > Access on a petal, peer with only a verse default -> a dimmed 'Inherited from fractal or verse' line under the role, no 'Granted here'.
- [ ] UI-07 · app · Access tab, Save role editor for a peer and confirm -> 'Granted here: editor'; expand 'Grant record (1)' -> '<peer>: editor, granted by you'.
- [ ] UI-07 · app · Access tab, Block access here and confirm -> grant record shows that entry as 'revoked'; effective role falls back to inherited; peer stays listed.
- [ ] UI-07 · app · Access tab with one peer connected and one disconnected -> filled dot (hover 'Online') vs hollow red dot (hover 'Offline'); owner row listed first with 'Owner'.
- [ ] UI-07 · app · Entity Settings > API Access with a token -> expiry reads 'Expires in N hours/days' (raw timestamp on hover), never a raw RFC3339 string.
- [ ] UI-07 · batch · R7 (role_propagation_durable_test) and R6 once `actions::tests::active_dialog_entity_settings` is added to its fe-ui filter.
- [ ] UI-06 · app · Click the already-active verse row in the hierarchy sidebar -> Inspector opens showing 'Verse: <name>', Name, copyable ID, fractal and known-peer counts, your role, Settings... button.
- [ ] UI-06 · app · With a petal active and nothing selected, open Inspector -> 'Petal: <name>' with node count; click a node -> node form replaces the card; click another petal -> card returns for that petal.
- [ ] UI-06 · app · As a Viewer, hover Settings... on the info card -> disabled with 'Editor role needed â€” you are Viewer'; as Editor+ it opens the Entity Settings dialog.
- [ ] UI-06 · batch · R3 (after registration) -> panels::hierarchy_info journey passes.
- [ ] UI-08 · app · open Verse Settings > Access as Manager/Owner, click 'Managers only...' then 'Confirm: revoke N grants' -> viewer/editor rows go to Blocked/inherited one at a time; the manager row is unchanged; verse default shows none.
- [ ] UI-08 · app · in Verse Settings > Access click 'Public' then 'Members only' -> the stored default access line on General flips viewer then none; the buttons are disabled while a request is pending.
- [ ] UI-08 · app · open Access on the seeded Genesis Verse -> the line 'This verse is local only. Create a new verse to share with peers.' shows and 'Generate invite' is disabled; hover shows the same text. Needs the panels/mod.rs hookup.
- [ ] UI-08 · app · in Fractal or Petal Settings > Access -> Public and Members only are disabled; Managers only is enabled only while a viewer/editor grant exists at that scope.
- [ ] UI-08 · batch · R6 filter actions::tests::active_dialog_entity_settings covers two viewer grants plus one manager (two sequential Revokes, then read-back) and a refused revoke halting the queue.
- [ ] R-UI-3 · app · Verse Settings > Access as a non-owner Manager, another peer granted viewer here by someone else, click Managers only -> confirm counts only your own grants; that peer is NOT revoked
- [ ] R-UI-3 · app · Verse Settings > Access as owner, Managers only... -> label 'Confirm: default none, revoke N grants'; after confirming, viewers/editors are listed one at a time, managers are unchanged, and the list refreshes
- [ ] R-UI-3 · app · API Access > Generate API token -> the expiry shows as a readable relative time, not a raw RFC 3339 string; hover shows the raw value
- [ ] R-UI-3 · app · open Settings > Access on the seeded Genesis Verse -> 'This verse is local only...' shows and Generate invite is disabled (only after the I-UI-1 NEEDS line lands)
- [ ] R-UI-3 · batch · R6 (after I-BATCH-1 adds actions::tests::active_dialog_entity_settings) and R7 role_propagation_durable_test

### endpoint_api_surface_20260725
- [ ] DATA-04 · batch · R1 `cargo test -p fe-database --test node_lifecycle_test` -> stamp_translate_is_rejected_but_rotation_and_scale_persist passes
- [ ] DATA-04 · api · on a promoted stamp, send the REST/MCP transform with the same position and rotation [0,1,0], scale [2,2,2] -> GET its properties shows stamp.override.rotation ~[0,0.479,0,0.878], scale [2,2,2]
- [ ] DATA-04 · api · on a promoted stamp, send a transform that moves its position -> request refused; stamp.override.* properties and the row are unchanged
- [ ] DATA-04 · app · rotate/scale a promoted stamp via the API while viewing its petal -> stamp visibly rotates/scales (needs the lib.rs PropertyChanged NEEDS wired, or a petal reload)
- [ ] DATA-05 · app · select a petal with a deleted node, open Inspector Query tab, click 'All nodes in this petal' -> deleted node absent and row count matches GET /api/v1/petals nodes.
- [ ] DATA-05 · app · GIS Copy API string for a node, tombstone the node, paste into Query tab -> zero rows.
- [ ] DATA-05 · batch · R3 (cargo test -p fe-ui --lib -- panels::inspector panels::query_tab actions::query gis::)

### project_completion_20261104
- [ ] NEW-05 · app · Hexon Manager > Browse, install a map from a registry > the Downloads row moves through a byte count and a total (needs the HEXON-16 bar), then the map appears under Installed.
- [ ] NEW-05 · batch · R4 (cargo test -p fractalengine --bin fractalengine -- terrain_bridge::) -> ordered byte progress, final n/n before RegistryInstalled, bearer-token and byte-budget tests unchanged.
- [ ] NEW-01 · batch · R2 (cargo test -p fe-ui --lib -- actions::node::) -> scene_write_door_retry_and_dead_letter_journey passes.
- [ ] NEW-01 · app · once NEW-02 lands, stall the DB thread and drag a node with Move, then Undo -> no 'busy' error appears and the Undo saves once the DB recovers.
- [ ] NEW-01 · app · once NEW-03 lands, keep the DB stalled about 8 s after Undo -> error toast '<label> was not saved: Database stayed busy...'; the edit appears under Unsaved edits.
- [ ] NEW-01 · app · Unsaved edits > Retry once the DB recovers -> edit saved and removed from the list; Discard -> edit removed and NOT saved.
- [ ] I-API-1 · api · with FE_API_USAGE_LOG=<file>, GET /api/v1/health and an authed GET -> 2 JSON lines; only the authed one has caller_did, route_class is the template
- [ ] I-API-1 · api · with FE_API_METRICS_BIND=127.0.0.1:9100, GET http://127.0.0.1:9100/metrics -> fe_api_requests_total lines; unset -> nothing listens on that port
- [ ] I-API-1 · api · POST /api/v1/verses/{v}/relays with a viewer token -> 403; as Manager on a host without a signer -> 503 'relay advertisement signer not configured'
- [ ] I-API-1 · api · POST /api/v1/debug/portal/ipc-post with a body over 256 bytes -> 413; without a bearer token -> 401
- [ ] I-BIN-1 · app · set FE_PEER_LABEL=alice, open a verse and a petal -> the window title reads '[alice] <petal> Â· <verse> â€” FractalEngine' and changes when the petal changes.
- [ ] I-BIN-1 · app · run fe-relay or the editor with FE_LOG_FORMAT=json -> stdout is one JSON object per line; unset -> the same text log lines as before.
- [ ] I-BIN-1 · api · POST /api/v1/verses/{verse_id}/relays with an Owner token -> 201; a second peer admits the advert (no 'advertised_by is not the signing author' warn).
- [ ] I-BIN-1 · app · entitlement build with a pinned issuer, Extensions store > a licensed build > Check licence -> Held (or Missing on 403), never 'not wired on this host yet'.
- [ ] I-BIN-1 · app · bind a hook to an uninstalled package that a configured source lists, wait about 20 s, Bindings > Refresh -> the row shows an Install action.
- [ ] R-NEW-2 · batch · R4 `cargo test -p fractalengine --bin fractalengine -- terrain_bridge::` -> install_from_registry_installs_a_sealed_container_end_to_end passes with ordered byte progress.
- [ ] R-NEW-2 · app · Hexon Manager > Install from registry, enter a listed tileset uri -> Downloads row shows growing 'x / y' bytes ending at y / y before it flips to Installed; never shows 64 MiB as the total.
- [ ] I-DB-1 · app · on a petal whose map has world_scale != 1, Add Node from the toolbar -> the new node appears at the map scale at once, not at 1x until reload.
- [ ] I-DB-1 · api · GET petal readings for a petal with IoT rows (gis-readings-* correlation) -> summary returned; as a non-member -> refused with 'Read readings failed'.
- [ ] I-DB-1 · api · export a petal snapshot, then import it while the petal exists -> PetalSnapshotImportRefused 'petal already exists'; no rows change.
- [ ] I-DB-1 · app · start the editor on an existing pre-clock database -> startup completes and logs 'field clock legacy floor seeded'; a second start seeds 0 rows.
- [ ] I-UI-1 · app · Inspector with no object selected in an open petal -> card 'Petal: <name>', node count, your role, Settings... (disabled for a Viewer with a reason).
- [ ] I-UI-1 · app · Road tool (L), click two points then Escape -> pending segment cleared, chain still open; second Escape ends the chain; third Escape deselects/returns as before.
- [ ] I-UI-1 · app · as a Viewer open a node's Custom Properties -> one line 'Editor access to this petal is required.', Add/Edit/Delete disabled, Copy works.
- [ ] I-UI-1 · app · Add property type geometry_point value {"type":"Point","coordinates":[1,2]} -> stored row type reads 'object'; value '1,2' -> inline error, draft kept.
- [ ] I-UI-1 · app · Tree row menu > Appearance at 1280x720 and at 1024x640 with UI scale 2 -> Save/Cancel/Reset visible inside the dialog without scrolling.
- [ ] I-UI-1 · batch · R3 `panels::inspector` (property + editor-batch legs), R2 `node_manager::tests::` (seven-gesture arity), R6 `dialogs::tests::` once registered, R6 `plugin::` (Tool::Road loops).
- [ ] NEW-03 · app · Stop the DB thread or fill its channel, then edit a node (move or rename) -> the status bar shows a spinner with 'Saving 1...' and no 'unsaved' chip yet
- [ ] NEW-03 · app · Let a write exhaust its retries -> an 'N unsaved' chip appears in the status bar; click it and the popup lists the label, reason, Retry and Discard
- [ ] NEW-03 · app · Click Retry on a row once the DB is healthy -> toast 'Retrying <label>', the row disappears, the chip clears and the edit persists after a reload
- [ ] NEW-03 · app · Click Discard on a row -> the row and chip count go away and the edit is not applied after a reload; Tab reaches the chip and both buttons
- [ ] NEW-02 · app · fill the DB queue (heavy import), drag-move a node, release, press Ctrl+Z -> no 'queue is busy' error; undo applies once the queue drains.
- [ ] NEW-02 · app · select a stamp while the DB queue is saturated -> selection shows pending, then promotes after drain; no 'queue is busy' error.
  - not finished: DEFERRED leg (b) viewport_pick.rs a_ribbon_hidden_by_its_layer_is_not_picked_until_shown: needs bounded(1) pre-filled stamp channel, selection_promotion Some while queued, then drain+pump and PromoteCheckedStamp arrives; blocked on a pub(crate) test pump seam 
- [ ] R-INT-2 · api · POST /api/v1/verses/{v}/relays with a 40 KiB JSON body and a Manager token -> 413, and no __relays row is written.
- [ ] R-INT-2 · api · POST /api/v1/verses/{v}/relays with no bearer token -> 401; the usage event for that request has no caller_did.
- [ ] R-INT-1 · batch · R2 (cargo test -p fe-runtime --lib -- scene_write::tests::) reaches the extended door verdict/journey tests.
- [ ] R-INT-1 · api · bind a scene.create refusing hook to a fractal, then POST /api/v1/petals/import-snapshot into it -> 409 refused, no petal created (after fe-api NEEDS lands).
- [ ] NEW-04 · batch · R2 (filter actions::node::) -> the petal-hint leg shows a refused follow-up write for p-node sends nothing while p-active is Sent.
- [ ] NEW-04 · app · with a scene hook bound to petal A that refuses, create an earthwork or dimension callout node in petal A while petal B is active -> node is created but its contract property writes are refused (not silently written).
- [ ] R-INT-3 · app · Inspector, Viewer role, open a node with custom properties -> Add/Edit/Delete disabled, one 'Editor access to this petal is required.' line, Copy works.
- [ ] R-INT-3 · app · Editor opens a property Edit draft, then the role drops to Viewer -> Save is disabled and nothing is sent.
- [ ] R-INT-3 · app · deselect all nodes in a petal -> Inspector shows 'Petal: <name>' card with ID copy and Settings...; selecting a node hides it.
- [ ] R-INT-3 · batch · R2 (node_manager::tests seven-source gesture arity) and the inspector property journey batch reach the extended tests.
- [ ] I-UI-2 · app · select the Road tool (hotkey L), click twice in the viewport -> a ghost polyline and a length label follow the cursor, and Escape clears the chain.
- [ ] I-UI-2 · app · with Measure active, click in the viewport -> Measure places anchors and Road does not claim the click (Road is claimed only while Road is active).
- [ ] I-UI-2 · app · Volume report with a footprint, change the section angle -> the section profile updates, and switching petal clears it.
- [ ] I-UI-2 · app · fill the DB channel, edit a node, then open the status bar chip -> 'Unsaved edits' lists the write with Retry and Discard.
  - not finished: HANDED-OFF (right_sidebar owner, R-INT-4): panels/mod.rs now passes road_state to render_right_sidebar, and tool_options::render already requires `road`. Both are broken at HEAD until the NEEDS line above lands. / DEFERRED (NEW-02 test legs in node_manager/mod.rs, trigger: a pub(crate) cfg(test) pump seam on the GUI door in actions/node.rs): pump_scene_door_at there is private and takes an explicit queue, so the full-channel undo leg and the stamp-promotion leg cannot b / DEFERRED (needs an AGENTS.md pointer from I-DOC): node_manager/mod.rs doc comments point to AGENTS.md road-builder. The NORMATIVE ClickPriority comment now reads 'Road claims right after Measure'. / Already present, no edit needed: RoadBuilderState init_resource (node_manager/mod.rs), SectionProfileState reset in maintenance.rs, status_bar already calls unsaved_edits.
- [ ] I-API-2 · api · GET /api/v1/petals/{petal_id}/snapshot.hexon with a Manager token -> 200 application/x-hexon+zip; with an Editor token -> denied.
- [ ] I-API-2 · api · POST /api/v1/petals/import-snapshot with an archive over the body limit -> rejected before any blob is written.
- [ ] I-API-2 · batch · R9 api_integration filter hexon_crates_publish_install_search_entries_asset_journey -> publish, install, list, asset bytes pass, and a licensed non-free asset returns 403 entitlement_required.
  - not finished: HANDED-OFF (R-INT-4): confirm the journey compiles against the real HexonBuilder, ApiState and registry signatures; the upstream NEEDS reports were not found on disk. / Not done: no api_integration.rs leg for the snapshot routes, because HEXON-12 put its journey in export_share_test.rs. mcp/tools.rs, auth.rs and Cargo.toml were not changed because no instruction targeted them.
- [ ] R-NEW-1 · batch · R2 (filter actions::node::) runs scene_write_door_retry_and_dead_letter_journey incl. the petal-hint leg.
- [ ] R-NEW-1 · app · with a DB stall, drag a node -> footer shows 'Saving 1...' spinner, then either saves or an 'N unsaved' chip appears.
- [ ] R-NEW-1 · app · click 'N unsaved' chip -> popup lists label+reason with Retry/Discard; Retry toasts 'Retrying Move'; Discard removes the row; chip hides at 0.
- [ ] R-NEW-1 · app · earthwork region in petal A while viewing petal B with a refusing hook bound only on A -> follow-up property write is refused, not admitted.
- I-DOC-CORE · partial, no checks of its own
  - not finished: HANDED-OFF (fe-format owner, HEXON-01/08): fe-format/src/AGENTS.md has no §petal-snapshot section; the pointer in handlers/AGENTS.md ## petal-snapshot and fe-hexon §unified-format is dangling until it lands. / HANDED-OFF (HEXON-18 / fe-terrain entitlement owner): fe-terrain/src/AGENTS.md has no §tileset-entitlement section; root AGENTS.md line 'Licensed hexons' points at it and is dangling until it lands. / HANDED-OFF (I-DOC-API): fe-api/AGENTS.md §units section and the one-line /// pointers in fe-api/src/types.rs (CITY-14 item 4) were not in my owns and were not done. / HANDED-OFF (I-DOC-UI): fe-ui/src/AGENTS.md §units, §ground-snap and the geometry-mirror update at ~line 1143 (CITY-14 item 5) were not in my owns and were not done. / HANDED-OFF (I-BATCH / test-batches): no tests were added or run, so no batch filter is needed from this slice. / DEFERRED: NodeCreated scale in DbResult (CITY-14 item 3), per the steer, deferred until a consumer needs it; trigger is a UI reader of DbResult::NodeCreated scale.
- I-DOC-API · partial, no checks of its own
  - not finished: HANDED-OFF (EXT-05 gate): NEEDS edits for fe-webview/src/portal_bundle/AGENTS.md and tools/qa/portal/AGENTS.md; the EXT-05 task text was not available to this worker. / HANDED-OFF (HEXON-06 owner): fe-api/AGENTS.md edits described in HEXON-06's task text were not available to this worker. / HANDED-OFF (NEW-05 owner): fractalengine/src/AGENTS.md edits described in NEW-05's task text were not available to this worker. / HANDED-OFF (UI-07 owner): fe-runtime/src/AGENTS.md edits described in UI-07's task text were not available to this worker. / HANDED-OFF (upstream NEEDS): the NEEDS lines from CITY-01..13 and the other listed upstreams were not readable here; the orchestrator must pass them to I-DOC-API again. / HANDED-OFF (I-DOC-CORE): CITY-14 parts for fe-format/src/AGENTS.md (scale), fe-terrain/src/AGENTS.md (scale), fe-database/src/AGENTS.md (placement default).
- [ ] I-SYNC-2 · batch · no new tests in this slice; dialback prober/responder tests must be reachable by a conductor/test-batches.md filter (I-BATCH to confirm).
- [ ] R-INT-4 · api · GET /api/v1/petals/{petal}/snapshot.hexon with no token -> 401; with an editor token -> 404 'not found or not permitted', no archive
- [ ] R-INT-4 · app · Road tool (L), click once and move the cursor -> length readout label follows the pointer and disappears over side panels
- [ ] R-INT-4 · app · select a region, open the Proposal report section -> a collapsible 'Section' block under the Volume report (only after the right_sidebar NEEDS lands)
- [ ] R-INT-4 · batch · R-fe-api line `--test api_integration` (conductor/test-batches.md:313) reaches the extended hexon journey
- [ ] I-BATCH-1 · batch · `cargo test -p fe-ui --lib -- node_manager::road_snap` (R5) reaches the new road snap and chain tests once the road slices land; not run this round.
- [ ] I-BATCH-1 · batch · `cargo test -p fe-database --test petal_snapshot_test` (R10) is reachable via the R10 --test line; not run this round.
  - not finished: HANDED-OFF I-BATCH-2: confirm whether fe-database/src/handlers/editor_batch (a separate module from the fe-ui one) carries tests that need a batch line; no current filter reaches it. / Already present, skipped: EXT-14 window_chrome line in R6 (test-batches.md:217).
- I-DOC-UI · partial, no checks of its own
  - not finished: HANDED-OFF (DATA-01, DATA-02, HEXON-16, HEXON-19, NEW-01, NEW-04, UI-05, UI-06, UI-07, UI-08, UI-10, CITY-13): their result reports with NEEDS lines against fe-ui/src/dialogs/AGENTS.md, fe-ui/src/editor_batch/AGENTS.md and the other owned files were not presen / HANDED-OFF (fractalengine/src/AGENTS.md §gpx, CITY-15 item 3): path_kind provenance section belongs to the main-binary doc slice. / HANDED-OFF (conductor/test-batches.md, CITY-15 item 4, I-BATCH-1): append R5 filters `node_manager::road_snap node_manager::road_chain node_manager::road_interaction node_manager::road_overlay`, R2 filter `node_manager::ground_snap`, and the source-path and Fi / HANDED-OFF (conductor/tracks/{road_builder,map_scale}*/metadata.json, CITY-15 item 5, T-CITY): status, doneness_note and OQ resolutions (road OQ-1 hotkey L, OQ-2 world-space radius, OQ-3 'Road N', OQ-4 both; map OQ-1 meters, OQ-2 height field, OQ-3 single writ / HANDED-OFF (fe-format, fe-terrain, fe-database, fe-api AGENTS, CITY-14 items 1-4 and types.rs pointers): owned by I-DOC-CORE and I-DOC-API, not this slice. / HANDED-OFF (road pen-create failure cleanup): fail_pending_pen_create (fe-ui/src/gis/mod.rs:568) has no production caller at HEAD, only tests. Owner: the slice that wires DbResult::Error for road/pen creates.
- [ ] I-DB-2 · app · a peer is demoted so its node loses every admissible version -> that node disappears from the viewport with no hierarchy reload.
- [ ] I-DB-2 · sim · p2p-core, a Manager renames a fractal and edits its description -> every peer shows the new name and description, and the published row carries _fe_clock.
- [ ] I-DB-2 · app · install two hexons whose entries share an entry_id into one petal -> both hexons list their own entries, and neither overwrites the other.
- [ ] I-UI-3 · app · open Peer Debug from the toolbar, press the refresh/request diagnostics control -> a redacted network diagnostics snapshot appears; before the first request the panel shows its empty state.
- [ ] I-UI-3 · batch · no new tests added. The existing gardener_console numeric render smoke in panels/mod.rs should still run, since only the call signature changed.
- [ ] I-BIN-2 · app · Settings > Network, turn on 'Serve as a relay for my verses', confirm, restart a default build -> log says the build lacks relay-server; nothing binds 443.
- [ ] I-BIN-2 · api · GET /api/v1/network/diagnostics with a Viewer token on fe-relay -> 200 with a redacted snapshot, not 503.
- [ ] I-BIN-2 · api · run fe-relay with FE_RELAY_SERVE=1 and no cert mode, built with relay-server -> process exits non-zero naming no_cert_mode.
- [ ] I-BIN-2 · batch · R8 (fractalengine-relay --features relay-server -- relay_serve; fractalengine --bin fractalengine -- relay_serve_bridge) covers the glue these wire.
- [ ] I-API-3 · api · POST /api/v1/verses/{verse}/relays as viewer or editor -> 403 and no `__relays` event. As owner with an https URL and expires_at_ms -> 201 plus a signed v1 row.
- [ ] I-API-3 · api · DELETE /api/v1/verses/{verse}/relays/{id} -> 204 and an empty-data tombstone event for that id. POST with an http:// URL or a past expiry -> 400.
- [ ] I-API-3 · api · GET /api/v1/network/diagnostics with a viewer token -> 200 with node and peers, no raw IP addresses. Without a token -> 401. With no diagnostics bridge wired -> 503.
- [ ] I-API-3 · batch · R9 (api_integration), filter relay_advertise_and_network_diagnostics_journey.
  - not finished: HANDED-OFF R-INT-6: Cargo.toml and auth.rs needed no change. NET-09/NET-25 reported no further api_integration NEEDS beyond the relay legs, which are written from their task text. / The journey covers the NET-25 refusals (past expiry, http, missing expiry) over REST. It does not cover the self-relay dial-back refusal, which needs FE_RELAY_HOST env control and stays in the relay_advertise.rs unit journey.
- [ ] R-INT-5 · api · PATCH a stamp node with a full transform (rotate + scale) via REST, then switch petal away and back -> stamp keeps the new rotation/scale, no revert
- [ ] R-INT-5 · app · drag-scale a promoted stamp with the gimbal, then reload the petal -> the stamp keeps the new scale
- I-DOC-NET · partial, no checks of its own
  - not finished: HANDED-OFF (I-DOC-API / fe-api owner): fe-api/AGENTS.md §routes MCP row still says '49-row ToolSpec table'; get_network_diagnostics now exists in fe-api/src/mcp/tools.rs, so the count may be stale. Recount there. / HANDED-OFF (NET-26): fe-sync/src/AGENTS.md §transport-admission-control item 2 residual text not amended; location not verified in this pass. / HANDED-OFF (NET-22): the hazard-B warning named in the NET-22 task is not written; no source text was available. / HANDED-OFF (R-NET-1): confirm the §reachability DNS rebinding residual wording against the dial-back prober code. / HANDED-OFF (R-NET-2): once the per-author quarantine eviction fix lands, add its policy to fe-sync/src/AGENTS.md §relay-advertise (currently counters and constants only). / HANDED-OFF (I-BATCH): no NEEDS line; conductor/test-batches.md already lists fe-sync/tests/peer_relay_e2e_test.rs (slow line, line 273 and 286), verified at HEAD.
- [ ] R-INT-6 · app · start the editor with Settings > Network > Serve relay on and FE_RELAY_SERVE_VERSES listing a verse you only joined -> relay refuses to start (unhosted_verse in log).
- I-CONDUCTOR · partial, no checks of its own
  - not finished: HANDED-OFF: upstream NEEDS lines from CLOCK-01, T-HEXON, NET-11, T-NET, NET-39, UI-12, T-UI, T-CLOCK, T-EXT, T-CITY, T-DATA, T-REL were not in this worker's inputs. The orchestrator must re-run I-CONDUCTOR against them or pass the lines in. / HANDED-OFF: qa/findings.jsonl closed_in for QA-0156, QA-0165, QA-0166, QA-0126 belongs to round-end refine, per plan line 708. / NOTE: doneness-ledger has no POLISH-W2..W6 rows (grep 'POLISH' finds only UXQA-ONBOARDING-FRAMING). Nothing was edited for them. / NOTE: the tracks.md hexon_unification 'Not started' paragraph was replaced with a landed summary. crate_id and .fecrate removal were not re-verified.

### hexon_entitlements_20261031
- [ ] HEXON-15 · batch · R4 (fe-terrain --features render, tiles::store::tests::test_install_and_list)
- [ ] HEXON-15 · app · install a Paid tileset with one free_entries tile, set it as petal map -> only the free tile renders, other tiles absent, no crash and no garbled imagery
- [ ] HEXON-15 · app · Hexon Manager Installed tab for a Free tileset -> status reads installed; for Paid with no free entries -> entitlement-required
- [ ] HEXON-16 · app · Install a paid hexon with free_entries, open Maps sidebar > Installed -> row shows a 'Preview' chip (hover: 'Free tiles only') and a 'Licence details' link; clicking opens the browser only on click.
- [ ] HEXON-16 · app · Install a free or unlicensed hexon -> no chip and no link on its row.
- [ ] HEXON-16 · app · Install a paid hexon with no free entries -> 'Locked' chip. A licence URL that is not http/https (e.g. file:// or javascript:) -> no link shown.
- [ ] HEXON-16 · app · Install a hexon from the registry and open the Downloads tab -> a progress bar reads '<got> / <total> (N%)' and stays at or below 100%. A chunked P2P download still shows the chunk bar and the small byte label.
- [ ] HEXON-16 · batch · R4 (`cargo test -p fractalengine --bin fractalengine -- terrain_bridge::`) -> free fixture reads Installed with no link; paid fixture reads InstalledPreview with its verification_url.
  - not finished: the worker reported partial without listing what is left (see its journal entry for HEXON-16)
- [ ] HEXON-14 · batch · R11 (cargo test -p fe-hexon -- --skip entitlement_client::tests:: --skip entitlement_gate::tests::) reaches registry::tests::test_install_and_list -> passes.
- [ ] HEXON-14 · api · GET /api/v1/crates/{uri}/entries/{non-free id}/asset on an installed Paid hexon -> HTTP 403 with code entitlement_required; the response must NOT contain the entry's bytes.
- [ ] HEXON-14 · api · GET the same route for a free entry of that Paid hexon, or any entry of a Free hexon -> HTTP 200 application/octet-stream with exactly the packaged bytes, no blob_path JSON.
- [ ] HEXON-14 · api · GET /api/v1/crates/installed?petal_id=P -> each row has an entitlement field: installed for Free, installed-preview or entitlement-required for Paid.
- [ ] R-HEXON-3 · app · Hexon Manager > Installed, with a Paid tileset that has free_entries -> 'Preview' chip and a 'Licence details' link; with a javascript: or spaced URL the link must NOT appear.
- [ ] R-HEXON-3 · api · GET /api/v1/crates/{uri}/entries/{non-free id}/asset?petal_id=.. on a Paid crate -> 403 code entitlement_required; a free entry -> 200 with nosniff and attachment headers.
- [ ] R-HEXON-3 · batch · R4 (cargo test -p fe-terrain --features render, test_install_and_list; terrain_bridge:: seeding journey) and R11 fe-hexon registry install test.

### release_ci_20260429
- [ ] R-REL-1 · app · none (CI-only). Run Release on a v*-rc tag in a fork without MINISIGN_SECRET_KEY -> publish job green, ::notice:: unsigned, notes say 'Checksums signed: no'.
- [ ] R-REL-1 · api · with MINISIGN_SECRET_KEY and committed docs/release-signing.pub, tag a release -> SHA256SUMS.minisig uploaded; minisign -V -p docs/release-signing.pub -m SHA256SUMS prints the tag.
- [ ] R-REL-1 · api · commit docs/release-signing.pub, remove the secret, tag an rc -> job still green but shows a ::warning:: about the missing key; no .minisig asset.
- T-REL · partial, no checks of its own
  - not finished: the worker reported partial without listing what is left (see its journal entry for T-REL)

## 3. Checks the plan already owed

Work that is not code: in-app looks, simulator runs, sweeps and decisions the plan classed blocked-user (plan section 6.2).
### CLOCK
- [ ] OWED-CLOCK-1 · sim · run `p2p-core` to verify the per-key clock (when you name it)

### NET (relay_operability, peer_networking, iroh_1_0_upgrade, p2p_asset_streaming)
- [ ] OWED-NET-1 · batch · relay_operability gates W1/W2/W3: serial build + tests per wave
- [ ] OWED-NET-2 · api · relay_operability AC-6: backup runbook round-trip against a running relay
- [ ] OWED-NET-3 · api · relay_operability AC-7: relay-assisted sync against the documented compose service
- [ ] OWED-NET-4 · batch · peer_networking gates W1..W5, including `cargo check -p fe-sync --features relay-server` and the relay/binary relay-server builds
- [ ] OWED-NET-5 · batch · peer_networking F3: run the P15 e2e log, `cargo deny check` over iroh-relay/server, full sweep
- [ ] OWED-NET-6 · api · production embedded relay on a real domain (ACME/operator cert), desktop relay serving, OD-3 self-signed phase (needs a provisioned host)
- [ ] OWED-NET-7 · batch · iroh 1.x sweep: fe-sync incl. `legacy_v035_fixture_migrates_and_reads_back`, fe-sim, fe-identity, fe-test-harness loopback
- [ ] OWED-NET-8 · api · iroh flag-day rollout of deployed peers/relays before 2026-12-31; commit/push; decide how to package the 7.1 MB redb fixture; remove the socket2 shim after a macOS CI run
- [ ] OWED-NET-9 · batch · Harness scenario 16 `two_peer_tileset_stream` (two-instance loopback transfer)
- [ ] OWED-NET-10 · app · tileset streaming: no spawn spike and frame-time delta on tile faults (NFR-1/NFR-2), in the app
- [ ] OWED-NET-11 · batch · conformance: `cargo test -p fe-hexon --all-features` (hosted and P2P sources pass the same suite)
- [ ] OWED-NET-12 · batch · p2p_asset_streaming single end-of-track sweep
- [ ] OWED-NET-13 · app · p2p_asset_streaming retro + archive after the sweep and NFR evidence
- [ ] OWED-NET-14 · api · decide the full FetchStrategy rebase for P2P downloads (see section 5)

### HEXON
- [ ] OWED-HEXON-1 · batch · hexon_unification phase 1: fe-format tests green, zero new deps, review the enum by eye (R11)
- [ ] OWED-HEXON-2 · batch · phase 2: fe-hexon and fe-hexon-registry tests (R11/R11-STORE); grep for `crate_id` and `.fecrate` is clean
- [ ] OWED-HEXON-3 · api · phase 3: cargo test -p fe-api (R9) and a manual `curl` publish against a dev node
- [ ] OWED-HEXON-4 · app · phase 4: export a real dev petal and decide with real data whether iot_reading rows stay in
- [ ] OWED-HEXON-5 · app · phase 5: export from node A, import on a wiped node, open it in the app
- [ ] OWED-HEXON-6 · batch · phase 6: sweep green, then conductor retro and close-out
- [ ] OWED-HEXON-7 · batch · hexon_entitlements gates W1/W2/W3 (cargo check)
- [ ] OWED-HEXON-8 · batch · hexon_entitlements full sweep, once
- [ ] OWED-HEXON-9 · app · hexon_entitlements: look at the Hexon Manager states, then retro + archive
- [ ] OWED-HEXON-10 · app · hexon_scale_orchestration phase 6: grid snaps, re-subdivides and fades; callout persists (real petal scale) plus the sweep

### EXT
- [ ] OWED-EXT-1 · app · extension_platform P4.4 GUI QA per `fe-ui/src/extensions/AGENTS.md` §gui-qa-path
- [ ] OWED-EXT-2 · sim · P6.3 two clean runs each of ext-fleet, ext-device, ext-install-gui, ext-bindings, then triage
- [ ] OWED-EXT-3 · app · plugin_host_revival sweep: workspace tests and the in-app consent/opt-in check
- [ ] OWED-EXT-4 · app · portal spikes SP-1..SP-9 on WebView2 (your Windows box)
- [ ] OWED-EXT-5 · sim · portal T5.5: run PJ1-PJ9 and the portal sim scenarios, `run.py validate portal-*` receipts, full sweep once (PJ8 is native-only)
- [ ] OWED-EXT-6 · app · RUNBOOK H4 earthwork petal authorization; R-GATE-11 GUI create augments; R-GATE-15 GUI queue test; door-parity journey; GPX serial-create deferral (owned by group NEW)
- [ ] OWED-EXT-7 · app · R-Z-16 / R-BIN-21: decide whether a bound `portal.open` bundle may mount by package without a repository node (see section 5)
- [ ] OWED-EXT-8 · app · mint a scoped share link from the Copy-for-BI card in-app (needs a GUI-to-API seam; documented instead by EXT-13)
- [ ] OWED-EXT-9 · app · QA-0174: GPX path vs 1:100 terrain (nodes at -27 m, focus-node 100x off); re-look after CITY-08 and the scale slices
- [ ] OWED-EXT-10 · sim · QA-0165 role-at-write decision and clock track (owned by CLOCK)
- [ ] OWED-EXT-11 · app · registry install shows real download progress (user decision 2026-11-13; sequence after EXT-16)
- [ ] OWED-EXT-12 · app · R-ADMIN-07: panel answers a concurrent CLI install with "consent declined" (kept, fails closed)
- [ ] OWED-EXT-13 · app · deferred, triggers not fired: R-UI3-16 context-menu property cache; R-UI3-17 App-level journey for draw_context_slot
- [ ] OWED-EXT-14 · api · deferred: R-STORE-16 constant-time token compare; R-STORE-17 fe-ext index verify reuse
- [ ] OWED-EXT-15 · api · deferred: R-TOOL-10 city-map example crate; R-TOOL-12 run.py staging a second example version
- [ ] OWED-EXT-16 · api · deferred: R-BND-10, R-PLUG-16 / R-NET-18 frame buffering, G0-C26, G5-C16, R-HOOKS-07, R-HOOKS-28, R-API-23, G3-C15
- [ ] OWED-EXT-17 · api · deferred by C-PHR: AOT `compile_cached` has no caller; `verify_revocation_list_fresh` has no production caller
- [ ] OWED-EXT-18 · app · deferred portal items: R-POR-IPC-21, R-POR-IPC-22, P1-09, P3-10 focusable badge, P3-11 "Admit after the cause is lifted"
- [ ] OWED-EXT-19 · app · out of scope by your default: M4 store experience, payment integration, KMS/HSM, default store endpoint, licensed-tileset row states, G2-C17, dropping tauri

### CITY
- [ ] OWED-CITY-1 · app · road_builder_ux phase 3: 3-segment chain, restart, track persists
- [ ] OWED-CITY-2 · app · phase 4 (AC-6.2): ghost colouring, guides vs radius, snap indicators, metre readout vs a known map distance, Alt suspend, screenshots
- [ ] OWED-CITY-3 · app · phase 5: confirm the manual verification outcomes; checkpoint commit
- [ ] OWED-CITY-4 · app · map_scale_authority phase 2: same GLB on 1:1 and 1:100 petals shows correct relative size
- [ ] OWED-CITY-5 · app · phase 5: the four in-app checks plus the sweep
- [ ] OWED-CITY-6 · app · phase 4: place a model on elevated terrain -> it sits on the surface

### UI
- [ ] OWED-UI-1 · app · ui_shell_architecture phase 0: FR-1/FR-2 repro matrix (plan.md:90)
- [ ] OWED-UI-2 · app · phase 1: click-claim behaviour (plan.md:124)
- [ ] OWED-UI-3 · app · phase 2: pixel-parity smoke (plan.md:161)
- [ ] OWED-UI-4 · app · phase 3: inject a panic in a scratch panel (plan.md:187)
- [ ] OWED-UI-5 · app · phase 4: every workflow from the right sidebar (plan.md:217)
- [ ] OWED-UI-6 · app · phase 5: tooltips, viewport wider (plan.md:241)
- [ ] OWED-UI-7 · api · decide release panic=unwind (ratified keep abort 2026-07-25; only if you ask)
- [ ] OWED-UI-8 · app · polish_lane W3 VISUAL / W6 VISUAL and your in-app look
- [ ] OWED-UI-9 · batch · terrain_proposal_delta_viz full sweep, once
- [ ] OWED-UI-10 · app · terrain_proposal_delta_viz: verify F5 in-app
- [ ] OWED-UI-11 · app · inspector_settings phase 1/3/4 manual checkpoints (plan.md:58,133,217)
- [ ] OWED-UI-12 · app · agent_vision AC-11 incl. minimized-window scene capture
- [ ] OWED-UI-13 · app · agent_input_surface: with `FE_API_INPUT_ALLOW=1`, Ctrl+V is refused
- [ ] OWED-UI-14 · batch · agent_vision / agent_input_surface full sweep, once

### DATA
- [ ] OWED-DATA-1 · app · RD1: typed value/draft/error/persistence proof across all layouts (staged build)
- [ ] OWED-DATA-2 · app · property grid at three sizes (Edit/Copy/Delete widths), fresh stage
- [ ] OWED-DATA-3 · app · tree adapter: final menu and destructive-action layout checks
- [ ] OWED-DATA-4 · app · RD4: node history v3 preview, chart/table, final coverage
- [ ] OWED-DATA-5 · sim · real Both scenario with QA-0035 repair, plus a same-stage restart (`tools/qa/rich_data_scenario.py`)
- [ ] OWED-DATA-6 · sim · two clean full-coverage passes; keep `catalog_source_fingerprint` null until the staged build
- [ ] OWED-DATA-7 · batch · final integrated checks, sweep, CI push and tagged artifact (you say when)
- [ ] OWED-DATA-8 · app · thin tree icons and primary/additional/partial row states: live multi/partial check
- [ ] OWED-DATA-9 · app · ordered selection and Inspector/Options/tool routing: alias, derived, concurrency-stream, QA-0069
- [ ] OWED-DATA-10 · app · group creation/membership, mixed properties, capability refusals, pivot, persistence
- [ ] OWED-DATA-11 · sim · committed-node publication/recovery: fresh paired run on two peers (`tools/qa/peers.ps1`)
- [ ] OWED-DATA-12 · app · closed-path witness: transform, Group, Undo, GPU picking proof on a rendered build
- [ ] OWED-DATA-13 · sim · local-copy adoption: owner adoption, peer bytes, failed-loader recovery, rendered GLB
- [ ] OWED-DATA-14 · sim · QA30 two-member Group baseline on the retained store restart; native Viewer/Editor observation gap
- [ ] OWED-DATA-15 · api · endpoint_api_surface phase 3: trigger op (decision, see section 5)
- [ ] OWED-DATA-16 · batch · endpoint_api_surface phase 6: sweep and in-app/endpoint verify
- [ ] OWED-DATA-17 · api · QA-0116, QA-0128, QA-0144 triage (see section 5)

### REL
- [ ] OWED-REL-1 · api · pick the Windows signing provider, set the Azure signing secrets and variables, confirm a signed run
- [ ] OWED-REL-2 · api · release_ci 2.7: run the release workflow end to end on a tag (7 archives, no macOS)
- [ ] OWED-REL-3 · api · cut the first tag: `v0.1.0` or `v0.1.0-qa.YYYYMMDD` (your push)
- [ ] OWED-REL-4 · api · oss_release 5.3, 5.4, 5.5, 5.6, 6.2, 6.5 (disclosure, SECURITY.md contact, CI scope, publishing gate, register review, first public push)
- [ ] OWED-REL-5 · app · project_completion P0 feature-catalog reconciliation; P2/P3 live-acceptance (QA-0024, QA-0069, selection/group edges, same-object conflict, rich-data Both)
- [ ] OWED-REL-6 · app · P1 native/UIA acceptance on the interactive desktop (provider root, NAV23, QA-0072, accessibility helper, QA0053)
- [ ] OWED-REL-7 · app · require-existing startup: live restart and fixture checkpoints without reseeding (retained stores held)
- [ ] OWED-REL-8 · app · QA-0063 peer node delivery, QA0073 camera ownership, QA0074 layout, VIS-001..003, QA47 invalid insertion: fresh peer/render/restart acceptance
- [ ] OWED-REL-9 · batch · P4 fmt/test/harness/iot-bridge sweep; P5 push, tag, wait for Release, check downloads/hashes/GHCR, retro
- [ ] OWED-REL-10 · batch · crate_consolidation phase 4 close-out: sweep, docker image build, retro (nothing to run while G-1..G-3 are DEFER)
- [ ] OWED-REL-11 · api · Windows code signing (needs the secrets from OWED-REL-1)

## 4. Security verifications

- [ ] SEC-1 · api · `fe-runtime/src/relay_advertisement.rs`, `fe-api/src/relay_advertise.rs` (relay URL request forgery). As Manager, `POST /api/v1/verses/{verse_id}/relays` with each of `http://127.0.0.1:8080/`, `http://localhost/`, `http://[::1]/`, `http://169.254.169.254/`, `http://192.168.1.10/`, `http://10.0.0.5/`, `http://user:pw@relay.example.org/`, `ftp://relay.example.org/` -> every one is refused (400); only a public https host is accepted. Write the same URLs as a `__relays` row from a Manager peer -> none shows up as an advertised relay or is ever dialled. (Today only "http/https plus host" is checked; verify after R-NET-1 / NET-14.)
- [ ] SEC-2 · api · `fe-database/src/handlers/crud.rs`, `fe-format/src/scale.rs` (no global scale cap, f64 to f32 overflow). Set a petal map `world_scale` to `1e40`, `1e-40`, `0`, `-1` and a non-number, then import a GLB and create a node there -> the node's scale is always finite and inside the map's scale bounds (never `inf`, `NaN` or 0) in the inspector, in `GET` node responses and after restart.
- [ ] SEC-3 · batch · `fe-sync/src/relay_advertise.rs` (one global quarantine FIFO). Have one Viewer-role author write more than 1024 `__relays` rows, plus one row from a second Viewer -> the second author's held row is still there, `quarantine_evicted` counts only the flooder's rows, and admitted rows from Managers are unaffected (R8 + R-NET-2).
- [ ] SEC-4 · sim · `fe-policy/src/role_grants.rs` (CLOCK-02, inbox 4, HIGH: grant windows trusted from the wire). Send a role statement whose `windows` claim a past Manager window from a non-owner grantor, or a window that starts before that grantor's own authority -> receivers ignore or refuse the windows, and role-at-write for edits inside that claimed span is unchanged. R-CLOCK-1 must not pass with this open.
- [ ] SEC-5 · api · `fe-api/src/petal_snapshot.rs` (inbox 5, HIGH: identity collision). As Manager of fractal A, import a snapshot whose `node_id`, reading ids or op-log ids already exist under fractal B -> the import is refused and no row of B changes; an ambiguous node id fails closed.
- [ ] SEC-6 · api · `fe-api/src/petal_snapshot.rs` + `fe_hexon::snapshot::restore_snapshot_archive` (inbox 6, MEDIUM: forged op-log). Import a snapshot carrying op-log entries with a bad or missing signature, of a role/grant/clock type, or pointing outside the snapshot's petal -> refused (or stored only in a marked quarantine); the live op log is unchanged and imported rows never carry a winning clock or a grant window.
- [ ] SEC-7 · api · `fe-sync/src/relay_server.rs` (inbox 7, MEDIUM: one client can close the relay). Connect repeatedly as a non-member so every attempt is denied -> members already connected stay connected and new members can still connect; the noisy source is throttled or logged, the listener is not closed.
- [ ] SEC-8 · api · `fe-database/src/handlers/petal_snapshot.rs` (inbox 8, HIGH: row validation). Import snapshots where a node's record key differs from its `node_id`, the `node_id` is not a valid ULID, or the id (or a node_log / op_log id) already exists anywhere in the database -> each is refused inside the restore transaction with nothing written.
- [ ] SEC-9 · app · `fe-runtime/src/scene_write.rs` (inbox 9, MEDIUM: hook gate bypass). Bind a scene hook that denies node creation, then import a petal snapshot into that scope -> the hook judges each imported node (an import over the per-write hook cap is refused), or the import is refused while a hook is bound; nodes are never created unjudged.
- [ ] SEC-10 · sim · `fe-sync/src/distributed_query.rs` (inbox 10, HIGH: legacy frame parser differential). Once a peer is known to sign, send an unsigned frame whose top-level JSON is not an object, and one the generic parse reads differently from the typed decoder -> both are dropped before dispatch; no handler runs for an unsigned frame claiming that peer's identity.
- [ ] SEC-13 · sim · `fe-sync/src/dialback_responder.rs` (inbox 13, MEDIUM: first answer wins). Let a second peer answer a dial-back request first with `ok=false` or a forged record, then the real prober with `ok=true` -> the outcome follows the authorized prober's `ok=true` record (or times out as a failure), not whichever record came first.
- [ ] SEC-14 · api · `fe-sync/src/dialback_prober.rs` (inbox 14, MEDIUM: DNS outside the probe budget). Point a probe at a host whose DNS answer never returns -> the probe ends within the total probe timeout and its permit is released, so further probes still run.
- [ ] SEC-15 · app · `fractalengine/src/main.rs` (inbox 15, MEDIUM: over-broad hosted verses). On a node that has joined a verse it does not own or host, start the hosting features -> that verse is not in the hosted set (relay admission, dial-back, advertisement) and the consent UI lists exactly the hosted verses.
- [ ] SEC-note · none · inbox entries 11 and 12 concern a private repository and are tracked in its own checklist.

## 5. Defaults taken without asking

### CLOCK
- Peers upgrade together, no dual-format writing, no version negotiation (clocked rows unreadable by old peers) — say if you want it changed.
- A revoke closes the author's grant window at the highest write clock the revoker had received from that author, so honest concurrent edits can be dropped — say if you want it changed (alternative: close at the revoke's own clock).
- Residual risk accepted: a revoked author can forge an old-clock write at or below the frontier for a key nobody wrote since — say if you want it changed.
- Grant history capped at 8 closed windows per role statement — say if you want it changed.
- Every member's latest version of every key is kept forever, property deletions included (no GC yet) — say if you want it changed.
- Role statements gain an optional `windows` field; mixed old/new swarms disagree about past edits — say if you want it changed.
- Node transform is one clocked key (max of clock, then author DID); `edit_seq` retired for clocked rows — say if you want it changed.
- `verse.namespace_id` is treated as insert-once identity, not a governance key (CLOCK-03) — say if you want it changed.
- Revoke then re-grant: the closed window keeps the old role for edits stamped inside it (CLOCK-02) — say if you want it changed.

### NET
- An advertised relay becomes home-eligible when every verse you are in advertises it; Settings toggle default ON — say if you want it changed.
- Only the download-rate cap for P2P tileset downloads (per download, not global); no full FetchStrategy rebase — say if you want it changed.
- Gossip signing is compat (`FE_SYNC_GOSSIP_SIGNING` unset); no date set to require it — say if you want it changed.
- Unsigned v1 `__relays` rows stay admissible for one release (`FE_SYNC_RELAY_ADVERT_SIGNED=require` refuses them) — say if you want it changed.
- No provisioned production relay host before the 2026-12-31 flag day — say if you want it changed.
- Advertised relays default to dial-allowlist only (`FE_SYNC_RELAY_ADVERTISE_POLICY` unset = dial-only; `pinned` inserts, `deny` ignores) — say if you want it changed.
- Only a Manager or above can advertise a relay; relay id is ASCII alphanumeric, at most 64 bytes; note at most 256 bytes — say if you want it changed.
- The embedded relay refuses to start unless access is members-only, hosted verses are named and metrics bind is loopback — say if you want it changed.
- `FE_LOG_FORMAT` unknown values fall back to text silently — say if you want it changed.

### HEXON
- Licensed tilesets: an "entry" is a member path inside the archive (preview tiles via `free_entries`) — say if you want it changed.
- REST asset fetch of a licensed entry answers 403 `entitlement_required` (no token header) — say if you want it changed.
- Snapshot import needs Manager+ on an already-existing parent fractal; it never creates a fractal or verse — say if you want it changed.
- Snapshots include iot_reading rows and only roles scoped exactly at the petal — say if you want it changed.
- `crate_registry` / `crate_entry` rows change to composite ids with no migration — say if you want it changed.
- A built package is sealed, a publisher DID mismatch is silently corrected to the signer, and an unsealed archive opens as unverified — say if you want it changed.
- Hexon builder default licence is Free and `min_engine_version` is empty — say if you want it changed.

### EXT
- A bound `portal.open` bundle with no repository node stays "bundle not on this peer" (fail closed) — say if you want it changed.
- Copy-for-BI card does not mint share links in-app; docs explain the token route — say if you want it changed.
- The seeded Genesis Verse stays local-only, so Generate invite is disabled with a reason — say if you want it changed.
- `blob.mirror` mirrors only blobs committed on this host — say if you want it changed.
- An expired issuer with a still-valid stored token shows Held — say if you want it changed.
- Window title peer label comes from `FE_PEER_LABEL` only — say if you want it changed.
- The Azure signing secret keeps its current name and is allow-listed in the leak grep — say if you want it changed.

### CITY
- Road tool hotkey is L (B is Brush, A/D fly keys); alternatives T or O — say if you want it changed.
- Pen anchors also snap to the terrain surface — say if you want it changed.
- Map world scale applies to new nodes even when the petal's terrain is disabled — say if you want it changed.
- Road snap radius is 3 m in world space (not screen space) — say if you want it changed.
- Road needs edit rights; new tracks are named "Road N"; Backspace undoes in-flight points first — say if you want it changed.
- An unparsable world scale falls back to 1.0 — say if you want it changed.

### UI
- Cross-sections (UI-03..UI-05) ship now; answer "spin out" to drop them — say if you want it changed.
- Section line is a chord through the volume centroid at an angle you set — say if you want it changed.
- Access home stays as the Access tab of Entity Settings, linked from the hierarchy info card — say if you want it changed.
- "Managers only" preset sets default access none and revokes explicit grants below Manager at that scope, behind one confirm — say if you want it changed.

### DATA
- No trigger ops exposed on REST/MCP (T5-TRIGGER option a) — say if you want it changed.
- Final coverage passes and the fresh Both scenario stay out of round 4 — say if you want it changed.
- QA-0116, QA-0128, QA-0144 are left to QA triage — say if you want it changed.

### NEW
- Dead-letter Retry sends again without Undo history or selection re-bind — say if you want it changed.
- Retry bounds 8 attempts (50 ms doubling to 2 s), 32 dead letters, session only — say if you want it changed.
- Registry installs capped at 64 MiB, whole-package progress only — say if you want it changed.
- Earthwork/callout follow-up writes stay blocking with the node's petal — say if you want it changed.

### REL
- crate consolidation G-1/G-2/G-3 all DEFER — say if you want it changed.
- Linux release signing with minisign and a key you generate — say if you want it changed.
- Windows signing via Azure Trusted Signing; unsigned until you set the secrets — say if you want it changed.
- First tag is `v0.1.0-qa.YYYYMMDD`, `v0.1.0` only after QA passes — say if you want it changed.
- CI test sweep stays scoped and README says so — say if you want it changed.

### Questions the slices raised
- CITY-05: Confirm that Pen anchors should also follow terrain height, not only Road (Q26). — default taken: the slice carried on with its stated assumption — say if you want it changed.
- R-CITY-2: Spec FR-8 binds A to toggle angle snap, but A is the camera fly-left key. Keep angle snap as a panel-only toggle (implemented), or choose another key? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- R-CLOCK-1: editor_since/manager_since (QA-0131) still come from the wire unproven and can seed a later proven window, bounded by the grantor's authority at that time. Accept this, or re-derive them locally even though fresh peers would then refuse pre-join edits by live members? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- R-CLOCK-1: Proving windows locally means a peer that joins after a revoke refuses that member's edits from before the revoke, while older peers keep them. Accept this divergence (current steer), or allow wire windows that the grantor's authority-at-time check alone bounds? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- HEXON-14: The REST asset route refuses licensed non-free entries with 403 because REST has no licensee token. Should API callers be able to send an entitlement token (for example in a header)? This is plan §9 item 14. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- I-DOC-API: EXT-05 and HEXON-06, NEW-05, UI-07, CITY-01..13 NEEDS lines are not in this worker's inputs. Their exact text needs to be supplied before those edits can land in fe-api, fe-runtime, fractalengine, portal_bundle or tools/qa/portal AGENTS files. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- UI-12: Ratify the two open questions in spec section 7 (node chains vs paths for utility segments; per-node vs per-layer utility_type) before the trigger fires? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- REL-10: G-1 (fold fe-plugin-test into fe-plugin as a test-utils feature): ACCEPT or DEFER? Default DEFER because the OSS plugin-author closure would grow. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- REL-10: G-2 (fold fe-hexon-registry into fe-hexon as a feature-gated bin): ACCEPT or DEFER? Default DEFER because the registry image must stay free of the engine closure. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- REL-10: G-3 (fold fe-query into fe-api): ACCEPT or DEFER? Default DEFER because fe-database also depends on fe-query, so the merge inverts layering. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- I-BATCH-1: Should the upstream NEEDS lines from CLOCK-03, DATA-02, EXT-01, EXT-04, EXT-08, EXT-14, HEXON-03, HEXON-09, UI-05, UI-06, UI-07 and CITY-01..13 be re-sent to this slice? They were not in this worker's inputs, so the batch registry may miss edits they asked for. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- I-DOC-UI: Re-issue the upstream NEEDS lines (DATA-01, DATA-02, HEXON-16, HEXON-19, NEW-01, NEW-04, UI-05 to UI-08, UI-10, CITY-13) for dialogs/ and editor_batch/ AGENTS.md, since the reports were not available to this slice. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- NET-37: Confirm the restart-on-revoke wording in docs/relay-operations.md §6 (the plan's wording has no code backing; the doc describes live revocation plus a SIGTERM fallback). — default taken: the slice carried on with its stated assumption — say if you want it changed.
- NET-37: Confirm the default flip trigger for FE_SYNC_GOSSIP_SIGNING quoted in SECURITY.md (next minor release, or every declared peer latched). — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-CLOCK: R-CLOCK-1 R6: accept editor_since/manager_since from the wire as a bounded trust (current code), or re-derive them locally and refuse pre-join edits on fresh peers? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-CITY: Confirm the OQ defaults recorded in both metadata.json notes (hotkey L, world-space 3 m radius, meters-only, height field, single write path). — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-HEXON: Confirm the defaults recorded in the unification metadata notes (turn4-plan.md section 9 q13, q14, q15, q16, q17) before the next round builds on them. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-REL: Do you want 6.4 (placeholder-signature register) to be rebuilt from the 45-hit list now, or handed to a later slice? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- I-BATCH-2: Should usage_middleware.rs get tests (NET-03 gate) or is it covered by usage.rs and network_ops tests only? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-UI: Should Task 4.9 (inspector 'Refactor and polish') be ticked without a separate polish pass, or kept open until one is run? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-UI: Should terrain S10 be ticked once R-UI-2 F6 (mount render_section_profile_block in right_sidebar.rs) lands, or is the section panel to be spun out with UI-12? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-EXT: NEEDS I-CONDUCTOR: conductor/tracks.md extension_platform_20261107 row should read 40 of 43 done, 3 open (P4.4, P6.3, P6.5). — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-EXT: NEEDS I-CONDUCTOR: doneness-ledger.md and accountability-log.md: record the 40 ticks and the portal T3.2/T6.2 ticks; T1.2 and T2.3 annotations. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-EXT: NEEDS I-CONDUCTOR: qa/findings.jsonl: no new rows from this bookkeeping slice. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- T-NET: Should P11 and P13 stay open until R6-05, R6-10 and R6-11 land (current choice), or should they be ticked as code-landed with the defect noted? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- I-CONDUCTOR: Which six tracks does UI-13 mean? I assumed the list in the assumptions above; confirm or correct it. — default taken: the slice carried on with its stated assumption — say if you want it changed.
- C-NET: Plan §9 q12: do you want a provisioned relay host and domain before the 2026-12-31 iroh flag day, so the embedded members-only relay can be exercised outside in-process tests? — default taken: the slice carried on with its stated assumption — say if you want it changed.
- C-NET: Plan §9 q10: which release or date should flip FE_SYNC_GOSSIP_SIGNING (and FE_SYNC_RELAY_ADVERT_SIGNED) from compat to require? — default taken: the slice carried on with its stated assumption — say if you want it changed.

## 6. Test batches to name

- **R1** hierarchy and navigation (CITY-02 placement default, `handlers::crud::`).
- **R2** scene editing (CITY-04 `node_manager::ground_snap`, CITY-10 dispatch/router, DATA-02 `editor_batch::`, NEW-01..04 door/undo/stamp journeys, HEXON-19 `actions::dimension_callout`).
- **R3** inspector (UI-06 `panels::hierarchy_info`, DATA-01 property form).
- **R4** terrain and maps incl. IoT sub-batch (UI-01..05 volume/section profile, EXT-10/11 readings, HEXON-15 tileset entitlement, fe-terrain scale tests; needs `--features render`).
- **R5** paths and stamps (CITY-05..13 road builder, CITY-08 gpx_bridge, CITY-09 id test in `gis/mod.rs`).
- **R6** dialogs and shell (CITY-10 toolbar/tool_options/tool_inspector, EXT-14 `window_chrome::`, NET-31 `dialogs::peer_debug`, UI-07/08, DATA-03, EXT-18).
- **R7** roles (UI-07 `role_propagation_durable_test`, CLOCK-04 `role_propagation_local_test`, CLOCK-02 role windows).
- **R8** P2P sync (NET-05/18/20 fe-runtime filters, NET-06/07/08 relay book, NET-11 tileset transfer, relay_serve with `--features relay-server`, NET-33 `peer_relay_e2e` slow, NET-30 `relay_serve_bridge`, CLOCK-11 replicator).
- **R9** API and MCP (NET-01 `usage::`, NET-04 `metrics::`, NET-09/25/32 relay and diagnostics routes, HEXON-12 `export_share_test`, EXT-05, EXT-13).
- **R10** identity and persistence (CLOCK `op_log::`, `handlers::replicated_row::`, `field_clock::merge::tests::`, HEXON-09/10/13 `petal_snapshot_test`).
- **R11** plugin, hexon, format (fe-format scale and hexon type tests via `-- --skip portal_bundle::`, fe-hexon package/manifest/signature tests, HEXON-01/02/03); **R11-STORE** (`fe-hexon --features remote --test remote_registry_test`, EXT-04, HEXON-03, NEW-05); **R11-ENT** (EXT-01 governance with `--features entitlement`, HEXON-14, `entitlement_gate::tests::`).
- **Portal QA lane** (not cargo): EXT-05 `portal_ipc_post` journey; EXT-06/07 spikes on your Windows box.
- **Misc**: EXT-08 docs link check (`tools/qa/docs_link_check.py`, no cargo).
- **Harness / Slow**: `two_peer_tileset_stream` and other two-peer scenarios; campaign-end only.
- **Simulator** (when you name it): `p2p-core` (CLOCK-12), `fleet-terrain` (EXT-15).
- **Full sweep**: `tools\qa\test_batches.ps1 -Batches All`, once, at campaign end.

### Batch ids named in worker QA lines
- **R4** named by 14 slices (EXT-16, HEXON-15, HEXON-16, NEW-05, R-CLOCK-2, R-EXT-1, R-HEXON-3, R-NEW-2, ...).
- **R6** named by 11 slices (CITY-13, DATA-03, EXT-05, I-UI-1, NET-30, NET-31, R-CITY-2, R-UI-3, ...).
- **R11** named by 10 slices (EXT-02, EXT-03, EXT-04, HEXON-04, HEXON-08, HEXON-11, HEXON-14, R-EXT-2, ...).
- **R8** named by 23 slices (CLOCK-11, I-BIN-2, NET-13, NET-14, NET-15, NET-16, NET-19, NET-20, ...).
- **R5** named by 4 slices (CITY-11, CITY-12, I-BATCH-1, R-CITY-2).
- **R7** named by 4 slices (CLOCK-04, R-CLOCK-1, R-UI-3, UI-07).
- **R9** named by 10 slices (EXT-13, HEXON-06, HEXON-12, I-API-2, I-API-3, NET-25, NET-32, R-EXT-3, ...).
- **R10** named by 8 slices (CLOCK-05, CLOCK-06, CLOCK-07, CLOCK-09, CLOCK-10, HEXON-13, I-BATCH-1, R-CLOCK-2).
- **R3** named by 11 slices (DATA-01, DATA-02, DATA-05, EXT-10, EXT-11, EXT-12, EXT-13, I-UI-1, ...).
- **R11-STORE** named by 2 slices (HEXON-13, R-HEXON-1).
- **R1** named by 4 slices (CLOCK-08, CLOCK-09, DATA-04, R-DATA-1).
- **R2** named by 7 slices (I-UI-1, NEW-01, NEW-04, R-CITY-2, R-INT-1, R-INT-3, R-NEW-1).

### Round 5 batch ids
- **R1** CITY5-01 `verse_manager::db_results::tests` (request_failed_clears_the_matching_road_create); DATA5-01 `handlers::stamp_transform::`.
- **R2** `actions::node::` and `node_manager::tests::` (NEW5-03, NEW5-05 door and undo legs), `node_manager::viewport_pick` (NEW5-04), `handlers::editor_batch::` (INT5-15).
- **R3** UI5-02 `hierarchy_info` scope row and the token-revoke journey.
- **R4** (`--features render`) UI5-03 `proposal_layer_mirror`, UI5-04 `section_responder::tests`, HEXON5-07 `entitlement` / `composite` / `registry`.
- **R5** CITY5-02/03/05 `road_chain`, `road_overlay`, `curve`, `road_interaction`, `gpx_bridge::`.
- **R6** NET5-08 / INT5-06 `settings::`, UI5-01 `dialogs::entity_access`, `dialogs::tests::` (DATA5-02, INT5-15), UI5-03 `ui_shell::`.
- **R7** UI5-05 `role_propagation_durable_test`, CLOCK5-02 `role_propagation_local_test`, `role_grants` windows (fe-policy).
- **R8** NET5-01 `relay_advertise::`, NET5-03/04 `relay_server` and `peer_relay_e2e_test`, NET5-06/07 `sync_thread` legs, NET5-02 `reachability`.
- **R9** NET5-05 `network::`, INT5-02 `mcp_integration`, INT5-14 `api_integration`, HEXON5-03/04/05 `export_share_test`, fe-runtime `app::` correlation.
- **R10** `node_lifecycle_test` (CLOCK5-01, DATA5-01, CITY5-04), `petal_snapshot_test` (HEXON5-03/08, INT5-13), `handlers::crate_registry::` (HEXON5-01), `field_clock::` (CLOCK5-04/05), `space_manager` (HEXON5-08).
- **R11 / R11-STORE** fe-format `signature` (HEXON5-06) and `snapshot` (HEXON5-09); `fe-hexon --features remote --test remote_registry_test` (NEW5-07); registry uninstall leg (HEXON5-01).
- **Portal QA lane** `fe-webview --test portal_ipc_journey` (EXT5-02), `run.py validate portal-core` (EXT5-03). **Simulator** (on your go): `fleet-terrain` (EXT5-06), `p2p-core` (CLOCK).
- The filters above come from `.omc/plans/turn5-plan.md` section 8; I5-BATCH registers the exact strings in `conductor/test-batches.md`, so check them there before running.

## Round 5

Added by the round-5 scribe from the T5 slice reports and the scoper entries. Nothing below has been run. Same line format and kinds as round 4; `(derived)` = the slice left no QA line.

### per_key_clock_20261113
- [ ] CLOCK-03 · batch · Run `tools\qa\test_batches.ps1 -Batches R10` (reaches fe-database `--lib field_clock::`, incl. `merge::tests::fold_is_order_independent`) -> `fold_is_order_independent` passes: the per-key winner is identical for every permutation of versions, including inadmissible newer ones.
- [ ] CLOCK-02 · batch · (derived) `cargo test -p fe-policy -- role_grants` (batch R7) -> grant-window tests pass: cap 8 drops the oldest, the frontier clamp, `as_of` / `held_at`, a Manager break closes the earlier Editor span.
- [ ] CLOCK-12 · sim · `p2p-core` steps 14p, then 13a/13b together: two editors write different keys of one node, then one is revoked -> both keys survive on every peer after the revoke and after a peer restart; the revoked author's later write is refused.

### relay_operability_20261031
- [ ] NET-02 · app · Start the GUI with `FE_LOG_FORMAT=json` and again unset, piping stdout of each run to a file; repeat for `fractalengine-relay` -> every line of the json run parses as a JSON object; the unset run keeps the plain text format.
- [ ] NET5-01 · batch · `cargo test -p fe-sync relay_advertise::tests::relay_advertisement_book_journey` (R8) -> a Manager's withdrawal that arrives before the row it withdraws still leaves the row unadmitted; a later re-advertisement is admitted.

### peer_networking_support_20261031
- [ ] NET5-02 · app · Open Settings > Network on the default build and wait about one minute -> the reason line changes from 'Waiting for the reachability check.' to a verdict-based message; on a publicly reachable host the 'Serve as a relay for my verses' box becomes enabled.
- [ ] NET5-08 · app · On a relay-server build, tick 'Serve as a relay for my verses' as the owner of two verses -> the consent text lists both verse names and Confirm is enabled; with no owned verse it says nothing would be served and Confirm is disabled. After a restart and a passing dial-back the log line lists the same served set.
- [ ] NET5-03/04 · batch · `cargo test -p fe-sync --features relay-server -- relay_server`, then `--test peer_relay_e2e_test` (R8) -> a snapshot pushed before `install_gate` still admits members; a revoke through the production reconciler disconnects within 5 s and a reconnect is denied by name.
- [ ] NET5-05 · api · MCP `tools/list` with a Viewer token, then `tools/call get_network_diagnostics` -> the tool is listed; peers and hazard verses are limited to readable verses; no raw IPs unless `FE_API_NETWORK_DETAIL=1` with an Owner token; with the bridge gone it answers 'network diagnostics unavailable' at once.
- [ ] NET5-06 · sim · A verse member advertises a DialOnly relay whose DNS answers a private address, then lists peers -> the relay never appears in the peer-address allowlist; one warn names the URL and reason; a public relay is allowlisted.
- [ ] INT5-04 · sim · Revoke a peer while relay admission is buffering its snapshot -> the peer stays out of relay admission (fail closed).

### hexon_unification_20260716
- [ ] HEXON5-03 · api · Import a snapshot as Manager, re-export the imported petal (`GET /api/v1/petals/{id}/snapshot.hexon`), compare `snapshot/oplog.jsonl` with the original, and count live `op_log` rows before and after -> every original history entry is present in the same lamport order; the live `op_log` gains only this node's own ImportPetal and ExportPetal entries (history stays in `op_log_imported`).
- [ ] HEXON-12 · api · Bind a scene hook that admits node creation WITH an augment, then `POST /api/v1/petals/import-snapshot` as Manager; then bind one that denies creation and repeat -> 409 'hook-augment-unsupported' for the first, the shared hook refusal for the second; no petal row and no new blob files in either case.
- [ ] HEXON-05 · batch · R11 `cargo test -p fe-hexon -- --skip entitlement_client::tests:: --skip entitlement_gate::tests::` -> it reaches `handlers::tests` (Script and PetalSnapshot give the explicit 'no install handler' error) and `p2p::announce::tests` / `p2p::discover::tests` (hexon_id URIs, `Option<HexonType>` kind filter); all pass.
- [ ] HEXON5-04 · api · After HEXON5-03/04 land: import a snapshot as a Manager token, then `GET /api/v1/petals/{id}/imported-history`; repeat as an Editor -> the Manager sees entries ascending by lamport with `imported_by` = that token's DID and one shared `import_id`; the Editor gets 404.
- [ ] INT5-14 · batch · Run the `api_integration` journey with the hexon-registry seam (R9) -> the imported-history leg answers tokenless 401, editor 403 or 404, owner 200.
- [ ] INT5-13 · api · As Manager `POST /api/v1/petals/import-snapshot` with an archive whose petal already exists, then with one a bound scene hook denies -> the existing petal gives 409; the hook denial gives the scene-denied status, never a 504; no blob is written in either case.

### extension_platform_20261107
- [ ] EXT5-03 · sim · `python tools/qa/sim/run.py validate portal-core` -> no errors; each `portal_ipc_post` pin names the bundle open at that step (dashboard for r_unsigned, Actions for r_frame..r_control); then `portal-core` step 2 gives no 409 `portal_fixture_mount_mismatch`.
- [ ] EXT5-04 · app · Open entity settings of a seeded local-only verse, Access tab -> 'This verse is local only. Create a new verse to share with peers.' shows and Generate invite is disabled with that hover text; a verse with a replica namespace shows neither.
- [ ] EXT5-06 · sim · `python tools/qa/sim/run.py validate fleet-terrain`, then step 1 `upload_gpx` for gpx_a/b/c -> no errors, the scenario uses `upload_gpx` on `tools/qa/sim/assets/roads/*.gpx` with no inline base64, and `lookup_node` finds Road A, Road B and Road C.

### portal_surface_20261107
- [ ] EXT5-02 · app · Hide and re-show a portal via the `qa_rows` hidden row (R6 journey) -> the portal returns to its prior visibility and `is_visible` reports the state the sender expects.

### road_builder_ux_20260716
- [ ] CITY5-01 · app · Road tool (L) with the create refused (role downgraded to Viewer after arming, or a read-only DB): click once and watch the toast; then switch to a writable petal and click again -> the failed create shows an error and the Road/Pen tool is free (no stuck first point); the next click creates 'Road 1'.
- [ ] CITY5-02 · app · 1:100 petal, Road tool, Freeform, spacing 2 m in Options, drag a 40 m stroke, End road, open the track's points -> about 20 points roughly 2 m apart; the ghost during the drag has the same density as the committed line.
- [ ] CITY5-05 · batch · R1 (`verse_manager::db_results::tests`) and R5 (`road_chain`, `road_overlay`, `curve`, `gpx_bridge::`) -> green, including `request_failed_clears_the_matching_road_create` and the 1:100 freeform rows; a create correlated `road-track:1` yields `path_kind = "designed"`.

### map_scale_authority_20260716
- [ ] CITY5-04 · app · 1:100 petal: Add node from the toolbar on an empty petal, then select it at once without reloading -> the Inspector shows scale 0.01 / 0.01 / 0.01 immediately (the I-DB-1 line covers placement, this covers the UI echo).
- [ ] CITY5-04 · api · `PUT /api/v1/petals/{p}/terrain` with `world_scale` 1e300, then 1e-300, then 0.01 -> the first two are refused as InvalidConfig; 0.01 is accepted (SEC-2 covers read-side sanitising, this the write refusal).

### inspector_settings_20260419
- [ ] UI5-02 · app · Select a node and open the Inspector Entity section -> the first line reads 'Node: <name>' above the Node ID row.
- [ ] UI5-02 · app · With a petal active, re-click the verse row to open the verse card -> no 'Your role' line (the active scope is the petal; the petal card shows it); the verse card reads '<n> online of <m> peers'.
- [ ] UI5-02 · app · With the DB thread unavailable, click Revoke twice on an API token -> a visible error line appears in Active Tokens; the token row is not silently left unrevoked.
- [ ] UI5-01 · app · Entity Settings > Access on the seeded Genesis Verse -> the local-only reason shows and Generate invite is disabled with a tooltip; a verse with a replica namespace has it enabled.
- [ ] INT5-08 · app · Select the verse card in the Inspector while a petal is active and your only grant is on the petal -> Settings is disabled with a reason.
- [ ] INT5-09 · app · Open Settings > Access on a verse created without a replica namespace -> the local-only note appears.
- [ ] INT5-12 · app · Demote yourself to Viewer while a node property draft is open in the Inspector -> Save is disabled for the draft; the values stay visible.

### terrain_proposal_delta_viz_20261025
- [ ] UI5-03 · app · With at least one proposal, hide the Proposal overlay layer in Maps, then open Terrain tools -> 'Cut/fill colors' is replaced by 'Proposal layer is hidden'; showing the layer again restores the checkbox with its previous state.
- [ ] UI5-03 · batch · R4 `cargo test -p fe-terrain --features render` and R6 `cargo test -p fe-ui --lib -- ui_shell::` -> the proposal-layer mirror rows (none/visible/hidden/shown) and the checkbox visibility rule pass; removing the mirror system turns a row red.
- [ ] UI5-04 · batch · R4 `cargo test -p fe-terrain --features render -- section_responder::tests` -> the section profile and the volume report agree on the proposed surface for the same footprint.
- [ ] UI5-05 · batch · R7 `cargo test -p fe-database --test role_propagation_durable_test` -> Access Load returns 256 grants for a scope seeded with 300; the three numerically newest hlc strings ('1000'..'1002') are present.

### endpoint_api_surface_20260725
- [ ] DATA5-01 · app · Open a petal with a promoted stamp; via REST (or MCP `update_transform`) send the stamp's own position with rotation [0,1,0] and scale [2,2,2] while the viewport is open, then switch petals and back -> the stamp rotates and scales live and keeps that pose after the respawn.
- [ ] DATA5-01 · api · REST transform on a promoted stamp with scale 20000, then 0.001, at its own position -> both refused (range 0.01..=10000 in `handlers/stamp_transform.rs`), nothing written to the row or properties; a plain node accepts the same values.
- [ ] DATA5-01 · batch · `cargo test -p fe-database --lib -- handlers::stamp_transform::` and `cargo test -p fe-ui --lib -- dialogs::tests::` -> the `euler_to_quat_xyzw` table and the TreeAppearance layout leg both run (non-zero test count) and pass.
- [ ] INT5-02 · api · MCP `get_network_diagnostics` as Viewer, then anonymous -> the tool is listed and redacted for the Viewer; unknown tool for anonymous.

### project_completion_20261104
- [ ] NEW5-05 · app · (derived) Stall the DB thread, edit one node, then a second before the first save resumes -> the saves reach the DB in the order made.
- [ ] NEW5-07 · api · (derived) Install from a registry whose download response has no Content-Length -> the Downloads row count grows, the total fills in, and it ends N/N before the map is listed as Installed.
- [ ] INT5-06 · sim · Demote a peer's Editor grant on a petal holding that peer's nodes, then watch the GUI -> the removed nodes disappear via NodeDeleted without a reload.
- [ ] DATA-04 · sim · Move a stamp node with the gizmo and read its `stamp.override.scale` and `stamp.override.rotation` properties -> both change with the transform and replicate.

### release_ci_20260429
- [ ] REL5-01 · api · Dispatch the Release workflow manually from a tag ref in a fork (Actions UI, Run workflow, pick the tag) -> builds run, the publish and GHCR jobs are skipped; a real tag push still publishes.

### oss_release_20260717
- [ ] REL-03 · app · Open `docs/diagrams/01-system-overview.md`, `06-network-topology.md` and `09-sync-and-caching.md` in the GitHub preview -> all three mermaid diagrams render (subgraphs T1-T7, iroh inside T4, no mDNS or cache-tier nodes).
- [ ] REL5-04 · batch · `git grep -c '"00".repeat(64)' -- 'fe-*' 'fractalengine*' tools extensions`, sum the counts -> equals the totals in `conductor/tracks/oss_release_20260717/spec.md` FR-2 and `SECURITY.md` (51 at the time of writing).
- [ ] REL-02 · api · Read the README 'Test / Lint / Format' block as a new contributor and run only what it lists -> no command requires clippy; the CI scoped-sweep sentence is present.
