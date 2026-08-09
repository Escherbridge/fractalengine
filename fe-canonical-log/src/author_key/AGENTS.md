# `author_key` — SPEC-2 author identity lifecycle

Normative source: `docs/spec/canonical-log/author-key-lifecycle.md` (owner-approved 2026-08-09).
This module owns key rotation (§3), active-key resolution (§4), Manager+ suspect-window
disavow (§6), disavow rescind (§6.1), and the D-CL18 continuity grant (§9). It owns no
storage, no I/O, and no network: every external fact arrives through a trait the caller
implements.

## Module map

| File | Spec anchor | What it owns |
| --- | --- | --- |
| `payloads.rs` | §3, §3.1 | `RotationStatement` (nine keys), `RotationPayload` (two keys), the equality table check against a received envelope |
| `rotation_proof.rs` | §3.2 | domain `fe-author-key-rotation-v1\0`, both DID bindings, distinctness, the successor possession proof |
| `lineage.rs` | §4 | `CausalOperationView`, `LineageIndex`, `KeyState`, fork detection |
| `disavow.rs` | §6 | six-key payload, `DisavowSubject`, `DisavowIndex` classification |
| `disavow_rescind.rs` | §6.1 | `AuthorityLevel`, rescind payload, `validate_rescind_authority` |
| `continuity_grant.rs` | §9 | domain `fe-owner-continuity-grant-v1\0`, dual-signed grant, `AttributionIndex` |
| `admission.rs` | §4.1–§4.3, §6.1, §9.2 | the four entrypoints a materializer calls, the seams, the quarantine taxonomy |

## Every check has a caller

`admission.rs` is the single path a materializer uses, and every validator in this module is
reachable from it: `RotationPayload::validate_against_envelope` and `verify_rotation_proof`
from `admit_rotation`, `DisavowPayload::validate_against_envelope` from `admit_disavow`,
`validate_rescind_authority` from `admit_disavow_rescind`, and
`ContinuityGrantPayload::validate_against_envelope` from `admit_continuity_grant`. Nothing
here is an opt-in helper a caller might skip. A check that reads as enforced without being
enforced is worse than an absent one, and this repository has shipped that mistake before.

`admit_disavow_rescind` and `admit_continuity_grant` are not in the original slice brief,
which named only `admit_rotation` and `admit_disavow`. They exist because §6.1 and §9.2
otherwise define signed artifacts with no admission path, which is exactly the dormant-gate
shape above.

## Causal resolution, never arrival order

`LineageIndex::active_key_at(causal_point, key, scope, view)` answers from the operations the
given point reaches, through `CausalOperationView::reaches`. It never consults arrival order,
a newest-observed epoch, or a wall clock. The same query at two different causal points
legitimately returns two different answers; that is §4.4 and §4.5, not a bug.

`DisavowIndex::matching_disavows_at` is causal for the same reason: a disavow the replaying
head does not reach classifies nothing, and a rescind subtracts only inside its own causal
future (§6.1 rule 3). `matching_disavows` without a causal point is the whole retained
evidence set and is deliberately separate.

Scope propagation (§2.5) is `Scope::contains` in one direction only: a rotation applies to a
query scope when the *rotation's* scope contains it. A verse-scope rotation therefore reaches
every descendant petal and resource, a petal rotation never widens to its verse, and nothing
crosses a verse boundary. No second scope type and no reimplemented containment exist here.

## §4.6 rotation forks are detected, never resolved

Two causally concurrent rotations from one predecessor at overlapping scopes are a fork.
`record_rotation` returns `RotationOutcome::Fork`, both records are retained, and
`active_key_at` answers `ForkedUnresolved` for the predecessor and for **both** successors.
The spec requires an "authorized Manager+ resolution operation" that it never defines, and the
owner has ratified deferring it. There is deliberately no resolver, no tie-break, and no
newest-wins rule in this module. Inventing one would be a silent protocol decision.

A causally *later* second rotation by an already retired predecessor is not a fork: it fails
the parent-induced active check and is rejected outright (§4.6 last sentence).

## §6.5 cascading revalidation is out of scope here

`DisavowIndex` classifies. It never deletes bytes, rewrites DAG edges, or drops signatures.
§6.5's second half — revalidating dependent authorization facts so a descendant resting on a
disavowed fact does not materialize — is the SPEC-4 materializer's work, not this module's.
The boundary is deliberate: this module answers "is this operation inside a suspect window",
and the materializer decides what that means for the projection.

## D-CL18: a regenerated key is a new principal

`ContinuityGrantPayload` links a lost principal to a new one for attribution and display.
`AttributionIndex` is a separate structure from `LineageIndex` and `DisavowIndex`, and
`admit_continuity_grant` writes only to it — the no-authority-effect property is structural,
not a convention, and a test pins it. A grant confers no lineage, no capability, no
retroactive validation, no disavow override, and no fork resolution (§9.3, §9.4).

## Author equivocation (SPEC-1 §3.4) is enforced here

Two distinct `op_id`s sharing one `EquivocationKey` quarantine **both** candidates and
materialize neither. `AuthorKeyState` retracts the first candidate's effect from lineage,
disavow, and attribution state when the second arrives, and locks both `op_id`s so
re-presenting the bytes cannot resurrect either. Only an authorized resolution operation
releases that lock, and no such operation exists yet, so the lock has no release path in this
module by design. Availability quarantines are the opposite: they are retryable and clear
themselves when the operation later succeeds.

## No policy numbers (D-CL24)

Quarantine bounds, retention windows, GC leases and rate caps do not appear here. The
quarantine reasons carry no duration and no cap; SPEC-5 owns those and takes them as caller
configuration. `RegisteredSchemas` deliberately has no `Default`: schema identity is a
registry fact the caller supplies, never a number this crate invents.

## Provisional wire numbering

SPEC-2 gives a normative integer-key table only for §3 (rotation payload), §3.1 (rotation
statement) and §6.2 (disavow payload). The two artifacts below are described in prose with no
key table, so the numbering here is **provisional, assigned by this crate under D-CL24, and
not wire-final**. No cross-implementation interop is claimed for these bytes; the owner
ratifies or replaces them later. This table is the single record of every key this slice
assigned.

`disavow_rescind_payload` (§6.1 rule 1 — prose only):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `disavow_op_id` | byte string, 32 bytes |
| 1 | `reason_code` | unsigned `u16` |

`continuity_grant_statement` (§9.2 — prose only):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `protocol_version` | unsigned integer, MUST be 1 |
| 1 | `verse_scope` | map, SPEC-1 §3.1 grammar, MUST be verse-wide |
| 2 | `lost_principal_did` | UTF-8 text |
| 3 | `lost_principal_public_key` | byte string, 32 bytes |
| 4 | `new_principal_did` | UTF-8 text |
| 5 | `new_principal_public_key` | byte string, 32 bytes |
| 6 | `reason_code` | unsigned `u16` |

`continuity_grant_payload` (§9.2 — prose only):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `statement` | the map above |
| 1 | `new_principal_signature` | byte string, 64 bytes |

## Signature domains this module adds

| Domain | Artifact |
| --- | --- |
| `fe-author-key-rotation-v1\0` | §3.2 successor possession proof over the canonical statement |
| `fe-owner-continuity-grant-v1\0` | §9.2 new-principal countersignature over the canonical statement |

Both are distinct NUL-terminated ASCII prefixes and both go through
`signing::sign_domain` / `verify_domain`, per D-CL3. Neither signs the outer operation ID:
including it would create a signature/hash cycle (§3.1 closing note).

## Conformance-test naming

SPEC-2 §7 numbers its fourteen required cases but does not name them, so tests are named
`spec2_case_NN_<what it asserts>` and a case with several independent halves appears under the
same number in more than one module. Cases 11 and 12 concern `fe-identity` secret handling and
Iroh transport keys; the crate-local half is tested here (a regenerated key has no lineage to
continue, and no transport identity is an input to admission — `admit_rotation` takes none),
while the `fe-identity` half belongs to the wave-3 slice that owns that crate.

## Duplicated CBOR field helpers

`mod.rs` carries `require_numeric_keys` / `entry` / `unsigned_at` / `u16_at` / `bytes_at` /
`text_at`, which are byte-identical in behaviour to the private helpers in `envelope.rs`.
`envelope.rs` is a frozen wave-1 file and its helpers are private, so this slice could not
share them. The integration wave should promote the `envelope.rs` copies to `pub(crate)` and
delete these; until then the two must not diverge, which is why the error type here is
`EnvelopeError` rather than a parallel taxonomy.
