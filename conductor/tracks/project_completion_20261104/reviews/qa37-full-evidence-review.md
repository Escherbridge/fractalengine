---
type: Review
date: 2026-11-04
title: QA37 analytic volume and intersection evidence review
---

# QA37: independent full frozen-evidence review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED as focused numeric, persistence/no-churn and sampled solid-box/material evidence, with the stated open findings and limits.** No packet blocker found. This is not a clean FULL pass, queued-authority acceptance, exact terrain-owner witness, peer evidence or campaign completion.

Reviewed frozen `qa/passes/37/packet-manifest.json`, SHA256 `7f6d00ee0e3b000b45b6d5978710271e054965b63f1fbd84334c2560064d6b0d`: 147 payloads, 148 files including the manifest, ten actual PNGs. Independently verified every payload's raw length/hash, all 129 original-payload anchors, six metadata/log source copies and all ten original screenshot byte copies. Independently inspected all ten actual images rather than trusting author flags. Read raw CLI outcomes, state/hierarchy/config/node/history/report records, actual original logs, fixture generator/descriptor and analytic derivation. Only read-only file/hash/JSON/arithmetic/image inspection and this main artifact were performed; no source/Git mutation, build/test, app/process/OS/input/capture, API/DB/store or network action.

## Exact run and source evidence

The fresh owned run is `.qa/run/20261104-095346-026`, root PID 120552, creation ticks `639294080276550131`, exact executable under `.qa/stage/fa955563-20261104084453622`. Context, stage manifest and launch log agree on source `fa9555632380977c41fe89ac984fcb491c47295a`. The stage records source_dirty=true and fingerprint `7F6169158690AB568D4F75350269EC0D7FABE59A2EB394EB5C1C2BACCB28EF6E`; do not represent it as a clean source commit or as the later v4 divider build. Independently hashed the actual staged executable to `676bde69afc2f2247e16797effa1340df427e6652d484fb8316113a88f519238`, matching stage/context.

The retained stage log shows successful staging in 327.5s with one unused-mut compiler warning, not zero warnings. Fresh terrain launch completes in 18.8s and records both actual terrain Petals. No retained-store restart or helper/native UIA path is used by these Owner HTTP QA actions.

## Actual images inspected

All images are genuine byte-exact 1280x720 source copies:

| PNG | Independently observed content |
| --- | --- |
| brush.png | Brush Options, radius 5m, initial operation/shape controls and the terrain scene |
| configured.png | Raise, delta 2, qa-raise material, Rectangle and approximately 100m2 draft |
| raise-selected.png | Selected r1 report, fill 200/cut 0/net 200, area 100 and 16384/16384 samples |
| lower-configured.png | Petal B, Lower, delta 1, qa-lower material and Rectangle |
| after-lower-timeout.png | One visible red lower footprint after the ambiguous placement timeout |
| lower-selected.png | Selected r2, cut 100/fill 0/net -100, area 100 and 16384/16384 samples |
| with-solid.png | r1 report, one overlap among five checked objects, inside-box row and 4.000m3 estimate |
| inside-properties.png | Inside box selected with position approximately[-0.254,0.5,0.441], zero rotation and size2/1/2 |
| outside-selected.png | Actual outside cube rendered and selected; Inspector position approximately[19.746,0.5,0.441], zero rotation and size2/1/2 |
| final.png | Petal B lower report remains cut100/fill0 with complete sample count |

These images also retain open low-contrast active-Petal breadcrumb text and narrow divider targets. The inside box is occluded by the raised terrain in its screenshot; its native geometry proof is the actual scene diagnostic plus Inspector/readbacks, not a claim that its complete solid is visible in pixels. The later active-Petal-header and divider source fixes receive no visual credit from this older staged run.

## Independent numeric oracles and actual atomic history

The stored footprints for r1 and r2 have corners[-5.254237651824951,-4.559264183044434], [4.745762348175049,-4.559264183044434], [4.745762348175049,5.440735816955566], [-5.254237651824951,5.440735816955566]. Independently applying the shoelace formula gives exactly100m2, and every coordinate is inside the generator's checked flat sector[-8,8]. The actual offline descriptor agrees on world_scale1,128-axis lattice and16384 expected samples. Its nominal encoded base is0; the runtime projection offset is discussed below and does not change a constant relative raise/lower depth.

| Separate Petal case | Independent expected cut/fill/net m3 | Actual saved cut/fill | Actual visible sample count |
| --- | --- | --- | --- |
| Petal A, r1, Raise2 | 0 / 100x2=200 / 200 | 0 / 200 | 16384/16384 |
| Petal B, r2, Lower1 | 100x1=100 / 0 / -100 | 100 / 0 | 16384/16384 |

The selected-report UI records and inspected PNGs agree with raw node/properties/summary readbacks; neither row was seeded with an asserted numeric answer. The displayed lattice step rounds to0.08m; the intended10/128 step is0.078125m. The stated0.001m3 raw tolerance is satisfied exactly by these saved values. Delta visualization's vertex-coarsening warnings are distinct from the actual report's complete integration sample count.

Both complete histories have coverage_start_version1, entries1..6, current_version6, current_state_verified=true and no next page. Versions5 and6 carry the same atomic_save first_version5/committed_version6 and batch ID within each object. Version5 is inside_atomic_batch and version6 available. Raise batch is `earthwork-volume-save-01M6KQYP1S3995JJ6DPW6FPXE6`; Lower batch is `earthwork-volume-save-01M6KR4FM2GNRBXH5Y69Y1MMPD`. The fields are cut_volume_m3 then fill_volume_m3. Independently comparing version4's full after_state with version6 after removing just those two new properties shows every other field, transform, edit_seq and original properties preserved. This is observed successful atomic-pair history, not an executed rollback or receipt-loss case.

## Both objects remain byte-exact through two observed returns

Independently compared the entire raw node and entire raw history response for BOTH raise and lower against BOTH return and second-return captures: eight comparisons are byte-identical. The author analytic file lists only the first lower return, but the packet summary and actual additional files supply the second; this review checked those bytes independently too.

| Object | Full node SHA256 | Full history SHA256 |
| --- | --- | --- |
| Raise | `e66ae39435dba03e851436cc4591011ab81adfaa5d8e7c3aa87bff3fa466794c` | `4cf1c4cfb6abcbde6802f3a0003b8464c606eaad0df02fd1630b72143234a8f7` |
| Lower | `7a8ada7a6053872d4804ba2684dca9c95128e6a5fc443c9bb5482aa660d1af66` | `cd890092ab17ff4cc545572657e8e46abe838849e548144327fca8878290cad1` |

This packet establishes actual departures rather than inferring them from a Back command. The saved navigation states explicitly show petal_id=None at frames21406,34712,68216,70970 and83655. Correct fresh Petal IDs are separately observed: A=`01M6KQPE94JF782M7Y7SRF35C2`, B=`01M6KQPEBS81CFVTDY090GP95Z`. Raise return states are A at34757/71004; Lower return states are B at70895/83702. Real visible-card click receipts and intervening B/A transitions support that sequence. The stale nth locator refusal is preserved before the later valid visible B action. No missing-departure assumption from QA36 is imported.

This grants focused observed no-churn credit through these captures. Generic settled=true expressly excludes DB/sync/terrain completion certainty and is not used as an exact background owner-generation witness. No forced cache reset, restart, lost receipt or concurrent authority transition is proved here.

## Real inside/outside geometry, material and sampled overlap

The two ordinary object-create receipts each confirm persist=committed/history_version1 and a native cube primitive dims[2,1,2], identity scale/rotation, plus material JSON {name:QA concrete,density_kg_m3:2400}. The saved node readbacks and actual selected native states independently establish materialization; the creation responses alone are not the oracle. Inside ID is `01M6KRPVS4AJM6J6QTPYK9DWE8`; outside ID is `01M6KRPW1EYJT6SJGHQP0YDSJJ`.

Actual SceneAnalysisSnapshot AABBs classify both as axis_aligned_solid_box. Inside X=[-1.2542376518249512,0.7457623481750488], Y=[0,1], Z=[-0.5592641830444336,1.4407358169555664], fully inside the raised footprint. Five request-time center/corner pristine-base probes each return B=-0.13673552870750427. Thus changed Y=[B,B+2]=[-0.13673552870750427,1.8632644712924957] contains the full box height. Independent interval intersection gives2x2x1=4m3. The actual r1 UI row says Solid box / sampled terrain and Overlap estimate:4.000m3, with full material JSON in accessible/readback content and visibly truncated material presentation in the narrow panel.

Outside X=[18.74576187133789,20.74576187133789] is beyond the footprint's maximumX4.745762348175049. It is loaded, rendered and readable through its own selected Inspector/native AABB. The report checks five geometric objects, lists only the inside impact and has no Open properties for impacted object QA solid box outside action. This is a genuine negative spatial control rather than a missing/unloaded node.

Both diagnostics explicitly retain terrain_epoch_verified=false, request_time_only_not_a_report_replay, projection_and_height_field_have_no_assignment_stamp and throttled_source_update_frame_unavailable. Five probes do not prove the entire report lattice or exact owner/assignment epoch. The constant projection offset explains why the runtime flat base differs from nominal encoded0; it does not license retagging old report state. Material density is data, not a mass calculation. No arbitrary GLB mesh-solid, rotated exact-solid or surface-only-contact claim is granted.

## Failures, logs, encodings and cleanup are honest

The single recorded Lower placement returns HTTP504 after5s. Its CLI refused=true does NOT prove pre-input refusal or non-consumption. Actual subsequent hierarchy/config readback has exactly one r2 proposal and one backing object, with the successful atomic measurements above; the recorded placement is not replayed. Preserve the timeout/error as an accepted/unknown input outcome resolved by observation, not a successful CLI response.

The descriptive Brush locator_not_found and later locator_index records are different: they contain no input commands and are retained as pre-input refusals. The subsequent actual exact Brush/visible B actions have their own receipts. Configuring draft controls has a raw-byte-identical terrain-before/configured comparison, so draft editing alone receives no terrain-write credit.

All137 non-PNG payloads decode strictly without NUL or replacement characters; cleanup-identities.json and stage-manifest.json carry preserved UTF-8 BOMs, all other payload texts are UTF-8. All135 JSON/JSONL files including manifest parse with nonfinite constants rejected. The finite Bearer/JWT/private-key-block/credential-assignment patterns independently report zero across payload texts and all five original log sources. This is finite pattern coverage, not exhaustive secret detection. No credential/private-store file was opened.

The actual stdout has1217 lines:1115 INFO,34 WARN,1 ERROR; stderr is empty. All five logs decode strict UTF-8 without BOM/NUL/replacement, have zero panic/current-read error markers and match summary hashes. The single ERROR is the504 trace. Warnings include actual delta-visualization lattice coarsening and stored-scene-recovery refusal/incompleteness; this is not a clean-log run or peer recovery success. The stage has one compiler diagnostic warning and zero compiler errors. Packet logs publish stage/launch/stop raw copies; stdout/stderr are preserved at their original run paths and hash-bound by summary, not falsely claimed as copied packet payloads.

Final layout reports two small-hit-target violations, both10px side dividers,34 unknown observations and coverage_complete=false. Actual images retain active-Petal breadcrumb contrast concerns. Future divider/header source approvals do not erase these recorded findings.

The stop receipt preserves the exact run root. Cleanup JSON matches the original context's ordered eight exact PID/start-ticks/executable identities and records each original identity absent at its saved check. No PID/title-only or universal-all-process absence claim is inferred. This review verified those saved facts and source-copy bytes without live OS inspection.

## Coverage disposition

Accept the focused fresh separate Raise/Lower analytic values, visible complete sample counts, successful two-field atomic history with other fields preserved, both-node/full-history byte equality through two observed returns, and actual material-bearing inside/outside sampled-box comparison. Retain all timeout, warning, layout and diagnostic limits.

No FULL/convergence pass, queued API credential continuity/QA0070, real Viewer/Editor native helper, peer authority/rendering, retained restart, rollback/failure injection, late/Resolve race, exact terrain-owner witness, integrated test sweep, strict relay/store test, green CI, tag/release/artifact or project-completion credit is granted.
