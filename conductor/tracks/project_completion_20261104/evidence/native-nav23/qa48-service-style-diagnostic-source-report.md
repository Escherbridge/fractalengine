---
type: SourceReport
title: Bounded service style diagnostic source author report
date: 2026-11-04
---

# QA48 optional style diagnostic: frozen source handoff

Author `/root/legacy_helper_io_fix`, 2026-11-04. Ready for independent review;
no self-approval, compilation, executed assertions or native acceptance claimed.
Baseline `a16eab87032a41d012e6936776d8dcd9228b7a54`; implements approved plan
`fe9ca08e23a9c7743a02ff13592fce3575a5bd7d0f4ef685129b264ef7a36565`.

Exactly four owned source paths changed, 95 insertions/5 deletions:

| Path under tools/qa/native_observer | Bytes | Raw SHA-256 |
| --- | ---: | --- |
| worker.cpp | 45633 | `8c7f7bffbcc011aa15c7fc2bfb77090b4067d6af32e1222aa4b5c999d31c27bc` |
| controller.py | 16856 | `5c7b1842df9745805981bb1d9f91befde01f9434a2db3566ae253030cf068f56` |
| scenario.py | 10143 | `0b6f2482fdbde1ed2a73f13f34b00156760672f4cbc3719484eaa6e95b591252` |
| AGENTS.md | 29298 | `bbfab95de071abfd811a7875faa9d9573b073688b621a920e17ae14fe77dcd44` |

Frozen packet `.omc/research/qa48-service-style-diagnostic-source-v1/` contains
four byte copies under raw/, source.diff (11040 bytes,
`e8c0ce8f28924f0aaa9c53fe5ee86e49e258ac4e5ba077b412282dd16c3b89c3`), and
manifest.json (1949 bytes,
`b3dbc57a872c51bd1aa12027d906a785c60ecd48c7f91779009270e483fd7c50`).

The worker names the same existing normalized DWORD style/exstyle reads, applies
the unchanged strict classifier, and throws a local ServiceStyleRefusal only for
its exact style failure. The subtype derives from Refusal: the existing generic
post-Invoke catch still handles it and retains submitted_state_changed after S_OK.
The dedicated top-level catch retains its single DWORD only for successfully
parsed observe mode. Emit additionally checks exact code, raw unspent/no-Invoke/
no-return flags and zero HRESULT. No new native read, census, predicate, action,
provider query, proof, root/target adoption, input or capture path is introduced.

Controller worker_output now requires explicit operation mode and confirmed helper
exit arguments. It accepts only exact original keys or those keys plus
service_style_u32, with type(value) is int and inclusive uint32 bounds. The optional
case additionally requires observe, integer exit2, exact refusal/zero HRESULT and
all three raw flags false. These checks precede the existing independent ledger
spent override. Legacy absence remains accepted; old controllers fail closed on
the extension. Exact stage/helper pinning and all byte/request/ledger/schema/
success/exit rules remain. Diagnostic zero is a sample, not read-success proof.

Only existing scenario.exercise is extended: production-parser legacy absence,
min/max/controlled representative uint32, bool/float/string/null/range failures,
wrong code/HRESULT/flags/key/mode/exit and duplicate key. Its existing spent ledger
also stays spent while a validated observe result reports raw false; this is an
offline parser/ledger assertion, not execution of controller.execute or native
sampling. No new test/function declaration or registration. Scenarios were authored,
not run. The controlled 0x94000000 sample makes no claim about QA48's actual style.

The exact diagnostic comparison still refuses; subsequent exstyle/rectangle/child
comparisons are not proven passed. Initial/repeated samples are not an atomic
timeline. Root must independently review/build/run and obtain fresh exact-value
evidence, interpret it against SDK22621 and separately review any policy change.
QA48 supplies no actual style value, positive service/UIA or Invoke acceptance.

The initial combined scenario/doc patch had a context mismatch and wrote neither
file; the corrected patch applied both. No other source/evidence/Conductor paths
were changed. No compiler/parser/formatter/tests, helper/app/process/OS/native,
API/DB/network action or Git mutation occurred. Root owns integration/execution.
