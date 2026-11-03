# Independent QA32 packet and pointer integration review

Reviewer: `/root/selection_corrections_review`, 2026-11-03.

**Approved the corrected packet's bounded executed findings**, against final
manifest SHA256
`e5c2bfc52c0ed59d57119f916ea6818186aac25088ce1401013d3432794bdf36`.
The initial review candidate was withheld pending the log-decoding correction
described below. Original application captures were not rewritten. The current
manifest explicitly retains both prior **unreviewed** candidate hashes and their
revision reasons: e4dd0979 before the independent native comparison, and 107451aa
before the BOM-aware log correction. Neither is falsely labeled approved.

This is an independent saved-evidence review, not a full QA pass, new app
execution, or release approval. Only this separate review was written. No target
scripts/builders, app/API/DB/live-process operations, input, Cargo/tests or network
were executed; packet, canonical records and production source were not edited.
No token files or raw secret values were accessed/published. Other agents' work
was preserved.

## Integrity and provenance

All 154 listed payloads match their lengths and SHA256 hashes, with unique names,
no missing/extra files and 155 total files including the manifest. All packet JSON
and JSONL records parse. The 121 original root files remain covered by the frozen
manifest; the additional native-transform report is a derived comparison. Six
PNG copies are byte-identical to their recorded source images and hashes, with
1280x720 dimensions. All six were visually inspected.

The context and verdict agree on exact Owner PID 93096, ticks
639293021548251006, executable in stage e5e6987d-20261103042513606. Its actual
sealed executable hash matches the recorded manifest. The saved previous-start
receipt matches QA31's exact lifetime; retained root, no reseed, unchanged port
and no stale-stage override agree across records. The stage records
source_dirty=true rather than a clean-checkout claim. Saved logs support 304.9
seconds for the stage and 14.9 seconds for launch. The end receipt matches the
exact root plus seven recorded descendants and marks all eight absent; this
does not assert a fresh or machine-wide process census.

## Four formerly failing current reads

Each exact affected node is separately observed as the actual native primary:
QA Portal 18, QA Scoped Creation 21, QA Initial TRS 21 and QA Asset Placement TRS
21. Each corresponding Inspector UI contains `Verified local bytes are available`,
and each state reports ui_known=0 and settled=true under its limited definition.
The native observer is an actual primary ECS sample, not an all-selection census
or scene-child/GPU witness.

Independently calculated expected transforms from the saved complete rows:
stored Point x/z plus elevation y, stored scale, and intrinsic XYZ quaternion
from the stored Euler radians. Hamilton quaternion composition/sign-equivalent
comparison produces exact positions/scales; the maximum quaternion component
error is **7.637217924738593e-8**, below 1e-6. This independently substantiates the
new comparison report instead of accepting its Boolean results alone.

All 17 saved state samples report ui_known=0 and settled=true. DB and sync
completion remain unknown; neither this nor the local-byte label establishes
terrain readiness, peer transfer, asset adoption or GPU rendering. Final stopped
stdout contains none of the named private-current transaction/read-deferred/
asset_metadata_changed error markers. Together with successful individual current
observations, this is focused runtime evidence for the four formerly failing
reads, not an absence-of-log argument alone.

## Restart responses and unresolved history advance

Independently compared all 20 complete QA31-to-QA32 row/history responses for the
ten retained objects. They are byte-identical across restart. All 21 copied origin
responses (those twenty plus the Group registry) match their actual QA31 sources.
The final Group registry remains byte-identical to the retained original.

The final interaction comparison has exactly **19 of 20** responses identical.
All ten complete object rows remain byte-identical. The sole difference is the
raise node `01M6D1HAXR53SVHRX1360QZHWC` history's reported current_version,
**102 to 106**. Every other field in that saved response, including its first
100 entries through version 100, is unchanged.

The additional page is not captured. Versions 101/102 were already beyond the
saved page, and the newly reported 103-106 entries are also unread. Their contents
and attribution remain unresolved. No explicit scene-mutation action appears in
the recorded command inventory, but that does not rule out an unintended input
side effect or another writer. The packet must not be used as proof that all
history stayed unchanged, that no mutation occurred, or that the extra versions
are harmless. Its existing limitation correctly requires the next page read.

## Selection, right panels and tools

Nine saved UI presentations independently agree on accessible selected rows,
count and human primary labels:

- The initial Cube 1/2 pair is shown in Inspector, Options and Move, primary Cube 2.
- Ctrl adds Cube 3, yielding three selected with primary Cube 3. Removing that
  primary falls back to Cube 2 with the original pair retained.
- A plain anchor on Cube 1 followed by Shift through Cube 3 includes the grouped
  and ungrouped rows, yielding all three with Cube 3 primary.
- Ctrl removing Cube 2 yields Cube 1/3, primary Cube 3. The original Group is no
  longer wholly selected; the screenshot shows its partial cue and Ungrouped 1/8.
- Rotate and Scale retain that Cube 1/3 pair, count two and primary Cube 3.

Native primary IDs and active-tool state agree with the exposed primary/tool
labels. Current-value readouts agree with the original persisted rows: pair 1/2
has mixed X/Z and common zero Y; pair 1/3 has mixed X and common zero Y/Z, common
zero rotation and unit scale. The screenshots show these cues in both right
panels. This checks selection and tool mode/scope presentation, not a numeric
transform or gimbal drag in this pass. No all-member native census is inferred
from accessibility selected flags.

Navigation up clears selection and the Petal cursor; the UI shows Nothing
selected. The observed return restores Petal A with no selection. The retained
path is selected/read only, with no closed-path transform, mesh-source or GPU
acceptance.

## Driver observations and timeout

The offscreen named locator refusal has no dispatched commands and unchanged log
watermark. The invalid `ui-state` CLI invocation is explicitly a driver argument
error with no application input, not a product startup/action failure.

One return-to-Petal-A input returns HTTP 504. A following read observes Petal A
applied; no replay is recorded. The sole structured ERROR in stopped stdout is
the associated tower_http trace classification, status 504, latency 5011 ms.
The timeout is not recast as definitely unaccepted input or a successful response.

Saved tree contents/order remain equal while node row Y positions shift uniformly
after a synthetic click, consistent with a changed hierarchy scroll offset. The
same-frame pointer timing explanation is source-inferred, not causally proven by
this packet. The later pointer correction has not been executed in QA32, so this
review does not close that issue.

## Corrected logs, scans and inherited-packet audit

Independent review found that the first candidate's builder decoded UTF-16 launch
and stop logs as UTF-8 with replacement. That made their reported line counts
17 and 5 instead of 8 and 2 and made their finite-pattern interpretation invalid.
The raw log hashes/bytes were intact. Approval was withheld for this derivative
issue; the root corrected only derived reports/manifest and the builder.

The corrected decoder uses strict UTF-16 BOM or UTF-8 BOM/UTF-8 decoding, with no
replacement path and an explicit NUL guard. Per-log encoding and non-lossy status
are published. Independently recomputed results:

| Log | Encoding | Bytes | Decoded lines |
| --- | --- | --- | --- |
| stdout | UTF-8 | 97718 | 715 |
| stderr | UTF-8 | 78 | 1 |
| stage | UTF-8 | 1368 | 14 |
| launch | UTF-16 BOM | 1658 | 8 |
| stop | UTF-16 BOM | 360 | 2 |

All raw log lengths/hashes match. Final stdout has **631 INFO, 16 WARN, 1 ERROR,
zero panic markers**. Its ERROR is the known timeout above; this is not a clean-log
claim. The initial 57265-byte live-log cut is independently verified as an exact
prefix of the final log, not a substituted final absence check.

Recomputed the four configured credential regexes on all 121 textual original
root artifacts and all five **correctly decoded** logs; counts remain zero. No raw
values were emitted. Finite patterns do not establish exhaustive secret absence.

As requested, also audited every referenced QA30 and QA31 log for this inherited
seam. All five QA30 and seven QA31 logs are strict UTF-8 without BOM; their decoded
line counts and finite-pattern counts match their approved summaries. **Neither
QA30 nor QA31 needs a derivative correction for this encoding issue.** Their
earlier review claims are not silently expanded by this audit.

## Storage and publication limits

Effective QA32 root/nested evidence attributes are text=unset under the scoped
-text rule. QA32 had no index entries during this review. As with prior packets,
raw staged-blob sizes/hashes must be checked after staging and before publication;
this is working-packet integrity approval, not an unstaged Git-blob claim.

No substantive correction remains in the corrected candidate within this scope.
Publication metadata can reference this review and retain the final reviewed
manifest anchor, with a regenerated manifest if metadata changes. Full QA/UX,
eligible adoption, recipient/native peer delivery/rendering, ordinary native
accessibility actions, closed-source execution, final checks, CI and releases
remain unaccepted.

## Separate pointer integration preservation verdict

**Approved integration preservation only** for main
`7b3a6dd7d14a11472100163e6d283a8156cd2ea5` versus source-approved
`e600c77db65cb15e5960a3534755b13468493be4`.

The integration changes only `fractalengine/src/input_bridge.rs` and its directory
`AGENTS.md`. Both resulting blobs are byte-identical to the source-approved commit.
Stable patch IDs match exactly:
`7cc0a2d5acd9e07627fff4c60619418bfa53b6fd`. No additional contribution paths appear.
This preserves the separately reviewed source; it is not a new semantic/security
review, build claim, or runtime acceptance of the pointer correction.
