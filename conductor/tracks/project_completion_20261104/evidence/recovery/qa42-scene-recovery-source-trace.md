---
type: SourceTrace
title: Scene publication diagnostics and truthful checked-read status plan
date: 2026-11-04
---

# QA42 scene recovery and terrain diagnostic source trace

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**Two concrete source defects warrant a narrow correction: publication loses the primary transaction error, and single-selection Waiting UI hides the existing terminal read error/retry control. The actual failed QA42 transaction guard is still unknown.** The persistent `scene_petal_mismatch` has a separate explanation: the diagnostic consumes a snapshot collected only for a resolved volume footprint, while QA42's selected/narrow/wide samples have ordinary object selection without such a footprint. This does not establish a wrong terrain assignment.

Read-only source, pinned SDK and retained evidence inspection only. No production source edit, compiler/test/implementation execution, application/process/OS/native/API/DB/network action or Git mutation occurred. Only this report was written. No credentials or complete private application log were published. Root owns QA44 and all execution; other authors' work was untouched.

## Evidence identity and source applicability

QA42 source is `925d7cbaddcdbd9996fc32d10078326cae0aaaa3`. At inspection main HEAD was `9d1e061d53d91db7680819609c8049129c708eee`; read-only comparison found no changes from QA42's source to the traced scene-publication/editor-batch/scene-analysis/Inspector/Options/diagnostic paths, and no pending changes in those paths.

The retained local stdout is 82,865 bytes, SHA-256 `aa8b7e47a41f78bd3a13c2891128f2b9ebf89d1042f6511578062fcc3537326e`, matching the frozen application-log summary. It contains 13 ordinary publication-deferred warnings: twelve during fixture seeding across the three cubes and one after the successful property addition. A separate recovery warning occurs on Verse navigation. Relevant public message excerpts are:

- At `22:08:58.935091Z`: `committed scene publication deferred: The query was not executed due to a failed transaction` for fixture Cube 1.
- At `22:11:31.516908Z`: `stored scene recovery refused or incomplete: scene recovery stopped after 0 rows enqueued; not complete: The query was not executed due to a failed transaction` for QA Terrain.
- At `22:14:04.125249Z`: the same publication-deferred message follows the property save.

The property save itself is independently confirmed by the exact API readback in the frozen packet. A post-commit publication failure is intentionally distinct from local commit failure. The retained log has no `scene current read transaction` diagnostic; the private current-read adapter must not be blamed merely because its separate diagnostic already exists.

## 1. Primary transaction error is lost in outbound publication

Concrete path:

1. `fe-database/src/lib.rs:2105–2115` handles `ReemitLocalVerseCatalog`, first running catalog repair, then `scene_publication::recover`.
2. `fe-database/src/handlers/scene_publication/publisher.rs:375–415` enumerates and prepares bounded eligible rows before emission.
3. `publisher.rs:417–423` wraps an error from the first prepared row's `emit` with the observed zero-enqueued recovery message.
4. `PreparedSceneRow::emit`, `publisher.rs:230–238`, assembles the existing fenced transaction, binds captured values and calls `query.await?.check()?` before any dependency/node enqueue.
5. Ordinary `publish_after_commit`, `publisher.rs:295–309`, uses that same prepare/emit path and only logs its propagated error.

The pinned local SDK `surrealdb-3.0.5/src/method/query.rs:454–465` implements `check()` by selecting its first indexed error. It discards the remaining response when returning that error. `take_errors()` at lines 423–436 can retain all indexed errors. A rolled-back transaction's earlier `NotExecuted` result can therefore hide the later primary failure; the repository already documents and handles this distinction in `scene_publication/current.rs:202–263` for private current reads. Outbound publication still lacks that handling.

This is the concrete diagnostic defect, not proof of a particular failed authority/node/journal/clock/asset fence. QA42's recovered error chain is insufficient to distinguish a legitimate changed precondition, a typed-value comparison defect, SQL evaluation failure or another primary transaction error. No raw DB inspection was performed. Preserve that uncertainty.

The publisher compares decoded/rebound node, journal, clock and asset snapshots in several guards (`publisher.rs:134–149`, `201–204`; `scene_publication/mod.rs:43–44`). JSON type loss is a plausible investigation direction because the pinned value projection conflates NONE/NULL and other typed values; it is **not an established QA42 cause**. In particular `schema.rs:273` declares node `created_at` as string, so a datetime assumption would be unsound. Do not replace or remove those guards based on the masked message.

## 2. Single-selection Waiting UI hides terminal error and recovery

`fe-ui/src/editor_batch/interaction.rs:141–165` returns `TransformRoute::Waiting` when the current selection has no matching complete interaction capture. That route deliberately gates ordinary single-object transforms too.

The real controller is maintained through `fe-ui/src/actions/mod.rs:1253–1255`. Its bounded read owner sends `ReadEditorBatch` at `editor_batch/mod.rs:607–620`. The database routes that through `editor_batch::read_target` (`fe-database/src/lib.rs:1804–1809`), including the real authority/current-state fence transaction in `handlers/editor_batch/capture.rs:230–238`.

On an owned terminal `RequestFailed`, `fe-ui/src/editor_batch/mod.rs:1103–1113` clears the read and retains `self.error`. A read timeout does the same at lines 558–566. Automatic pure-read allocation requires `self.error.is_none()` at lines 568–572, so a terminal failure intentionally waits for explicit refresh. This protects against uncontrolled retry and must remain.

However:

- `fe-ui/src/panels/inspector.rs:313–316` renders only “Loading the selected object's stored state before enabling transforms.” for single-selection Waiting.
- `fe-ui/src/ui_shell/right_sidebar.rs:609–610` renders only the equivalent loading text in Options.
- The shared complete-selection UI already renders the actual `state.error` and “Refresh selected objects” at `editor_batch/ui.rs:174–196`; Group controls also retain error/refresh at lines 439–449.

Thus a terminal read failure can leave single-selection Inspector/Options indefinitely displaying loading with no local error/retry affordance. This is a concrete UX/recovery defect even though QA42's packet does not expose the owned batch error needed to prove which failure caused its particular Waiting state. Do not bypass capture validation or fall back to visible ECS transforms to make the tools appear enabled.

QA42's wide UI retains the waiting label above its scrolled viewport; Brush/armed Options show it visibly. Its successful single-property addition uses its existing property path and does not establish that the checked transform capture succeeded. The outbound publication warning and Waiting state could share a lower-level guard issue, but that causal link remains unproved.

## 3. Terrain diagnostic demand differs from ordinary object selection

`fractalengine/src/qa_debug_bridge.rs:379–380` emits `stale / scene_petal_mismatch` whenever the snapshot's Petal differs from the selected object's active Petal. This check precedes the terrain assignment check at lines 417–421, so this reason does **not** identify `ActivePetalTerrain` as mismatched.

The snapshot resource is initialized empty in `fe-ui/src/plugin.rs:927`. Its collector runs after transform propagation and before report synchronization at lines 929–934. `fe-ui/src/scene_analysis.rs:284–286` publishes an empty default snapshot whenever `VolumeReportState::has_selection()` is false. `has_selection()` means a resolved report `selection_key`, not a selected Node or an open report panel (`panels/volume_report_panel.rs:271–274`). Report synchronization resolves a selected proposal/region or valid captured drawn footprint at lines 330–343, then sets that key at lines 357–363.

QA42's selected/narrow/wide state samples all precede its Circle arming, and the fixture/packet contains no resolved report footprint for those samples. The default snapshot therefore explains why those ordinary object diagnostics can persistently return the mismatch reason. No timer or extra waiting alone creates report demand. This is separate from publication recovery and does not establish a core report failure. Conversely the packet has not proved that a genuinely demanded current report snapshot works.

A small diagnostic wording correction could treat an empty/no-Petal snapshot as unavailable, while retaining `stale / scene_petal_mismatch` for a snapshot belonging to a different actual Petal. Keep all existing Owner/ViewScope/selected-marker/tree/geometry/terrain checks. Do not populate another object's data, force broad collection, invent current epochs or weaken stale refusal. If selected-object diagnostics must operate without a volume footprint, that is a separate bounded-demand design decision rather than a reason to make the collector run unconditionally.

## Minimal author plan and next evidence

1. **Add safe primary-error preservation to the existing outbound publication transaction.** Inspect all indexed errors before returning. Select a deterministic primary non-NotExecuted error, expose only fixed operation/statement stage, SDK kind, a whitelisted reason and count, with a safe unknown fallback. Use the existing private-current diagnostic policy as the pattern; avoid dumping raw SQL, bindings, rows, credential/error text or cause chains. Preserve actual SQL, authority, proof, row/point/journal/clock/asset fences and enqueue ordering exactly. Extend the existing meaningful transaction-error journey rather than adding a parallel synthetic test registration.
2. **Expose existing checked-read status/retry in both single Waiting branches through one shared UI helper.** Read the current `EditorBatchState` error/pending state and reuse its explicit `refresh()` pure-read action. Distinguish terminal refusal from active loading, preserve drafts and unknown accepted-write ownership, and keep every transform disabled until a fresh validated capture exists. If batch verification also needs primary-error diagnostics, apply the same bounded policy at `capture::verify`; do not infer its failure from outbound logs.
3. **After independent source review, root should use one fresh disposable normal fixture.** Capture the fixed outbound primary statement/stage/reason during fixture publication and Verse recovery, plus the owned selection-read outcome after selecting one cube. That is the smallest missing evidence needed to choose the actual guard fix. Confirm error/retry UI becomes truthful; one explicit refresh must send a read, never replay a write. Do not open retained campaign storage through an unreviewed startup path.
4. **Probe report demand separately.** Use an ordinary scoped selected proposal/region or valid nonempty preview footprint, then observe the normal report/scene synchronization and request the existing selected diagnostic. Merely opening the report panel or selecting a cube is insufficient. Keep missing coverage and real mismatches explicit; no terrain write is required solely to falsify an empty-snapshot diagnostic.

No production fix is approved by this trace. The next safe step is the small diagnostic/status slice above, followed by actual primary-error evidence. Do not alter scope/ownership/history authority, manufacture journal baselines, strip preconditions, promote conservative geometry or change recorded outcomes to obtain a passing screenshot.
