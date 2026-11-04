---
type: Review
status: approved_publication_bookkeeping
---

# QA38 through QA41 publication bookkeeping review

**APPROVED for the exact 345-path publication scope below.** Current HEAD is `d8da3a46e9861e9191845ea283e37fb7e5ea3472`; index was empty during inspection. Root must stage the explicit allowlist and independently compare every staged packet blob to its frozen manifest before committing. This approval is document/evidence bookkeeping, not new runtime, native helper, capture, FULL-pass, final-sweep, CI or release acceptance.

This final review supersedes the current-document hold in `qa38-39-publication-bookkeeping-review.md`. Read-only Git inspection was explicitly authorized by root for this lane. Only this review file was written. No source, packet, index, history, app, process, API, database, build, test or network mutation occurred. The preexisting `conductor/.conductor_session_log` was excluded throughout.

## Frozen raw packets and attributes

Recomputed the full packet inventory and all recorded payload hashes again against the final publication state. All pass; all four original manifests remain unchanged.

| Packet | Manifest SHA-256 | Payloads | Actual PNGs | Files including manifest |
| --- | --- | ---: | ---: | ---: |
| QA38 | `133024b57f5a2eaf59e3ed27f88a70b1e5ecc8fe9e3f6980dd7e25f4b8f4378a` | 152 | 16 | 153 |
| QA39 | `ec654fd786690db3bc28b41f23681db4356ee2fe9021a2de3d24e0fc315e1ec7` | 26 | 1 | 27 |
| QA40 | `e548af07241ec36ba0e6c6af5a044340d0778e9aa283fe74516c893f4116698b` | 17 | 1 | 18 |
| QA41 | `3bf5ba5a56cc2dc0a8825a4930d0d6a20c2198ab24ba35b02b39edd91175e359` | 132 | 17 | 133 |

Exactly **331 raw packet paths** are in scope. `git check-attr --stdin text` confirms **all 331 have text unset**. The `.gitattributes` diff only adds `-text` rules for passes 38–41 plus separators; existing rules remain. The unanchored spelling for 40/41 still applies correctly to every intended path, as verified by Git rather than inferred. No packet newline normalization is permitted.

The independent full reviews preserve their distinct verdicts: QA38 failed with new S1 QA0071; QA39 accepts one normal scoped terrain update/readback; QA40 failed before input with three Owner-tree 409s; QA41 accepts bounded corrections while retaining layout failures and its mandatory external erratum. Their older builds/runs remain distinct from current d8 source. This bookkeeping lane relies on those full reviews for their visual interpretation; it did not repeat QA38/39's completed image reviews. This reviewer personally inspected the actual QA40 image and all 17 QA41 images in the preceding full evidence lanes.

Finite scans and credential limitations remain explicit: QA38 135 text files, QA39 24, QA40 15, QA41 114, including their stated BOM handling. Earlier full/bookkeeping reviews independently checked the exact finite patterns and zero matches. There is no generalized secret-absence claim.

## Findings prefix and exact status counts

The original **61-record HEAD findings body remains an exact raw prefix**, **62,585 bytes**, SHA-256 **`30cbf89257db8f08d1f8819393f38bf925be90bb79ba42b235bee1fc2bd80aa6`**. No original record was closed, rewritten or reformatted. Exactly two new records are appended. All **63 IDs are unique**; **16 records remain open/reproduction**, split **three S1, eight S2, five S3**.

QA0071 and QA0072 both remain `open_reproduced`. Their new source/focused metadata points to actual existing reviews. QA0071 now distinguishes source-reviewed and focused Circle checks from broader matching live acceptance, with `source_reviewed_focused_circle_passed_broader_pending`; it preserves the unknown historical QA0024 trigger and missing journal/broader gesture gates. QA0072 identifies reviewed d8 ordering and focused Owner recovery, while preserving broader native acceptance. The original reproduction `first_seen/last_seen` fields describe the failed packets; the separate focused fields describe QA41. They do not rewrite failed historical evidence.

Older historical root-cause notes in the original 61 records are preserved intentionally; the checkpoint/metadata carry the current source and acceptance state. The current register counts match those current documents.

## Conductor history and current claims

Reverified the historical preservation receipt against `git show HEAD:.../checkpoint.md` and the actual archive. `checkpoint-history.md` consists of the stated **95-byte OKF ProgressHistory prefix** plus the **16,248-byte** historical tail starting at `# Historical acceptance baseline`, exactly equal byte-for-byte to that HEAD tail. Tail SHA-256 **`2d2ab7c5e5d6681a05eb577fc552ab2a929320f93eccd842c32a5ba76c966319`**. This is preservation of the specifically receipted historical body, not a claim to preserve the replaced current-state header.

The final current checkpoint, metadata and plan:

- Name current source d8, successful 86.5-second d8 stage and earlier 209.7-second 5a stage separately from the retained failed 10194 build attempt. No active build is claimed.
- Record QA41's exact eight identities absent at its saved cleanup timestamp and no owned app. No new process inspection was performed by this reviewer.
- Retain failed QA38/QA40 verdicts and bounded QA39/QA41 evidence. QA41's 102 physical-ID joins, one legitimate Circle/node and valid saved draft are correctly separated from native provider, authority and full visual acceptance.
- Preserve 13 residual 0.75 target failures, the 52-pixel narrow Inspector, Add's four-pixel bottom clipping, ruler overlap, incomplete checker coverage and other missing Results/gesture/camera/size/scale checks.
- Keep journal, retained restart, queue/revocation/concurrency, late ownership, rollback, general geometry, complete catalog, five live UX audits, two clean FULL passes, one final integrated sweep, exact-SHA CI, all Release jobs and downloaded archive verification pending.
- Keep the residual layout work isolated and source authoring distinct from acceptance. Relevant plan checkboxes remain incomplete/partial.

The QA41 sentence now links its external erratum with correct grammar. Relative Markdown links in the three current documents resolve. Existing Conductor markdown files touched/new in this scope carry their expected OKF frontmatter; metadata and JSON receipts remain machine data.

The retained failed build-attempt log/metadata and earlier source-review files are already tracked, unchanged in this pending publication; they are not extra unstaged additions silently added to this allowlist.

## Review copies, erratum and native source freeze

All five new Conductor review copies have OKF `type: Review` frontmatter. After removing only that frontmatter and its separating blank line, each body is **raw-byte identical** to its corresponding `.omc/research` original:

| Review copy | SHA-256 |
| --- | --- |
| `reviews/qa38-full-evidence-review.md` | `54a2028540225fc3915ae9beafabaf2e712a55cbfbbaf1dc7def855545a431cc` |
| `reviews/qa39-full-evidence-review.md` | `48d51d6ce5a611ca923990f43b76267ca5cc6215e419e54f1ba87f6141e4dc00` |
| `reviews/qa40-full-evidence-review.md` | `151e2f8c72462189cba41e45faf36e918cd4b265f017f352b836b0c005924d9d` |
| `reviews/qa41-full-evidence-review.md` | `96df8deea1a7562eeef4bb744d210a4389ce9400ebae08412ba4cf0d8918e1af` |
| `reviews/qa41-residual-layout-design-review.md` | `5e9ec7797a01c0d4dd26c668abca06e1f7643f964632d1f4bb0c5b3a4c38e65c` |

The external `evidence/qa41-summary-erratum.json`, SHA **`9a9cff4ae43a5c5b63bfbd4bfa28941e01289489def0073578ed8246425fa8ea`**, correctly binds the erroneous `limitations[6]` zero-byte-artifact sentence to the unchanged QA41 manifest. The unsupported command failed before its output write; no artifact exists and its raw stderr was not archived. The original wrong frozen sentence is not silently edited or adopted. Publication must retain the erratum link beside QA41 claims.

The two new `evidence/native-nav23/` artifacts are explicitly **source-frozen / both full reviews pending**, not approvals. The report has additive OKF ImplementationReport frontmatter; its source body exactly equals the isolated author report. Its copy SHA is **`72540a1c11d2bab82f917387ba5fb0c0b664d376be32d6571de93287596974d9`**; underlying report SHA is **`4e13019585b2a9b58f14dbb07cb832103c9c25235bd95656ebf578debf2bf036`**. The copied raw hash manifest is byte-identical, SHA **`6e3b29d54137fe2df6facae39d91af20ec2db643c213bf25d2e3f4a007710bb1`**. All **24** current isolated source-file hashes match that manifest at review time. This confirms the freeze being described, not source correctness. Its source diff is identified as **`85a3f1e82fd3e466f2bf7cec6c0393aba77b60a9dbd5a70a4770d1f1c809aea8`**, foundation 023a; integration must preserve later d8 ordering. Full functional/security reviews, compilation, actual UIA/lifecycle/durability and capture remain unperformed/unaccepted for this slice.

## Exact publication allowlist

Approve only these **345 paths**:

1. Every frozen file under `qa/passes/38/` (**153**), `qa/passes/39/` (**27**), `qa/passes/40/` (**18**), and `qa/passes/41/` (**133**): **331 total**, exactly their manifest inventories including each manifest.
2. `.gitattributes` and `qa/findings.jsonl`: **two**.
3. Under `conductor/tracks/project_completion_20261104/`: `checkpoint.md`, `checkpoint-history.md`, `metadata.json`, `plan.md`: **four**.
4. Under that track's `reviews/`: the five exact copied review names listed above: **five**.
5. Under that track's `evidence/`: `qa41-summary-erratum.json`: **one**.
6. Under that track's `evidence/native-nav23/`: `qa-native-navigation-nav23-v1-source-report.md` and `qa-native-navigation-nav23-v1-hashes.json`: **two**.

Do not stage by the entire Conductor directory. Explicitly exclude preexisting **`conductor/.conductor_session_log`**, `.omc` research/state, isolated worktrees, future active packets, and all source/tooling changes. A later OKF copy of this bookkeeping review would be one separately enumerated extension to **346 paths**; its source body should remain exact.

Read-only `git diff --check` passes for the allowed tracked documentation/findings/attributes diff. Git emits ordinary prospective CRLF notices for normal text docs; raw packet attributes are independently unset. No staged-blob verification is claimed at this empty-index review point. Root owns explicit staging, all 331 staged-blob checks, exclusion verification and commit.

## Final reviewed working-byte anchors

| Path | Bytes | SHA-256 |
| --- | ---: | --- |
| `.gitattributes` | 415 | `de2e3f97ea2d820fdad9b5faf872c77fc90e8c2af1f8ce355784966861820731` |
| `qa/findings.jsonl` | 66018 | `9fa3afb34370f7a4d1203a766a867f1d8c56ab540bb3093ab42854e2692e81dc` |
| Track `checkpoint.md` | 8266 | `1a36afe20c08033530b7540cd2cf55d228c2e4eb0e3c28d2a24515e63c2f4367` |
| Track `checkpoint-history.md` | 16343 | `1d60ccd5e889155ef9748dd4fdfdf21af404edb2f38c1451bd6253dfbbf83860` |
| Track `metadata.json` | 4488 | `44d7dd12babf05a43738664b92f45e2bfcf2bb92bad49de4b7eda4644861fc1f` |
| Track `plan.md` | 10973 | `69e4b4773a44d4e2fb85bb4d342f230b75cc09471324562c25136bd0318d545e` |

No remaining publication blocker was found in this scope. All broader goal and acceptance gates remain as recorded.
