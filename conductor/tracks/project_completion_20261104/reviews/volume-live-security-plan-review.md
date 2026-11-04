---
type: Review
date: 2026-11-04
title: Volume live-plan security and terrain grant continuity review
source_artifact_sha256: 6d5d5aee6e43cf78238cd4ce46425d2992b3dfa2f2ba43f69221502ab47e9f00
---

# Volume live acceptance: independent security/source plan review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the bounded numeric/source-transition work on fresh, locally owned
disposable fixtures, with the plan's holds and evidence limits.** The stable-scope
ordinary Editor PUT is a legitimate setup/source-transition action. **The terrain
API's queued credential/scope continuity is a real source-supported security gap
and must be fixed before claiming that security acceptance or final completion.**
It is not merely an unusual actor label. No live bypass or exploit was executed.

Reviewed `.omc/research/qa-volume-live-acceptance-plan.md`, SHA256
`8a3e2d28ed3f98e6625d4cb1c9ad62adbd03b3f06a6a2a5bc68bcf18acadf04e`
(27,070 raw bytes), actual REST/MCP admission/core/runtime/DB code, API credential
contracts, replicated Petal confinement, background batch actor and startup Seed
dispatch. This is a security/source plan review; it does not execute or certify
the geometry, fixture or UI oracle.

## MED TERR-S1: queued terrain writes lose original credential and scope fences

The concrete production path is:

1. HTTP auth verifies the JWT and current credential generation/JTI/revocation/
   expiry. REST set/delete require Editor and DB-resolved token scope coverage.
   MCP's table dispatch likewise requires Editor and resolves/checks the Petal
   scope before its handler. These are real admission checks; a Viewer or an
   initially out-of-scope token does not pass this route.
2. Both routes call set_petal_terrain_core without claims or the admitted scope.
   It validates TerrainConfig/publisher pins, then queues SetPetalTerrain with
   Identified { did: api, role: editor }. Original token scope, exact admitted
   ancestry, subject, expiry, JTI and credential-generation ownership are absent.
3. Runtime forwards that command unchanged. Correlation controls which reply is
   delivered; it does not reconstruct discarded authorization.
4. DB prepare_terrain_write resolves the target's scope again. authorize_edit
   uses lifecycle_auth_context, which caps the supplied Editor role by the actual
   local writer's current role. prepare_scope_permission then captures and fences
   that local writer's authority and the newly resolved ancestry in the containing
   operation/row transaction. Those fences are useful and real, but concern the
   current local writer and current scope, not the lost original token grant.

Consequently, there is no use-time comparison with the scope admitted to the API
caller, and no use-time credential expiry/revocation/generation check for this
queued terrain request. Merely changing api to claims.sub would not repair those
missing constraints.

### Concrete reachable scope-change mechanism

The repository permits legitimate same-Verse Petal reparenting by an authorized
replicated writer: handlers/replicated_row.rs::confine_petal gathers both new and
durable old scopes when fractal_id differs, confines both to the same Verse and
requires the writer floor on both. It does not make Petal ancestry immutable.

A source-supported ordering is therefore:

- API admits Petal P under token-covered Fractal A.
- Before the resulting SetPetalTerrain reaches its DB use, an independently
  authorized queued Petal row moves P to Fractal B in the same Verse.
- SetPetalTerrain then resolves P under B. If the local publisher remains Editor+
  there, its new local authority/ancestry transaction can pass, even though the
  original token only covered A.

This does not require an initially out-of-scope request, a fabricated role, raw DB
SQL from the tester, or assuming the api string itself bypasses authorization.
It is a use-time grant/ancestry continuity gap between two valid source paths.
The same discarded credential metadata prevents this command from excluding a
credential revoked/expired after admission but before its transaction under the
campaign's required use-time contract. No concurrency or revocation case was run
by this review; the finding is source-supported rather than observed exploitation.

## Minimal corrective contract

Use a narrow trusted local-versus-API terrain grant, constructed from verified
REST/MCP admission rather than deserialized request JSON. Carry the authenticated
credential subject/issuer, token scope and role ceiling, exact admitted target
scope/ancestry, expiry and the existing durable credential/JTI-generation fence
needed for current-use validation. Keep request/reply correlation separate from
the grant. Preserve the actual runtime local publisher identity.

At actual DB use, freshly resolve the target, require exact admitted ancestry and
token coverage, retain the current local publisher Editor floor/transactional
authority fence, and fence relevant credential generation/revocation/expiry in
the same transaction as operation plus row. A role string alone or only another
API-layer check before queueing does not close the queue/use gap. Keep full typed
ancestry guards and shared terrain validation. GUI Local writes retain their
actual DB-local contract without fake token metadata.

Extend existing authorization/lifecycle journeys for admitted-then-reparented
targets, revoke/expiry/generation changes while queued and after preparation,
unchanged valid scoped Editor success, Viewer/foreign denial and exact correlated
failure. Do not replay an accepted unknown mutation to produce evidence. Review
the actual implementation before closing this finding or the corresponding
queued-authority acceptance gate.

### Do not invent an original human/peer actor

ApiClaims.sub is documented and minted as the issuing node's DID. The optional
client claim selects upload ownership only and is explicitly not an authorization
subject. A bearer request does not prove the human or external process holding
that token has its own DID role. The minimal fix should preserve the verified
credential grant and accurately label any recorded issuer/client provenance,
not resolve an invented remote caller role or relabel api as a historical actor.
Any additional operation audit fields must distinguish authenticated credential
provenance from actual local publication; absence of that audit field cannot be
filled in retrospectively by guessing.

## Permitted bounded volume execution

The plan's fresh-fixture shrink PUT can proceed for numerical/source-transition
acceptance under the existing endpoint contract when the disposable fixture is
actually locally owned, the current token is admitted as Editor+ for that exact
Petal, ancestry is kept stable and no credential/role/reparent race is being used
as a success case. Normal config validation, local writer authority, transactional
op/row write and correlated echo remain in force. This gives no queued-original-
grant or original-holder provenance credit and does not close TERR-S1.

Record that source edit as API setup, not GUI gold. Preserve all unrelated config
fields/region identity and independently read back the stored config. A timeout
may mean the write applied; inspect actual state and never resend automatically.
The same rules apply to an unchanged-source echo test. Do not insert numeric
volumes or fabricate source/report/identity facts to obtain zero/coverage results.

The background volume pair is a separate path: checked Read/Commit/Resolve use
the actual runtime local DID; batch manifest actor_did and journal source_did
record that actual actor. This is not evidence that the earlier terrain PUT
retained its initiating credential metadata. Keep full pair/history/source
readbacks, actual identities, unknown per-frame owner-witness limitations and
unexecuted rollback/late-reply/reset assertions explicit.

## Retained-store and lifecycle holds

The retained restart hold is correct. Main still dispatches DbCommand::Seed before
startup completion; launcher ResumeRun skipping fixture generation is not a
require-existing DB startup contract. Existing seed may skip a present Genesis,
but that does not prove no seed dispatch/write under an unexpected store state.
Do not start the retained campaign root until the reviewed live require-existing
implementation establishes its guarantee. No second offline store opener, fake
DID, fresh fixture copy, unsupported legacy reader or baseline rewrite is allowed.
Facts through version106 and the unavailable history prefix remain intact.

Fresh owned terrain fixtures, bounded observations, immutable failed attempts,
exact-run cleanup, private credentials and progress reporting are suitable for
the permitted numeric work. Five object terrain probes are not a whole-lattice
or epoch witness, API object arrival is not native geometry proof, conservative
world bounds are not exact mesh-solid volume, and screenshots alone do not prove
atomic visibility. The plan correctly reserves those separate evidence doors.
Real terminal reset remains a separate disposable-owned gate; invoking a UI
post-join callback in an authored journey does not execute an actual DB join.

## Disposition

Continue the plan's approved fresh-fixture numeric subset while keeping TERR-S1
explicit implementation/security work. Correct section7's classification from a
possibly cosmetic caller-marker followup to the concrete queued grant/ancestry/
credential gap described above. Fix and review it before claiming queued role,
scope or credential preservation and before final project completion/release.
No broader terrain API or remote-actor authority is approved by this plan review.

Only read-only plan/source/pinned contract inspection and this review artifact
were performed. No source or Git mutation, compiler/test, application,
OS/process/input/capture, API/DB/store or network operation occurred.
