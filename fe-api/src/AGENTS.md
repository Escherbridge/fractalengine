# API source boundaries

## Terrain credential continuity (QA-0070)

This section supersedes the legacy CallerAuth description for SetPetalTerrain
in the parent directory guide. REST retains its admitted full scope; MCP retains
ToolCall.scope. The private terrain_credential builder binds claims.sub to the
canonical DID derived from the actual configured Ed25519 verifier, independently
of DB publisher identity. Client selects upload ownership only. A required
Local/Api TerrainWriteGrant replaces this command's CallerAuth; API JSON or
absent correlation cannot select Local. Grant fields and snapshots are
nonserialized and Debug redacted. This is trusted in-process construction, not
cryptographic isolation from arbitrary host Rust.

Direct ReadDb and the one typed ReadTerrainApiCredential worker door share
api_auth's fixed metadata query/decoder. No RawQuery, writable facade, missing
generation or Local fallback exists. Capture has one five-second deadline
including permit wait, API enqueue, DB forwarding and result wait, with try_send
refusal at bounded enqueue/forwarding. Eight per-host permits stay owned by queued
or actual engine reads after caller timeout; abandonment is not engine cancellation.
The direct owned task retains its lease through actual query completion. Private
pending read entries are capped, pruned and matched by exact family/lease/request/
issuer/JTI; generic failures/rows cannot complete capture. Late reads cannot revive
an abandoned future and enqueue a terrain write. The 8 KiB projected snapshot bound
does not bound INFO's internal catalog enumeration or allocation.

The immutable grant retains admitted token projection/generation, exact ancestry,
issuer, signed scope/ceiling/expiry/JTI. DB preparation compares those against
fresh authority; the containing operation/row transaction fences required tables,
original generation/token projection, strict expiry and actual local publisher
Editor/ancestry/default-access. Known-row deletion cannot silently become a
compatible rowless credential. Current-generation rowless dev credentials and
literal-legacy compatibility remain; no invented individual revoke semantics or
remote bearer-holder principal is added. Already enqueued mutation timeouts
remain unknown outcomes; no automatic replay or retroactive cancellation.

Existing router/channel/prepared journeys are extended without new test
declarations. The simple MCP model exercises transport, not real durable guards;
real-store terrain and API publisher-cap journeys own those assertions.

## §portal-bundles — export route (portal_surface_20261107 T4.2 / S8)

`GET /api/v1/nodes/{node_id}/portal/export.zip` (mounted beside the portal-bundles routes in `server.rs`) calls
`portal_bundles::export_portal_bundle`: Viewer+ with node reach, one uniform 404 for unknown/unreachable/not-a-repository
nodes (no existence oracle). The archive is written with the `zip` crate (deflate only, no default features) and carries
`fe-portal.json` (`fe-portal-export/1`) plus the bundle members with the in-archive prefix stripped.

- **Double revocation check.** The stored bundle is inspected, then revocation/kill-list is re-checked with the full build
  facts (`revoked_on_this_peer`, matched by hash under any id) immediately before writing; a revoked build is never exported.
- **CSP composer.** `static_csp` rebuilds the policy from the manifest's `PortalCsp` plus admitted feed origins
  (`admit_export_feeds`: https only, `is_csp_source` charset, max 16 feeds); any source failing the check refuses the export
  rather than being dropped, so the exported CSP is never weaker than declared.
- **Seal.** Member reads verify the seal (`VerifyIfSealed`); a tampered member is a refusal.
