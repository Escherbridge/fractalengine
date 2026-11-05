---
type: Review
title: QA49 focused evidence review
date: 2026-11-04
status: approved_at_recorded_scope
---

# QA49: independent full focused evidence review

**APPROVE the bounded numeric-replay result plus preserved S2 camera failure;
no integrity blocker or material erratum.** Reviewer `/root/legacy_helper_io_fix`,
2026-11-04, did not author the packet. Only this report is written.

Manifest SHA-256 `e65926480471830c92d98a9302bcc1c228c2871b873238a02d30f3db6ac323d1`
and all46 payload byte counts/hashes match. Original-artifact anchors match.
Seven copied context/cleanup/launch/stop/stage-log/receipt/stage-manifest originals
match exactly. All five published PNGs match original run-local screenshot bytes
and their response sizes; **all five actual1280x720 images were viewed**.

The raw command sequence selects Cube1 in the tree, focuses Position X once,
Ctrl+A then text_entry2.500 without refocusing, scrolls Inspector and clicks Apply
once. Complete draft and scrolled AccessKit textinputs both contain exactly2.500.
One Ctrl+Z and one Ctrl+Shift+Z follow. Input receipts report expected command
counts/success. Initial cross-Verse navigation explicitly reports expected old-
token remainder refusal and a log-generation gap; no atomic/gapless batch credit.

| Read | Stored X | Object version | Last operation |
| --- | ---: | ---: | --- |
| Before | 0 | 4 | property_set |
| Apply | 2.5 | 5 | full_transform |
| Undo | 0 | 6 | full_transform |
| Redo | 2.5 | 7 | full_transform |

Independently compared complete raw node/history responses: original entries1..4
remain identical prefixes, history has4/5/6/7 entries, current_state_verified=true
and no next page. Edit sequence advances0/1/2/3. Y/Z, rotation, scale, properties
and every other non-position/non-edit_seq node field remain unchanged. Each new
entry's after-state position matches the stored node. Saved/later selected state
retains X2.5 and no known pending UI operation; its settled definition explicitly
does not certify DB/sync/terrain completion. This verifies one object's stored
transform/history, not inverse-link receipts, native mesh picking/projection,
group atomicity, conflicts, peer convergence or full rich-data simulation.

Visual findings agree with the limited verdict:

- 164635 draft: selected Cube1, visible X2.500, cubes/grid/blue plane visible.
- 164717 applied and164859 later: visible X2.500 and saved-state feedback; world
  framing matches, while object placement changes. Asset panel requests refresh.
- 165103 Undo: X0.000 with world framing shifted.
- 165118 Redo: X2.500 but viewport almost empty, only a red fragment at the top.

Raw draft/after/later pose substrings are independently byte-identical, hash
`6eb33c4228c611b8d7584c4f845a483588af61a0f74f6c4a348f5febb8d20bfc`.
Undo/Redo focusY becomes0 -> -0.7408829927444458 -> -2.2526397705078125;
camera positionY correspondingly becomes2.2358551 ->1.4949721 ->-0.0167847,
while yaw/pitch/distance/FOV stay fixed. **QA-0073 is a real S2 camera ownership
failure**, not an invalid-numeric failure or proof of lost mesh residency. The
later camera source correction is absent from this stage; no repaired acceptance.

Stage71cd5f8e-20261104164443718 records commit71cd5f8e9f05bd39dbb1a61c17a95ca054dda5fe,
dirty=true and source-match-at-launch=true. Actual staged app/helper hashes match
their sealed manifests. The exact original receipt/log record a15.4s production
build/terminal0, not tests. Local raw recovery examined5/enqueued5 is present at
the probe's exact source-log hash; enqueue/warning observations certify no peer
delivery. Displayed asset audit/provenance requires refresh and is not reverified.

All eight recorded root/descendant identities exactly match cleanup, absent at
2026-11-04T23:56:13.9606743Z; no global orphan claim. Strict-decoded40 covered text
payloads plus2 private logs independently reproduce zero matches for four finite
credential patterns. PNGs were visually inspected, not text-scanned; scan/manifest
metadata were written later. Universal secret absence remains false. Private
stdout84461/hash a40d52e3bac31c1d8f0c046a1fdeec7b6cf253b72ef8fd29e7079acb479d00e5,
INFO556/WARN9/ERROR0, panic/current-error0 match; stderr empty.

No native capture freshness, UIA/Invoke, lifecycle/restart, FULL, final integrated
sweep, CI/tag/release credit. Packet/raw originals unchanged; no implementation/
parser/compiler/test execution, app/OS/native/API/DB/network action or Git mutation.
