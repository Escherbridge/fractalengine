---
type: Review
---

# R5-NEW review (NEW5-03..07, INT5-11), base a85dc9ad

Prompted to refute. Security review done in a fresh pass over the scene-write door and the registry download path.

## Findings

- N1 FIXED - fe-ui/src/node_manager/mod.rs:1159 - exhaustion leg pumped `now + 10s` nine times; each retry is scheduled from the previous pump's instant (+10s + backoff), so pumps 2..9 were held and the write never dead-lettered (`settled == 1` would fail). Pump k now lands at `now + 10s * k`.
- N2 FIXED - fe-ui/src/actions/node.rs:374 - NEW5-05 returned `Busy` to `FullChannel::Report` callers whenever any write was queued, even with room on the channel; that made blocking `submit_scene_write` return `Busy` (AGENTS.md §scene-write-door: blocking submits never do) and dropped edits the old code sent. A correlated plain write now queues behind the head for every caller, as the gated path already does; `Busy` stays the full-channel-with-empty-queue answer.
- N3 FIXED - fe-ui/src/actions/node.rs:374 - the queued plain write went through `queue_retry` (attempts 1, +50 ms) though it never tried a send, losing one retry; it is now pushed with attempts 0, no backoff. Check and push also happen under one lock (was check, unlock, relock).
- N4 FIXED - fe-ui/src/actions/node.rs:1033 - the order leg was not mutation-sensitive: with the check removed, write B met the full channel and queued anyway. The leg now submits a reporting write C that would be `Busy` without the check and asserts A, B, C arrive in order.
- N5 FIXED - fe-ui/src/node_manager/mod.rs:1171 - exhaustion leg left its dead letter in the process-wide list; it now asserts the newest letter is "Move" (observable Unsaved edits entry) and discards it.
- N6 FIXED - fe-hexon/tests/remote_registry_test.rs:302 - the tamper flipped the archive's middle byte, which can land in an unread member (license, blob) of an unsealed fixture, so verification could pass. It now flips a byte inside manifest.json's data, located through the local file header, which always breaks the manifest read or its listed content hash.
- N7 FIXED - fe-hexon/tests/remote_registry_test.rs:338 - dropped a vacuous `all(...)` over an event list the next line asserts empty.
- N8 HANDED-OFF (later round, R2 test owners) - fe-ui/src/actions/node.rs:202 - tests that build an app with NodeManagerPlugin run `drain_gui_scene_writes` on the global door without GUI_DOOR_TEST_LOCK; a parallel one can pump a locked test's queued write, and N2 widens which writes queue behind one. Every such test should hold the lock.
- N9 KEPT (red by hang, not assertion) - fe-ui/src/node_manager/mod.rs:1177 - if reconcile reverted to a blocking send the frame-never-blocks leg deadlocks on the full bounded(1) channel instead of failing; the runner's timeout still turns it red.
- N10 KEPT (user default) - fe-ui/src/actions/node.rs:374 - uncorrelated writes (for example UpdateNodeTransform) still bypass a backing-off head; N7 policy default accepted.
- N11 KEPT - fe-ui/src/undo.rs:931 - on Full, reconcile resets `started`, so the read-back retries after SAVE_TIMEOUT (15 s), not sooner; matches the task and the "Checking the saved outcome shortly" advisory.

## Security

- S1 PASS - fe-hexon/src/remote.rs:373 - a Content-Length above the cap is refused before `on_bytes` or any allocation; the body loop re-checks the cap before each extend; `download_cap` never exceeds PACKAGE_MAX_BYTES; progress completion never implies install (verification runs after, asserted by the tamper leg).
- S2 PASS - fe-ui/src/actions/node.rs:374 - queued plain writes are only ever sent on the caller's own sender; a queued write cannot skip the gate (plain path is by definition ungated: no petal or not a scene write); test seams are all `#[cfg(test)]`; no secrets logged; send failures warn.
- SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names a file this gate owns.

## Completeness

- NEW5-03 COMPLETE fe-ui/src/actions/node.rs:757 (GUI_DOOR_TEST_LOCK), :765 (pump_gui_door_after), fe-ui/src/node_manager/mod.rs:1048 (retry leg), :1140 (exhaustion leg, fixed by N1)
- NEW5-04 COMPLETE fe-ui/src/node_manager/viewport_pick.rs:853
- NEW5-05 COMPLETE fe-ui/src/actions/node.rs:374 (fixed by N2-N4), test leg :1033
- NEW5-06 COMPLETE fe-ui/src/undo.rs:913, test leg fe-ui/src/node_manager/mod.rs:1177
- NEW5-07 COMPLETE fe-hexon/tests/remote_registry_test.rs:280, :302 (fixed by N6), :331
- INT5-11 COMPLETE fe-ui/src/node_manager/mod.rs:1048 (leg a), fe-ui/src/node_manager/viewport_pick.rs:853 (leg b)

Verdict: PASS-WITH-FIXES
