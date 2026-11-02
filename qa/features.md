---
type: FeatureCatalog
title: FractalEngine user and agent feature coverage
date: 2026-11-01
source_commit: 214045aab523259c56c701c8fe9741117e38a7d0
catalog_source_fingerprint: pending_main_integration_review
execution_status: NOT_RUN
---

# Feature catalog

This is the source-grounded coverage contract, not a QA result. Its 132 capability families expand into the
explicit checkpoints in [feature-checkpoints.json](feature-checkpoints.json). Each checkpoint has a stable ID,
region, current reachability, entry, required outcome and source paths. The family table below links each set to
one long region journey. Historical focused passes do not automatically cover these newly specified variants.

`Y` means an established GUI route; `A` means API/CLI/QA-tool only; `O` means an OS accessibility/performance
acceptance check; `P` means planned; `U` means no production route established. All records start NOT_RUN. A
currently broken GUI control stays Y and fails its required outcome; it cannot be excluded merely because it is
broken. A visible but incomplete Polygon mode therefore remains a required case. A genuinely unrendered helper
or enum is U with a reason. Planned routes must become Y and gain concrete checkpoints when W2–W6 enables them.

GUI coverage for a pass is PASS Y checkpoints divided by every current Y checkpoint. API/QA and OS checks have
separate denominators and remain mandatory acceptance work. P/U records are audited exclusions, not passes.
Variants are explicit rather than an implicit Cartesian product: each listed role, mode, operation, state or
route must execute; repeated instance names with identical behavior do not create infinite cases. Every family
is covered only when all its current variants pass. Refresh this catalog after every polish wave and recheck
the full UiAction/rendered-control/MCP inventories; do not freeze a pre-polish denominator.
The JSON catalog's `catalog_source_fingerprint` is deliberately null until main integration/source reconciliation.
The final strict checker requires that reviewed fingerprint to match the actual staged source; ledger provenance
alone cannot establish that an old catalog describes newly implemented features.

For each pass write `qa/passes/<NN>/checkpoints.jsonl`, one final record per checkpoint, with `checkpoint`, `outcome`
(PASS/FAIL/BLOCKED/UNKNOWN/NOT_RUN), current `pass_id`, `stage_commit`, `stage_fingerprint`, declared owned `run_id`, actual `commands`, structured
`assertions`, hashed `artifacts`, explicit `visual_verdict`, `log_watermark`, `new_logs` and non-PASS `reason`.
The exact PASS schema and summary command are in [the shared procedure](scenarios/README.md#evidence-schema-and-reproducible-coverage).
A secret-free non-execution example:

```json
{"checkpoint":"F054.raise-square-UI","outcome":"NOT_RUN","pass_id":"NN","stage_commit":null,"stage_fingerprint":null,"run_id":"<owned-run-id>","commands":[],"assertions":[],"artifacts":[],"visual_verdict":null,"log_watermark":null,"new_logs":[],"reason":"example only; replace pass/run context before use"}
```

PASS needs the actual UI action, read/judged PNG, authoritative state and durable readback where relevant. Fixture
setup via API, source inspection, a scenario's existence, a stale build or an unknown layout fact cannot prove a
checkpoint. An unobservable applicable assertion remains UNKNOWN. Whole-pass coverage cannot inherit old pass
records. Layout-check zero measured violations with incomplete coverage is a bounded result, not blanket success.

The pass verdict lists denominator/numerator per R1–R11 and separately API/OS, every blocked/unknown checkpoint,
new findings, and gold tasks. Two consecutive final-code passes must each have all gold tasks PASS, GUI coverage
100%, all mandatory API/OS acceptance complete and zero new S1/S2. User-only review remains separately pending.

The source snapshot is provisional: S7 fixture scenarios are adopted as canonical supplements, while their
main integration/build/live validation was pending at authoring. S2 accessible purpose names may change while
preserving visible labels. Resolve actual name/role from the live tree and record it; an absent/ambiguous required
control is a finding, not permission to bypass that feature through API. The [source reconciliation](source-inventory.md)
contains all inventoried actions, rendered controls and MCP tools with family owners and source attribution.

## Capabilities and mandatory variants

Every suffix below completes the stable ID (`F001.tree-enter`, for example). The journey executes every listed
variant; the source column expands through the source alias table below. Required prerequisites and numeric
oracles live in the journey. All outcomes are NOT_RUN until the pass ledger supplies evidence.

| Family / region / reachability | Entry and mandatory checkpoint suffixes | Required observable outcome | Source | Journey |
|---|---|---|---|---|
| F001 / R1 / Y | Verse picker/tree/card enter/back; empty browser; long-name scroll<br>`tree-enter`, `card-enter`, `picker-enter`, `back`, `empty-browser`, `long-name-scroll` | navigation IDs/names and visible cards agree | SB, VP | [J01](scenarios/J01.md) |
| F002 / R1 / Y | Fractal row/card/back; expanded tree under different verse<br>`tree-enter`, `card-enter`, `back`, `other-verse-expanded` | complete correct ancestry, descendants clear | SB, NAV | [J01](scenarios/J01.md) |
| F003 / R1 / Y | Petal row/card/back; enter expanded petal from verse-only cursor<br>`tree-enter`, `card-enter`, `back`, `direct-from-verse-cursor` | old scene leaves/new scene appears, no mixed scope/crash | SB, NAV | [J01](scenarios/J01.md) |
| F004 / R1 / Y | Chevrons vs active/inactive names at all three levels<br>`verse-chevron`, `verse-active-name`, `verse-inactive-name`, `fractal-chevron`, `fractal-active-name`, `fractal-inactive-name`, `petal-chevron`, `petal-active-name`, `petal-inactive-name` | collapse/navigation are independently checked; current trap is a finding | SB | [J01](scenarios/J01.md) |
| F005 / R1 / Y | Create verse/fractal/petal: name/Create; blank; Enter; Escape; Cancel<br>`verse-create`, `fractal-create`, `petal-create`, `blank-name`, `enter-submit`, `escape-cancel`, `cancel-button` | exactly one persisted row or none on refusal/cancel | CE, SB, VP | [J01](scenarios/J01.md) |
| F006 / R1 / Y | Add Node vs Add GLTF Model<br>`add-empty-node`, `add-gltf-node` | distinguish empty row from asset-bearing import | SB, GL | [J01](scenarios/J01.md) |
| F007 / R1 / Y | Node row select/Options; inactive-petal node row<br>`active-node-select`, `node-options`, `inactive-petal-node` | authoritative selection/Inspector; current inactive behavior explicit | SB | [J01](scenarios/J01.md) |
| F008 / R1 / Y | Verse/fractal/petal/node eyes; mixed rollup; active-petal guard<br>`verse-eye`, `fractal-eye`, `petal-eye`, `node-eye`, `mixed-rollup`, `inactive-petal-guard` | effective visibility + persisted overrides/ancestors correct | SB, VIS | [J01](scenarios/J01.md) |
| F009 / R1 / Y | H; Shift+H; Alt+H; Escape isolate<br>`hide-H`, `solo-ShiftH`, `show-AltH`, `escape-solo` | durable hide/show, session-only solo, no hidden-object pick | KS, VIS | [J01](scenarios/J01.md) |
| F010 / R1 / Y | Manage Groups; Add; rename; eye; up/down; Delete<br>`open-groups`, `add`, `rename-enter`, `rename-escape`, `visibility`, `move-up`, `move-down`, `delete-cancel`, `delete-confirm` | registry name/order/visibility persists | RS | [J01](scenarios/J01.md) |
| F011 / R1 / Y | Selected-node group join/leave; cap/empty/denied<br>`join-group`, `leave-group`, `empty-groups`, `group-cap`, `viewer-denied` | membership/effective visibility and disabled reason | RS | [J01](scenarios/J01.md) |
| F012 / R1 / Y | Reachable entity/node rename/delete including active ancestor<br>`rename-node`, `rename-entity`, `delete-node`, `delete-active-petal`, `delete-active-fractal`, `delete-active-verse` | no stale nav/index/entities or deleted husk | NO, ES | [J01](scenarios/J01.md) |
| F013 / R1 / P | Sidebar row Rename/Settings/Copy ID/Delete menus<br>`row-rename`, `row-settings`, `row-copy-id`, `row-delete-cancel`, `row-delete-confirm` | W2-S1 adds routes; refresh denominator after landing | SB | [J01](scenarios/J01.md) |
| F014 / R2 / Y | Select S; Move G; Rotate R; Scale X; repeated-key toggle<br>`select-button`, `select-S`, `move-button`, `move-G`, `rotate-button`, `rotate-R`, `scale-button`, `scale-X`, `pen-button`, `pen-P`, `brush-button`, `brush-B`, `measure-button`, `measure-M`, `repeated-key-toggle` | tool/Options/selection consistent | TB, KS | [J02](scenarios/J02.md) |
| F015 / R2 / Y | Viewport select/empty deselect; sidebar select; Deselect<br>`viewport-node`, `viewport-empty`, `sidebar-node`, `deselect-button`, `hidden-node-refusal` | NodeManager/Inspector same node; hidden exclusion | NP, SB, TOP | [J02](scenarios/J02.md) |
| F016 / R2 / Y | Orbit/pan/wheel zoom; UI wheel capture<br>`orbit`, `pan`, `viewport-wheel`, `inspector-wheel`, `floating-window-wheel` | camera changes only intended components | CAM | [J02](scenarios/J02.md) |
| F017 / R2 / Y | Each Move/Rotate/Scale gimbal axis; Escape cancel<br>`move-X`, `move-Y`, `move-Z`, `rotate-X`, `rotate-Y`, `rotate-Z`, `scale-X`, `scale-Y`, `scale-Z`, `escape-cancel` | committed REST transform matches viewport; cancel no commit | GI | [J02](scenarios/J02.md) |
| F018 / R2 / Y | Inspector Position/Rotation/Scale XYZ, Apply/Enter, invalid input<br>`position-X`, `position-Y`, `position-Z`, `rotation-X`, `rotation-Y`, `rotation-Z`, `scale-X`, `scale-Y`, `scale-Z`, `apply-button`, `enter-commit`, `invalid-number` | exact typed transform and units/readback | IN | [J02](scenarios/J02.md) |
| F019 / R2 / Y | GLTF path/Browse/name/Import, valid/invalid/cancel<br>`browse-valid`, `typed-valid-path`, `import-name`, `import-success`, `browse-cancel`, `malformed-gltf`, `no-petal`, `no-path` | renderable one-node import and scoped run-local answer | GL | [J02](scenarios/J02.md) |
| F020 / R2 / Y | Ground Add Empty Node / Add GLTF Model<br>`ground-empty-node`, `ground-gltf-node` | correct world placement and parent | CM | [J02](scenarios/J02.md) |
| F021 / R2 / Y | Node Edit Properties / Rename... / Duplicate<br>`edit-properties`, `rename`, `duplicate` | correct target, new duplicate ID, rename ack | CM, NO | [J02](scenarios/J02.md) |
| F022 / R2 / Y | Clear Properties; leaf/cascade Delete; confirm/cancel<br>`clear-properties`, `delete-leaf-cancel`, `delete-leaf-confirm`, `delete-subtree-cancel`, `delete-subtree-confirm` | clearing retains node, deletion correct subtree only | CM, NO | [J02](scenarios/J02.md) |
| F023 / R2 / Y | Node Options Save/Cancel and action parity<br>`save-name`, `cancel-name`, `clear-properties`, `delete-parity` | same durable effects as context surface | NO | [J02](scenarios/J02.md) |
| F024 / R2 / Y | Webpage/config URL Save/Open Portal; invalid URL<br>`save-webpage-url`, `open-webpage`, `save-config-url`, `open-config`, `invalid-url` | correct field persists, errors visible, no false Saved | IN, NO | [J02](scenarios/J02.md) |
| F025 / R2 / Y | Portal back/close/load failure/selection change<br>`load-page`, `follow-link`, `back`, `close`, `load-failure`, `selected-node-change` | local page history and complete teardown | PT | [J02](scenarios/J02.md) |
| F026 / R2 / Y | Download asset Save/cancel/error<br>`asset-save`, `save-cancel`, `write-error`, `overwrite-after-input-release` | output bytes/hash and status, confined path | AC | [J02](scenarios/J02.md) |
| F027 / R2 / Y | Copy API String / Report / Query<br>`copy-api-string`, `report-object`, `query-object` | real egress text and toast; synthetic clipboard isolated | CM, EG | [J02](scenarios/J02.md) |
| F028 / R2 / P | Grid/angle/scale snap; axis lock; undo/redo; F; Delete shortcut<br>`grid-snap`, `angle-snap`, `scale-snap`, `uniform-scale`, `axis-lock`, `undo`, `redo`, `frame-F`, `delete-shortcut` | W2/W4/W5 planned; current hints are not working controls | TO, KS | [J02](scenarios/J02.md) |
| F029 / R3 / Y | Inspector Properties / API Access / Query; no selection<br>`properties-tab`, `api-tab`, `query-tab`, `no-selection`, `change-selected-node` | no stale selected/token/form state | IN | [J03](scenarios/J03.md) |
| F030 / R3 / Y | Property key/value/type variants, Add/delete, selection reload<br>`string`, `number`, `bool`, `datetime`, `geometry-point`, `geometry-polygon`, `blob-ref`, `hexon-ref`, `address`, `array`, `object`, `invalid-number`, `invalid-json`, `add`, `delete-cancel`, `delete-confirm`, `selection-reload` | typed JSON preserved, invalid numeric/JSON refused | IN | [J03](scenarios/J03.md) |
| F031 / R3 / Y | Field key/type/Add Field/delete/pending ID<br>`key`, `type`, `add-field`, `delete-cancel`, `delete-confirm`, `pending-id-disabled` | schema persists; no DeleteFieldDef pending-N | IN | [J03](scenarios/J03.md) |
| F032 / R3 / Y | Annotation title/body/color Save Annotation<br>`title`, `body`, `color`, `save`, `node-change` | correct node properties and visible marker/color | AN | [J03](scenarios/J03.md) |
| F033 / R3 / Y | This node / This node's device data / All nodes in this petal / no-petal template<br>`this-node`, `device-data`, `all-petal-nodes`, `no-petal-disabled` | real SQL/table/scope and independently expected rows | QT | [J03](scenarios/J03.md) |
| F034 / R3 / Y | SQL editor Run/Ctrl+Enter; empty/filter/error/timeout<br>`run-button`, `ctrl-enter`, `filter`, `empty-results`, `syntax-error`, `pending`, `timeout` | correlated result/error, no stale result, proper pending state | QT, AQ | [J03](scenarios/J03.md) |
| F035 / R3 / Y | Results open/Close Results, vertical resize/scroll/virtualization<br>`open`, `close`, `resize`, `scroll`, `virtualized-200` | count/time/header correct, viewport protected | RD | [J03](scenarios/J03.md) |
| F036 / R3 / Y | Numeric/text ascending/descending headers; column resize<br>`numeric-ascending`, `numeric-descending`, `text-ascending`, `text-descending`, `mixed-types`, `column-resize` | seeded 10/20/30 order and stable row targeting | RD | [J03](scenarios/J03.md) |
| F037 / R3 / Y | Copy cell/Copy row/context/toolbar<br>`copy-cell`, `copy-row`, `context-action`, `selected-row-toolbar` | correct selected content action; host clipboard suppressed | RD | [J03](scenarios/J03.md) |
| F038 / R3 / Y | Row select/frame/match; foreign row; nav invalidation<br>`select-current-row`, `frame-current-row`, `scene-match`, `foreign-node-refusal`, `stale-petal`, `stale-verse`, `pending-nav-change` | correct current-petal node only, camera focus, stale blocked | RD | [J03](scenarios/J03.md) |
| F039 / R3 / Y | Data Query / Annotations / Layers / Paths / Export<br>`query-tab`, `annotations-tab`, `layers-tab`, `paths-tab`, `export-tab`, `independent-buffers`, `empty-state`, `pending-state`, `error-state` | independent buffers and empty/pending/error states | GP | [J03](scenarios/J03.md) |
| F040 / R3 / Y | Data Annotated / Property typed filter / Bbox<br>`annotated`, `property-string`, `property-number`, `property-bool`, `property-json`, `bbox` | rows independently match fixture, scope and filter | GP | [J03](scenarios/J03.md) |
| F041 / R3 / Y | Bbox min/max XZ, Center on Selection/Origin, Run<br>`min-X`, `min-Z`, `max-X`, `max-Z`, `center-selection`, `center-origin`, `run`, `invalid-bounds` | numeric bounds and result inclusion exact | GP | [J03](scenarios/J03.md) |
| F042 / R3 / Y | Annotations list row selection vs Query results<br>`annotation-select`, `query-row-select`, `independent-result-buffers` | correct node/independent data; no unrelated rows | GP | [J03](scenarios/J03.md) |
| F043 / R3 / Y | Export base/source Petal/Node/Bbox/Use viewport selection<br>`api-base`, `source-petal`, `source-node`, `source-bbox`, `use-viewport-selection`, `unavailable-source` | source-specific correct SQL/URL and unavailable state | EG | [J03](scenarios/J03.md) |
| F044 / R3 / Y | All rendered BI copy rows/formats<br>`sql`, `query-endpoint`, `parquet-url`, `csv-url`, `duckdb-snippet`, `parquet-roundtrip`, `csv-roundtrip`, `encoding-auth-crs` | encoding/base/CRS/auth strings + actual API format roundtrip | EG | [J03](scenarios/J03.md) |
| F045 / R3 / Y/A | Share TTL and Mint via curl copy<br>`ttl-valid`, `ttl-invalid`, `mint-curl-copy`, `mint-api-readback` | command generation only; mint is API, invalid fallback recorded | EG | [J03](scenarios/J03.md) |
| F046 / R3 / P | Scoped GUI SQL/no active petal refusal<br>`selected-petal-scope`, `no-petal-refusal`, `foreign-table-refusal`, `cross-petal-refusal` | W5 replaces RawQuery; unsafe current path is finding | AQ | [J03](scenarios/J03.md) |
| F047 / R4 / Y | Installed Set petal map/clear; no map; petal switch<br>`set-map`, `clear-map`, `no-map`, `petal-switch`, `denied-set-rollback` | config readback and visible correct terrain | HM | [J04](scenarios/J04.md) |
| F048 / R4 / Y | Map scale preset/slider/clamp<br>`native-preset`, `scale-preset`, `scale-slider`, `invalid-scale-clamp` | native/world scale and measurement units correct | HM | [J04](scenarios/J04.md) |
| F049 / R4 / Y | Layer visible/opacity; Mesh/Splats/Hybrid<br>`layer-hidden`, `layer-visible`, `opacity`, `mesh`, `splats`, `hybrid` | persisted layer/mode and actual render change | LM | [J04](scenarios/J04.md) |
| F050 / R4 / Y | Palette Raise/Lower/Flatten/Ramp/Slope/Pad/Cut/Fill<br>`raise`, `lower`, `flatten`, `ramp`, `slope`, `pad`, `cut`, `fill` | eight proposal semantics and Proposal only disclosure | TT | [J04](scenarios/J04.md) |
| F051 / R4 / Y | Proposal radius/target/delta/Add/select/delete<br>`radius`, `target`, `delta`, `add`, `select`, `delete-cancel`, `delete-confirm` | origin 24-gon/params persist; correct selected/deleted ID | TT | [J04](scenarios/J04.md) |
| F052 / R4 / Y | Brush radius/strength/material; Raise/Lower/Level/Smooth<br>`radius`, `strength`, `material`, `raise-stroke`, `lower-stroke`, `level-stroke`, `smooth-stroke` | weighted freeform stroke, durable region node | TT, BS | [J04](scenarios/J04.md) |
| F053 / R4 / Y | Brush/Circle/Rectangle/Polygon, Apply/clear<br>`brush`, `circle`, `rectangle`, `polygon-draft`, `apply`, `clear-draft`, `insufficient-polygon` | defined shape differs from weighted stroke; incomplete Polygon producer is blocker | TO | [J04](scenarios/J04.md) |
| F054 / R4 / Y | Rectangle Radius5/Raise2/Apply; separate-petal Lower positive1<br>`raise-square-UI`, `raise-square-persistence`, `lower-square-UI`, `lower-square-persistence` | area100, fill200/cut100; exact footprint + positive oracle | TO, VR | [J04](scenarios/J04.md) |
| F055 / R4 / Y | Volume selected/drawn/region, pending/no map/overlap<br>`selected-volume`, `drawn-volume`, `region-volume`, `pending`, `no-map`, `overlap`, `missing-sample`, `late-tile-refresh` | true correlated figure, units/step, no missing-sample zero pass | VR | [J04](scenarios/J04.md) |
| F056 / R4 / Y | Volume totals/per-item/Nodes inside/Copy as TSV<br>`totals`, `per-item`, `nodes-inside`, `copy-tsv` | independent numeric/composite/containment assertions | VR | [J04](scenarios/J04.md) |
| F057 / R4 / Y | Heatmap/live preview, committed region delete<br>`heatmap`, `live-preview`, `preview-no-db-write`, `committed-region-delete`, `base-restored` | true base unchanged, preview no DB write, revert correct | TP, TT | [J04](scenarios/J04.md) |
| F058 / R4 / Y | Measure Tape/Area; anchors; Undo/Clear/Close polygon<br>`tape`, `area`, `append-anchor`, `undo`, `clear`, `close-polygon`, `independent-distance`, `independent-area` | independent distance/slope/bearing/area and closed state | TO, ME | [J04](scenarios/J04.md) |
| F059 / R4 / Y | Measurement copy rows/Save as callout<br>`copy-distance`, `copy-slope`, `copy-bearing`, `copy-area`, `save-tape-callout`, `save-area-callout`, `incomplete-disabled` | durable dimension_callout snapshots geometry | TO | [J04](scenarios/J04.md) |
| F060 / R4 / Y | Scale bar/Metric grid/Offline tiles<br>`scale-bar`, `metric-grid`, `offline-on`, `offline-off` | correct metric overlay and no network fallback | RS | [J04](scenarios/J04.md) |
| F061 / R4 / Y | Global/This petal(shared)/Session override horizons<br>`global`, `petal-shared`, `session-override`, `precedence`, `session-lifetime` | precedence, proper DB/session lifetime | RS | [J04](scenarios/J04.md) |
| F062 / R4 / A | IoT/device/sensor payload and sim ingress<br>`register-device-format`, `ingest-device-payload`, `list-device-payload`, `ingest-reading`, `query-timeseries`, `sim-start-stop-step-fault` | no dedicated GUI located; inspect ingested data via Query | API | [J04](scenarios/J04.md) |
| F063 / R5 / Y | Paths New Path/select/edit/Back/Delete<br>`new-path`, `select-path`, `edit-path`, `back`, `delete-cancel`, `delete-confirm` | track row/editing/stamps consistency | PE | [J05](scenarios/J05.md) |
| F064 / R5 / Y | Import GPX... valid/malformed/cancel<br>`import-valid-gpx`, `import-malformed`, `import-cancel`, `no-duplicate-after-failure` | real track coordinates/projection, no partial duplicate | GX | [J05](scenarios/J05.md) |
| F065 / R5 / Y | Pen P click/drag/Alt-drag corner/smooth anchors<br>`pen-P`, `click-corner`, `drag-smooth`, `alt-drag-symmetric`, `deferred-new-track` | points/handles and one deferred auto-created track | TO, PN | [J05](scenarios/J05.md) |
| F066 / R5 / Y | Polyline/Catmull-Rom/Bezier/tension/samples/Smooth path<br>`polyline`, `catmull-rom`, `bezier`, `tension`, `samples`, `smooth-path`, `disabled-without-track` | independent resampled geometry; proper disabled state | PL | [J05](scenarios/J05.md) |
| F067 / R5 / Y | New anchor and selected Corner/Smooth/Symmetric/smoothness<br>`new-corner`, `new-smooth`, `new-symmetric`, `selected-corner`, `selected-smooth`, `selected-symmetric`, `smoothness` | correct persisted handle geometry/constraints | PL, PE | [J05](scenarios/J05.md) |
| F068 / R5 / Y | Radius X/Z/Segments; Add Ellipse/Circle/Rectangle<br>`radius-X`, `radius-Z`, `segments`, `ellipse`, `circle`, `rectangle`, `no-track-disabled` | generated geometry/count; no-track state honest | PL | [J05](scenarios/J05.md) |
| F069 / R5 / Y | Point select/Y/Remove/Annotate and point/handle/segment drag<br>`point-select`, `point-Y`, `point-remove`, `point-annotate`, `point-drag`, `handle-drag`, `segment-drag`, `release-single-commit` | correct index/geometry, one release commit | PE, PN | [J05](scenarios/J05.md) |
| F070 / R5 / Y | Point annotation title/body/color Save/Cancel<br>`title`, `body`, `color`, `save`, `cancel` | waypoint at actual anchor, no cancel row | PE | [J05](scenarios/J05.md) |
| F071 / R5 / Y | Border color/width, Closed zone, Fill color/alpha<br>`border-color`, `border-width`, `closed-zone`, `fill-color`, `fill-alpha` | visible ribbon/fill and gis.track.* persisted | PE | [J05](scenarios/J05.md) |
| F072 / R5 / Y | Enable snapping/grid/anchor radius and path gimbal<br>`snapping-enabled`, `snapping-grid`, `anchor-radius`, `whole-path-gimbal` | metric placement and whole-path transforms durable | PE, PN | [J05](scenarios/J05.md) |
| F073 / R5 / Y | Asset filter/select/clear/manual blob://<br>`asset-filter`, `select-asset`, `clear-asset`, `manual-blob-uri`, `missing-asset` | actual installed model selection and missing states | PL | [J05](scenarios/J05.md) |
| F074 / R5 / Y | Fixed spacing/count/tangent align/Stamp along path<br>`fixed-spacing`, `fixed-count`, `tangent-align`, `stamp-apply`, `descriptor-readback` | instance count/positions and path_asset descriptor | PL | [J05](scenarios/J05.md) |
| F075 / R5 / Y | Stamp Index/Select/Promote/Scale/Rotate/Slide Apply<br>`index`, `select`, `promote`, `scale`, `rotate`, `slide`, `apply` | single stamp ownership/overrides and arc-length constraint | PL | [J05](scenarios/J05.md) |
| F076 / R5 / Y | Stamp context Promote/Scale-Rotate/Slide/Delete<br>`promote`, `scale-rotate`, `slide`, `delete-instance` | proper object class/scope; no accidental parent removal | CM | [J05](scenarios/J05.md) |
| F077 / R5 / Y | Path context Edit/Add Stamps/Delete Track; anchor Corner-Smooth/Delete Point<br>`edit-track`, `add-stamps`, `delete-track`, `anchor-corner`, `anchor-smooth`, `delete-point` | whole-track warning vs point deletion and correct geometry | CM | [J05](scenarios/J05.md) |
| F078 / R5 / Y | Export GPX save/cancel/reimport<br>`export-save`, `export-cancel`, `parse-export`, `reimport-roundtrip` | parsable run-local output and coordinate roundtrip | PE | [J05](scenarios/J05.md) |
| F079 / R6 / Y | Caption minimize/maximize/restore/close, drag/double-click/resize grips<br>`minimize`, `maximize`, `restore`, `caption-close`, `caption-drag`, `caption-double-click`, `resize-grips` | same owned window, exact terminal process lifecycle | TOP | [J06](scenarios/J06.md) |
| F080 / R6 / Y | Cancel close/Close FractalEngine and dirty manifest reason<br>`cancel-close`, `confirm-close`, `dirty-manifest-reason` | confirmation not bypassed; final action last | TOP | [J06](scenarios/J06.md) |
| F081 / R6 / Y | Hierarchy toggle/Ctrl-B/compact Tools/Panels/Deselect<br>`hierarchy-toggle`, `ctrl-B`, `compact-tools`, `compact-panels`, `compact-deselect` | equivalent responsive controls at all widths | TOP | [J06](scenarios/J06.md) |
| F082 / R6 / Y | Inspector/Options/Terrain tools/Volume report/Settings/Maps/Groups/Data<br>`inspector`, `options`, `terrain-tools`, `volume-report`, `settings`, `maps`, `groups`, `data`, `toggle-vs-reveal` | section toggle vs reveal, no viewport takeover | RS, TOP | [J06](scenarios/J06.md) |
| F083 / R6 / Y | Render distance/Mesh budget/Entity cap/Stamp ceiling<br>`render-distance`, `mesh-budget`, `entity-cap`, `stamp-ceiling`, `durable-readback` | actual residency plus durable settings | RS | [J06](scenarios/J06.md) |
| F084 / R6 / Y | Orbit sensitivity/Zoom speed/Relay/Hexon registry<br>`orbit-sensitivity`, `zoom-speed`, `relay-valid`, `relay-invalid`, `registry-valid`, `registry-invalid`, `save-restart-indication` | valid/invalid settings, save/restart indication | RS | [J06](scenarios/J06.md) |
| F085 / R6 / Y | Mirror/Sharded/Balanced; Durability R; bucket presets/numeric<br>`mirror`, `sharded`, `balanced`, `durability-R`, `bucket-preset`, `bucket-number`, `mode-limit` | confirmed verse settings, R mode limits | RS | [J06](scenarios/J06.md) |
| F086 / R6 / Y | Reset Database current location and confirmation<br>`reset-open`, `reset-cancel`, `reset-confirm`, `isolated-data-gone` | isolated terminal fixture only; typed RESET planned W2 | SB | [J06](scenarios/J06.md) |
| F087 / R6 / Y | Info/error notices/status/role/budget/peer segment<br>`info`, `advisory`, `error`, `error-history`, `role`, `mesh-budget`, `peers`, `counters` | actual success/error agreement, visible lasting failure | PM, ST | [J06](scenarios/J06.md) |
| F088 / R6 / Y/A | All native open/save/folder answer/cancel/empty/invalid-kind/path cases<br>`open`, `save`, `folder`, `cancel`, `empty-queue`, `kind-mismatch`, `traversal`, `junction`, `alternate-stream`, `reserved-device`, `drive-relative`, `atomic-batch` | no blocked frame, typed refusal and confinement | FD | [J06](scenarios/J06.md) |
| F089 / R6 / Y | Every panel/modal at three sizes, long names/paths, UI scale1.25<br>`size-1280`, `size-1920`, `size-1024`, `all-panels`, `all-dialogs`, `long-name`, `long-path`, `UI-scale1.25`, `compact-scale2`, `divider-endpoints` | real hit/clip/text provenance, >=45% viewport, unknowns counted | LP | [J06](scenarios/J06.md) |
| F090 / R6 / Y | Panel wheel vs viewport wheel, text-focused B/G/R/X/Enter<br>`inspector-wheel`, `viewport-wheel`, `floating-window-wheel`, `text-B`, `text-G`, `text-R`, `text-X`, `text-Enter` | no unintended camera/tool changes | TOP, KS | [J06](scenarios/J06.md) |
| F091 / R6 / Y | Escape gesture/solo/path-edit/selection/tool ladder; context/modal case<br>`gesture`, `solo`, `path-edit`, `selection`, `tool`, `context-menu`, `modal` | real staged ordering; missing modal rung is finding | KS | [J06](scenarios/J06.md) |
| F092 / R6 / P | Restart now, shortcut help, panel Retry/quarantine<br>`restart-now`, `shortcut-help`, `panel-retry`, `quarantine-recovery` | W2/W3 planned; panic=abort caveat forbids fake live proof | MOD | [J06](scenarios/J06.md) |
| F093 / R7 / Y | Entity gears at verse/fractal/petal; General/Access/API<br>`verse-gear`, `fractal-gear`, `petal-gear`, `general-tab`, `access-tab`, `api-tab`, `local-role` | correct entity scope/local role, reachable Claim limitations | ES, VP | [J07](scenarios/J07.md) |
| F094 / R7 / Y | General Rename/Public-Private/Save Description<br>`rename`, `private`, `public`, `unlisted`, `description-save`, `loading`, `error` | correct durable row and real loading/error/default state | ES | [J07](scenarios/J07.md) |
| F095 / R7 / Y | Claim Ownership/Confirm Claim/Cancel, Delete confirm<br>`orphan-claim`, `confirm-claim`, `cancel-claim`, `owned-unavailable`, `delete-cancel`, `delete-confirm` | orphan-only behavior, safe normal-owner unavailable state | ES | [J07](scenarios/J07.md) |
| F096 / R7 / Y | Access peer badges/online state<br>`peer-role-badge`, `online-badge`, `offline-badge` | currently read-only; do not claim assignment by reading badge | ES | [J07](scenarios/J07.md) |
| F097 / R7 / P | Access role assignment/revoke/downgrade confirmation<br>`assign-role`, `revoke-role`, `downgrade-cancel`, `downgrade-confirm` | W3 server-authorized controls not yet located | ES | [J07](scenarios/J07.md) |
| F098 / R7 / Y | Invite role/expiry/Generate/Copy to Clipboard<br>`role-viewer`, `role-editor`, `role-manager`, `expiry-1h`, `expiry-24h`, `expiry-7d`, `expiry-30d`, `generate`, `copy`, `second-host-join` | second host joins with exact role, expiry/refusals | ES | [J07](scenarios/J07.md) |
| F099 / R7 / Y | Join text/Join/Cancel; invalid/expired/already joined<br>`join-valid`, `join-invalid`, `join-expired`, `already-joined`, `cancel`, `no-partial-on-error` | no partial hierarchy on error, real live replica on success | JOIN | [J07](scenarios/J07.md) |
| F100 / R7 / Y | Inspector API role/expiry/client/Generate/dismiss/copy token-curl<br>`role-viewer`, `role-editor`, `role-manager`, `expiry-1h`, `expiry-24h`, `expiry-7d`, `expiry-30d`, `client`, `generate`, `dismiss`, `copy-token`, `copy-curl` | crypto/client/scope verification; no token screenshots/logs | IN | [J07](scenarios/J07.md) |
| F101 / R7 / Y | API list/node-scope variants/paging/Refresh/Revoke/confirm/cancel<br>`node-scope`, `petal-scope`, `page-next`, `page-previous`, `refresh`, `revoke-cancel`, `revoke-confirm`, `revoked-denial`, `unrelated-token-intact` | revoked token denied, unrelated token intact | IN | [J07](scenarios/J07.md) |
| F102 / R7 / Y | Entity API scope/role/expiry/client/Generate/scoped revoke<br>`scope`, `role`, `expiry`, `client`, `generate`, `scoped-revoke`, `revoked-denial` | same lifecycle with entity scope readback | ES | [J07](scenarios/J07.md) |
| F103 / R7 / Y | Actual Viewer/Editor/Manager/Owner identity and remote downgrade<br>`identity-viewer`, `identity-editor`, `identity-manager`, `identity-owner`, `remote-downgrade`, `server-denial`, `stale-role-navigation` | UI restriction plus independent server refusal, stale role reset | ES, KS | [J07](scenarios/J07.md) |
| F104 / R8 / Y | Status peers -> PeerDebug -> Close<br>`peer-segment`, `peer-debug`, `close`, `relay-status`, `offline-status` | own real peer/relay/offline status, no unsolicited invite send | PD, ST | [J08](scenarios/J08.md) |
| F105 / R8 / Y | Two-host joined UI edits: node/transform/data/terrain/eyes<br>`replicate-node`, `replicate-transform`, `replicate-property`, `replicate-terrain`, `replicate-visibility` | remote screenshot+API converge, correct scope/author | NAV | [J08](scenarios/J08.md) |
| F106 / R8 / Y | Reconnect/leave/reopen and remote deletes<br>`disconnect`, `reconnect`, `leave-replica`, `reopen-replica`, `remote-delete`, `no-duplicates` | durable agreement without duplicate entities | NAV | [J08](scenarios/J08.md) |
| F107 / R9 / A/Y | camera/shot/current/orbit/geo/tile/focus/no-ui dimensions<br>`camera`, `current-shot`, `orbit-shot`, `geo-shot`, `tile-shot`, `node-focus`, `petal-focus`, `no-ui`, `dimensions`, `restore`, `normal-F12` | actual PNG/pose/restore and scope/capture gate semantics | API | [J09](scenarios/J09.md) |
| F108 / R9 / A | move/click/double/right/drag/key/text/scroll/capture_after/settle<br>`pointer-move`, `click`, `double-click`, `right-click`, `drag`, `key`, `text`, `scroll`, `capture-after`, `settle`, `modifier-release`, `no-cursor-warp` | real UI effect/refusal/modifier release/no cursor warp | IB, API | [J09](scenarios/J09.md) |
| F109 / R9 / A | ui/find/role/interactive; click-name/type-into/wait-for<br>`ui-find`, `role-filter`, `interactive-filter`, `named-click`, `named-type`, `named-wait`, `gone-wait`, `ambiguity-refused`, `finite-deadline` | ambiguity refused, physical bounds directly clickable | QB | [J09](scenarios/J09.md) |
| F110 / R9 / A | state/path/wait-state and logs/since/level/new warnings<br>`state`, `path`, `state-wait`, `logs-since`, `logs-level`, `new-warnings`, `redaction`, `bounded-ring`, `navigation-generation` | truthful settled fields, bounded/redacted/scope-confined logs | QB | [J09](scenarios/J09.md) |
| F111 / R9 / A | dialog answers and window size/UI scale/maximize/restore/minimize/close<br>`dialog-answer`, `physical-size`, `ui-scale`, `maximize`, `restore`, `minimize`, `close-confirm` | actual loopback/root checks and real close confirmation | QB | [J09](scenarios/J09.md) |
| F112 / R9 / A | Input-off/role/foreign scope/malformed/rate/busy REST+MCP<br>`gate-off`, `non-owner`, `foreign-scope`, `malformed`, `rate`, `busy`, `rest-mcp-parity`, `relay-no-gui` | identical typed shared-surface refusal; relay no GUI mutation | API | [J09](scenarios/J09.md) |
| F113 / R9 / Y/A | Synthetic copy sink + ordinary noninjected copy sentinel<br>`synthetic-copy-text`, `synthetic-copy-image`, `normal-copy-sentinel`, `clipboard-chord-refusal` | synthetic no OS write, normal user copy functions | IB | [J09](scenarios/J09.md) |
| F114 / R9 / O | Normal OS AccessKit with debug/input off<br>`owned-hwnd-uia`, `named-buttons`, `finite-bounds`, `input-debug-denied` | owned-HWND read-only UIA sees named provider; REST debug refused | QB | [J09](scenarios/J09.md) |
| F115 / R9 / O | Controlled inactive/active timing<br>`inactive-baseline`, `active-provider`, `same-cadence`, `frame-sample-summary`, `incomplete-baseline-refusal` | actual samples, fixed same cadence, vsync/full-app caveat | QB | [J09](scenarios/J09.md) |
| F116 / R9 / A | Every registered MCP tool and REST method/path in source-inventory.md; API only<br>One explicit suffix for each MCP tool and REST method/path; see JSON and source inventory. | explicit scope/role and independent readback, separate denominator | API | [J09](scenarios/J09.md) |
| F117 / R10 / Y | Stop/relaunch same owned QA root after edits/settings/map<br>`same-root-restart`, `same-node-identity`, `durable-node-edit`, `durable-setting`, `durable-map`, `durable-asset`, `no-duplicate-entities` | same identity/data/assets/settings; new run is not restart | MAIN, NAV | [J10](scenarios/J10.md) |
| F118 / R10 / A | Key/store/token migration/backup administration<br>`key-migration`, `store-migration`, `token-migration`, `backup-administration` | no GUI editor located; CLI/API/harness proof only | MAIN | [J10](scenarios/J10.md) |
| F119 / R11 / Y | Maps Installed/Available/Downloads/filter/Refresh/Close<br>`installed-tab`, `available-tab`, `downloads-tab`, `filter`, `refresh`, `close`, `empty`, `populated`, `error`, `progress` | all populated/empty/error/progress tab cases | HM | [J11](scenarios/J11.md) |
| F120 / R11 / Y | Install from file valid/cancel/wrong archive type<br>`install-valid`, `install-cancel`, `wrong-archive-kind`, `corrupt-archive` | one installed TerrainTileset, typed refusal for legacy Terrain | HM | [J11](scenarios/J11.md) |
| F121 / R11 / Y | Registry URI/Install from registry valid/unavailable<br>`registry-uri`, `registry-valid`, `registry-unavailable`, `hash-mismatch` | controlled local lookup/hash/integrity and failure feedback | HM | [J11](scenarios/J11.md) |
| F122 / R11 / Y | Available Download / Downloads Cancel<br>`download-progress`, `download-complete`, `cancel`, `bytes-hash` | real provider bytes/hash/progress/cancellation | HM | [J11](scenarios/J11.md) |
| F123 / R11 / Y | Installed Seed/Remove-confirm/Open folder<br>`seed-on`, `seed-off`, `remove-cancel`, `remove-confirm`, `active-map-safety`, `open-folder` | persistence, active-map removal safety, run-only folder | HM | [J11](scenarios/J11.md) |
| F124 / R11 / Y | Petal Manifest add ID/type/Required/Remove/JSON/Save/Close dirty<br>`add-id`, `type`, `required`, `remove`, `json`, `save`, `close-clean`, `close-dirty-cancel`, `close-dirty-discard` | requirements exact, discard safety, duplicate render distance planned removal | MF | [J11](scenarios/J11.md) |
| F125 / R11 / Y | Actual GPX/GLB/hexon interoperability<br>`glb-byte-roundtrip`, `gpx-coordinate-roundtrip`, `hexon-bundle-roundtrip` | supported data/bytes roundtrip, no file-exists-only oracle | API, GX | [J11](scenarios/J11.md) |
| F126 / R11 / A | Plugin/Rhai/read/transaction execution<br>`plugin-read`, `plugin-transaction`, `rhai-execution` | no execution panel located, W6 library seam is not new GUI | API | [J11](scenarios/J11.md) |
| F127 / R11 / U | Atlas model/room/tag editor resources/helper<br>`tag-input`, `tag-add`, `tag-remove`, `atlas-room`, `atlas-model` | no production renderer/opener for tag_input_ui found | TAG | [J11](scenarios/J11.md) |
| F128 / R6 / U | ActiveDialog::InviteDialog variant<br>`enum-opener`, `missing-renderer` | result handler opens it but transient renderer has no branch | DIALOG, PM | [J06](scenarios/J06.md) |
| F129 / R3 / A | Bounded nested schemas and typed values<br>`schema-install-v1`, `nested-typed-roundtrip`, `exact-large-integer`, `additive-schema-evolution`, `rejected-value-atomic`, `rejected-schema-atomic`, `schema-version-provenance` | Exact nested data/integer9007199254740993; rejected value/revision leaves schema/state/history unchanged | `fe-api/src/property_schema.rs`, `fe-database/src/handlers/field_def.rs`, `fe-database/src/handlers/node_history.rs` | [J03 rich follow-up](scenarios/J03-rich-data.md) |
| F130 / R3 / A | Committed ordinary objects and replay<br>`atomic-create-baseline`, `committed-property-set`, `property-delete`, `full-transform`, `per-object-checkpoint-map`, `paginated-history`, `per-object-version-order`, `earlier-replay`, `current-state-unchanged` | Baselinev1; exact readbacks; paginated versions; Alpha v3 position3,0,4/gain2 survives current gain3/position6,0,8 | `fe-api/src/node_history.rs`, `fe-runtime/src/node_history.rs`, `fe-database/src/handlers/node_history.rs` | [J03 rich follow-up](scenarios/J03-rich-data.md) |
| F131 / R3 / A | Explicit affine derivation and appended sensor events<br>`pinned-affine-calibration`, `payload-provenance-bytes`, `chronological-raw-six`, `derived-per-anchor-arrays`, `per-anchor-statistics`, `half-open-window`, `latest-event-time`, `stored-xyz-distance` | Six raw/six derived; A21/23/25, B9/10/11; means23/10, sums69/30; windows/latest and XYZ10/8/4m | `fe-api/src/iot.rs`, `fe-api/src/timeseries_query.rs`, `tools/qa/rich_data.py` | [J03 rich follow-up](scenarios/J03-rich-data.md) |
| F132 / R10 / A | API verification after observed same-root restart<br>`same-root-current-state`, `earlier-version-replay`, `persisted-schemas`, `renderable-and-provenance-assets` | Exact current/old hashes, schemas and real GLB/provenance bytes; OS/GUI restart proof remains F117 | `fe-database/src/handlers/crud.rs`, `fe-database/src/handlers/node_history.rs`, `tools/qa/rich_data.py` | [J10](scenarios/J10.md), [rich follow-up](scenarios/J03-rich-data.md) |

F129–F132 and eleven F116 additions follow frozen rich-data API contracts in `qa/rich-data-core` (`1fe217f`),
pending main integration/fingerprint review. All are API-only. Source additions are not execution evidence or
GUI routes. Refresh concrete GUI labels/variants after separately owned Properties/History/chart work integrates.

## Source aliases: source alias and render/dispatcher anchors

Paths/line numbers are actual source inspection anchors; use final source fingerprint in executable catalog.

| Alias | Source | Production function anchors (first ten) |
|---|---|---|
| SB | `fe-ui/src/panels/sidebar.rs` | `left_sidebar`:26, `sidebar_verse_header`:132, `eye_button`:186, `render_verse_tree`:199, `render_fractals`:296, `render_petals`:384, `petal_eye`:492, `push_scope_eye`:503, `apply_node_row_verb`:528, `render_nodes`:559 |
| VP | `fe-ui/src/viewport.rs` | `viewport_overlay`:22, `viewport_petal_space`:64, `viewport_verse_browser`:169, `viewport_fractal_browser`:396, `viewport_petal_browser`:599, `truncate_id`:827, `find_petal_name`:831 |
| NAV | `fe-ui/src/navigation_manager.rs` | `navigate_to_verse`:22, `navigate_to_fractal`:31, `navigate_to_petal`:38, `navigate_to_fractal_in_verse`:43, `navigate_to_petal_in_fractal`:55, `has_valid_ancestry`:68, `active_scope`:75, `back_from_petal`:88, `back_from_fractal`:92, `back_from_verse`:98 |
| CE | `fe-ui/src/dialogs/create_entity.rs` | `render_create_dialog`:15, `build_create_command`:123 |
| KS | `fe-ui/src/node_manager/shortcuts.rs` | `shortcut_allowed`:20, `visibility_key`:37, `apply_visibility_key`:49, `escape_rung`:103, `handle_tool_shortcuts`:130 |
| RS | `fe-ui/src/ui_shell/right_sidebar.rs` | `slug`:87, `from_slug`:100, `line`:126, `update_residency_readout`:139, `toggle`:159, `is_active`:168, `reveal`:176, `reveal_slug`:182, `section_label`:198, `active_section`:216 |
| VIS | `fe-ui/src/visibility/groups.rs` | `groups_from_dtos`:30, `dtos_from_groups`:44, `next_group_id`:59, `with_group_added`:86, `with_group_visible`:97, `with_group_renamed`:108, `with_group_removed`:123, `with_group_moved`:128, `set_group_registry`:143, `apply_registry_loaded`:168 |
| TB | `fe-ui/src/panels/toolbar.rs` | `shortcut_hint_line`:94, `input_context_id`:117, `input_context`:122, `stash_input_context`:129, `mode_button_fill`:136, `tool_tooltip_text`:151 |
| NP | `fe-ui/src/node_manager/viewport_pick.rs` | `handle_viewport_click`:18, `nearest_in_subtree`:123, `pick_node_aabb`:150, `ray_aabb_hit`:187 |
| CAM | `fe-renderer/src/camera.rs` | `default`:42, `default`:86, `compute_orbit_transform`:118, `apply_orbit`:127, `apply_zoom`:142, `apply_zoom_proportional`:148, `scaled_max_distance`:161, `scaled_min_distance`:173, `ground_margin`:184, `exp_approach`:196 |
| GI | `fe-ui/src/node_manager/gimbal_interaction.rs` | `update_hovered_axis`:24, `pick_axis`:75, `ring_screen_distance`:115, `handle_gimbal_interaction`:157, `axis_vec`:310, `axis_screen_dir`:322, `segment_dist_2d`:338, `draw_gimbal_system`:352 |
| IN | `fe-ui/src/panels/inspector.rs` | `right_inspector`:32, `build_nav_scope`:154, `inspector_entity_section`:167, `radians_to_degrees`:222, `degrees_to_radians`:227, `size_to_scale`:235, `scale_to_size`:244, `combined_local_extents`:253, `sync_inspector_units`:296, `units_axis_row`:357 |
| GL | `fe-ui/src/dialogs/gltf_import.rs` | `import_enabled`:21, `render_gltf_import_dialog`:31 |
| CM | `fe-ui/src/dialogs/context_menu.rs` | `menu_for`:73, `verb_label`:106, `verb_tooltip`:130, `verb_label_for`:164, `verb_tooltip_for`:172, `track_delete_confirm_message`:188, `verb_is_seam_gated`:205, `verb_action`:214, `anchor_index`:233, `next_corner_kind`:246 |
| NO | `fe-ui/src/dialogs/node_options.rs` | `render_node_options_dialog`:18, `rename_request`:250, `node_options_save_url`:258 |
| PT | `fe-ui/src/panels/portal_toolbar.rs` | `right_portal_toolbar`:8 |
| AC | `fe-ui/src/panels/asset_card.rs` | `asset_card_section`:18 |
| TO | `fe-ui/src/panels/tool_options.rs` | `options_arm`:52, `arm_has_live_controls`:66, `render`:83, `render_options_hints`:117, `render_select_options`:131, `render_transform_options`:150, `render_pen_options`:168, `render_corner_editor`:192, `render_brush_options`:240, `shape_apply_footprint`:264 |
| AN | `fe-ui/src/panels/annotation_card.rs` | `annotation_card_section`:14, `parse_hex_color`:110, `rgb_to_hex`:134, `annotation_save_actions`:144 |
| QT | `fe-ui/src/panels/query_tab.rs` | `sql_literal`:25, `query_templates`:34, `inspector_query_section`:65 |
| RD | `fe-ui/src/ui_shell/results_dock.rs` | `pending`:34, `summary`:38, `display_order`:44, `begin`:48, `complete`:59, `fail`:104, `finish`:117, `invalidate`:122, `sort_by`:132, `contains_node`:165 |
| GP | `fe-ui/src/panels/gis_panel.rs` | `render_gis_panel`:26, `render_tab_bar`:153, `render_annotations_tab`:184, `render_query_section`:232, `run_query`:383, `render_results`:444 |
| EG | `fe-ui/src/panels/egress_card.rs` | `egress_section`:14, `render_source_selector`:77, `build_sql`:135, `source_error`:152, `render_share_section`:163 |
| AQ | `fe-ui/src/actions/query.rs` | `next_ui_query_id`:15, `submit`:23 |
| HM | `fe-ui/src/dialogs/hexon_manager.rs` | `format_size`:25, `render_hexon_manager`:42, `render_installed_tab`:243, `render_installed_map_card`:292, `map_action_denial_reason`:408, `render_scale_controls`:433, `render_available_tab`:515, `render_downloads_tab`:605 |
| LM | `fe-ui/src/panels/layer_manager_card.rs` | `layer_manager_section`:15, `render_view_mode_row`:54, `render_overlay_row`:99 |
| TT | `fe-ui/src/panels/terrain_tools_panel.rs` | `ensure_defaults`:46, `render_palette`:51, `has_real_scale`:89, `distance_unit_suffix`:96, `scale_chip_text`:108, `render_world_scale_chip`:116, `render_controls_and_emit`:140, `render_proposal_list`:259, `sculpt_footprint_area`:325, `circle_footprint`:356 |
| VR | `fe-ui/src/panels/volume_report_panel.rs` | `key`:52, `footprint`:60, `display_label`:68, `resolve_volume_selection`:83, `clear`:134, `sync_volume_report_request`:152, `drain_volume_report_result`:224, `resolve_volume_display`:253, `volume_unit_suffix`:264, `node_kind`:289 |
| TP | `fe-ui/src/terrain_preview.rs` | `proposal_op_snake`:54, `region_record_preview_params`:71, `shape_commit_footprint_world`:83, `sculpt_shape_draft`:97, `brush_stroke_draft`:127, `palette_draft`:153, `selected_proposal_draft`:175, `publish_proposal_preview`:206, `drain_preview_reports`:277 |
| BS | `fe-ui/src/node_manager/brush_interaction.rs` | `from_state`:31, `is_active`:75, `cancel`:80, `preview_snapshot`:94, `ground_hit`:108, `append_spaced_samples`:121, `stroke_action`:144, `petal_map_is_loading`:157, `handle_brush_interaction`:162 |
| ME | `fe-ui/src/node_manager/measure.rs` | `label`:44, `apply`:63, `is_empty`:68, `reduce`:84, `as_f64`:140, `readout`:145, `measure_world_scale`:180, `format_length`:187, `format_area`:195, `readout_rows`:204 |
| PE | `fe-ui/src/panels/path_editor_card.rs` | `type_glyph`:36, `format_distance_m`:48, `path_editor_section`:70, `sanitize_path_editor_numeric_state`:94, `render_track_list`:106, `render_edit_view`:274, `render_style_controls`:543, `render_snap_controls`:688, `sanitize_numeric`:716, `smoothness_slider_enabled`:734 |
| GX | `fe-ui/src/panels/gpx_import_card.rs` | `gpx_import_section`:16 |
| PL | `fe-ui/src/panels/tool_panel.rs` | `installed_assets`:34, `filter_assets`:59, `to_sdk`:82, `label`:109, `to_proposal_op`:136, `sanitize_f32`:212, `sanitize_numeric_state`:222, `queue_action`:270, `drain_pending`:275, `render_path_asset_section`:283 |
| PN | `fe-ui/src/node_manager/path_point_interaction.rs` | `marker_want`:46, `height_delta_from_cursor`:77, `pen_observe_alt`:119, `pen_release_decision`:147, `pen_anchor_fields`:180, `pen_gesture_fate`:237, `collect_foreign_anchors`:269, `candidates`:319, `snap_path_position`:341, `apply_snapped_point_release`:417 |
| TOP | `fe-ui/src/ui_shell/topbar.rs` | `try_request_chrome`:80, `request_from_ui`:91, `with_brand_icon`:97, `brand_texture_id`:105, `is_free_space`:132, `title_bar_gesture`:142, `resize_direction_at`:164, `viewport_outside_resize_bands`:192, `viewport_interaction_rect`:205, `resize_cursor`:226 |
| PM | `fe-ui/src/panels/mod.rs` | `gardener_console`:82, `render_toast`:401 |
| ST | `fe-ui/src/panels/status_bar.rs` | `status_bar`:16 |
| FD | `fe-ui/src/file_dialogs.rs` | `fmt`:14, `refused`:18, `from`:30, `as_ref`:39, `deref`:45, `read_lease`:50, `write_bytes`:53, `copy_from`:62, `validate_for_write`:73, `choose`:90 |
| LP | `fe-ui/src/ui_shell/layout_policy.rs` | `key`:25, `begin_frame`:29, `observations`:44, `scroll`:48, `protected_width`:87, `can_show_left`:93, `can_show_right`:98, `right_width`:103, `left_width`:109, `editor_height`:116 |
| MOD | `fe-ui/src/ui_shell/modal.rs` | `panic_payload_string`:47, `guarded_catching`:69, `guarded`:98, `transient_order`:132, `is_dialog_family`:143, `resolve_exclusive`:165, `dialog_family_exclusive`:176, `should_show_tooltip`:190, `cascade_confirm_message`:200 |
| ES | `fe-ui/src/dialogs/entity_settings.rs` | `build_entity_scope`:13, `render_entity_settings_dialog`:31 |
| JOIN | `fe-ui/src/dialogs/join.rs` | `render_join_dialog`:23 |
| PD | `fe-ui/src/dialogs/peer_debug.rs` | `render_peer_debug_panel`:9 |
| API | `fe-api/src/mcp/mod.rs` | `tool_available`:140, `operator_opt_in_refusal`:156, `tool_specs`:176, `tool_definitions`:313, `mcp_handler`:329, `handle_tool_call`:394, `authorize`:453, `id_arg`:493, `resolve_hierarchy_scope`:514, `check_required_args`:569 |
| QB | `fractalengine/src/qa_debug_bridge.rs` | `cursor`:45, `install`:53, `prepare_accessibility`:81, `layout_snapshot`:109, `rect`:123, `node_name`:127, `role_key`:151, `flatten`:158, `dialog_title`:261, `snapshot`:278 |
| IB | `fractalengine/src/input_bridge.rs` | `install`:112, `suppress_synthetic_clipboard`:133, `unresolvable`:166, `window_failure`:170, `character`:201, `resolve_key`:207, `is_modifier`:290, `holds_command_modifier`:299, `clipboard_chord`:304, `key_event`:314 |
| MAIN | `fractalengine/src/main.rs` | `main`:30, `drain_scene_changes_to_store`:924 |
| MF | `fe-ui/src/dialogs/petal_manifest.rs` | `render_petal_manifest`:10 |
| TAG | `fe-ui/src/atlas/tag_panel.rs` | `validate_tag`:6, `remove_tag`:24, `tag_input_ui`:32 |
| DIALOG | `fe-ui/src/dialogs/mod.rs` |  |


