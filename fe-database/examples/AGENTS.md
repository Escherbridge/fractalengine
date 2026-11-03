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
`.qa/run`; existing roots refuse. The child duplicates the actual wrapper lifecycle
lock handle. A deliberately leaked Context retains that lease and all context pins
until OS process exit, including output publication, error/timeout paths and Tokio
runtime teardown. This is bounded process-lifetime ownership, not a claim that
`drop(db)` joins every SDK worker. Acquisition failures before Context exists open
no database and publish no preparation success.

The stopped reader is intentionally unavailable: `inspect` validates the recorded
lifetime, preparation and canonical input, then returns
`fixture_store_confinement_unavailable` before any SDK store open. It issues no SQL,
creates no DB path and runs no SDK housekeeping. Discovery/baseline/adopted/resume
therefore cannot produce a successful readback or restart with this interim helper.
Preparation/sealing remain usable. The originally authored typed SELECT oracle is
preserved in Git history at 0d09c566, not as reachable unsafe code. A later typed
readback must use the application's existing DB owner/correlation path and receive
separate review; no positive durable-adoption claim is available from this slice.

Pinned local dependency inspection: SurrealKV 0.21.1 `src/lockfile.rs` opens LOCK
read/write/create/truncate, and `src/levels/mod.rs` replaces manifests by rename.
SurrealDB 3.0.5 `src/kvs/surrealkv/mod.rs` exposes a path-based normal store open,
not an existing-handle/no-reparse/read-only open contract. A recursive scan and
no-delete handles alone cannot prevent new entries or in-place reparse conversion
under write sharing; read-only sharing would also conflict with SDK housekeeping.
No unsupported physical-confinement promise or fallback opener is retained.

The wrapper's C# operation owns the exact Process object and extra SafeHandle
references to its executable pin and parent lifecycle lease. One absolute 35 s
operation deadline starts before asynchronous Process.Start; stdin write/flush/close,
incremental stdout/stderr reads and process exit share that deadline. Pipe ingress
is bounded while reading, including zero permitted stderr bytes. Cleanup requests
termination only through the retained Process and waits at most a further 5 s for
confirmation. A stalled start continuation still owns the references, and checks
cancellation before doing I/O. If cleanup cannot confirm termination, references
are intentionally retained until host exit and the caller gets
`helper_lifetime_unknown`; a returning script cannot free that lease for another
run. A still-running cleanup releases them only if it later confirms exact process
exit.
Inherited pipe handles cannot extend the caller's wait indefinitely. Raw exception
or stderr text never crosses the wrapper boundary.

| Current bound | Contract |
| --- | --- |
| request/preparation/current/stage JSON | 64 KiB each |
| canonical GLB | 1 MiB plus one-byte overflow probe |
| helper lifetime | 30 s cooperative; wrapper has one 35 s deadline plus 5 s owned cleanup |
| unknown helper lifetime | retains lease/executable references until confirmation or host exit |
| stdout/stderr | 4 MiB/zero bytes, incremental 4 KiB reads |
| stopped-store inspection | unavailable; refusal precedes any SDK open or SELECT |
| recorded descendants | max 32 |

Source-review scenarios below are unexecuted acceptance cases, not runtime proof.
They introduce no new test functions and require the parent-owned execution lane.

| Fixture condition | Required ownership/output |
| --- | --- |
| parent exits after child duplicates lease | OS retains child's Context lease through terminal child exit |
| child stalls before draining a 64 KiB stdin | common deadline, exact-owned kill/join; no synchronous caller write |
| stdout exceeds 4 MiB or emits any stderr | early bounded refusal, exact-owned cleanup, no raw pipe text |
| child exits but a pipe remains inherited/open | common deadline, no unbounded post-exit GetResult |
| process start or termination stays stalled | caller returns unknown after bounded cleanup; extra SafeHandle references stay held |
| junction appears anywhere in stopped DB tree | no SDK opener exists; no DB member is read or written |
| ordinary existing stopped legacy store | same named confinement refusal; no successful oracle/resume report |

The intended next readback still requires DB-derived key presence, complete CBOR
witness digests, exact public data/numeric checks, complete reference sets and
contiguous bounded history. No prior SELECT oracle is exercised or credited by
the current stopped-store refusal. Preparation, normal API creation,
native Review, one explicit Confirm, provenance, transfer, restart availability and
GPU appearance remain separate evidence cuts. The helper issues no creation API
or adoption. Fs availability is not cold native iroh absence/network transfer
proof. No recipient gets fixture bytes. Public SQL and Viewer debug floors are
unchanged. Root owns actual execution and acceptance.
