---
type: Review
---

# R-NEW-2: adversarial review of NEW-05 (registry install byte progress), security review included

Diff reviewed: `git diff 7af0a144 -- fe-hexon/src/registry_source.rs fe-hexon/src/remote.rs fractalengine/src/terrain_bridge.rs` (commit 6dff0306). Security inbox: no entries name these files.

F1 FIXED - fractalengine/src/terrain_bridge.rs:947 - install_from_registry sent (0, entry.size) and then forwarded the transport's own (0, Content-Length); the registry lists size = file length, so the end-to-end journey's strictly-increasing assertion saw 0 then 0 and would fail. The forwarder now drops repeats and regressions (a header-time total correction at 0 still passes).
F2 FIXED - fractalengine/src/terrain_bridge.rs:963 - final (n, n) guard rewritten on the last forwarded pair; n never falls below an already reported count, so progress stays monotonic up to RegistryInstalled.
F3 FIXED - fe-hexon/src/remote.rs:365 - without Content-Length the total was the cap, so an unlisted package showed "x / 64 MiB"; the task asked for the listed size or 0 (unknown). Added expected_total (transport passes download_cap only when entry.size is listed, remote.rs:495) and the throttle now reports while total is 0.
F4 FIXED - fe-hexon/src/remote.rs:365 - the progress readers took &mut dyn FnMut (not Send), which made the pub futures download / download_capped / protocol non-Send for every caller; made them generic (F: FnMut + ?Sized) so no-op callers keep Send futures.
F5 FIXED - fe-hexon/src/remote.rs:398 - download_capped duplicated download_capped_with_progress line for line; it now delegates with a no-op callback, and RemoteProtocolV1Transport::download delegates to download_with_progress (one uri check, one cap rule).
F6 KEPT (correct) - fe-hexon/src/registry_source.rs:1969 - fetch_hexon_with_progress runs requested_uri_matches, check_protocol, refuse_killed in the original order and verify_fetched_hexon on the complete bytes; fetch_hexon delegates; nothing is written before verification.
F7 KEPT (correct) - fe-hexon/src/remote.rs:372 - a declared Content-Length past the cap is refused before the body is read; the per-piece cap check is unchanged; the reported total is bounded by the cap, and received only grows, so the throttle subtraction cannot underflow.
F8 KEPT (bounded) - fractalengine/src/terrain_bridge.rs:935 - each progress event allocates the uri String on the worker thread; at most about 100 + 3 events per install (throttle at max(64 KiB, total/100)), never on a Bevy system.
F9 KEPT (correct) - fractalengine/src/terrain_bridge.rs:1118 - drain arm writes bytes_received / total_bytes_estimate only; no allocation; channel send failures warn.
F10 KEPT (pre-existing, steer 2026-11-13) - fractalengine/src/terrain_bridge.rs:908 - operator registry is the trust anchor (any valid seal); not touched by NEW-05.
F11 KEPT (secrets) - fe-hexon/src/remote.rs:112 - bearer token travels only in the Authorization header; progress and errors carry uri and counts, never the token.
F12 KEPT (journey) - fractalengine/src/terrain_bridge.rs:3236 - journey extended, no new test function; 3 x 256 KiB LCG tiles exceed hyper's read buffer, so the body needs at least two pieces and the event count holds without timing assumptions; reached by R4 `terrain_bridge::`.
F13 HANDED-OFF (HEXON-16) - fe-ui/src/dialogs/hexon_manager.rs:717 - registry installs now leave total_chunks 0, so the Downloads tab shows only the small "x / y" label and no bar; draw the ProgressBar from bytes_received / total_bytes_estimate when total_chunks is 0.
F14 HANDED-OFF (I-DOC-CORE) - fe-hexon/src/AGENTS.md §registry-source-remote - document download_with_progress / fetch_hexon_with_progress, the Content-Length refusal and expected_total (listed size, else 0).
F15 HANDED-OFF (I-DOC-API) - fractalengine/src/AGENTS.md:756 - the "Install from registry (PAS-P5 S7b, A9)" paragraph still describes FetchStrategy::fetch_header; production uses fetch_hexon_with_progress (whole sealed package, PACKAGE_MAX_BYTES 64 MiB) and the chunked core is test-only.

Verdict: PASS-WITH-FIXES (one would-fail journey assertion and one spec deviation fixed; sealed verification unchanged and still before install).
