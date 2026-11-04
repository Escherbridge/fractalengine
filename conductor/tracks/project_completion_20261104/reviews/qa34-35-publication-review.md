---
type: Review
date: 2026-11-04
title: Focused visual evidence publication byte-preservation review
source_artifact_sha256: fecfcad03fdbeb9869593d7c1b46c5c574f397add374b25653be56e4c78ce226
---

# QA34/35 publication bookkeeping and index review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for publication of the staged focused evidence.** No blocking bookkeeping or byte-preservation defect found. This follows the independent full evidence review; it adds no runtime acceptance or full-pass credit.

| Packet | Final manifest SHA-256 | Payloads / total packet files | Retained full-review anchor |
| --- | --- | --- | --- |
| QA34 | `23dba66af8e83817f75911ca22e28ab951a0ffed80ed0fd7cb45d339a0262da5` | 40 / 41 | `bcb12de4995097144f0d73f0e905df42708368587b368ae702e132fce4d2eda5` |
| QA35 | `ff998b7d26c3b32a5334ee27a32c2c7df8cfa0caa0805f050bdd1acddf5f929e` | 46 / 47 | `e2140e1c91f988e7d201f8cbabd5083e5b98118c09fcecfeb359aed5e291d2e1` |

Independently read every staged packet blob via the Git index and compared it to its raw working-file bytes: all **88 packet files** match exactly. Recomputed all final payload sizes/hashes and manifest hashes. Each packet contains the exact original full-review artifact, SHA-256 `6fb17680dff9a1d806bbd500f8fe6677168a87396d0fab56e18c48779c6d2285`, with the same review hash and original manifest anchor in metadata.

To verify preservation beyond trusting the new manifest, independently reversed only the review-bookkeeping changes in memory: removed the added review payload, restored the prior verdict's pending label/absence of review anchor, and restored prior manifest review/count fields. Serializing with the original CRLF format reconstructs **both exact previously reviewed manifest hashes** above. This confirms the original file inventory and payload metadata are unchanged except for the explicitly reviewed verdict bookkeeping. Current payload bytes match those hashes; raw CLI, images, contexts, cleanup and log captures remain intact. The original authored summaries retain their earlier pending wording as captured derivations; the final verdict/manifest and included independent review record focused approval explicitly.

Both verdicts now state `approved_focused`, retain their original review anchors, and keep `full_test_sweep_executed`, `full_coverage`, `gold_pass_credit` and `convergence_credit` false. VIS-002 and VIS-003 remain open. No earlier evidence limitation or failed attempt is removed.

The index contains exactly **90 intended paths**: the 88 packet files, `.gitattributes`, and the Conductor evidence-review copy. The staged attributes contain the explicit `-text` rules for QA34 and QA35. The Conductor review has valid Review frontmatter with the original artifact hash; its body lines exactly match that artifact. `conductor/.conductor_session_log` is excluded from the index. No production file is staged in this slice. CRLF-aware `git diff --cached --check` returns 0.

The active combined Rust build was not touched. No Git mutations, source edits, applications, OS/process inspection, API/DB/store access, network requests, compiler/test execution or new evidence capture were performed in this lane. Work consisted of read-only index/file/JSON/hash checks and this report.
