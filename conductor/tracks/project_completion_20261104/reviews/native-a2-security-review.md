---
type: Review
date: 2026-11-04
title: Native witness correction security source review
source_artifact_sha256: 9c6d8bf93bd083148e5618e1ae4e90e6b86a879220c1ce449fa7cf668dbec5c9
---

# A2 native witness corrections: independent security source review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for this corrected witness source slice.** No blocking security finding
remains in the four reviewed correction areas. This supplies no compiler, native
runtime, UIA delivery, helper, navigation-profile or capture acceptance.

## Exact review boundary

Reviewed seven frozen uncommitted files in `.omc/worktrees/qa-native-a2` atop
`9902f43105e922fa9097c00904976d50ac5e821f`, their author report/receipt, the main
original witness functional review and approved A2 plan. Rechecked the unchanged
A1 ownership seam, complete-output reader, serialized property book/native ABI,
real role/access source, direct painter producers and pinned Bevy/egui lifecycle
and layout contracts relevant to these changes.

All seven raw file hashes matched the author receipt exactly. Captured
`git diff --no-ext-diff --no-color --` for the receipt's seven paths has SHA256
`ccd0926bcc60cb32d439e09790e50450d24300b6f66f25b97c2fdd7cfae68adc`.
`git diff --check` returned 0; ordinary line-ending conversion warnings do not
constitute compiler or test evidence. Test-attribute counts in changed Rust files
remain 0 before/after; existing called journey functions were extended.

| File | Raw working-file SHA256 |
| --- | --- |
| `fe-ui/src/native_accessibility/AGENTS.md` | `9c5098a0d19a4099742d5f650153a69fd5495e7265c1579b29878c0dd907ee2e` |
| `fe-ui/src/native_accessibility/journey.rs` | `add50270a47676f2b6ef1dbd57b04f9b6aba3506b4c8937294a893955b269d58` |
| `fe-ui/src/native_accessibility/mod.rs` | `bf7cf30eea208565a246fa19ac720706735e1b4f4c4f771cc9d88effb9f32265` |
| `fe-ui/src/native_accessibility/surface.rs` | `af594161067fedcaf7ee779ac077128e7b0b3a319a055797bb0367c67cffaed9` |
| `fractalengine/src/native_window_witness/AGENTS.md` | `1c15f6a246173c6beb0f1961d8cb69a966accfd98017e9798ebf79f5ab90f6f9` |
| `fractalengine/src/native_window_witness/journey.rs` | `627a36d33be72e87808efc6387f83cbf2aac600d483ce0ea102b4453d34b0aea` |
| `fractalengine/src/native_window_witness/mod.rs` | `acf3ad0c5a4d14bfa9f026d13fd66787939cd6bef673f89dddc7d87c250e29f0` |

## Correction acceptance

**Completed-output authority:** SurfaceSnapshot now includes the original Guard,
and snapshot requires equality with the actual current Guard. Annotation also
requires ui::complete's retained actual output Guard and pass to equal the bound
OwnedRoot Guard and surface pass. Prepared retains that same completed Guard.
Last compares exact current lifetime and Guard, bound Guard, prepared Guard and
the same surface snapshot before the serialized property publication. Reading a
fresh annotation-time Guard can no longer bless an earlier completed output.
Changes to navigation, scene/source/hierarchy, access generation, selection,
capture eligibility or supported state cause a mismatch/refusal. Late surface
transitions or completed-pass changes also invalidate the prepared snapshot.

**Suspend/resume lifetime:** one Lifecycle resource owns the shared MessageCursor
used in First, before annotation and before Last publication. Pinned
bevy_winit-0.18.1 state.rs emits the lifecycle message before the corresponding
app update, including its final Suspended update; Windows retains the raw wrapper
where Android removes it. Only Running permits annotation/publication. Every
non-Running message retires property, preparation and binding and advances the
surface epoch; entering Running from an unavailable state retires again. Thus
unchanged Window/Context/HWND identity across Windows suspend does not reuse the
binding. A later eligible completed pass must allocate fresh checked epochs.
Transitions consumed after rendering invalidate that rendered surface rather
than being silently retagged. The shared cursor prevents separate scheduled
copies of the observer replaying old lifecycle events.

**Positive access:** product_guard derives capture_view from the actual canonical
role.can_view() and !VerseManager::scene_access_blocked(). RoleLevel::None is
excluded by the canonical role implementation; a retained older Viewer role does
not override blocked access. Both classifier eligibility and current snapshot
require the field, while Guard equality carries changes through annotation and
Last. This is positive-profile eligibility metadata, not a role grant, HTTP floor
change or replacement for ordinary action/backend authorization.

**Direct painters:** renaming layers_supported to areas_supported corrects the
prior overstatement. Areas visibility is explicitly only the registered-Area
check. Source inspection found the three documented direct layer_painter calls
across host/UI/terrain/renderer/webview: numeric/fixed-unit ruler output, fixed
unscaled chip and edited-track point glyph/ordinal labels. The ruler is explicitly
audited fixed/numeric content. The path-label renderer's initial branch returns
exactly when editing_track_id is absent or points are empty; capture_labels_clear
uses that conservative condition, independent of later projection skips. That
field is in the completed Guard and rechecked through Last, covering labels
rendered after gardener. No claim that Regions or Areas enumerate arbitrary
painting remains. New direct producers require an explicit source audit or
profile refusal before admission; this is an audited producer contract, not
general runtime paint introspection or atomic privacy isolation.

## Preserved ownership and failure boundaries

The read-only Ownership seam still uses the original unique primary-window/context
mapping, full Entity identity, actual egui Context equality and exact handler Arc
identity, including the retained detached handler. It continues to require the
existing Adapter match; no queue drain, replacement lookup, mutable adapter access
or quarantine release is added by the witness corrections.

Binding still retains the actual RawHandleWrapper through property cleanup.
Lifecycle observation, wrapper extraction/retirement, annotation and publication
carry NonSendMarker. Immediate component observers clear only cached native
ownership under the same serialized book while the retained wrapper stays owned
until main-thread retirement. Existing checked process-static epochs survive
ordinary reset; exhaustion/foreign property/native failure poison future
publication. The current-process cross-thread property-cleanup argument remains
the documented qualified inference, not a newly established universal Win32
thread-safety guarantee. Terminal, capture-lock, missing/ambiguous ownership,
unsupported surface, stale pass and failed property checks remain unavailable.

No new external endpoint, credential, role value, user content, UIA action or
native input is introduced. Root metadata remains bounded static-grammar
identity/classification counters on the actual completed normal tree, and the
ordinary forwarding path remains the only publisher. A successful property set
still does not prove the normal provider consumed that tree. The host <=250ms
uptime bound is publication-age checking only; it does not prove WGC pixel
freshness or safe historical capture.

## Authored and unverified acceptance

The existing UI journey authors actual completed Context/AccessKit output then
changes real navigation and primary selection, checks the old output Guard stays
old, and authors No-access and edited-label refusals. The blocked-access
classifier case is a pure eligibility check. The pure App journey sends actual
lifecycle messages while preserving a Context entity and checks retirement,
nonresetting allocation and absence of fabricated preparation. These assertions
were not executed. They do not establish successful real HWND/root
annotation/publication, Windows suspend behavior, UIA delivery, actual late-state
FFI refusal or live peer Inspector/rendering acceptance.

The initial profile remains empty Properties in an active Petal under declared
nonsecret fixtures and coordinated operators. No-active-Petal navigation,
selected-object/asset Inspector auditing, retained original COM operator helper,
one-attempt invocation ledger and exact-window capture remain separate mandatory
work. WGC remains unavailable until its freshness predicate is established.
QPC/TimeSpan units alone cannot supply that predicate.

Parent integration must preserve the exact reviewed contributions alongside current
main changes, then compile and execute the required native/live acceptance.
Functional review is a separate lane. This security approval does not close the
overall native QA milestone or any campaign coverage gate.

Only read-only source, pinned SDK, Git diff/status/hash and retained documentation
inspection occurred before writing this artifact. No compiler, Cargo, tests,
apps, OS/process/input operations, UIA/WGC, API/DB/store access, network, Git
mutation or production-source edit was performed.
