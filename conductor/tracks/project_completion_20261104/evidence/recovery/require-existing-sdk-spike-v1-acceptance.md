---
type: TestPlan
title: Partial SDK admission spike authored acceptance
date: 2026-11-04
---

# Authored acceptance matrix

No case below has been executed. The new core module is not wired into the SDK
connector or a strict-existing physical opener. These are required disposable
fixture checks after source review and completion; retained roots remain HOLD.

| Boundary | Case | Required observation |
| --- | --- | --- |
| Existing lock helper | Missing LOCK | Error; no LOCK created |
| Existing lock helper | Another owned lock holder | Error; current LOCK content is byte-identical |
| Existing lock helper | Released existing LOCK | Acquires existing entry; PID update only after exclusive lock |
| Normal lock path | Ordinary first creation | Existing behavior unchanged |
| Catalog helper | Missing manifest | Error through existing load path; no replacement manifest |
| Catalog helper | Unsupported manifest version/missing referenced SST | Existing typed loader error; no create fallback |
| Physical handoff, not implemented | Replace/redirection during ancestor/catalog/lock handoff | Refuse actual identity mismatch; no alternate store |
| Recovery policy, not implemented | Corrupt WAL/repair-request/missing side store | Refuse rather than heal/drop records; no false clean recovery |
| Core admission | Missing/incompatible storage version | Error; no version initialization/SDK bootstrap |
| Core admission | Missing namespace/database/table/field | Error; no get-or-create/schema change |
| Core admission | Required field kind/flexible/readonly mismatch or computed field | Error; no repair |
| Core admission | Missing/duplicate owner row, non-string owner or foreign DID | Error; no owner creation/change |
| Core admission | Wrong/missing String requirements on owner/id fields | Reject input spec before query |
| Core admission | Identifier injection or overlong field/value lists | Reject input spec; fixed query shape retained |
| Core admission | Matching existing catalog/schema/version/actual owner | Return the consumed Datastore; no second open; read transaction cancelled |
| Core admission | Computed/virtual field tries network/script/write | Closed capabilities/read-only transaction refuse; no external action or DB write |
| Core admission | Timeout/failing query/cancel | No returned admitted object; no readiness/bootstrap/services |
| Connector, not implemented | Strict-open/admission failure | No check_version/bootstrap/initial credentials/router tasks/connection-ready |
| Connector, not implemented | Successful strict-open plus admission | Promote same engine ownership; bootstrap only afterwards |
| Host, not implemented | Existing true key/secret mismatch, queued Seed | Refuse without healing; Seed never admitted |
| Host, not implemented | Valid retained restart | Logical rows/histories/assets/blobs/keys preserved; maintenance reported separately |

Only the existing lock-acquisition test is extended in the copied source. The
remaining rows are authored requirements, not claimed registered/runnable tests.
Actual engine confinement and required schema coverage need independent review
before these cases can be executed meaningfully. Source inspection cannot certify
read-only query execution, cancellation cleanup or safe physical identity handoff.
