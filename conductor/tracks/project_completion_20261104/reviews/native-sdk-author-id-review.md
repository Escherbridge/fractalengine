---
type: Review
date: 2026-11-04
title: AccessKit Windows AutomationId mapping compile correction review
source_artifact_sha256: 5c6249262e806f407ca7b0059b000c18274d9c46e130f2063f79e48bce6efbb7
---

# Native AccessKit AuthorId compile correction: independent review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the three exact method-token replacements.** Pinned SDK source
establishes that AccessKit AuthorId is the field exposed as Windows UIA
AutomationId. This is a source/API correction, not successful compilation or
native-provider delivery evidence.

## Exact frozen correction

Reviewed the two dirty files in `.omc/worktrees/qa-native-a2`, baseline
`5a51daa1e3fe4f5abf5e1f6bb060b353f29966cf`, plus the writer report/hash receipt.
Raw-byte comparison against that committed baseline proves the working files are
exactly the result of replacing `set_automation_id` with `set_author_id` at all
three existing calls, with no other byte changed:

| File | Calls | Corrected raw SHA256 |
| --- | ---: | --- |
| `fe-ui/src/native_accessibility/surface.rs` | 2 | `e92d4349720d1a754326e24bf42d45f3accf44e07c82b506348c17af62ca5c6b` |
| `fractalengine/src/native_window_witness/mod.rs` | 1 | `0d94e2a3ec2f3a6f34e6e42ffabf6736b715ae574989f45741b862141ab2b974` |

Correction diff SHA256 (`git diff --no-ext-diff --no-color`):
`3606acbe9e92df8858ef312f0ab0dbfb5802784f6e8b0dbec9cdf2bb50b81c21`.
`git diff --check` returned 0. No test declaration, journey, dependency or directory
document changed. Targeted Rust source search in native UI/host/tools found no
remaining invalid setter or `automation_id()` getter assertion needing a rename.

## Independent primary implementation evidence

Inspected installed registry source under
`C:/Users/atooz/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`:

- `accesskit-0.21.1/src/lib.rs:1753-1755` declares the author-defined automation
  identifier and generates `author_id`, `set_author_id` and `clear_author_id`.
- `accesskit_windows-0.29.2/src/node.rs:308-309` implements its internal
  automation_id getter as the underlying node's author_id.
- That backend's `node.rs:972` maps UIA_AutomationIdPropertyId to automation_id.
  The properties macro at `:838-848` dispatches the property ID to that getter,
  and actual COM GetPropertyValue at `:689-692` calls the dispatch for the resolved
  current node.
- UIA_FullDescriptionPropertyId still maps to description at `:964`; the existing
  set_description(status) call is byte-preserved.

This establishes the intended Windows property mapping independently of any
compiler suggestion about similarly named methods.

## Preserved behavior and acceptance

The marker values `fe.surface.other.v1`, `fe.surface.credential.v1` and
`fe.native.window.v1` are unchanged. Root identity, FEW1 description, completed
Guard/pass/lifecycle/source/role gates, retained HWND/handler ownership,
classification, forwarding and Last property publication are byte-preserved.
No helper, action, capture or permission path is added.

Parent should integrate only the three exact token replacements into main,
preserving its existing line endings and unrelated integrated work, then confirm
the integrated byte diff and retry the production build. This review does not
waive normal accessibility delivery, annotation/publication, lifecycle or peer
acceptance. The retained failed build remains failed evidence until an actual
retry succeeds.

Only read-only source/pinned SDK/Git diff/hash inspection and this review artifact
were performed. No compiler, Cargo, formatter, tests, app, OS/process/input,
API/DB/store/network operation, production-source edit or Git mutation occurred.
