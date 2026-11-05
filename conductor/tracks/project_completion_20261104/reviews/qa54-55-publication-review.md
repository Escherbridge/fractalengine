---
type: Review
title: QA54 and QA55 publication bookkeeping review
date: 2026-11-04
---

# QA54/55 publication bookkeeping review

**APPROVED for the exact 142-path publication allowlist below.** This is a publication accuracy review, building on the separate full frozen QA54 and QA55 packet reviews. It supplies no new source, runtime, FULL coverage, restart, peer, SDK opener, CI or release acceptance. No packet erratum or publication blocker was found.

Reviewed `.omc/research/qa54-55-publication-allowlist.json`, SHA256 `6bb95bceead79788e8af1d84421daac087890b8fc31568d7f95533177ab6f0a9`, against main HEAD `6e5d9c921f98bf36819a37ba1247293cb9543fe6`. The final metadata was reread after the root's nested native-status corrections. All 142 paths are unique; the 142 hash-map keys agree with the path list, and every current file hashes to its recorded value. Approval is bounded to those bytes. Later edits require reconciliation.

## Packet and copied-review preservation

- The allowlist contains exactly 126 packet blobs: QA54's 32 payloads plus manifest, and QA55's 92 payloads plus manifest. Independently recalculated every payload's byte length and SHA256 against the frozen manifests; both complete file sets match exactly, without omissions or extras. Seven PNGs remain included unchanged. The two manifest hashes remain `0d480915b9a7ff6b6758ebef5d8f0d5289ee99826279f52b31925d4202ea7d1f` and `54d4382b690b557e597e60651998411fa0be1afee1355047413155114b159753`.
- The actual seven images and original receipt/driver/log/cleanup anchors were already inspected in the independent full packet reviews. This lane did not substitute a second interpretation of that evidence. The copied QA54 full review, QA55 full review and QA55 preliminary review have valid added OKF `type: Review` headers; after removing only those headers, each body is **byte-identical** to its original `.omc/research/` report. No review scope was silently expanded.
- `.gitattributes` adds only `/qa/passes/54/** -text` and `/qa/passes/55/** -text` with spacing. These preserve raw packet bytes against checkout conversion. The exact packet hashes, rather than normalized text, were used above.
- Active UI source files and `conductor/.conductor_session_log` are absent from the allowlist. The publication does not claim to validate concurrent UI authoring.

## Findings and checkpoint archive

`qa/findings.jsonl` preserves the complete HEAD file as an exact raw byte prefix. All original 64 JSONL records are unchanged; only QA-0074 is appended. The result has 65 records: 17 in statuses beginning `open` (3 S1, 9 S2, 5 S3), six `fixed_source_live_pending`, one `fixed_eventual_cleanup_verified_review_pending`, 40 `fixed_verified_focused`, and one `closed`. The checkpoint and metadata counts reconcile exactly.

QA-0074 accurately records the observed default Inspector boundary: at 1280px width the 260px body places Edit/Copy/Delete at x1300/1340/1386, with clipped previews. It preserves the horizontal-scroll hint and does not claim permanent unreachability. It leaves source and live acceptance pending and distinguishes QA-0039 hit targets.

Checked `.omc/research/completion-checkpoint-archive-qa55-receipt.json` against the actual prior HEAD checkpoint and current archive. The 9,247-byte old checkpoint hashes to `ae6f83c8461127a61a4f614f2298a4967027cde44e89aa3f46e7be1374a32a42` and appears verbatim in the appended history. The existing history's first 33,670 bytes hash to the receipt's `03d72b836fcb619a8f33aaa5d75402ff851c397bf7e53714c4a13d107b27bd54`. That preserved worktree prefix normalizes exactly to HEAD history; its differing raw HEAD size/hash comes from existing CRLF representation, not changed historical prose. The new current checkpoint updates present facts while the previous current record remains archived, including its former source/stage/status scope.

## Current claims and unfinished gates

The checkpoint, final metadata and appended plan continuation consistently distinguish:

- Main `6e5d9c92` exact geometry source is independently reviewed and committed, **unbuilt and unobserved live**. Stage `22039d5a-20261104174412728` is the actual QA54/55 stage and is stale relative to that source and the pending UI changes. Its existing 68.3-second build receipt does not become a new combined build.
- Responsive UI v1 was reviewed but requires GRID-B1 for whole Confirm/Cancel controls at the reachable minimum body around 52 logical points; v2 remains authoring. No rebuilt visual or functional acceptance is claimed.
- Shared PID122836 and QA53 PID23304 have recorded stop/absence receipts at the stated times, with eight recorded identities per check. The checkpoint explicitly avoids an exhaustive unknown-orphan claim. This review performed no OS inspection; “none” is reconciled recorded lifecycle evidence, not a fresh process census.
- QA54 remains a failed distributed preflight while Genesis was active, before scenario object writes, followed by actual owned navigation and a complete empty read query. Its missing log projection and out-of-range locator refusal remain preserved. Setup creation of the Petal is not erased by the bounded “before scenario writes” claim.
- QA55 retains all 38 PASS receipts and 521 recorded assertions, actual version-3 product preview, original-checkpoint nested JSON equality including integer `9007199254740993`, unchanged current-row/history bytes, and actual chart values 21/23/25. The driver still has `coverage_ready=false`, `log_review_required=true`; GUI observations are supplemental to the API driver. There is no FULL, playback-timer, group, restart, durability, mesh/native picking or real-peer credit.
- Shared logs retain 14 publication dependency deferrals, two expected invalid-input warnings and nine startup/environment warnings, with zero errors. Mirror mode, replication factor 1 and one local host are explicitly not peer acceptance. The local data assertions do not certify publication dependencies.

The final metadata removes the stale QA51 facts-as-next-implementation wording in the native remaining/status/next-plan fields and points to the actual QA53 plan/source reviews. Older dated plan checkpoint sections remain historical context; the explicit QA53-55 continuation and current checkpoint record the later state. All originally unchecked correctness, permissions, terrain/reporting, lifecycle, Both/concurrency/restart, catalog, five live audits, two clean FULL passes, integrated Rust/harness sweep, exact-SHA green CI, tag and seven-archive download/hash gates remain required. No box was checked merely because focused evidence was published.

## Root QA documentation change

The `tools/qa/AGENTS.md` change is a measured operator precondition: open the requested owned Verse/Petal, then confirm the read-only distributed query reports catalog readiness before distributed cases. QA54/55 support that practical sequence; the text does not assert navigation alone guarantees readiness. It preserves the refused attempt and requires a fresh pass instead of replaying an existing scenario directory.

The source supports the documented fence: `fe-sync/src/distributed_query.rs:892` first requires the open Verse fabric, and `:899` checks `query_catalog_ready` before query planning, returning the exact catalog-not-ready refusal at `:902`. This doc-only paragraph changes no API behavior or scope/authority fence.

## Require-existing evidence preservation

Exactly six allowlisted recovery items preserve the bounded design and its independent review plus four partial-spike artifacts. The design and independent review have OKF headers (`Plan`/`Review`) and exact original bodies. The spike report and acceptance have OKF headers (`ImplementationReport`/`TestPlan`) and exact original bodies. The manifest and source patch are raw byte-identical copies, hashes `bb8cf01cc51613e037fb52deccb853290008975b6ef4ae4d8f3311089d7a1dd9` and `289c95ec0d815e6ba669270fcd93f2f2697f56fed6b8034433fc1a39ac0e6c79`.

The spike report's prominent **PARTIAL / not safe opener / not production-integrable / not approved or executed** statements remain intact. Its required identity-bound existing physical opening, same-engine admission/promotion, side-store/recovery and connector/app wiring gaps are not obscured. The design's independent approval does not approve the incomplete spike code. Authored acceptance is not executed acceptance; retained Owner/recipient roots remain HOLD. Ordinary confined lock/WAL maintenance remains distinguished from forbidden replacement/reseed/healing/logical initialization before owner validation. Publication of these incomplete engineering artifacts authorizes no opener, second offline SDK connection, invented DID/provenance or retained restart. Deep code approval of the spike is deliberately outside this bookkeeping verdict.

Only this review report was written. Read-only source/Git inspection and evidence hashing/comparison were used; no implementation parser, test, compiler, formatter, app, helper, OS/process/native, API, DB or network action, and no Git/source/doc mutation was performed.
