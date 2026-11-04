---
type: Review
date: 2026-11-04
title: Native witness integration preservation review
source_artifact_sha256: 4420882ff0c24f7cbfe729cf76573b3bf177a694729037e1eaea330698904b01
---

# Native witness A2 integration preservation review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the reviewed index integration.** The combined original witness
and corrected contribution is preserved exactly, and the existing main changes
are retained. This is source/integration approval, not build or native acceptance.

## Exact inputs and index

- Main parent: `9cdd31325b83f5870bad78d7c8d949ff40f7bdd2`.
- Source base: `eda6f0eb327fb28a117ff506a04e0843d57092bd`.
- Original source witness: `9902f43105e922fa9097c00904976d50ac5e821f`.
- Corrected source tip: `5a51daa1e3fe4f5abf5e1f6bb060b353f29966cf`.
- Reviewed main index was uncommitted and contained exactly the expected 27 paths.
- Index diff SHA256 (`git diff --cached --no-ext-diff --no-color`):
  `fcf166a2571ad658a05e4bbebd0f0213a9726c21cc07a54cedd64ffbdae706c4`.
- Source combined diff SHA256 (source base to corrected tip):
  `c8fc2b443ff8730b6321f5232e70529fedb417f093b2779950afce1f48b83856`.

The isolated source checkout was clean at the corrected tip. Main's unrelated
unstaged `.gitattributes` and preexisting `conductor/.conductor_session_log`, plus
untracked `qa/passes/34/` and `qa/passes/35/`, were outside this staged slice. This
review did not stage, modify or validate those unrelated artifacts.

## Exact preservation proof

Twenty-six of the twenty-seven complete staged file blobs are byte-identical to
the corrected source tip. Their individual stable patch IDs also match the
corresponding source-base-to-tip contributions. The only differing full blob is
`fractalengine/src/AGENTS.md`, whose main parent already contains the live
SceneAssetAudit documentation absent from the source base.

For that sole additive conflict, independent raw-byte checks establish:

1. The source tip's host AGENTS blob is exactly its source-base prefix plus a
   467-byte NativeWitness suffix.
2. The staged host AGENTS blob is exactly the entire 125,198-byte main-parent
   blob plus that same 467-byte source suffix.
3. Source suffix SHA256 is
   `b8f61daa677fddd3ac89c87f8f29d263a9e9898409988ffb69285efac7a2cd91`.
4. Final staged host AGENTS SHA256 is
   `40e5e770d19d884b4199dbfa1696e34b873b4a72d3e8a33b84c486d7e93f8250`.

Thus both main's full SceneAssetAudit section and source's exact NativeWitness
section are retained without rewriting existing line endings or text. A combined
host-document patch ID differs because its parent/context differs; exact byte
composition supplies the preservation proof. The parent's reported sequencer
recovery and removal of three marker lines were not replayed. The final index
proves their intended result without requiring trust in those historical steps.

Comparing all staged paths' main-parent blobs against source-base blobs found
only that host-document overlap. The other twenty-six paths had identical
parents (or were absent in both), so exact source-tip blobs preserve every
preexisting main contribution on those paths. The staged path set has no extra
files. The working tree matches the index on all twenty-seven paths. The index
contains no conflict markers, and `git diff --cached --check` returned 0.

## Corrected state retained

All seven correction files are exact committed source-tip blobs, including the
completed Guard/pass binding, retained Prepared Guard, shared lifecycle cursor,
Running-only annotation/publication gates, explicit can_view/unblocked capture
eligibility, conservative edited-path-label refusal and source-audited direct
painter documentation. No initial unsafe A2 branch was retained in place of its
correction. Original A1 ownership/quarantine and normal forwarding seams are the
same reviewed source; no manual conflict changed their behavior.

The main index therefore preserves the prior source security review's scope and
limitations. There is still no operator helper, navigation profile, selected
Inspector acceptance or WGC freshness proof in this milestone. Parent-owned
compilation, actual normal accessibility delivery, real annotation/publication,
lifecycle and required peer/native live acceptance remain outstanding.

## Operations

Read-only Git status/show/diff/patch-id and raw-byte/hash inspection, followed by
this review artifact, were the only operations. No Git mutation, production edit,
compiler, Cargo, test, app, OS/process/input/capture, API/DB/store or network
operation was performed. No claim about current running processes is inferred
from this source review.
