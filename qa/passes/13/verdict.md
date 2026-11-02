---
type: QA Pass Verdict
pass: "13"
date: 2026-11-02
stage: 2661374-20261102062342042
focused_smoke: true
full_coverage: false
status: functional-pass-distributed-restart-failure
---

# Rich data, native views and persistence

The initial real-API scenario passed38 checks, including preflight. It created
Alpha/Beta/Gamma, installed and revised nested descriptors, retained the integer
9007199254740993, committed property and full-transform changes, replayed exact
versions, paginated history, appended out-of-order raw/derived measurements,
checked provenance bytes and per-anchor aggregates, and checked latest event time.
Actual returned object IDs, schemas, states, rows, trajectories, request receipts,
hashes, CSV/JSON and offline HTML remain under rich-data/20261101-191920-658.

The driver kept coverage_ready=false because deliberate invalid-property and
incompatible-schema cases logged warnings. log-review.json retains their exact
targets/sequences and root's reconciliation against the owned validator logs.
Original summaries and hashes remain unchanged; independent warning review is
still pending. There was no retention gap in those cases.

Native UI evidence shows Alpha selected in the correct Petal with full current
transform retained after navigation. Its read-only version3 preview shows position
[3,0,4], while the live object remains [6,0,8] with current gain3. The measurement
chart/table displays calibrated21/23/25 in event-time order. The schema editor
reports stored revision2 and prepares revision3 with identical rules; no save was
sent. Root opened and judged every generated PNG. These focused checks do not
exercise every control or complete the final layout/UX audit.

## Actual same-build restart

Root47816 and seven fresh captured descendants were stopped; independent checks
found all eight exact identities absent. The same saved root restarted as80424
on the same stage, with reseeded=false and matching build inputs. Four exact
hierarchy color/icon preferences survived and were inspected in the restarted UI.

Read-only restart verification passed exact current/historical state hashes,
schema descriptors, uploaded GLB/provenance bytes, movement/history checks, local
measurement rows and local aggregates. It then failed distributed-calibrated:
the result was empty and all shard/host coverage arrays were empty. A later
read-only query at UI frame13034 repeated it. This is openQA-0035; no readings were
re-ingested, and an empty distributed result is not accepted as complete. Startup
catalog restoration is being corrected in an isolated source lane.

The campaign is unfinished. No full coverage, two-pass convergence, final suite,
CI success or release publication is claimed by this verdict.
