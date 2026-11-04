# Terrain API grant and read ownership (QA-0070)

terrain_grant owns required Local/Api SetPetalTerrain admission and one fixed
credential-read DTO. API grant fields are private, nonserialized and Debug
redacted, with configured Ed25519 issuer binding; this remains trusted in-process
construction. Admitted ancestry, scope/ceiling/expiry and original generation/token
capture travel to DB use. Local producers explicitly choose Local. Client upload
ownership is not an authorization subject and the DB publisher stays independent.

The private read uses a capped map in PendingApiRequests, separate from generic
QueryResult and wildcard-error ownership. Exact Arc lease/request/issuer/JTI bind
delivery. Closed/expired entries are pruned on enqueue, delivery and draining.
API enqueue and DB forwarding use try_send so a full bounded channel cannot defeat
the five-second deadline. Eight permits shared per API host remain attached to
queued/actual engine reads after caller abandonment. Expiry refuses new work and
result use; it does not cancel an already submitted engine read. The direct API
read task likewise retains its permit until actual query completion. No waiter
pruning claim is engine cancellation or an engine-wide INFO memory bound.

The terrain mutation reply family and accepted-unknown/no-replay semantics remain
unchanged. A new private credential result cannot complete a mutation waiter,
and generic RequestFailed/Error/QueryResult cannot manufacture an API grant.
An abandoned capture never resumes to enqueue a write. Public Rust constructors
are trusted host boundaries, not cryptographic isolation from host code.

Existing runtime correlation/dispatcher journeys exercise foreign-family and
foreign-owner refusal, closed-entry pruning, retained engine ownership and full
DB-forwarding rejection. They are authored until the integrated sweep executes.
