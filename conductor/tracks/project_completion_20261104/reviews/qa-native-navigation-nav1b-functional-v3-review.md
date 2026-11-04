---
type: Review
date: 2026-11-04
title: Native navigation functional v3 review
---

# NAV-1B v3: independent NAV-F1 functional source re-review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.
Verdict: **APPROVED for source integration of complete NAV-1B v3. NAV-F1 is
addressed; no additional blocking functional, inspected API or readability issue
found in this correction.** This supersedes the NAV-F1 hold on the exact v2
source, while preserving its other findings, source assessment and execution
limitations. No build/test or live-navigation result is claimed.

Applied shared UI/UX guidance and reviewed the actual five-file correction,
production role-result owner/dispatcher and expanded existing journey against
the preserved complete v2 source. Only this MAIN artifact was written. No
production edit, Git operation/mutation, compiler/formatter/build/test, app,
OS/native API/process/input/capture, API/DB/store or network action occurred.
Root's main/live QA39 app was untouched.

## Exact complete revision

Checkout `.omc/worktrees/qa-native-navigation-helper`, foundation
`541aef483c2bffb01975744e882ed310d1bfc612`.
Independently verified all25 current raw file byte counts/hashes against the v3
manifest and the following artifact digests inside the checkout's `.omc/research/`:

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| `qa-native-navigation-nav1b-v3-source.diff` | 122,314 | `5ac3688beb10967c4b961cee7f524c9878d176bee42d8f40c714a8b37be44a33` |
| `qa-native-navigation-nav1b-v3-nav-f1.delta.diff` | 12,612 | `9ff6a2f416a795732d6a2a3038a6784c9246e5809b089bfbe8995e30b9a35f68` |
| `qa-native-navigation-nav1b-v3-hashes.json` | 7,298 | `e14503f105ed4ed6c0ce63f001a0426aeac5bfc923019aa695ddb2831a6adba2` |
| `qa-native-navigation-nav1b-v3-source-report.md` | 8,439 | `de5fa8410e3291dfe8f71d33d53778e76382f7c5214a47aabe2a2a466d5d3d6d` |

Preserved S1 v2 manifest remains
`054ab8b832edde26f24a6e4535ab5e5491c5d0b6399bad4805beaafcf609ab59`,
and its correction remains
`0aa6524b7715246ed4dafc15f688c3e0e131607858512b30f9538cfc626923c3`.
Within that22-file scope only the navigation directory doc and UI journey
changed. All actual S1 production retirement/identity/provider/typed-delivery
files remain raw-byte-identical. Three existing source-owner/doc files extend
the review scope to25, yielding exactly five changed files in the correction.

Actual registered declarations over the25-file scope count68, matching the
receipt's foundation/v2/v3 counts. The original22-file scope remains21. The
larger count is existing db_results coverage, not newly registered tests.

## NAV-F1 is corrected in the real source owner

`VerseManager::accept_scene_role` now initializes access_generation only after
the exact current-scope check, using get_or_insert_with. This closes the default
None epoch gap before the unchanged product_guard reads it. Repeated matching
resolution preserves the same epoch; a prior invalidation-generated Some epoch
is retained rather than replaced. This supplies source identity, not a role
grant or a substitute for current DB-local Viewer proof.

The existing role parser, blocking logic, denied-state clearing and role-refresh
ownership remain. Unknown/None roles still become RoleLevel::None, mark access
blocked and fail can_view/current-source eligibility. An epoch initialized for a
denied role does not authorize navigation. Stale-scope replies return before
epoch initialization and before the existing role-cache handler updates state.
Actual denial still clears scene sources, which advances the hierarchy revision;
the correction does not make a previously invalidated proof current again.

Independently checked unchanged node_index.rs SHA256
`efce459c980bad4b10016234a00c6950698bf1bade32d21a75b0e2fab6f0724e`.
Its existing assertion that role resolution cannot erase a prior invalidation
generation is retained. The get_or_insert implementation directly preserves
that invariant; the assertion has not been executed in this lane.

## One typed production result seam, with coherent owners

`db_results::apply_local_role_result` is crate-private inside the existing
crate-visible db_results module. It accepts the actual DbResult type and returns
without effects for other families. For LocalRoleResolved it calls the existing
manager acceptance and local-role cache handlers, then applies the same matching
scope/Pen cancellation predicate that production previously used.

The real dispatcher's early LocalRoleResolved arm now calls this seam, and its
later arm is empty instead of updating the cache a second time. Manager access
state, displayed local role and dependent Pen state are therefore handled once
for the same result. The earlier cache update introduces no extra role grant or
external producer; it removes the fixture/production split. Both manager and
cache handlers retain their current-scope checks, and stale replies leave Pen
state untouched through the matching-scope condition.

The signatures, module visibility and calls match by source inspection. This
small shared reducer is preferable to duplicating the role effects in the
native fixture or constructing an unrelated full dispatcher resource graph.
It does not alter DB wire types, HTTP/MCP floors, durable role mutations or the
actual per-target Viewer read. No general authority cache/service was added.

## Fresh bootstrap authored through the corrected path

Harness::unresolved now creates the real default manager and role cache.
Harness::guard returns the unchanged actual product_guard result; the former
Guard.access assignment, manual source refresh/proof-key reassignment and direct
LocalUserRole assignment are removed. Fixture role resolution supplies a typed
LocalRoleResolved to the SAME production seam described above.

The same registered journey now authors these cases:

- Fresh current Verse, no active Fractal/Petal, matching Viewer.
- Fresh current Fractal, no active Petal, matching Viewer.
- Fresh current Verse, no active Fractal/Petal, matching Editor.

Each starts with absent epoch/role, refuses unresolved target submission, refuses
a stale-scope role result, accepts the matching role through production, and
checks the actual product_guard epoch plus repeated-resolution preservation.
Normal rendering then queues exactly one bounded typed target read. The explicit
successful target-view fixture reaches the completed navigation classification,
navigation_petal_v1 profile and actual provider remap/Snapshot. Assertions retain
the original Verse-only cursor rather than silently selecting a Fractal/Petal.

Separate None and unknown-role cases use the same production seam and assert
blocked/can_view refusal, no queued target read, no positive surface and no remap.
The existing S1 stale-ID/old-deadline/recovery/deferred-callback/read-retention
journeys now obtain their initial access through that same real source path.
These are meaningful authored checks of NAV-F1 rather than another arbitrary
Guard fixture.

The role replies and target-view success are still typed fixtures. This is not
real DB Viewer/Editor acceptance, an executed full dispatcher/Windows handler
journey, or proof that these assertions pass. Those distinctions remain explicit
in source docs/report and in this approval.

## Prior NAV-1B contracts and remaining gates

The S1 production seam is byte-unchanged: failure retirement advances the checked
process-static observed source before try_lock, keeps accepted in-flight read
ownership and tombstones, and cannot revive old bindings during full recovery.
The full-tree/reference remapper, actual output/provider ordering, strict
immutable one-second deadlines at all typed use boundaries, ordinary physical
IDs/focus and unrelated A1 same_base behavior retain their prior reviewed source
assessment. This correction changes none of those production paths.

Root may integrate the exact approved source and run its compilation/integrated
existing-journey sweep. Actual fresh matching DB-role bootstrap, the complete
native handler/Adapter.next route, provider ordering and recovery, every
render/annotation/Last/PreUpdate/begin/callback expiry boundary, stale-scope/denied
refusal, lock/context/reset behavior and accepted-read ownership still require
execution evidence. No live closure follows from this source approval.

Operator helper/staging and durable attempt ledger, original same-worker COM
element/InvokePattern and watcher ownership, uncertain worker lifetime handling,
all-Verse bootstrap, selected Inspector/node/group support, WGC/capture and final
project/CI/tagged-release acceptance remain later work. The existing approved
helper and identity design boundaries remain binding.
