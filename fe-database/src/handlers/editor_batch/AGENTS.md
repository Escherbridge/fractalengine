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
