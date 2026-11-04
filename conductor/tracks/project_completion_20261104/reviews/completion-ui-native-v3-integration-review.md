---
type: Review
status: approved_exact_source_integration
---

# Combined native/UI v3: independent exact staged integration review

**APPROVED for the exact staged 58-path composition described below, before root commit/build.** All 47 staged source paths preserve the independently approved final native v3 and residual-layout v3 source, with only ordinary CRLF/LF normalization in 29 index blobs. Main's prior source and d8 Owner-before-provider ordering are preserved. No source re-authoring or conflict resolution was needed. This approves exact integration preservation, not compilation, schedule materialization, passing journeys, native operation, live UI, capture, FULL passes, final sweep, CI or release.

HEAD: **`34b4450c09783f089687a171e14ac5ad878544df`**. Main Rust source at this HEAD remains byte-identical to d8 before the approved staged changes. Index remained frozen during inspection. Only this report was authored; no source/index/history mutation, compiler/parser/formatter/test, app/helper, OS/native/process/input/capture, API/DB/network action occurred. Read-only Git/source/review/hash inspection only. Root alone owns commit and build.

## Exact staged composition

The staged binary diff is **435,148 bytes**, SHA-256 **`03830c5b72dc170b925a31c14e8c92d031dba2fc205211170ec0fa4be68c9ba8`**.

The index's complete changed-path set equals `.omc/research/completion-ui-native-v3-stage-allowlist.json` exactly: **58 unique paths**, no extra/missing path, no session log. Allowlist bytes **2821**, SHA **`dfefc0219224873b4b1ea48ff26f7cf6982fe2987f3b0227c10669fd7f175f40`**.

Independent verification, rather than reliance on the parent's assertions:

| Check | Result |
| --- | --- |
| Native source paths | 28, exact approved final v3 manifest |
| Residual-layout source paths | 19, exact approved final v3 manifest |
| Source path overlap | Zero |
| Main working files vs approved raw source/hash/size | All 47 exact |
| Index source blobs vs approved raw files | 18 raw-exact; 29 exact after CRLF -> LF only |
| Source index mismatch | Zero |
| Existing main HEAD inputs vs slice's declared base | All 30 exact after CRLF -> LF only |
| New paths absent from main HEAD | All 17; no tracked-file collision |
| Review/report/hash-manifest support paths | All 11 checked as below |

The copy/preflight receipt `.omc/research/completion-ui-native-v3-copy-receipt.json` has **9470 bytes**, SHA **`92d82fe094078b4a848037d2d616b9d54a270aa7bf998e6229b3f87d7eff303e`**. Its main-before HEAD and every one of its 47 copied raw file lengths/hashes agree with independently read files. Its before-base result was independently repeated using immutable Git blobs; it is not merely accepted from the receipt.

## Approved source identities and preservation proof

Native source baseline: **`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c`**. Full 28-file manifest SHA **`d5dcbd663d02913d01ab5528ffcb1265a6ab2ace0d5a768e6199db6af86420fa`**; approved full patch **`45c40eda6e6b2e9ba989121ff7af98d5a9db0e4cfb06c34f2dbca2f8a9156744`**. Complete source approval is recorded by the independent security and functional v3 reviews.

Residual-layout baseline: **`d8da3a46e9861e9191845ea283e37fb7e5ea3472`**. Full 19-file manifest SHA **`62c34c5ac10fa0faf536877e766d023034b6fe889e807077e9756889179db59c`**; approved full patch **`611890ec0a3ab56f37556e6b7295b1ce93b32b49f24bd2614a7bbbbeec799d52`**. Read both final complete functional review and root's separate boundary review: both approve source integration with execution gates. Root's report explicitly records that it did not author the isolated slice.

For each source path, the full approved current raw source was hashed against its manifest, compared to main's raw working file, and compared to its actual index blob. All 29 normalization-only comparisons remove **only** CR bytes immediately preceding LF; no whitespace-insensitive patch match, formatting tolerance, hunk-only approximation or semantic guess is used. Full-file equality therefore preserves every approved source contribution.

For each existing path, independently compared main **HEAD** blob against its slice's immutable declared base. All 30 match after that same limited newline normalization. Therefore no later main change was overwritten by a stale full-file copy. The native and UI path sets are disjoint, so no merge of the two implementations was necessary. All 17 newly introduced paths are absent from main HEAD, consistent with the recorded no-collision preflight.

## Existing host and interaction guarantees retained

Independently compared **25 unchanged source sentinels** between index and main HEAD: all native UIA/provider/navigation identity sources selected by their actual tracked Rust paths, plus Owner QA bridge, shared runtime app/messages/native-navigation types, panels root, DB-result root and renderer camera. Every sentinel is raw-byte identical. The exact 58-path index inventory additionally excludes all other unintended source changes.

In particular, the actual source still registers:

`EguiPostUpdateSet::ProcessOutput -> qa_debug_bridge::serve -> native_window_witness::ProviderHandoff`

The crate-visible set, prepare membership and Owner service's explicit `.before(ProviderHandoff)` remain unchanged; native annotation/provider/alternate-publisher guards are not replaced. Native helper source does not copy any earlier host files over d8.

The staged native complete files retain the approved no-Owner-probe CLI entry, exact retained worker/COM/proof ownership, durable reservation/intent, strict unknown handling, shared admission caps, paired fresh-incarnation/issuer/bootstrap reseal, and confirmed historical-stage classification. Source equality includes the documented nonblocking saved-start subset hardening observation; it is not silently altered or falsely represented as an executed exploit.

The staged layout complete files retain the separately approved shared tree reservation, outward divider geometry, compact Inspector activation lifecycle, bounded actual scene-chrome reservation, ruler placement/calibration separation, outer camera/transform and actual vision-writer ordering. The nineteen-file approved slice includes `vision_bridge.rs`; its current complete blob matches the final v3 manifest, so the v2 finding about the real FOV writer is not accidentally reverted by an earlier copy.

The actual geometry-ready -> HUD bridge -> draw ordering and outer post-update edges are the reviewed source composition. No actual Bevy schedule build or runtime timing has been performed here. Native fixed/numeric paint auditing, conservative label/capture refusal, strict pointer/camera ownership, current roles and terrain grant behavior remain as bounded by the independent source reviews and pending integrated checks.

## Eleven support paths are exact copies

The seven staged review copies have additive OKF Review frontmatter. After removing only that frontmatter/separating blank line, every working copy's body is raw-byte identical to its `.omc/research` original, and the staged review blob itself equals its working copy:

- `qa-native-nav23-security-v2-review.md`.
- `qa-native-nav23-functional-v2-review.md`.
- `qa41-residual-layout-functional-v2-review.md`.
- `qa-native-nav23-security-v3-review.md`.
- `qa-native-nav23-functional-v3-review.md`.
- `qa41-residual-layout-functional-v3-review.md`.
- `qa41-residual-layout-boundary-v3-review.md`.

The older v2 requests remain historical review evidence; the v3 reviews are the final source approvals. No review body was rewritten to imply execution.

The two staged author-report bodies are also raw-byte identical beneath their added OKF ImplementationReport frontmatter to the final isolated native/layout v3 reports. Their author-time source-only wording remains preserved; the added status records later independent source approval and pending execution. Both staged JSON hash manifests are raw-byte identical to the final isolated manifests, including original newlines. These four evidence artifacts remain protected by the existing `-text` attribute rule.

## Whitespace qualification and exact allowed scope

The **default** `git diff --cached --check` returns 2 solely for **255 CR-at-EOL reports** in the two intentionally preserved evidence JSON manifests. It reports no source whitespace defect. Read-only:

`git -c core.whitespace=cr-at-eol diff --cached --check`

returns **0**. The manifests must not be rewritten to silence a raw newline warning. This is the qualified whitespace result; a default warning-free check is not claimed.

Approve the exact **58 paths** in the hashed allowlist: 47 source paths plus those seven review copies, two author reports and two raw hash manifests. Exclude the preexisting unstaged **`conductor/.conductor_session_log`**, all unrelated current checkpoint/metadata/ledger/findings/packet updates, `.omc` artifacts and evolving worktrees. At inspection, the sole unstaged tracked change was that excluded session log. An eventual exact-body OKF copy of this final integration review is a separately permitted 59th publication path, not authority to stage a broader directory.

Root may commit the reviewed exact composition and proceed to the authorized first integrated build/parse/journey checks after its remaining independent gates. Any staged source change after this fingerprint requires corresponding re-verification. No remaining exact-integration blocker was found.
