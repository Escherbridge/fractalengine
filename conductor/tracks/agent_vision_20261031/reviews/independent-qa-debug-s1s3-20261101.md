---
type: Review
date: 2026-11-01
commit: 7dcc0be
reviewer: independent harness_security_review agent
scope: static security recheck and committed smoke evidence
---

# S1/S3 debug bridge independent review

All previously reported HIGH/MED findings are corrected in the immutable commit. The reviewer used git show
while another writer worked on S4, made no source changes, and ran no cargo, apps or tests.

| Finding | Resolution checked |
| --- | --- |
| Background event payload assigned to the current UI verse | Payload requires an explicit matching origin; unscoped/foreign events retain only static diagnostic metadata. |
| Partial Bearer redaction | The complete next non-whitespace credential span is removed before output bounding. |
| Invalid queries bypass rate admission | Full opt-in/Owner/rate admission precedes parsing; admitted_run avoids double charging. |
| Password label references | Protected nodes cannot supply accessible names or values through labelled_by. |
| Bounded protection traversal | Unvisited nodes become protected; traversal password state also suppresses names. |
| Object api_base | A string is required before URL parsing; malformed metadata returns a typed refusal. |
| Infinite wait deadline | Deadlines must be finite and positive. |

The committed pass02 verdict documents live named-rectangle clicking, authoritative state, REST/MCP foreign-scope
refusals, fake-JWT redaction, typed malformed-client refusals, current-source staging, run-local storage/profile
and exact recorded-process cleanup. It correctly makes no full-coverage or convergence claim.

Material evidence remains pending: campaign-end journeys/clippy, same-run secret restart, non-unit-scale live
clicking, normal OS accessibility-provider integration and actual FE frame-time comparison. S4 will complete
standalone/modal title aggregation. Existing layout failures QA-0015 and QA-0018 remain open for S6.

VERDICT: PASS FOR REPORTED HIGH/MED CORRECTIONS; CAMPAIGN VERIFICATION PENDING
