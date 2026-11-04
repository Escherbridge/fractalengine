---
type: Review
date: 2026-11-04
title: QA36 publication bookkeeping review
---

# QA36 publication bookkeeping: independent review

2026-11-04. **APPROVED for the intended QA36 evidence/documentation commit scope described below.** The initial stale pending-review wording in metadata and VIS-002/VIS-003 was corrected by the author and the resulting actual diff was re-read. This is publication bookkeeping approval, not new runtime acceptance or staged-blob verification.

## Exact intended scope

Include `qa/passes/36/` (154 payloads plus its manifest), `.gitattributes`, `qa/findings.jsonl`, Conductor completion-track `checkpoint.md`, `metadata.json`, `plan.md`, and the two new reviews `reviews/qa36-full-evidence-review.md` and `reviews/volume-live-security-plan-review.md`. These are **162 paths** before any separately requested publication-review copy. Exclude all `qa/passes/37/` payloads and the pre-existing `conductor/.conductor_session_log` change. Do not use broad `git add conductor` or `git add qa` to infer this allowlist.

The only `.gitattributes` additions are `/qa/passes/36/** -text`, a blank separator, and `/qa/passes/37/** -text`. They prevent Git text conversion of raw evidence. The pass-37 attribute does not authorize including that still-unreviewed packet. Independently checked all **155 pass-36 paths** using `git check-attr`: every `text` attribute is `unset`. No staged payload claim is made; root must compare staged blobs with the manifest before committing.

## Packet and review preservation

Re-hashed the frozen QA36 manifest: `2c28abd2f725696bffd655dc1727d0a1114077cb6f6ac1b1c9b2f24a96ea6473`, unchanged. Rechecked all 154 payload SHA-256 values against it. The previous full evidence review inspected all 14 actual PNGs and verified finite credential/BOM scan, real required camera.pose fields, partial 5402/16384 withholding, completed 16384/16384 true-zero atomic versions 5/6 with unchanged 1–4, exact history bytes, and eight recorded cleanup identities. This publication pass does not repeat execution.

Both new Conductor reviews have OKF frontmatter with `type: Review`. After removing only that frontmatter and its separating blank line, their bodies equal the original source review text exactly under normal text newline decoding. QA36 body hash is `6e60a5ffeb269e50d4766a721bfcedbfe64a1684a5199989fecb03f37aea3334`; the volume-plan source body hash is `6d5d5aee6e43cf78238cd4ce46425d2992b3dfa2f2ba43f69221502ab47e9f00`, matching its declared `source_artifact_sha256`.

## Findings and claim limits

The original **60 finding records are preserved as an exact raw byte prefix** of `qa/findings.jsonl`. Precisely one JSONL record, unique ID QA-0070, is appended. Parsed totals are **61 entries**, with **14 statuses beginning `open`**: **2 S1, 7 S2, 5 S3**. The new S2 record describes the source-confirmed terrain grant/ancestry continuity gap, explicitly says no live bypass executed, keeps actual local publisher checks, identifies token subject as issuer-node DID, and leaves queued-authority/final acceptance blocked. The linked volume-plan review supports these claims. No old finding was closed or rewritten by this diff.

The checkpoint current header and corrected metadata explicitly limit QA36 to focused evidence. In particular, unchanged v6 history is described as stability across navigation commands and **no observed departure/return**. VIS-002 remains partial because active-Petal breadcrumb contrast is poor; VIS-003 retains measured 10-pixel divider failures, broader scale/modal/cancellation and isolated-v3 source/live gates. The combined stage is recorded as 327.5 seconds with one unused-mut warning; no warning-free, error-free, final-sweep or full-pass assertion is introduced. Native/helper/capture, QA-0069 broader acceptance, QA-0070 implementation/runtime, complete catalog, two full passes, exact-SHA CI and release gates remain pending.

QA37's author observations (200 fill, 100 cut, 4 m³ box/material) are explicitly unfrozen/unreviewed and give no final acceptance. Later appended checkpoint text and `current_owned_apps` supersede the earlier active-run narrative with the author's exact stop/eight-identity absence report at 17:19:41Z. This bookkeeping review does **not independently certify QA37 results or cleanup**; its raw files remain excluded from this commit. QA36's eight-identity cleanup was independently checked in the copied full review. Historical checkpoint paragraphs are retained under a historical heading rather than erased.

## Exact reviewed working-tree hashes

| Path | SHA-256 |
|---|---|
| `.gitattributes` | `1c810535de7230b809ad9c03c9262755aa5bf2b9ab23d6637670524aa174299b` |
| `qa/findings.jsonl` | `30cbf89257db8f08d1f8819393f38bf925be90bb79ba42b235bee1fc2bd80aa6` |
| completion `checkpoint.md` | `3f7a321c36b3acd77b63b207669dd041f59af03f45c43df53ab6eb7649667d3d` |
| completion `metadata.json` | `bd68449392ac82d6b6e1ff52ec9ef97c1481db431e47a01efbe1807a5fdcb583` |
| completion `plan.md` | `3b6dfdc9895a153fe79d81a9fed2c12ddf3508a76eef0dfaca273704ed03009a` |
| `reviews/qa36-full-evidence-review.md` | `d9a2508203cebed1a20c17f5f0fbc79317d3a90451c13bc97c5d55eff5936178` |
| `reviews/volume-live-security-plan-review.md` | `32a477ab5d3d4a9607a05b082a63775cbdfc2b8f0671186b025ffa0d7b31cce8` |

The allowlisted tracked `git diff --binary HEAD` hash is `458dd45f3bd30436d6c595ab6d0a4187f6bf6c145bf7d7c2c3e8796e9cc4de41`; the two untracked review bodies are separately hashed above. Allowlisted `git diff --check` passes. Git emits normal policy warnings about future LF/CRLF conversion for documentation; all QA36 raw paths are explicitly protected with `-text`.

No production source, Git index/history, tests, builds, apps, process state, API, DB, or network was changed or executed in this review. Only this review artifact was authored. Root retains responsibility for explicit staging, exact staged raw-byte verification, commit, and later acceptance gates.
