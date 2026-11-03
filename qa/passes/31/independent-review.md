# Independent QA31 evidence and QA32 wrapper review

Reviewer: `/root/selection_corrections_review`, 2026-11-03.

**Approved QA31's bounded executed evidence**, against packet-manifest SHA256
`51f3635da638460f5c277977eec59a82afce977153e914d8875a0cb046e966ec`.
The current-read failure remains open in the executed d0 stage. This is not
acceptance of later source corrections, eligible asset adoption, peer rendering,
the full application QA catalog, final checks, CI or releases.

The author and reviewer worked in separate lanes. Verification used immutable
packet bytes, referenced saved QA30 originals/start receipts, explicitly named
local log files and the sealed executable. No target script, app/API, DB, live
process query, input, Cargo/test or network operation was executed. The builder
was inspected as source, not run. No token files or raw credential values were
read/published. Only this report was authored; packet/source/canonical files were
unchanged.

## Integrity, retained lifetime and captured cleanup

- All 113 listed manifest payloads have exact lengths and SHA256 hashes, with
  unique names, no missing files and no extra files. There are 114 packet files
  including the manifest. All 104 verdict source-file entries independently match
  their original frozen bytes. JSON/JSONL source records parse successfully.
- The three restart-origin copies exactly match the reviewed QA30 saved source
  files. They are not reconstructed or normalized responses.
- The recorded Owner context, verdict and ready start receipt agree on PID 92012,
  ticks 639292997983727113, stage
  `d0e0d21c-20261103034659162`. The executable bytes independently match
  `CCA8E5D49A0CA155944CCC262F7ACCCFF69C6EF6C8BECE58A4B50380D0C3F654`.
  The context retains source_dirty=true and the exact source fingerprint, rather
  than implying a clean checkout.
- The previous-start receipt names the exact QA30 Owner lifetime. Retained root,
  no reseed, unchanged port and no stale-stage override agree across saved
  metadata. The saved launched-stage and launch logs support 271 seconds and
  24.7 seconds respectively, with no compiler warning in that launched stage.
  Earlier failed/superseded builds remain separately labeled.
- The end receipt matches all eight full identities from the context's root plus
  recorded descendants and marks every identity absent. This is the saved result,
  not a fresh census or proof about unrecorded children.

## Restart and actual Cancel

The restarted Group registry response and both complete member responses are
byte-identical to their QA30 baseline counterparts. Registry Group
`01M6GDVPBSY2W51384QZGGBCAS`, name `QA30 restart group`, order 0.0, visible=true,
and both exact memberships survive this retained-store restart. This establishes
the specific saved Group/member persistence case, not every persistence feature.

The recorded actual Cancel action targets the current named cancellation control.
The following five response pairs are byte-identical across that action: registry,
Cube 1 row/history and Cube 2 row/history. Their responses are successful, not
matching error bodies. This substantiates cancellation without a saved mutation.

The screenshot source and public PNG copy match exactly, SHA256
`20830a3d67273c929534f51608e9971e765afa397126cd0b268dbd85d93854ba`.
Independent visual inspection confirms horizontal, readable Confirm delete and
Cancel text, on a separate row inside the right panel at the recorded 1280x720
size. Saved geometry is Confirm 67.9375x24 and Cancel 34.28125x24 physical pixels;
Cancel's allowed/clickable hit rectangle agrees exactly. The prior vertical-letter
defect is not present in this sample. This provides focused visual/action evidence
for QA-0066, not a WCAG verdict or every-window-size acceptance.

## Empty Group in a nonempty Petal

The new Group `01M6GHH0NCY8MWHV8D25ARCV2D`, name `QA31 empty group`, order 1.0,
visible=true, is added, deleted, restored by Undo and removed by Redo. Independently
checked all four complete registry results: the original Group remains exact;
the added/restored vectors append only the exact new empty Group.

All ten distinct Petal object rows and all ten object history responses are
successful and byte-identical before versus after this lifecycle. None initially
references the empty Group. The saved UI exposes no loaded members and disabled,
unselected empty-Group selection; actual add/delete/Undo/Redo inputs are retained.
The zero-object-effects evidence is not inferred merely from an empty selection.
This is explicitly a zero-member Group among ten existing objects, **not** a
zero-node-Petal lifecycle case.

## Local-copy refusal and authorized GLB downloads

The actual Review local scene asset copy input is followed by the explicit
unavailable reason: only ownerless, unanchored legacy copies can be adopted.
The resulting UI has no Publish/adopt action. The evidence correctly records
Cube 1 as an ineligible anchored/owned copy; it does not call the refusal a
successful adoption or claim an adoption mutation.

Before and after this refusal, each of the node, asset-ID and content-hash routes
records HTTP 200. All six saved downloads are byte-identical, exactly 1664 bytes,
SHA256 `ed52f7192b8311d700ac0ce80644e3852cd01537e4d62241b9acba023da3d54e`.
Independent binary parsing confirms glTF magic, GLB version 2 and a declared
length of 1664 matching actual bytes. The packet claims header/length validation,
not independent rendering or complete model semantics. No independent BLAKE3
digest was computed; server ETag/content-hash labels remain server provenance.

## Persistent current-read failure and observation limits

The valid filtered plain-tracing capture contains 36 diagnostic observations,
nine each for the four exact named nodes. Every recorded primary tuple is
statement 11 / asset_metadata / Thrown / asset_metadata_changed / errors 11.
The frozen initial empty JSON-parser result cannot establish no failures and is
appropriately identified as an invalid parser attempt on plain tracing.

The final local stdout independently contains 228 lines with the diagnostic
asset_metadata_changed marker, 57 each for those same nodes. The packet's 36
count describes its retained sampled diagnostic capture, not a final-log total;
there is no contradictory zero-failure or fixed-runtime claim. These diagnostics
identify the failing fence/statement, not by themselves every underlying
serialization cause. The later e406/e5 typed projection correction is explicitly
outside the executed d0 stage and remains subject to fresh runtime proof.

All eight enumerated state samples retain pending={ui_known:4,db:null,sync:null}
and settled=false. Their terrain scene-report mismatch is not proof of terrain
pipeline failure/readiness. Aggregate pending does not provide a producer census,
and no global quiescence or unknown accepted-write conclusion is implied.

## Logs, scans and storage

Independently verified all seven referenced local log byte lengths/SHA256 hashes,
line and structured-level counts. Executed-stage stdout is 216286 bytes, 1399
lines, 1092 INFO and 240 WARN, with zero structured ERROR or recorded panic
markers. Stderr is 78 bytes. The warnings remain an explicit limitation, not a
clean-log claim. Compiler failures from the earlier unlaunched attempt remain
separate from the successful launched-stage evidence.

Recomputed the four configured credential-pattern counts for all 98 textual
original artifacts and all seven logs; all match the saved zero counts. The
other six source entries are binary GLB captures. Finite regex scanning is not
exhaustive credential detection; raw logs remain local and no token files were
read. The report makes no claim that every possible secret format is absent.

Effective attributes for both QA31 JSON and nested binary paths are text=unset
under `/qa/passes/31/** -text`. No staging was performed by this reviewer. QA31
had zero index entries during review, so final **staged-blob byte/hash validation
remains required before commit**, as qa/AGENTS.md specifies. The working packet
has the exact frozen hash above; this is not an index-byte approval before it is
staged. Legacy attributes are not broadened.

No substantive correction is required to QA31's bounded findings. When publishing
review metadata, preserve this exact reviewed-manifest anchor and regenerate any
changed derived manifest; do not rewrite the original captures or import QA32
execution into QA31.

## QA32 lifecycle wrapper delta

**Approved independently for source/parse readiness**, communicated separately
to the root before completing packet review. The exact sources are:

| Wrapper | SHA256 |
| --- | --- |
| `.omc/research/qa32_launch_owner.ps1` | `23d71e9dfb42fbbb90e740bb0277daa61ef4663852e215099c8ac4587c85bd34` |
| `.omc/research/qa32_stop_owner.ps1` | `2283fa2365e8d591f52cf4bef87d11051d672303dce8bda8d0a844db3a570a55` |

PowerShell static Parser.ParseFile returns zero errors for both. The approved31
source hashes remain unchanged. The full delta is mechanical QA31-to-QA32
output/context/function relabeling plus previous Owner QA30-to-QA31; previous
recipient remains QA28. New QA32 context is not used as the previous lifetime.

At the wrapper review cut, the retained current.json still named exact Owner
92012/ticks639292997983727113/d0 stage; its eight saved known identities were
absent, as were all eight exact QA28 recipient identities. New QA32 context and
failed-handoff outputs did not exist at that cut. These are time-scoped saved/file
checks, not assertions about app activity after approval was delivered to root.

All existing stage/source/hash, retained-root/no-reseed/no-stale-override,
unique-ready-attempt/pinned-root handoff, failure recovery and exact known-identity
cleanup controls are unchanged. Effective QA32 -text is present; QA29 remains
unaffected. Target scripts were not executed by this reviewer. Actual preflights
and any QA32 runtime result remain distinct from this static approval.
