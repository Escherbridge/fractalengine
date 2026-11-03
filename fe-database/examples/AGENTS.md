# Database diagnostic examples

## Disposable legacy adoption fixture

`legacy_adoption_fixture` is a staged QA helper, not a production API. Invoke it
through the lifecycle-owning QA scripts. Input is one JSON request on stdin (64 KiB);
output is one JSON result (4 MiB) with static error codes. Raw database/native errors,
identity/token files and namespace secrets are never published. The wrapper checks
SHA256; the helper checks canonical bytes and BLAKE3. `generate`/`seal` write only a
new run's GLB and preparation receipts. They never open a DB or adopt an asset.

The generator is deterministic for a recorded 32-hex preparation identity. It emits
an embedded GLB2 with two closed prisms, 24 triangles and contrasting standard
materials. The identity is in nonvisual extras to distinguish cold fixtures.
Verification compares every canonical byte and container/chunk framing. The
existing ordinary-adoption journey applies the real scene-byte verifier and checks
determinism, nonce separation and corruption. No test function is added or executed.

`files` pins existing ancestors with Windows handles and uses exclusive final
creation. Only generate may create a new direct timestamp child of this build's
`.qa/run`; existing roots refuse. Inspection requires an existing nonempty DB and
a bounded manifest-file probe for the pinned SurrealKV version; it never creates
parents. The child duplicates the actual wrapper lifecycle-lock
handle, retaining that same lease throughout the diagnostic. Wrapper and helper
verify exact PID/start ticks/exe identities; the helper rechecks after DB close.
Recycled PIDs are distinguished. Unverifiable identities refuse. Recorded-child
absence does not prove absence of children born after the last snapshot.

`inspect` executes fixed SELECTs with bound identities only. It never applies
schema, seeds, reconciles, adopts or writes logical rows. SurrealKV may perform
normal open/close housekeeping; this is not a forensic read-only storage mode.
Preparation/stage/source/current lifetime, source SHA/BLAKE3/size, Genesis creator
and actual runtime DID must agree. Runtime DID is observed from the real bootstrap
token after normal authenticated readiness, not accepted as authorization. Actual
target Owner derives from the stored Verse creator and Fractal owner; a real
sharing namespace is required.

| Bound | Contract |
| --- | --- |
| request/preparation/current/stage JSON | 64 KiB each |
| GLB/canonical blob | 1 MiB plus one-byte overflow probe |
| helper lifetime | 30 s; wrapper waits 35 s and owns its exact helper process |
| SELECT/inspection | 2 s each/25 s total |
| output/per SELECT decoded result | 4 MiB/2 MiB |
| asset/hierarchy singleton | 2-row ambiguity probe |
| all node references | 257-row overflow probe including tombstones; fixture expects 0 or 2 |
| model/device references | 2-row probes; any reference refuses this focused fixture |
| asset/node CBOR witness | 64 KiB size preflight before fetching |
| history | only the actual 2 fixture IDs; 33-row probe, max 32 contiguous facts each, 32 KiB per fact |
| adoption operations | 3-row probe; max 1, 64 KiB preflight; native request-ID grammar |
| recorded descendants | max 32 |

Presence comes from DB object keys, not rebinding JSON null as Surreal NONE.
Complete witnesses use the server's CBOR/base64 expression. Reports publish their
BLAKE3 digests instead of reversible payloads or arbitrary journal properties.
Current properties must be an empty object; public name/numeric fields are checked.
Baseline/adopted phases require exactly QA Legacy A/B, one lawful Petal and no
tombstones, model or payload references. An unexpected set refuses whole inspection.

Discovery requires 0 references/no adoption. Baseline requires 2/no adoption. Adopted
requires one native operation, matching present-tense provenance and the immutable
baseline's node/history/reference/blob witnesses. Asset identity/hash/MIME/size/time
remain unchanged; uploader ownership remains absent. Resume verifies existing
Genesis/asset before GUI seeding and writes no report. Phase reports use create-new
files, never overwrite. Changed stage/preparation refuses rather than reseeding.

Preparation, normal API creation, native Review, one explicit Confirm, provenance,
transfer, restart availability and GPU appearance remain separate evidence cuts.
The helper issues no creation API or adoption. Fs availability is not cold native
iroh absence/network transfer proof. No recipient gets fixture bytes. Public SQL
and Viewer debug floors are unchanged. Root owns actual execution and acceptance.
