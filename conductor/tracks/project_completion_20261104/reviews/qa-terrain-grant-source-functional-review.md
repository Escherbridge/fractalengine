---
type: Review
date: 2026-11-04
title: Terrain grant functional source review
---

# QA-0070 terrain grant: independent functional and source API review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.
Verdict: **APPROVED for source integration. No blocking functional,
readability, constructor-inventory or inspected source API/type issue found in
this frozen revision.** This is not a compiler/test result or QA-0070 closure.

Reviewed the approved v2 plan, independent security source review, exact frozen
patch/manifest/report, production adapters/DTOs/dispatch/SQL composition and
extended existing journeys in `.omc/worktrees/qa-terrain-grant-continuity`.
No production source edits, Git mutation, compiler/formatter/build/test,
application/process/input/capture, API/DB/store or network operation occurred.
Only this MAIN review artifact was written. Other authors and root's live app
were untouched.

## Exact reviewed revision

Base: `fa9555632380977c41fe89ac984fcb491c47295a`.
Independently verified all35 manifest file byte counts and raw SHA256 values;
there are no mismatches. Exact artifact digests:

| Artifact | SHA256 |
| --- | --- |
| Isolated `.omc/research/qa-terrain-grant-source.patch` | `085fe65db89eac3a958595f7db9bae577149051d9fc339248df8d3081146cce6` |
| Isolated `.omc/research/qa-terrain-grant-frozen-manifest.json` | `03fcbe6377050735c803740409498baa39e54cd98708d8af12b301e7815cba6c` |
| Isolated `.omc/research/qa-terrain-grant-source-report.md` | `98d57b63c2722a2f2dfc6d1ad2d02714c0d510387f1cee5bd654390252cc2d61` |
| MAIN approved v2 continuity plan | `64f1b3cb57eb0213d22bf4e2a24f8c64362278c86758efc4ad28f59d52543114` |

Approval applies to these exact artifacts/source hashes, including four new
source/document files. The security approval is separate evidence; this review
independently checks the functional/API and maintainability seams below.

## Required command and adapter inventory

`DbCommand::SetPetalTerrain` requires TerrainWriteGrant in place of this command's
old CallerAuth field. Production REST PUT/DELETE retain the actual admitted scope
from authorized_petal_scope; MCP retains ToolCall.scope and the resolved Petal,
then all three call the same private API capture adapter. None re-resolves a
replacement admitted ancestry or constructs Local. Shared config-object parsing,
TerrainConfig validation and publisher-pin validation remain before mutation
enqueue. Runtime's mutation correlation and stored echo matching remain intact.

All six production GUI terrain producers in actions/gis, hexon and
terrain_proposal explicitly select Local. The production DB dispatcher destructures
the new required grant and handles the new typed read command. Existing peer/MCP
model wildcard patterns remain compatible transport fixtures; they are not used
as durable authorization evidence. Other commands keep their existing CallerAuth
contracts, including render-distance and unrelated lifecycle/history doors.

The actual ApiState initializer inventory contains20 struct literals across the
server, API unit/integration fixtures and test harness; each initializes the
new per-host limiter. Its Arc ownership is shared by all calls using that
ApiState. The change does not create a fresh limiter inside each REST/MCP handler.
The new module exports/imports and method signatures line up by source inspection.
The actual GUI result dispatcher offers the new result to PendingApiRequests
after its ordinary match, and the headless dispatcher does likewise; the added
read family has a complete serviced path rather than a constructor-only change.

## Type/API and dependency consistency

The new immutable runtime grant, snapshot and read owner are Clone and
nonserialized, with redacted Debug implementations. Their fields are compatible
with the existing Debug/Clone/message transport. The lease contains an Arc,
AtomicBool and owned semaphore permit; it does not introduce a non-Send UI/COM
handle into Bevy/crossbeam messages. The required grant enum explicitly separates
actual Local publisher work from verified API admission. Private DTO fields and
the public trusted Rust constructor remain an in-process convention, not a
cryptographic boundary against hostile code inside the host.

The API direct task owns its cloned ReadDb and request before tokio::spawn, with
no borrowed ApiState or temporary query data escaping into the task. ReadDb's
query facade already supports the fixed script and existing bind/await/check/take
shape. It does not acquire a writable facade. Runtime owns DTOs/limits; API owns
verified claim adaptation; DB owns metadata SQL/decoding/fences. This dependency
direction remains coherent and does not create a runtime-to-database dependency.

The configured verifier API is the actual Ed25519 VerifyingKey accepted by
fe_identity::did_key_from_public_key. RoleLevel::from/is_at_least, Policy and
TokenScopePolicy/Scope APIs match their actual definitions. The fixture seed-key
copy uses NodeKeypair::from_bytes(&[u8;32]) and the existing signing/verifying
interfaces. Root Cargo.toml already supplies Ed25519 pkcs8 support and the JWT
RustCrypto feature. Cargo.lock changes add only the corresponding already-resolved
dependency edges: runtime fe-policy/Ed25519 and API test JWT signing.

Pinned versions are tokio1.52.3, surrealdb/core3.0.5 and jsonwebtoken10.4.0.
Inspected tokio1.52.3's actual Instant conversion and Arc<Semaphore>::acquire_owned
definitions; they match the new deadline and owned-permit use. These checks find
no source-visible API mismatch. Only compilation can establish full Rust trait,
lifetime, feature and enum-exhaustiveness acceptance across the integrated build.

## One budget, exact read ownership and late results

The API establishes one five-second deadline before permit acquisition and
uses one outer timeout around acquisition, direct/worker capture and final
hint/strict-expiry checks. A queued response does not renew it. The special read
uses try_send at API admission and DB forwarding; a full bounded channel cannot
block this family synchronously. A refused pending-map admission is not forwarded.
Forward failure cancels/removes the exact entry.

PendingApiRequests has a separate capped credential map. Matching uses the exact
private Arc lease plus request ID/issuer/JTI/deadline, before removing its waiter.
Wrong-family QueryResult/RequestFailed/Error cannot enter this map; a foreign
owner with the same textual ID cannot consume the real entry. The generic
wildcard/correlation queues remain unchanged. Closed/expired owners prune on
admission, delivery and each API-command drain, including a drain with no receiver.

RAII AbandonRead cancels admission on success/error/timeout/drop. Cancellation
does not release every permit reference or claim engine cancellation. The owned
direct task retains its request through actual query completion and its active
recheck; dropping its JoinHandle detaches it rather than aborting it. The channel
command, DB helper and reply retain the same lease. A canceled queued read refuses
before querying; an in-flight read refuses result use afterward. A late result
cannot revive the abandoned API future and enqueue a mutation. A stuck read may
conservatively occupy a permit; source does not promise forcible engine abort.

The actual terrain MUTATION waiter remains **10 seconds**, unchanged by this
patch. It starts after grant capture/enqueue and is distinct from the new
five-second read budget. A timed-out enqueued write still has an unknown outcome;
there is no replay, retroactive cancellation or source substitution.

## SQL conjunction, bindings and result slots

The shared fixed capture script uses BEGIN, LET inventory, two required-table
IF guards, one bounded generation/token SELECT and COMMIT. Under the repository's
actual indexed-response convention, SELECT is slot4, matching
TERRAIN_CAPTURE_SLOT. The direct facade and actual worker use the same script
and decoder, rather than maintaining two subtly different credential semantics.

The decoder requires one valid generation singleton, actual accepts_jti
compatibility, at most one exact two-field sub/revoked token row, matching issuer
and revoked=false. The SELECT probes at most two token rows to reject ambiguity.
The encoded returned row projection is checked against8KiB; the private snapshot
also bounds generation/tokens. INFO's internal catalog work is not covered by
that output bound. No schema/token inventory is returned as an API response.

DB preparation independently applies actual local publisher Editor authority,
checks the original exact canonical admitted ancestry/Petal, signed token scope
and role ceiling, captures current durable metadata and compares it with the
ORIGINAL admission snapshot. Known-token deletion cannot silently become allowed
rowless compatibility. Required-table guards run again INSIDE the containing
write transaction before empty-token interpretation, followed by generation and
exact token projection equality and strict time::unix signed-expiry refusal.
Local writes retain the actual publisher path without invented credential data.

PreparedTerrainWrite composes these credential guards with existing ParentMutation
authority/default-access and full unique ancestry guards. Parent bindings use
entity_ names, credential guards use terrain_credential_ names, terrain UPDATE
uses pid/config and the operation helper reserves __op_; the inspected binding
sets do not collide. The one commit_operation_atomic script includes ordinary
operation creation, all guards and UPDATE RETURN AFTER in one transaction.
The helper removes BEGIN/op-log/COMMIT slots and retains every caller result;
pop() therefore still selects the final UPDATE after added LET/IF statements.
There is no early RETURN before the UPDATE or a hardcoded obsolete guard offset.
Committed readback, exact API reply correlation and best-effort post-commit
replication remain in the original order.

The approved plan and independent security review inspected the pinned INFO/LET,
transaction metadata, object::keys and time::unix source. This review verifies
their composition and indexing against the actual helper. It does not execute
the script, prove its driver conversion behavior, or establish concurrent
catalog-cache/storage isolation from those source facts.

## Journey meaning and readability

Independently counted the four extended files' declarations: runtime22, REST6,
DB handler5, real API-cap1; total34. Helpers are actually called from those
existing journeys. They are not unused assertion functions or new test
declarations hidden behind the source report.

The runtime extension reaches real pending routing/forwarding and checks foreign
family/owner refusal, closed pruning, retained permit occupancy and a full DB
channel. REST reaches direct and serviced channel-only paths and production
reply preparation; signature-valid foreign-sub cases cover known and rowless
metadata. Capture refusal waits on the request's own deadline, then compares
stored terrain and complete operation rows. DB prepared-write cases exercise
the actual captured fences after revoke/insert/delete/reset/reparent/table loss/
expiry, with unchanged terrain and no orphan operation on refusal. The real
API-ceiling/local-role-cap Node::set_terrain constructs a genuine fixture-signed
Manager API grant with durable metadata; it was not converted to Local to
preserve a passing demotion assertion. Simplified peer/MCP models remain
explicitly transport-only.

The chosen value object, small API facade and RAII lease are appropriate existing
architecture patterns. They keep the core points of interaction visible and
avoid a generic auth service, new persistence epoch, redundant builder or general
raw-query mechanism. The prepared SQL fence follows the repository's existing
composition idiom. No readability refactor is required to approve this bounded
change. The new directory notes state the limits and supersede the old grant
paragraph explicitly; invalid-UTF8 legacy guides are absent from the modified
manifest and were not bulk-rewritten.

## Required execution before closing QA-0070

- Compile the exactly integrated source and all affected callers/matchers;
  execute the existing integrated sweep under the once-at-end policy. No compiler,
  formatter or assertion result was produced in this lane.
- Execute both fixed direct and real serviced channel-only metadata scripts,
  slot decoding, required-table failure cases and rowless prepare-to-table-loss.
  Confirm terrain/ordinary-operation rollback together, strict signed expiry,
  original-scope movement refusal and known-to-rowless deletion refusal.
- Execute actual token-cap AND local-publisher demotion/Viewer cases, valid
  unchanged-scope success, generation/legacy/reset/revoke/insert/delete cases,
  bounded read ownership and full/closed/expired/late delivery. Measure retained
  queued/engine work, not just waiter counts. A ten-second write timeout never
  supplies replay or cancellation authority.
- Establish supported mutator ordering through the real serial DB worker and
  reset stop/join boundary. Sequential prepared-change assertions are not
  simultaneous engine isolation proof; direct/shared-handle concurrency claims
  need corresponding storage/catalog evidence. No unconditional credential writes
  were added to manufacture conflicts.
- Preserve the separate volume, retained-store startup, adjacent endpoint,
  native/UI/capture, CI/release and project-wide gates. This approval concerns
  only the exact narrow terrain grant source, not those outcomes.
