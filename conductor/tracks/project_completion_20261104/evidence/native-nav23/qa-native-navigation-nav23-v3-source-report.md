---
type: ImplementationReport
status: independently_source_approved_execution_pending
---

# NAV23 v3: confirmed historical-stage lifecycle repair

Author `/root/legacy_helper_io_fix`, 2026-11-04. Source-only freeze for both full
independent re-reviews. No compilation, executed fixture, native acceptance or
complete-source approval is claimed.

## Exact scope and preservation

Same isolated checkout `.omc/worktrees/qa-native-navigation-helper`, HEAD
`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`. No Git mutation, main edit or app,
runtime, database, renderer, pointer or physical-interaction change. All 25 approved
NAV-1B v3 raw source/doc anchors rechecked unchanged. Later main host/UI changes,
including d8 Owner-before-provider ordering, must survive eventual integration.

V2's 28 raw snapshot files rechecked with zero size/hash mismatches before edits.
Its complete source patch, v1 delta, manifest and report remain frozen:

| Preserved v2 artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| source.diff | 201242 | `b1372272984e68df812e3fa07399a878ee982f2974df72ae47b2d45f7942689f` |
| review.delta.diff | 61054 | `42377e8fe7a0a4082f180d0f517e65133093b3770caeb58ebfcd87ef995deee5` |
| hashes.json | 8144 | `05f266c26c3f816a2ee670650e5a7da63a28b4d00ad328643b7ca39192ab60b1` |
| source-report.md | 14411 | `a576f0d4dcc49285c09fc0ce3d90210e0b1ec3775a79d89f603c8dde73388a38` |

Read MAIN `.omc/research/qa-native-nav23-security-v2-review.md`, SHA-256
`e0d269ffaac730d855dc8531473c4361cc5dd2a75c66ad9dcfe8a99ff3744bef`.
Parent approved the focused design direction before editing. That is architectural
direction, not full source or runtime approval. Existing helper plan/identity/native
boundaries, directory guides and shared UX guidance remain applicable.

The full v3 slice remains **28 files**. Exactly **four** paths change from v2:

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `tools/qa/_native_observer.ps1` | 21164 | `878bf3ba80ab03faf218db33e31eb70909fb18fc4576658b4f15c2b6dfdae9a0` |
| `tools/qa/native_observer/lifecycle_scenario.ps1` | 19301 | `ae322a252a363a1a71e3c899f064f6a8d7c6a9e9212ebca901a1c057141340ab` |
| `tools/qa/native_observer/AGENTS.md` | 21259 | `22ff52e6010ab62b541aace83bb894ae478e813af3e6e784c1bc8539eafde85b` |
| `tools/qa/AGENTS.md` | 87980 | `3b2efe0a03eec9fb8f0dbb4b053885617ddc60198922a842a3089acd1d6821fc` |

The other 24 v2 files are raw-identical. EOF handling, shared caps/admission,
actual paired reseal, fresh issuer, native CLI's no-token/no-HTTP boundary and
ordinary unpaired upgrade options are preserved exactly. No new source path or
registered test declaration is introduced.

## Frozen v3 packet

All artifacts are in this checkout's `.omc/research/`:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `qa-native-navigation-nav23-v3-source.diff` | 212946 | `45c40eda6e6b2e9ba989121ff7af98d5a9db0e4cfb06c34f2dbca2f8a9156744` |
| `qa-native-navigation-nav23-v3-history.delta.diff` | 22630 | `fb193841726105f64e99d7beaee3e4585cb949ba51e14936c0d233006515e45c` |
| `qa-native-navigation-nav23-v3-hashes.json` | 7616 | `d5dcbd663d02913d01ab5528ffcb1265a6ab2ace0d5a768e6199db6af86420fa` |

`qa-native-navigation-nav23-v3-snapshot/` retains all 28 exact raw files plus
the packet artifacts and this report. The full patch is against HEAD023a;
the delta compares v2 raw files. Normalized patches serve review; exact raw hashes
and snapshots govern source identity.

## NAV23-V2-S1 production correction

The counterexample is supported unpaired upgrade: observe on A, confirm exact
worker exit, archive the start and restart on B, retain the live run/B, then clean
prunes A. V2 later called the real stage reader for A before confirming the receipt,
so valid retained history poisoned subsequent stop/restart/clean.

`Get-QaNativeWorkerRecords` now separates historical ownership from materialized
live-image capability. Every receipt still requires bounded canonical JSONL,
exact schema/keys/operation and this Run's exact current or archived start. App
PID/birth/exe, recorded stage path, supported stage kind, well-formed recorded app
SHA, native helper schema/file/hash and original bootstrap ledger/run/file-ID/path
remain bound. Observe-only records still require their app-derived run ID and
no bootstrap/ledger identifiers. An unmodified copied foreign subtree/start cannot
become this Run's owned history. No missing file itself establishes confirmation.

Process identities have one shared exact-shape/type validator. Worker identities,
when present, must use the staged helper path and be born no earlier than the app
or controller. Terminal code/type/range and phase order are checked. Only the exact
three-line `reserved -> worker_started -> exit_confirmed` sequence with identical
valid original/terminal worker identity is confirmed. Duplicate/reordered terminal
markers and changed identities refuse; a lone terminal is never confirmed.

A valid early-start failure can produce a two-line terminal without a durable
worker_started record. That remains unconfirmed, preserving the producer's actual
failure contract without granting absence. A null terminal identity also cannot
confirm. Exit codes admit the actual signed/unsigned Windows return-code range,
not a string, floating value or boolean masquerading as an exit result.

After full receipt/provenance classification, **confirmed** history needs no
old stage.json, application executable or helper binary. Its original recorded
start/stage/bootstrap metadata remains retained and checked. Consequently existing
clean policy can prune obsolete confirmed-only stages and otherwise dead runs;
there is no new retain-everything rule or dropped historical record.

**Unconfirmed, unknown and reapable** history still calls actual
`Get-QaStageManifest`, which validates the existing stage/application hash, and
matches the complete actual stage manifest to an exact owned start. Missing or
changed stage refuses. `Stop-QaNativeObserverWorkers` is otherwise unchanged:
it still refuses a live/unverifiable controller, pins the real helper image,
checks its actual SHA, uses exact worker PID/birth/exe ownership and confirms
owned exit before releasing its process object. No absence is inferred from a
pruned stage, no unknown becomes pristine and no attempt is reset or replayed.

`Get-QaNativeRetainedStages` and clean.ps1 remain unchanged. Unknown history keeps
its real stage/run dependency; confirmed history no longer has an undeclared
dependency on an intentionally prunable binary. This fixes the dependency at
the ownership validator rather than retaining every old run/stage forever.

## Existing meaningful lifecycle journey

The existing evidence-boundary journey already calls scenario.exercise and its
unregistered PowerShell lifecycle support. V3 removes that support's constant
Get-QaStageManifest override. Scratch stages now contain real stage.json plus
dummy image bytes with matching hashes, read by the actual production stage
reader. These files are never executable or launched. Only unrelated process and
native-bootstrap authority seams remain controlled.

The added regression sequence authors these production-boundary checks:

1. Create Stage A and a properly bound three-line confirmed receipt.
2. Archive the exact A current/start; publish current B with a different actual
   scratch stage/hash, process incarnation and fresh recorded bootstrap/ledger.
   This models the supported unpaired upgrade; no app start is claimed.
3. With A present, temporarily make the record unconfirmed and ask production
   `Get-QaNativeRetainedStages`: it must retain A through the real stage reader.
4. Restore the confirmed record and ask the same production reference decision:
   A must no longer be retained solely for that confirmed history.
5. Resolve/check the absolute scratch target inside the scratch stage root and
   remove prunable A. B remains present. Complete production ownership parsing
   and native-helper stop must still succeed, without touching the process API
   seam for the already-confirmed helper.
6. With A absent, unstarted, reapable, lifetime-unknown and lone-terminal histories
   must refuse before any process/reap call. Duplicate terminal and mismatched
   identity histories also refuse. A foreign Run in the archived start is still
   rejected despite the otherwise confirmed receipt.

The 255-receipt plus ledger/history-cap cases now also use actual scratch stage
files rather than a constant reader. Prior ordinary/native paired transition and
issuer cases remain intact. There is no new registered test declaration. Fixture
authorship gives zero actual app/native/process/durability/upgrade checkpoint credit.

## Verification boundary and remaining work

Read-only source/Git inspection, byte/hash processing and source/report authoring
only. No compiler/parser/formatter, tests/journeys, app/helper, process/native API,
input/capture, HTTP/database/network or Git mutation was executed. The scratch stage
deletion above is authored fixture source, **not performed in this turn**.
`git diff --check` and added-source trailing-whitespace inspection are clean; they
do not establish parsing or execution success.

Both full independent reviewers must inspect the complete v3 slice and focused
delta, including strict phase/identity handling and real-stage fixture dependency.
Parent alone integrates reviewed exact raw source and runs the integrated sweep.
The real sequence observe A -> exact confirmed exit -> unpaired restart B -> clean
prunes A -> next stop/restart/clean remains an execution gate. Unknown/reapable A
must retain real dependencies, and a genuinely missing stage must refuse safely.

All prior native request EOF, inherited handle/absolute timing/late-start,
durable ledger/crash/unknown cleanup, capacity, real paired restart/reseal/issuer,
Viewer/Editor one-visible-Petal and independent state/data/role checks remain
required. Owner HTTP floors, original one-second host proof and alias retirement
stay unchanged. No source packet claims GUI consumption or rendering; capture is
still unavailable. Final campaign checks, exact-SHA green CI and tagged download
delivery remain root-owned unfinished work.
