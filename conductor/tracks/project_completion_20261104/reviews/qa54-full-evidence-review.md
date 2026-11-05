---
type: Review
title: QA54 focused frozen evidence review
date: 2026-11-04
---

# QA54 independent full focused-packet review

Verdict: **APPROVED as an immutable failed-preflight packet plus subsequent owned navigation/readiness evidence.** This is not a successful rich-data scenario or FULL pass. No correction to the frozen packet is required. QA55 is a distinct later attempt in the same run and cannot turn QA54 into PASS.

Manifest `qa/passes/54/packet-manifest.json`: **5717 bytes**, SHA256 **`0d480915b9a7ff6b6758ebef5d8f0d5289ee99826279f52b31925d4202ea7d1f`**. All **32 payloads**, **29 original anchors**, local_source copies and the sole actual PNG match recorded bytes/hashes, with no missing or extra payload. The driver artifact index has eight valid linked hashes; its sole receipt, context, summary and exports agree. Read shared UI guidance and inspected the actual screenshot with view_image. No implementation or live runtime was executed.

## Failed preflight and preserved continuation

The screenshot `screenshots/175203-136-qa54-rich-preflight.png` is a readable/nonblack 1280x720 actual app image with Genesis Verse active, no selected object and an empty Inspector. Its source receipt has include_ui=true/source orbit. The raw preflight state/UI agree. The screenshot does not show the intended rich Petal already active, and does not prove a scenario edit.

Driver preflight declares the intended QA Basic Verse and new Rich Simulation QA54 Petal. Its requests are hierarchy/schema/log reads, MCP tools/list and read-only query POSTs. The distributed query returns HTTP200 with the embedded refusal `distributed catalog is not ready for this Petal; restore its owner and ancestry first`. The receipt is FAIL, despite two preliminary assertions passing. Both debug-log projections return404; watermark/end-watermark say unavailable and log_retention_gap=true. These are preserved evidence limitations, not a clean empty warning window.

Context has no objects, versions, states or completed scenario steps; measurement exports/candidates are empty. Summary correctly records one failed step, data_assertions_succeeded=false, coverage_ready=false, log_review_required=true and root-recorded driver exit2. No scenario object mutation request precedes the refusal. This does **not** mean setup made no writes: `owner-create-rich-petal.jsonl` separately records the actual Petal creation, and the basic fixture was already prepared.

Subsequent ordinary navigation receipts open the owned Verse and Fractal, preserve an out-of-range `--nth` locator refusal as status0/refused=true, then click the unique visible named Petal card. The corrected input receipt has one accepted click; the later state names the intended Petal, with no selection. The subsequent catalog read returns complete empty readings with mode mirror/replication_factor1/one answered local host/no missing hosts. This supports observed readiness after owned navigation. It does not establish an internal catalog root cause, an authority bypass, an independent peer or completed scenario writes.

The operator locator error remains a refused command, not a product failure or accepted input. No old artifact was rewritten to erase it. QA55 attempt-context explicitly links this failed predecessor and the readiness evidence before its separate attempt.

## Driver and staged-source provenance

Independently recomputed the actual driver's four-file canonical fingerprint from fe.py, rich_data.py, rich_data_scenario.py and rich_data_report.py: `55266bae3220b160b57a2759c8b7c25d40ae4350017d81c44a95cc2a73f46824`. The current scenario's canonical JSON hash is `d14893061bb5c929bd2256dfd706b3cd88586f402fa7bb3a04845096abffa8ef`. Both match every driver/context provenance declaration inspected. Driver code returns2 when coverage_ready is false; that is consistent with this failed attempt. It was read, not imported or executed.

The shared app is PID122836, birth ticks639294365584083852, run20261104-174918-156. Owned context and driver provenance agree on source22039d5add074d4df0e18f5847286606d95407ac and fingerprintCCD017690F94B09F690A5962E7C7FDB92953F69E70AF9632744D99731C13A0BC. Captured stage_matches_source is true **at launch**. The stage manifest explicitly has source_dirty=true, so this is not clean-commit-only provenance or a present-day current-source acceptance claim.

The frozen manifest is byte-identical to the retained actual stage; actual fractalengine.exe, seal_hexon.exe, legacy_adoption_fixture.exe and native_observer.exe hashes all match. The root build receipt records session74576 terminalexit0/staging68.3s. Its referenced original stage log hash matches `71c4d63d15044feb35bb5fa898e4792c6813be15351594ee12fbc082eeb3f04b`, with completed Cargo output, worker.cpp and final staged path; the PowerShell wrapper around ordinary stderr is not a compiler diagnostic. Terminal exit is root receipt provenance, not a new process inspection. Launch log records the exact PID/run and completion16s. Later source changes are unverified by this sealed stage.

## Shared logs, scan, cleanup and limits

Both packets intentionally summarize the same full app lifetime. The original private stdout independently matches **111858 bytes**, SHA256 `646015f1accec9ce63a35d352d272ae120211114339b1d14cb57cd7faf96b139`; strict UTF-8 yields787 lines,695 INFO,25 WARN,zero ERROR/panic/compiler diagnostics/private-current-read markers. stderr is empty. Counts match the packet. Fourteen warnings are publication dependency deferrals; two correspond to QA55's expected calibration/schema negatives; nine are startup/environment warnings. These later shared warnings do not become QA54 per-step observations. Publication deferrals remain unresolved delivery limits, not clean logs or proof of failed local data results.

Recomputed the four fixed credential patterns over all **30 explicitly listed text inputs** using their declared strict encodings: all counts match and are zero. This is finite-pattern checking, not universal credential absence. Raw application logs remain private; only verified summaries are published.

Cleanup receipt at **2026-11-05T01:03:49.8744670Z** contains exactly the same eight complete recorded identities as owned context, all pid_present=false and recorded_identity_present=false. The shared stop log says stopped/run retained. This is timestamped recorded-identity absence, not an exhaustive orphan census or new OS check. The app/run/store is shared with QA55; these are not two independent app starts or restarts.

No FULL coverage, retained-store/restart/durability, peer delivery/render/pick, native provider, historical playback timer, completed-save/group matrix, final integrated tests, CI or release credit. Screenshot tree elision and some empty glyphs remain visible; this is not blanket UI polish acceptance. Only this report was written, with no source/packet/Git mutation, implementation parser/test/build/compiler/formatter, app/process/OS/input/native/API/DB/network operation.
