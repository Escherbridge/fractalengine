---
type: Review
date: 2026-11-04
title: Earthwork volume correction security source review
source_artifact_sha256: e78b95e989b30ee4c6e04d4c1ffc8cb022a406098b1be7153b74ecc8e0d33a55
---

# Earthwork ownership corrections: independent security source review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for this corrected source slice.** The three original MED ownership and
projection blockers are addressed in the reviewed production paths. No new backend
authorization or transaction weakening was found. This is source approval only;
QA-0069 remains open pending integration, build and actual acceptance.

## Frozen boundary and preservation

Reviewed `.omc/worktrees/qa-volume-measurement-ownership` atop
`bef56e790fdd80ce5688055c820ee11709e0a3f9`, the nineteen dirty tracked files,
author report and `qa-volume-correction-frozen-manifest.json`, original functional
and security reviews, and QA33's saved-evidence/source investigation. The source
report records preservation of seven inherited dirty files; this review inspected
the complete final correction rather than discarding that inherited work.

All nineteen raw working-file hashes exactly matched the frozen manifest. The
captured complete `git diff --no-ext-diff --no-color` SHA256 is
`8df02dd60399e4a53e241b0fc2f31c7f5e23e4c5c856c84a69bb1841686255fb`, matching the
author's receipt. `git diff --check` returned 0. Independent declaration counting
found 152 test attributes before and 152 after in the correction-touched Rust
files. These checks do not execute Rust or the authored journeys.

## MED VOL-S1: original-demand gate now precedes projection

The actual Update dispatcher recognizes matching owned capture, direct commit,
Resolve and failure packets before generic editor/replica receipt routing. Its
owner-controlled projection_receipt requires exact accepted result ownership and
the original nonretired write. Before any receipt-derived committed-version,
admission, property, descriptor or Inspector mutation it requires:

- current running/editable/unblocked access and exact navigation/Petal/scope;
- loaded nonpending confirmed terrain, current raw fingerprint and binding;
- the original report still being the current retained report/source, matching
  assignment identity, finite positive scale, complete bounded expected/sample
  coverage and a finite nonnegative pair;
- current height generation, original scene incarnation/access generation,
  original node SceneAdmission and original node cache epoch;
- exact original receipt/request, target/member/state identity, only the expected
  cut/fill property changes, genuine positive increasing history version and no
  regression against the manager's known version.

Resolved receipts additionally require the current captured member set to match
the receipt and the captured terrain/target contract. The existing complete
external_editor_batch/editor_batch preflight still runs before the mutation loop.
It is additional validation, not a substitute for the Demand check. The receipt's
own new local admission cannot satisfy the original-source check because that
check has already completed before insertion. Invalid, retired or stale matching
owned packets are consumed by the private owner path and do not fall through to
generic TRS/selection/Undo projection.

Outcome recognition remains separate from current projection. An exact earlier
committed receipt may drain its accepted owner while current projection is
withheld; finish only marks a current confirmation when the dispatcher positively
projected it under the checked current demand. Malformed/foreign receipts do not
certify the save or automatically resend it.

## MED VOL-S2: terminal reset retains accepted ownership

VolumePersistence is an independent initialized Resource. UiManager contains only
the derived VolumeView, so its actual clear_for_terminal_reset no longer destroys
the accepted write/capture/Resolve identity. clear_read_models first retires the
volume owner: unsent/read/display/confirmed intents clear, accepted writes become
retired and remain owned. The terminal PostUpdate path returns before pump and
leaves the view empty, emitting no measurement write, capture or Resolve command.

The bounded terminal channel drain takes at most CHANNEL_BUFFER results, releases
the receiver mutex before mutating World, and gives results to the outcome-only
observer. Only exact owned validated settled outcomes can clear that write slot;
RequestFailed and unrelated packets do not relabel an unknown save as canceled.
This observer has no property/cache/TRS/UI projection path. Results already moved
to a Bevy message buffer may instead remain unobserved during terminal mode; their
unknown owner remains retained until the actual worker boundary, rather than
being falsely reported settled.

The renamed trusted maintenance hook clears remaining retained outcomes only
from the existing Supervisor::poll_database branch after the Supervisor's actual
owned DB JoinHandle is finished and join returns successfully. An abnormal join
sets unconfirmed failure and returns before that hook. No arbitrary JoinHandle
proof, UI flag or fabricated join capability was added. Exact settled outcomes
may drain earlier; remaining unknown ownership clears only at this successful
worker boundary. The public hook remains a trusted host integration seam, not a
user/API authority token. Its actual production caller was inspected.

## MED VOL-S3: direct and resolved saves share property-only projection

Both direct and resolved eligible receipts use the same actual dispatcher branch.
It advances genuine committed version and local scene admission and feeds the
existing property/descriptor/Inspector hydration. It never calls committed::apply,
inserts Transform or DbConfirmedTransform, calls NodeManager transform acceptance,
or changes user Undo/redo. The checked target is an ordinary earthwork node;
path/stamp member shapes are explicitly excluded by capture validation.

The owner no longer carries or pumps a GetNodeProperties refresh intent, including
current-equal and uncommitted-resolution outcomes. A matching stored pair can skip
a save without inventing a saved version. This removes the new owner's prior
property-read -> generic replica current -> full-TRS/gesture-cancel escape. Existing
generic external/replica behavior is unchanged for its ordinary callers. A
genuinely independent external update remains subject to those existing semantics;
this correction does not promise to suppress such updates.

### Late reply and repeated Resolve boundary

Reviewed the possible direct-commit fallback after the owner drains. There is no
retired-request tombstone that would classify an artificially duplicated later
EditorBatchCommitted as private volume work; generic external receipts can accept
an equal version with a matching local admission. That is not established as a
reachable current production ordering and is not reported as a new defect.

The actual DB worker uses one rx.recv loop, awaits Commit/after-commit publication,
then sends its sole direct reply through blocking send_result before processing a
later Resolve. Runtime drain_db_results writes channel results to Bevy messages
in receive order. Accepted Resolve A may time out and be followed by Resolve B,
but the earlier direct commit still precedes both. A stale Resolve A can become
unowned when B is current; EditorBatchResolved has no generic external-commit/TRS
fallback and does not satisfy the unrelated selected-set owner. B can then drain
the exact current Resolve correlation. This reasoning depends on the inspected
single-writer/FIFO route; a future reordered or duplicate-delivery transport would
need explicit private-request classification/retention. No response duplication
was invented to claim a live failure.

## Confirmed terrain identity cache

The confirmed fingerprint is now updated at the actual active-Petal DB document
replacement seam, once per confirmation. Optimistic and cleared replacements
invalidate it, including the layer opacity branch missed in the inherited work.
Production assignment inspection found only the two intended terrain document
methods writing terrain_json; other direct assignments found in the inspected
UI sources are test fixture setup. Ordinary ResMut/UI borrows no longer trigger
recursive canonicalization. pending_set and actual binding/source comparisons
remain separate gates, so optimistic bytes do not receive confirmed authority.

## Preserved backend and accepted-write contracts

The correction does not change the original checked batch backend, generic replica
projection, schema/history transaction or role enforcement. The existing local
Editor/ancestry/publisher fences still validate at DB use. The one-node Properties
batch carries both volume fields in one containing transaction, with full captured
state, Petal terrain source and journal guards retained. No two legacy independent
property writes, field-level automatic rebase, fabricated receipt, peer convergence
claim or historical fact rewrite is introduced.

Full/Disconnected before acceptance remains definitely unsent. Accepted timeout
or disconnection retains the original request/capture and permits only bounded
read-only Resolve retries; it does not replay the mutation. Original source loss
withholds current confirmation while preserving the actual accepted outcome.
Complete finite true zero remains valid; missing/partial/nonfinite coverage does
not become persistable numeric zero. Broader geometry, scale and producer scheduling
remain the separate functional review lens.

## Required execution and limits

The existing production-dispatcher journey authors direct/resolved and changed
height/raw/epoch/access/retired cases, with current confirmation versus zero stale
cache/admission/version mutation, no generic refresh command, and preservation of
newer preview TRS, confirmed baseline, selection/gesture and Undo state. The actual
read-model/UI reset seam is exercised in authored code with accepted ownership;
failure keeps it unknown and the trusted callback clears it. These callback
assertions do not execute a real Supervisor worker join. All are authored and
unexecuted in this review.

Parent must preserve this exact contribution during integration, compile, perform
actual navigation/no-churn, unavailable/true-zero/source, direct/resolved late-save,
newer-gesture and terminal/reset acceptance, then run the final integrated sweep.
Old facts 103-106 remain auditable. No QA-0069 closure is supplied here.

Read-only source, pinned contracts, Git status/diff/hash and retained evidence
inspection plus this review artifact were the only operations. No Cargo/compiler,
tests, app/process/input/capture, OS/API/DB/store/network operation, production edit
or Git mutation was performed. The parent's separate live app was not touched.
