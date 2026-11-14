---
type: Review
---
# R5-DATA review of DATA5-01 (stamp override fan-out test)

R5-DATA-1 KEPT (real DB thread, no stub) - fe-database/tests/node_lifecycle_test.rs:49-65 - Harness passes `Some(scene_tx)` to `spawn_db_thread_with_sync_and_lifecycle`; the real handlers broadcast, nothing is mocked.
R5-DATA-2 KEPT (ordering sound) - fe-database/src/lib.rs:1090,2171 - both sends happen before the reply the test waits on (read-back / NodeMutationCommitted) on one DB thread, so `try_recv` draining has no timing dependence.
R5-DATA-3 KEPT (mutation check holds) - node_lifecycle_test.rs:~897-915,~925-940 - deleting either `send_stamp_override_changes` call empties the fanned list and fails the `len() == 2` assert; the refused-move and plain-node legs assert emptiness so an unconditional fan-out also goes red.
R5-DATA-4 KEPT (values, not just keys) - node_lifecycle_test.rs:~910 - each PropertyChanged value is compared to `get_properties` (stored row), plus scale [1.5;3] literal on the commit leg.
R5-DATA-5 KEPT (Lagged handling) - node_lifecycle_test.rs:~99-108 - capacity 1024 and `Lagged => continue` cannot hide a drop here; other Harness tests unaffected (receiver is just unread).
R5-DATA-6 KEPT (selectable) - conductor/test-batches.md:51,59 - node_lifecycle_test is already in the 22-test R-batch filter; no new test function added (journey extended).
R5-DATA-7 KEPT (hygiene) - no new swallowed send, no hot-path allocation, no clippy items, no leak-list words added.

SEC-INBOX: no entries name fe-database/tests/node_lifecycle_test.rs (inbox has no matches) - n/a.

DATA5-01 COMPLETE fe-database/tests/node_lifecycle_test.rs:829-980

Verdict: PASS (not compiled or run per round rules; types and fields checked against fe-runtime messages and lib.rs).
