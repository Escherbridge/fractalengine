---
type: Review
date: 2026-11-04
title: Terrain grant exact integration review
---

# QA-0070 exact staged integration: independent review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the exact 35-file staged source integration.** No merge overlap, lost existing contribution, frozen-source mismatch or unexpected staged path was found. This is preservation approval, not compilation, test/runtime acceptance or QA-0070 closure.

## Exact proof

- Main HEAD: `35485826b7008745133918e7ca67f3a278d39a12`.
- Approved isolated source commit: `d448b2f99b0255529184ecccaed7d16877d1e156`, original base `fa9555632380977c41fe89ac984fcb491c47295a`.
- Main staged `git diff --cached --binary`: **86898 bytes**, SHA-256 **`1ce60bcfddb2695b658f0fed5541d755b3eb11cdfda3122e194fc213de6eb57d`**.
- Independently computed isolated `git diff --binary fa955563 d448b2f9` is **byte-for-byte identical** to that entire staged patch, not merely patch-ID equivalent.
- Staged path set is exactly the frozen manifest's **35** paths, with four approved new files. Every staged full blob equals the approved frozen working-tree file and committed source file after only CRLF-to-LF normalization. Every frozen raw file still matches its approved manifest hash/byte count. Every staged blob hash matches the main integration receipt.
- For all existing changed paths, main parent HEAD content equals the isolated original base after newline normalization; the same four paths are absent in both parents. Thus **there are no intervening main content overlaps in these 35 paths** requiring a semantic merge, and no main parent contribution was replaced by an older version.
- Every changed working-tree file equals its staged blob after newline normalization. No conflict marker was found. `git diff --cached --check` passes.

Original frozen source approval anchors remain the 35-file manifest SHA-256 `03fcbe6377050735c803740409498baa39e54cd98708d8af12b301e7815cba6c` and source patch artifact `085fe65db89eac3a958595f7db9bae577149051d9fc339248df8d3081146cce6`. The committed Git patch has its own exact hash above; full-file comparisons establish preservation against the original raw-source review anchors. Read main's independent functional approval and the previously completed security approval, both for source integration only.

## Preserved contracts and main changes

Because the complete contribution is identical to approved source, the required Local/Api terrain grant, original admitted ancestry and signed claims, configured issuer binding, retained original JTI/generation/token projection, bounded private read routing, eight-permit/five-second capture ownership, same-transaction required tables/credential/strict expiry and actual local publisher/ancestry fences remain intact. The actual 10-second mutation waiter and real Api Manager-cap fixture remain. No Local downgrade or generic reply escape was introduced during integration.

The 35-path scope leaves all other index paths at main HEAD. Additionally compared the following 13 whole index blobs directly with main HEAD; each is byte-identical: divider layout_policy/results_dock/topbar, panels/mod and sidebar, viewport header/card source, plugin wiring, UI native accessibility mod/surface, host native witness mod, host native accessibility mod, and terminal-reset mod. The existing divider v4, header and native A2/reset contributions are therefore preserved. No native-navigation NAV-1B worktree contribution was inferred as merged or approved here.

## Exclusions and remaining gates

Unstaged `.gitattributes`, QA-0071 finding changes in `qa/findings.jsonl`, untracked QA38 payloads and the pre-existing Conductor session log are outside this staged integration. Do not include them through broad staging. Parent reports QA38 exact owned stop and eight identities absent at 18:20:31Z; no new process or cleanup verification occurred in this lane.

Root retains commit/build ownership. Actual constructor compilation, extended existing journeys, composed SQL/response slots, both metadata-table losses, channel-only capture, queued/prepared authorization refusal/rollback, real permit/deadline/late-result behavior and supported DB-worker/reset ordering remain the execution gates described in the source reviews. No universal concurrent shared-handle isolation, retained-store, native/capture, CI/release or overall completion acceptance is granted.

Only this review artifact was authored. No production or Git mutation, compiler/build/formatter/test, app, OS/process/input/capture, API/DB/store or network operation occurred.
