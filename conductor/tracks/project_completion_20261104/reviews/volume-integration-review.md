---
type: Review
date: 2026-11-04
title: Earthwork volume integration preservation review
source_artifact_sha256: 6c43869e57cd2dd1e5b880c676203c611f16203d6076b90e20e182c5a7c10250
---

# Volume ownership integration preservation review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the reviewed staged integration.** All reviewed volume contributions
and the overlapping main changes are preserved. No missing new-field constructor
or new binding conflict was found in the inspected integration. Compilation and
live acceptance remain required; QA-0069 is not closed by this review.

## Exact boundaries

- Main parent: `2bce88aff512a9998eb120ddbcde8b78b4f7aa84`.
- Source base: `24b5660c`.
- Combined corrected source tip: `3ed94d8b4b87ba362023c0dfe163eceb6ff20f82`.
- Source checkout `.omc/worktrees/qa-volume-measurement-ownership` was clean at
  that tip. Its frozen correction manifest and prior source security review
  remain the source-approval references.
- Main index contains exactly the expected 43 changed paths.
- Staged diff SHA256 (`git diff --cached --no-ext-diff --no-color`):
  `8a6066e8505d31fe188ecaf6bac9c94ea04d9d79bba9db8fee9c4eccd556ea8f`.
- Source-base-to-tip diff SHA256:
  `e2137f2a3a61ecfa4135f2d362e7ccd94093034f8abff6b9a313c5e54cb73f91`.

Main's unrelated unstaged `.gitattributes` and preexisting session log, plus
untracked QA34/35 packets, are outside this staged slice. No Cargo/lock file is
in the slice. The review did not modify or stage any file or inspect processes.

## Full contribution preservation

Thirty-five complete staged blobs are byte-identical to the corrected source
tip. All eight differing blobs are exactly the paths where the main parent
already differed from the source base:

- `fe-database/src/op_log.rs`
- `fe-runtime/src/AGENTS.md`
- `fe-ui/src/AGENTS.md`
- `fe-ui/src/actions/mod.rs`
- `fe-ui/src/maintenance.rs`
- `fe-ui/src/plugin.rs`
- `fractalengine/src/AGENTS.md`
- `fractalengine/src/terminal_reset/mod.rs`

For the five overlapping Rust files, independent raw-line comparison found only
insertions in source-base -> main-parent. Each exact main-only insertion appears
once in the staged blob. Removing those insertions from the staged bytes produces
the complete source-tip blob exactly. This proves both sides were retained,
rather than relying on successful three-way application or whitespace-insensitive
patch IDs. It covers:

| File | Existing main contribution retained exactly |
| --- | --- |
| `op_log.rs` | 378-line live asset audit journey plus 28-line legacy GLB fixture verification block. |
| `actions/mod.rs` | Both one-line A2 surface-transition/native-overlay hooks. |
| `plugin.rs` | Existing one-line Inspector-tab transition hook. |
| `terminal_reset/mod.rs` | Existing native_accessibility::retire call before UI read-model reset. |
| `maintenance.rs` | Existing four-line EguiContext/EguiInput retire_input loop, followed by volume retirement before asset retirement. |

Thirty-nine per-file stable patch IDs match the corresponding complete source
contributions. The four exceptions are the three documentation conflicts and the
maintenance conflict with its additional existing native retirement context;
their stronger raw-byte proofs are described here.

## Additive documentation conflicts

Each staged AGENTS blob starts with the entire exact main-parent blob, preserving
the existing runtime audit, UI VIS-001/native accessibility and host audit/native
witness material. Each then contains the complete source-added section body
byte-for-byte. The sole source-suffix difference is one omitted leading blank LF
at each append boundary; no section-body text or existing parent byte is lost.
This is an additive separator difference, not a semantic change. Historical
conflict-marker removal was not replayed by this review.

| Document | Source added bytes | Staged added bytes | Final index SHA256 |
| --- | ---: | ---: | --- |
| `fe-runtime/src/AGENTS.md` | 592 | 591 | `51bb8e27c295ae85086afe070330e1d2628fc8594c39b57a6d9dc1500b9e5824` |
| `fe-ui/src/AGENTS.md` | 584 | 583 | `955664b0d5d786b1f762fe23b68c6c5a6809cf5b4c777fb462698fef73f6c9e9` |
| `fractalengine/src/AGENTS.md` | 1490 | 1489 | `b802c37eb49fdb4c79bf0f7695ab79e2ed2af2a94b32f3bd556002a4f8854606` |

## Behavior-critical merge checks

The new owner and dispatcher are complete exact source-tip blobs. The prior
three-MED source approval therefore carries forward: original-demand/source,
height, access/incarnation, node admission/epoch and retired eligibility precede
receipt-derived projection; the owner is an independent Resource; direct and
resolved receipts use the same property-only projection with no generic refresh
escape, Transform/NodeManager/Undo takeover or mutation replay.

The manually merged maintenance file preserves both terminal responsibilities in
order: retire buffered/current native input, then retire volume intent while
retaining accepted unknown ownership, then existing asset retirement and resets.
The actual Supervisor successful DB join hook is the source correction plus the
unchanged main native retire call; the abnormal-join return remains before owner
clearing. The exact terminal observation code is the reviewed source blob.

Runtime EditorBatchCapture's optional terrain_source_fingerprint, DB same-row
capture population and whole-state transaction fences are exact source-tip
contributions. Targeted constructor inspection found the known complete capture
literals supplying the field: DB capture, UI receipt snapshot, Inspector fixture
and both volume journey fixtures. The snapshot preserves the original field;
unbacked fixtures use None. Mechanical GPX, vision, screenshot, terrain-bridge and
lazy-tile constructors retain the source's None/false defaults or actual confirmed
assignment witness. No missing constructor was found in this census; this is not
a compiler result.

All seven A2 correction blobs still equal committed native source `5a51daa1`
exactly. Additional byte-equality sentinels confirm viewport VIS-001, native
surface/lifecycle/book and the API/DB live asset audit modules remain unchanged
from main parent. The complete staged path set also shows no hidden edit to those
other modules. Root documentation prefixes retain both witness and asset-audit
contracts. No initial unsafe native state replaced a correction.

The working tree equals the index on all 43 staged paths. A byte scan of those
index blobs found no conflict markers. `git diff --cached --check` returned 0.

## Acceptance limits and operations

Source/integration approval permits the parent to commit and build this reviewed
combination. Actual terrain coverage/source transitions, navigation without
history churn, direct/resolved late outcomes, newer gestures and terminal reset
still require live acceptance and the final integrated checks. Existing retained
facts 103-106 remain evidence; this review does not change them or certify peer
convergence, normal native delivery or capture freshness.

Read-only Git show/diff/status/patch-id, raw-byte/hash comparison and targeted
constructor/source inspection, followed by this artifact, were the only
operations. No source edit, Git mutation, compiler, Cargo, tests, app, process/OS,
input/capture, API/DB/store or network operation was performed. The parent's report
that apps/builds were stopped was not independently verified by process inspection.
