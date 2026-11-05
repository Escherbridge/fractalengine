---
type: ImplementationReport
title: QA57 rounded margin and font height correction
date: 2026-11-04
---

# QA57 control height v2 — HGT-B1 correction

Frozen isolated candidate for independent rereview. Canonical production remains
exact before bytes; v1 raw/manifest/patch/report are preserved. Only this v2
directory was written. No native candidate was changed. Baseline identity remains
`6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`, qualified by exact before hashes.

HGT-B1 correctly identified v1's raw2*padding calculation as insufficient.
Pinned egui0.33.3 Button passes Vec2 padding into Frame::inner_margin
(`widgets/button.rs:285–300`, `containers/frame.rs:252–254`). Pinned epaint0.33.3
Margin::from(Vec2) rounds each side (`src/margin.rs:115–118`); its sum at:81–82
adds converted top/bottom. Thus1.5 becomes2+2=4, rather than raw3.

V2 uses **egui::Margin::from(active_padding).sum().y** in the allocation height.
The separate target/interact minimum, shared confirmation helper/state, fixed
cell/compact action routing, callbacks, IDs, drafts, permission/pending gates and
viewport/Inspector floors retain v1's behavior.

Font-path inspection also found that direct TextStyle::resolve bypasses ordinary
WidgetText overrides. Button::new uses Button fallback (`button.rs:44–48`);
AtomKind::into_sized invokes WidgetText::into_galley (`atomics/atom_kind.rs:86`).
Plain text resolves FontSelection::Default with that fallback
(`widget_text.rs:753–754`, `style.rs:157–168`). Explicit Small RichText uses
override_font_id first, otherwise its explicit Small style
(`widget_text.rs:406–418`). V2 measures exactly those resolved Button/Small fonts,
plus the Default-resolved Body path. Existing button width measurement reuses
the same resolved Button font. It does not use RichText::font_height, whose
different precedence would miss this actual layout path.

Repo source search found no production override_font_id/override_text_style
assignments in fe-ui/src or fractalengine/src; the app's current text-style-map
path stays intact. No theme framework, settings surface or global confirmation
width-policy change is added, and arbitrary themed confirmation behavior is not
claimed as newly verified.

The existing journey now retains its20-point Button/4.0-padding profile and adds
a dominant24-point Button/1.5-padding profile. Both run complete normal/armed
rect/hit/child/table intersections at52, pair±1,260/640 widths, zero horizontal
offset and real arm/cancel with no writes. Original style is restored. No new
test declaration/registration; these assertions remain unexecuted.

Manifest **4017 bytes**, SHA256
`9b2755dccf8889582469ff681e0aff1451924fef9dec1fb572eb438ee6b3a5bf`.
Full source.patch **8026 bytes**, SHA256
`309835233eac7bb8edb357d309618f6d17b47059d022fd510395bb6ac769915d`.
v1-to-v2.patch **7513 bytes**, SHA256
`164e61018047743c4cafabe8930b4a65a8654c8b3a7a9bf3776766eca01c21be`.

| Raw candidate suffix | Bytes | SHA256 | Full added/removed |
| --- | ---: | --- | ---: |
| property_grid.rs | 8043 | 05a6d570675ea0ee3f8ab0de3a5c175bb803bc1dc114d6c81204edc2b2b29667 | 15/4 |
| inspector.rs | 285662 | b86166aa2c4a8d22d814b5d0e50d11e22734e222541271452db26b3878f613f8 | 38/0 |
| AGENTS.md | 73300 | 4ebb8a5d1c1f0642c16cf5716d44db4d32c73770d2bfdfb1ca6428a64f5c9967 | 20/0 |

Manifest pins canonical before/raw and v1 raw hashes plus both independent v1
reviews. Filesystem/text/hash checks found production mismatches0 and frozen-v1
anchor mismatches0. No implementation parser, tests, formatter, compiler/build,
apps/OS/native/API/DB/network or Git mutation. Independent source reviews, root
integration/build and matching live whole-target checks remain required; no
live-fix/FULL credit is claimed.
