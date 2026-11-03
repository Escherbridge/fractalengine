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

Shared registry admission also fences historical checked-delete IDs, so ordinary CreateGroup and legacy registry/CAS writers cannot sidestep the proof route once retained references were cleaned. Only the typed proven restore uses the separately named retained-reference fence. IDs removed solely by UndoCreatedGroup retain the existing Redo behavior; this change does not invent a global Group identity journal for older untracked deletions.
