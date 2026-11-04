# Checked editor batches

`capture.rs` resolves one exact stored Petal ancestry and the actual DB-thread identity at Viewer
or Editor. It rejects missing, tombstoned, duplicate or foreign targets, preserves requested order,
and captures full raw node/point/history fences plus the exact server-encoded Petal row. Promoted
stamp identity and owning path snapshots are included. Reads execute all captured fences before
returning. Captures are expected-state data, never an authority grant. The serialized DB writer
remains mandatory. Bounds are 256 targets and 4 MiB of expanded capture/receipt/facts/bindings. Commit rechecks the
fully assembled manifest, SQL and bindings after fact IDs, record times, save tags and final markers
exist. Maximum-width signed/unsigned HLC values and a conservative clock-string envelope reserve
the only fields populated by the atomic seam; replacing them with actual clocks cannot increase size.

`prepare.rs` compares the submitted capture to a fresh authorized one before preparing any effects.
Targets must have exactly one typed edit per ordered captured member. Ordinary transforms reject
paths/stamps. Path affine math is shared with native preflight through runtime's EditorAffine;
timestamps, corner/smoothness and opaque trailing fields remain exact, while relative handles use
only the linear rotation/scale. PathRestore admits the same topology and coordinate slots only.
Bulk properties are capped at 32 distinct keys per object, validate every target's current schema
even when a field is unchanged, and refuse derived identity/path/stamp/view keys.

Stamp preparation consumes the existing sealed stamp guards/writes/facts into the containing
transaction. Its bindings are namespaced as complete SQL variable tokens; guards precede all writes.
Scale/rotation use override properties, never stored node TRS. StampRestore reuses those same sealed
source/role/schema guards without executing provisional override writes, then writes/deletes only
the three named override keys. It preserves original JSON values, explicit null and key absence;
array shape, finite bounds and quaternion normalization are validated, and restoring inherited arc
cannot move the origin. Exact inverse edits do not bypass the reserved generic-property restriction. Arc must equal the captured stored
override or shared-sampler default: group transforms cannot move a stamp's derived origin. A stamp
and its independently transformed owning path are a conflicting dependency and refuse as a whole.

Groups remain nonowning, many-to-many Petal collections. CreateGroup validates a fresh ULID, bounded
registry/name, and absence of retained references before atomically adding the registry and every
captured membership. Legacy IDs are preserved. Legacy API registry writes retain their caller-role admission and also
seal the actual local publisher's Editor authority/ancestry at transaction use, even for Identified callers. New registry IDs cannot resurrect references held by
any live or tombstoned node; existing legacy/API registry writers share that transactional fence.
DeleteGroup is the inverse for a fully captured group: an uncaptured stored reference refuses the
whole delete. UndoCreatedGroup removes that registry entry and restores the captured per-member membership
representations in the same transaction, preserving preexisting null and explicit empty arrays.
No object is deleted. Remaining display-order values stay unchanged; creation appends
at max(order)+1. Normal membership removal deletes an empty key. RestoreMemberships preserves exact
Absent/Null/Groups representations, including explicit empty arrays, for truthful inverse edits.

`mod.rs` appends one EditorTransaction manifest, all materializations and immutable per-object facts
in one existing atomic operation transaction. Post-write checks verify every node and changed
registry against the complete receipt before commit. A failing guard/fact/check rolls everything
back, including the intention. Generic facts carry atomic_save_id; each object's final nonbaseline
fact alone carries matching atomic_save_request_id and editor_request_id. HLC does not group saves.
The complete receipt is retained in the immutable operation payload, including unchanged members.
Resolve reads that exact manifest and independently guarded current rows under the original scope,
actor/request and exact ordered target set. It never guesses commitment from current coordinates.

Dispatch emits one EditorBatchCommitted/Resolved GUI result, never single-member materialization
replies. The UI validates the whole receipt before applying manager projections or recording one
Undo action. Inverse commands recapture current context and compare every expected version/state.
Ordinary per-object journal versions remain separate from global or measurement/event-time cuts.

CommitCheckedEditorBatch and ComparePetalVisibilityGroups remain UnmappedDeferred in the canonical
inventory. Neither the local manifest nor legacy registry/property notifications establish an atomic
P2P group wire contract; that requires separately reviewed mapping, transport and paired convergence.

## Current manifest ownership

The atomic seam creates its op-log intention before caller guards. Batch guards therefore require exactly one current request manifest, and compare that entire stored payload to the internally assembled actor/scope/receipt payload before materializations or facts. Zero is not the success count. A preexisting request ID produces a second intention and refuses the whole transaction, rolling back the new intention as well. The additional payload binding participates in the final expanded 4 MiB budget. Op-log serialization strips only top-level nulls; nested exact receipt nulls remain intact.

## Server-derived Group targets and exact deletion inverses

`groups.rs` authorizes the actual identity and Petal ancestry before expanding a Group. It queries every stored reference in stable node-ID order with a 257-row overflow probe, caps the complete set at 256, and fences that complete set again inside the transaction. Hidden live members participate. Any tombstoned reference causes a whole-operation refusal with its protected-reference count; no deleted object is silently omitted, mutated or resurrected. Group captures with zero targets still carry the full Petal/registry/permission fences and produce one real immutable receipt.

DeleteGroup accepts only the matching Group target. Its atomic manifest retains the exact before capture. RestoreDeletedGroup accepts only the original delete_request_id and rederives the row position, ID/name/visibility/order and per-member Absent/Null/Groups representations from that same actor/scope/Petal durable deletion. Client-supplied inverse context must equal the freshly derived context. The full original manifest is fenced unchanged at use; the current registry must equal the delete receipt registry, and any collision or retained live/tombstoned membership rejects the restoration. All current rows, history versions, schemas and Petal authority are fenced before writes. Registry insertion restores the original vector order and never deletes an object.

A successful restore stores group_restore_of in its own atomic manifest. Consumption guards exclude only that exact current restore request (the log-first seam has already inserted it), rejecting any prior restore of the same proof. Read checks use no exclusion. An empty Group therefore cannot be restored repeatedly even if an outside writer resets the registry to the prior empty state. Redo is a fresh checked delete with a fresh inverse proof.

Pure Group reads and outcome resolutions remain bounded at 4 MiB across returned snapshots and expanded fences/bindings; writes retain the final manifest/fact/SQL/binding bound. Legacy Nodes reads still require nonempty IDs. Existing database/runtime journeys were extended for whole-member races, protected tombstones, exact legacy row/order restoration, forged actor/context, consumed proof, empty Groups and zero-node Petals. They have not been executed by the source-author lane. This remains a local atomic editor contract, not an atomic P2P Group wire claim.

Shared registry admission also fences historical checked-delete IDs, so ordinary CreateGroup and legacy registry/CAS writers cannot sidestep the proof route once retained references were cleaned. Only the typed proven restore uses the separately named retained-reference fence. Historical checked-created IDs also require the typed creation Redo proof; generic creation and registry writers cannot reuse them after Undo. Older untracked deletions remain outside this journal contract.

## Proven creation Undo and Redo

creation.rs owns the immutable same-actor/scope/Petal lifecycle. Every successful CreateGroup or RedoCreatedGroup stores group_create_before alongside its exact receipt. UndoCreatedGroup names that creation request and must restore its original exact memberships under the matching registry vector; its manifest stores group_undo_of and group_undo_before. Each creation proof can be undone once. Undo may follow a checked Delete and its exact restoration: those owned inverses legitimately advance current counters, while fresh full current captures and schemas remain fenced.

RedoCreatedGroup carries only undo_request_id. The writer loads both original creation and matching Undo, validates their ordered complete target IDs, exact registry vectors and membership representations, and derives the restored ID/name/visibility/order from those manifests. Current registry and memberships must still equal the proven undone lifecycle. Current nonmembership fields and versions come from the fresh full capture and are fenced, rather than being overwritten by historical object snapshots. The restored Group must have no retained live/tombstoned references anywhere. Full original payloads remain exact transaction guards. The new manifest records group_redo_of, and one Undo proof cannot authorize multiple Redo commits. A successful Redo creates a new creation proof for the next Undo.

The generic historical-ID guard still refuses checked-deleted IDs and now also checked-created IDs. Its preflight read excludes no intention; only the transaction guard excludes the exact current request, after the first exact-own-manifest guard proves it. Legacy registry/CAS writers receive no exclusion. Typed Redo and proven deletion restoration use the explicitly named retained-reference fence instead of weakening generic admission. Old unproven creation receipts refuse with an unavailable-proof error.

Expanded proof payloads participate in the existing final 4 MiB bound. Existing database and Inspector journeys add Create/Delete/UndoDelete/UndoCreate/RedoCreate/RedoDelete, repeated fresh proofs, forged/consumed/cross-actor proofs, competing prepared requests and current-source races. These additions are source-authored and unexecuted in this lane.

## Complete lifecycle reservation

Normal CreateGroup and each fresh DeleteGroup reserve their unchanged-context Undo/Redo storage envelope before the atomic seam inserts any intention. Proven inverse/creation Redo operations retain their actual 4 MiB envelope check, without recursively reserving their proof history. The ceiling remains 4 MiB. compile is the single production assembler for manifest/self-payload binding, tagged facts, SQL and bindings; the existing boundary journey calls its nonpersisting admission adapter and then the real commit path.

The reservation serializes named components rather than multiplying a capture by an unexplained factor:

| Component | Conservative source and coverage |
| --- | --- |
| Stable authority/schema basis | Only unchanged entity-authority and membership-schema namespaces enter the reservation. Mutable current entry, request, member history, writes and fact envelopes are excluded and replaced by the canonical maximum templates below. The actual current complete plan still passes its independent 4 MiB check before reservation. |
| Two future source proofs | Each uses a maximal manifest built by the production manifest serializer. All three supported before-capture slots are populated together, even though a real operation populates at most one, plus the complete receipt and every proof marker. This bounds creation plus Undo proofs, and the smaller single deletion proof. |
| Future own intention and self binding | The same maximal payload is serialized once in a maximum-clock OpLogEntry and once in its exact manifest-payload binding. |
| Future captured objects and paths | The production capture fence serializes full raw state and a conservative full journal envelope for every target and promoted owning-path context. Each node chooses the larger before/after membership snapshot; unrelated properties remain complete. Original raw extra columns remain in normalized full row fences. Opaque journals of owning paths outside the mutated member set remain unchanged and are retained separately; selected members and selected owning paths use canonical future journal templates. |
| Future writes, checks and facts | Production write/confirmation helpers supply complete property/TRS bindings and SQL. A per-target fact includes both possible baseline state slots, largest membership data, current schema versions and all final receipt tags. One such envelope also occupies each latest-history fence. Future unchanged-context mutations are continuous single facts; any first-mutation baseline already exists in the current plan. |
| Registry and proof SQL | Both registry variants are retained together. The actual proof/consumption, complete-membership and retained-reference query builders supply future SQL; maximum-width target/request/actor/scope bindings are explicit. |

Every future request string reserves 128 bytes of control characters, which JSON escapes to the maximum six output bytes per input byte. Node IDs, actor, scope and property bags remain unchanged. Positive versions use i64::MAX; stored edit-sequence and fact cursor/HLC envelopes use the longer signed width. ULIDs reserve 26 bytes and record timestamps reserve the longer Chrono extreme RFC3339 length. Initial receipts must retain two positive journal-version slots for one Undo and one Redo.

The opaque Petal snapshot is never decoded or trusted as client authority. The same DB read also computes the typed registry-omitted encoding length using object::remove($this, 'visibility_groups'); this internal Captured/PreparedBatch size basis never crosses the runtime DTO or grants authority. Possible full-snapshot growth is bounded by appending the full replacement registry's CBOR maximum and maximum enclosing map-header growth to that canonical registry-omitted base: nine-byte container/string heads, UTF-8 string bytes, a one-byte bool and nine-byte float per row, then base64 four-byte blocks plus one alignment block. The registry model retains both variants, and the placeholder remains ASCII like the server's base64. Existing data are preserved byte-for-byte in every actual guard.

Original and proposed nonabsent membership values are also validated against the captured unchanged schema snapshot, so saving a Group cannot newly promise an inverse that its current schema already forbids. Later external role/schema/source changes remain legitimate conflict boundaries. Size reservations do not grant authority, replace any proof, inflate limits or bypass actual final-envelope checks.

The existing database journey searches both sides of the lifecycle boundary using otherwise-valid nested property bags and real DB preparation, confirms rejection leaves no manifest/member prefix, then actually commits the last admitted size through Create/Undo/Redo/Delete/Restore/RedoDelete. No new test declaration is added; source-authoring has not executed the journey.

The installed SurrealDB 3.0.5 encoding path was inspected read-only: fnc/encoding.rs converts to public values and rpc/format/cbor/convert.rs maps the registry's float, bool, text, arrays and objects directly to CBOR primitives. Decimal/date/record tags are not introduced by writing the typed Group DTO. This verifies the registry-only growth bound; the rest of the opaque typed Petal encoding is retained unchanged.

The normalized reservation is a fixed point for unchanged-context Delete/Restore/RedoDelete after typed cleanup, and the full opaque-aware reservation is nonincreasing: current request IDs, UUID/timestamps, latest changed-member journal rows and counters are never added to it. Namespace-selected authority/schema guards and bindings remain identical. Every changed-member/root/path edit sequence and history cursor has its shared maximum width before row/fact serialization. Registry-omitted Petal encoding avoids accumulating growth from its current registry or a legacy integer-to-float rewrite. All future proof, own-manifest, raw capture, write, check and fact shapes use the same canonical data each cycle. Each actual operation independently retains its complete current-envelope check.

The existing boundary journey now binary-searches to the last admitted nested-bag byte, checks its reservation lies within one small model stride of 4 MiB, and asserts identical canonical byte counts through repeated Delete/Restore cycles. The first Delete uses request a; later requests contain 128 control bytes requiring maximum JSON escaping. Real owned journal facts cross a decimal version-width boundary, and all five Deletes/four Restores must commit. This is source-authored coverage, not execution evidence.

Flexible registry rows may contain unknown fields or legacy typed numeric representations absent from VisibilityGroupDto. Every maximal proof capture therefore retains the greater of the ACTUAL original encoded petal_snapshot byte length and the canonical registry-omitted-plus-typed-registry bound. This accounts for the immutable historical before snapshot even when a typed registry write later drops unknown row fields. The opaque floor can shrink after that cleanup; it cannot increase in subsequent unchanged-context replay, while canonical request/counter/history widths stay fixed.

The existing byte-tight lifecycle journey now inserts a 32 KiB unknown flexible registry-row extension before preparation. It checks that the full original encoded snapshot is retained in the durable creation proof, confirms typed cleanup occurred, completes creation Undo/Redo, then requires nonincreasing reservations and successful repeated Delete/Restore cycles with maximum request escaping and widening journal cursors. No new test declaration or execution is introduced.

## Earthwork terrain source binding

capture.rs fingerprints terrain from the SAME Petal row already used for the
opaque typed snapshot and metric scale. The shared canonical JSON identity is
additional expected-state data; the existing full-view comparison and containing
transaction's typed Petal/node/role/schema/history fences remain unchanged.
Missing terrain yields None, never a fabricated valid source. Generic batch and
Group budgets continue measuring the complete serialized capture, including this
fixed-width optional witness. No separate DB read or terrain mutation is added.

The existing history journey adds one earthwork target with two Set properties,
induced second-field fact failure/complete rollback, current-DB/new-source capture,
post-capture terrain refusal, real zero and exact durable receipt resolution.
Runtime sample coverage is an additional producer admission check, not a property
of this capture. Source authoring is not execution evidence.
