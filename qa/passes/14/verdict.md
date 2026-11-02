# Focused integration pass14

This is a focused live pass on stage75d2860, not a full coverage/convergence
pass. It does not satisfy the final integrated suite or release gate.

- The production GUI compiled after correcting the missing egui import in the
  Undo shortcut guard. The immutable stage manifest/hash and source fingerprint
  were verified. The saved QA root restarted without reseeding as PID84248.
- Read-only recovery of the original completed fleet passed24 assertions:
  original object versions/state digests, six raw plus six calibrated readings,
  actual two-shard coverage and one answerer, no missing/truncated metadata,
  original chronological calibrated values and REST/MCP parity. Nothing was
  re-ingested. The first scratch check incorrectly read MCP's `data` instead of
  `rows`; its failure is retained beside the corrected successful check.
  This cross-build proof does not replace a fresh driver scenario and strict
  same-new-stage restart.
- The native report picker opened saved brush regionr1. Its visible report shows
  earth material,38.83m3 fill and the known concrete box's2.270m3 sampled overlap.
  Runtime world bounds give whole-box volume3.125m3. All five pristine probes
  equal−0.13673552870750427; actual grounded origin elevation matches the
  independently decoded tile mean0.13673553465355326. The changed Y interval
  predicts2.2704029455780983m3. The diagnostic explicitly does not verify a
  historical terrain assignment epoch or exact mesh-solid intersections.
- Native Inspector editing and keyboard Undo/Redo passed exact stored/ECS
  readbacks. A legacy currentv4 gained supported baselinev5 and first editv6;
  Undo/Redo advancedv7/v8. A second editv9 followed by two Undo and two Redo
  advancedv10–13 correctly. Two final Undo operations restored the initial
  transform atv15. Cube3's original properties and rotation/scale are preserved.
  Path-point, stale peer and recovery journeys remain unexecuted.
- Settled screenshots reproduce QA-0038: nested object-report scrolling can hide
  the object name/method above the visible material row. Source fix3a3251b has
  independent design approval; next-stage live re-verification is pending.
- Layout check retained11 measured small targets and explicit unknown text
  coverage. Actual chrome/tree action targets need repair/classification under
  QA-0039; informational hover text must be distinguished without weakening
  genuine action minimums. The layout check is not a clean result.
- A scratch helper lacked its import path for layout_check. The error and
  correction are retained; the public QA CLI was unchanged.
- Root84248 and all seven freshly captured descendants were independently proven
  absent before the next Rust integration. No process-name cleanup was used.

Independent evidence/warning reconciliation remains pending at this commit.
Inspector lifecycle review found a same-target programmatic-tab transition that
can retain token state; its author is correcting it. Live database Reset remains
disabled until API/sync/plugin writers and runtime caches can be safely quiesced.
