---
type: SourceReport
title: Observed exact service style author report
date: 2026-11-04
---

# QA50 exact observed style: frozen source handoff

Author `/root/legacy_helper_io_fix`, 2026-11-04. Ready for independent source
review before build; no self-approval or executed acceptance.
Baseline `71cd5f8e9f05bd39dbb1a61c17a95ca054dda5fe`; approved plan SHA-256
`59d5ba166d3e7acc677bcc9f0ec420f106daed2840686420fa627c06a989fc91`.

Exactly two owned paths, 13 insertions/4 deletions overall:

| tools/qa/native_observer path | Before SHA-256 | Frozen SHA-256 | Frozen bytes |
| --- | --- | --- | ---: |
| worker.cpp | `8c7f7bffbcc011aa15c7fc2bfb77090b4067d6af32e1222aa4b5c999d31c27bc` | `719558c5a52d57ad16ce065ee11cad920c7de0b3bd251283655fedf384f1ba21` | 45651 |
| AGENTS.md | `bbfab95de071abfd811a7875faa9d9573b073688b621a920e17ae14fe77dcd44` | `b7e509200941fe10363e37ca4c103df8bf837af0bca7acaab329e20ec948c59d` | 29880 |

Frozen packet `.omc/research/qa50-exact-service-style-source-v1/` contains both
raw files, source.diff (3005 bytes,
`d6569a17f091ac1bcc3f0154946b437b2a047e823803b74503c0b5fefbfb0c4f`), and
manifest.json (1204 bytes,
`2672f2bab76d9c21017ceb40b3cd0c36d5b7f229de6389619ea32a9aa38ed955`).

The sole C++ delta is the expected_style expression:
WS_VISIBLE|WS_POPUP becomes WS_VISIBLE|WS_POPUP|WS_CLIPSIBLINGS (exact0x94000000).
Its existing direct inequality refusal remains unchanged. Old0x90000000, any
required-bit removal, and any extra bit fail that full-value comparison; there
is no mask, alternate value or ignored bit. This is source arithmetic/review
reasoning, not executed negative-case coverage.

Every other C++ byte is preserved: exact exstyle0x080800a0, zero outer/client
coordinates, refusal priority, identity/class/process/root-thread/owner/parent/
child checks, census/counts, original optional window set, watcher and rechecks,
bounded style diagnostic, controller/stage pinning, COM, proof/deadline, ledger
intent/spent handling and final Invoke/outcome floors. No added native reads or
actions. No controller, scenario or test declaration changes.

Directory rationale now records the required full style and QA50's observed
2483027968/0x94000000. SDK22621 names its difference WS_CLIPSIBLINGS and official
documentation describes painting/clipping. The causal OS normalization mechanism
remains unproved; no universal Win32 behavior, downstream component admission,
positive UIA/provider/navigation/Invoke, FULL, CI or release credit follows.

Independent review must verify the exact one-expression delta and unchanged
strict negatives/floors. Root owns restaging and fresh process/controller evidence;
later component failure must remain refused and receive its own investigation.
Old immutable stage bytes and QA50's failed result are preserved.

Only the two assigned source/doc paths and this source packet/report were written.
Other authors' Rust, evidence, Conductor and goal work remain untouched. No
compiler/parser/formatter/test execution, helper/app/OS/native/API/DB/network
action or Git mutation occurred; Git was used only for read-only diff/HEAD.
