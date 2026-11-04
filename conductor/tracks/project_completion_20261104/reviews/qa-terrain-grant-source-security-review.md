---
type: Review
date: 2026-11-04
title: Terrain grant security source review
---

# QA-0070 terrain grant: independent security source review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for source integration. No blocking security/source-contract defect found in this frozen revision.** This implements the narrow approved v2 design without treating authored assertions as executed proof. QA-0070 remains open pending exact integration, compilation, composed SQL behavior, supported ordering and runtime acceptance.

## Exact reviewed revision

Isolated worktree `.omc/worktrees/qa-terrain-grant-continuity`, HEAD/base `fa9555632380977c41fe89ac984fcb491c47295a`.

| Artifact inside the worktree `.omc/research/` | Independently verified SHA-256 |
|---|---|
| `qa-terrain-grant-source.patch` | `085fe65db89eac3a958595f7db9bae577149051d9fc339248df8d3081146cce6` |
| `qa-terrain-grant-frozen-manifest.json` | `03fcbe6377050735c803740409498baa39e54cd98708d8af12b301e7815cba6c` |
| `qa-terrain-grant-source-report.md` | `98d57b63c2722a2f2dfc6d1ad2d02714c0d510387f1cee5bd654390252cc2d61` |

All **35** raw source file byte counts and hashes match the manifest, including its four new source/document files. Rechecked the entire manifest after source inspection; still exact. Read the approved v2 plan/security review, actual production source, changed caller/constructor inventory and authored journeys. Approved plan hash remains `64f1b3cb57eb0213d22bf4e2a24f8c64362278c86758efc4ad28f59d52543114`. Read-only worktree `git diff --check` passes.

## Original admission and issuer survive the queue

`fe-api/src/terrain.rs` retains the actual `authorized_petal_scope` result for REST PUT/DELETE. MCP's `set_petal_terrain` retains `ToolCall.scope`; it does not re-resolve a replacement scope and call it the original. Both call the same private `terrain_credential::capture` with actual claims, petal and admitted ancestry. Shared TerrainConfig and publisher-pin validation remain before enqueue.

The required `SetPetalTerrain.grant` replaces this command's old permissive CallerAuth. API production constructs only `TerrainWriteGrant::Api`; GUI GIS/hexon/proposal producers explicitly construct Local. Missing correlation does not choose a grant and API request JSON cannot deserialize one. The existing peer stub's `..` pattern is transport compatibility, not production authority evidence. Other commands retain their existing CallerAuth contracts; this patch does not certify those adjacent doors.

`TerrainApiGrant` is private-field, nonserialized, cloned and Debug-redacted. Its trusted constructor derives canonical issuer DID from the exact configured Ed25519 verifying key, requires claims.sub equality and token_type `api`, converts and retains the actual typed Editor-or-higher ceiling, and bounds petal/scope/JTI/generation strings and signed expiry conversion. This is explicitly trusted in-process construction from already verified claims; it is not a new signature verifier or protection from arbitrary hostile host Rust.

The actual API builder additionally rechecks scope policy, canonical full admitted ancestry and target petal before capturing durable metadata. It retains original signed scope, role, expiry, JTI, issuer and the **original** generation/token projection in the immutable grant. The optional client/upload identity is never an authorization principal. Issuer and actual DB publisher are not conflated. The signature-valid foreign-sub journey covers both known-token and rowless cases, so the added subject assertion is not assumed to exist in generic JWT verification already.

## Durable capture and exact private routing

Direct ReadDb and the one typed `ReadTerrainApiCredential` worker door use the same `api_auth::terrain_capture_sql` and `decode_terrain_snapshot`. The command carries no SQL, arbitrary table, projection, store URL, key, bearer or public schema/token inventory. There is no RawQuery/public endpoint/writable-facade addition and no Local, invented generation or permissive fallback. The configured direct reader is used when present; otherwise the existing configured API-to-DB transport is used. A failed/malformed/unavailable direct result does not select a second permissive source.

The fixed read executes BEGIN, three required-table guard statements, the singleton generation projection with a two-row ambiguity probe on matching token rows, then COMMIT. `TERRAIN_CAPTURE_SLOT = 4` corresponds to the actual SELECT after BEGIN/LET/IF/IF. Decode requires exactly one valid generation row, current-compatible JTI, at most one token, exact two-field token object, issuer equality and revoked=false. The serialized returned rows are limited to 8192 bytes; generation and request fields are bounded. The projection contains only generation and the exact matching token's sub/revoked fields. The implementation and docs correctly do not equate that output cap with a bound on INFO's internal catalog enumeration/allocation.

`PendingApiRequests` owns a separate at-most-eight map. A reply needs the exact `TerrainApiCredentialRead` family and matching private Arc lease, request ID, issuer, JTI and immutable deadline. Wrong-owner same-ID results leave the real waiter untouched. Generic QueryResult, RequestFailed and Error routing cannot consume a credential waiter or manufacture a grant, because this map is outside generic family/wildcard queues. The API capture repeats exact ownership/current-active checks before accepting its reply.

Closed/expired owners prune at enqueue, delivery and every command drain. The API enqueue and the special DB forwarding path use `try_send`; a full bounded channel refuses without synchronously blocking beyond the capture budget. If enqueue ownership was refused, no command is forwarded. Failed forwarding cancels/removes the exact read entry. Other pre-existing commands' blocking sends remain outside this narrow new door; this patch does not claim a global nonblocking dispatcher.

## Deadline, permits and late work

The five-second capture deadline is created once before permit acquisition. One outer timeout covers permit wait, direct query or API enqueue/forwarding/reply, final revocation-hint check and strict expiry check. It is never renewed by queueing, response or retry. Eight permits are initialized once per ApiState and shared across its REST/MCP captures. The separate read owner owns the permit through an Arc lease; cancelling active admission does not drop every queued/engine reference.

For direct reads, the spawned owned task retains the request through actual query completion and its post-query active check. Dropping the awaiting API future detaches rather than aborts that task; AbandonRead marks admission inactive, and the read task still owns the permit until completion. For channel reads, queued API/DB command and eventual reply retain that same owner; waiter pruning cancels permission to use the result but is not engine cancellation. The DB helper checks active before query and after completion. Late direct/channel results cannot resume the abandoned capture future and enqueue a write. This is conservative occupancy: a stuck engine read can hold a slot; no claim of forcibly cancelling it is made.

The existing mutation waiter remains **10 seconds**, as in the actual base source. It is distinct from the new five-second capture budget. A mutation already enqueued can have an unknown outcome after timeout; no automatic replay, cancellation, source substitution or post-commit credential reclassification was introduced.

## Preparation and the containing operation/row transaction

`prepare_terrain_write` freshly resolves the target scope and requires the **actual local publisher** Editor floor. For Api it checks original target, canonical admitted full ancestry and equality with the fresh scope, typed token ceiling and signed scope coverage. It then captures current durable credential metadata, whose generation and exact matching-token projection must equal the original API admission snapshot. In particular, deletion of a previously known token between API capture and DB preparation cannot silently become an allowed rowless credential.

The existing `prepare_scope_permission` remains a separate conjunction: current local creator/default-access/role snapshot plus full unique Fractal/Verse/Petal ancestry. Its authority and ancestry predicates remain in the prepared write. No original-token role string is substituted for actual local writer authority.

`terrain_credential_fence` adds both required-table checks, original generation equality, original exact token projection equality and strict SQL `time::unix() >= signed_expiry` refusal. All guard strings/bindings are retained in `PreparedTerrainWrite`, and appended to the same statement list passed to `commit_operation_atomic` before the UPDATE. That unchanged helper places ordinary operation creation, local authority guards, credential guards and terrain UPDATE inside **one BEGIN/COMMIT** and fails on transaction errors. Result handling takes the final caller slot, which remains UPDATE RETURN AFTER; added LET/IF slots do not shift the code onto a guard result. Existing terrain validation, committed readback, API correlation and best-effort post-commit replication remain.

Rechecked the relevant pinned SurrealDB-core 3.0.5 source: INFO parses as Expr::Info; LET accepts an expression; InfoStatement::Db uses `ctx.tx()` and its transaction catalog for `all_tb`; unstructured tables is an object; object::keys requires an Object. `time::unix` with no argument returns current Unix seconds. These source facts support the authored SQL shape and strict signed-expiry comparison. They do **not** execute the composed transaction, prove response-slot conversion behavior, or establish concurrent catalog-cache/storage isolation.

Current-generation rowless dev JTIs continue through actual `accepts_jti`, with fe1 generation/nonce validation. Non-fe1 compatibility exists only while durable generation is literal `legacy`. Missing tables/generation, malformed generation, duplicate tokens, wrong issuer or revoked token fail closed. A token inserted/deleted/changed after capture or preparation changes the expected projection and refuses. No individual revoke semantics for an unpersisted JTI or new epoch/forced metadata row is invented.

## Existing authority test meaning and claim limits

The API-ceiling/local-role-cap fixture still uses a **real Api grant**, minted with a fixture key matching the actual fixture verifier, Manager ceiling and captured durable generation/JTI. `Node::set_terrain` was not downgraded to Local. Its existing real DB worker demotion assertions remain. The runtime dependency additions and ApiState limiter initializations are mechanical; unrelated production API endpoints are unchanged.

The four extended journey files retain their existing declaration counts: runtime 22, REST 6, DB handler 5, role cap 1, **34 total before and after**. Authored cases cover exact family/owner refusal, retained permits, full forwarding, direct/channel-only capture, foreign issuer, unavailable/table-loss/wrong-family/deadline abandonment, current/new/legacy generation success, Viewer refusal, preparation/commit revoke/delete/insert/reset/reparent/expiry refusal, terrain unchanged and no orphan ordinary operation. The simplified MCP/peer models remain transport fixtures rather than durable guard proof.

The source report correctly distinguishes capture-then-change from actual concurrent isolation. Ordinary supported terrain/revoke/access/replicated-row mutators run through the existing serial DB worker; reset has its independent stop/join boundary. Public low-level/shared handles can exist outside that worker. This review grants no universal concurrency guarantee for them, and no unconditional token/generation writes were added to manufacture conflicts.

## Required execution gates before QA-0070 closure

- Compile the exactly integrated source, all actual constructors/matchers and extended existing journeys; run the integrated check sweep under the project's once-at-end policy. This review ran none.
- Execute the fixed direct and actual serviced channel-only credential query paths, response slot decoding and both required metadata-table loss cases, including rowless preparation followed by table removal. Preserve refusal/rollback with no terrain update or orphan operation; a missing table must never be accepted as an empty token projection.
- Execute unchanged valid scoped Editor/Manager success and actual local-publisher demotion/Viewer denial, original-scope reparenting before preparation and before commit, strict expiry, revoke/delete/insert/reset/generation changes and known-to-rowless deletion refusal. Verify the exact admitted ancestry and original projection survive the real transport.
- Measure the eight-permit/five-second ownership behavior through queued, actual engine, abandoned and late-result boundaries, not merely waiter counts. Confirm full channels cannot block this read family, and late capture cannot enqueue a mutation. Keep accepted mutation timeout unknown and never replay it to produce acceptance.
- Establish actual supported mutator ordering through the real DB worker and reset boundary. Any claim about concurrent direct/shared handles requires corresponding engine/read-conflict/catalog-cache evidence, beyond source predicates and sequential prepared-write journeys.
- Preserve independent issuer versus local publisher provenance, old findings/history, retained-store restart hold, volume-specific acceptance and adjacent endpoint limitations. Source approval does not close those gates or certify native/capture/CI/release/project completion.

Only this main review artifact was authored. No production or Git mutation, compiler/formatter/build/test, app, API/DB/store execution, OS/process/input/capture or network operation occurred.
