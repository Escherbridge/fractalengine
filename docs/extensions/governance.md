---
type: reference
title: Extension governance contract
track: extension_platform_20261107
status: draft (P0.5; built in P2.2 / slice E10 and P2.3 / slice E10W; OD-E1 decided 2026-11-08, D2 applies the S-PLUGIN row)
---

# Extension governance

Who may run which extension, where, with what, and how it is stopped. Built on
plugin_host_revival (signing, trust roots, consent modal, revocation list, registry install) and
hexon_entitlements (`EntitlementVerifier`), never beside them (spec §6.4). Package bytes:
[package-format.md](package-format.md). Capabilities: [manifest-v2.md](manifest-v2.md).

Three facts shape everything below:

1. **Extensions write as the host's local DID** through the typed plugin bridge (spec §4.4,
   R22): no `ElevatedQuery`, no `RawQuery`, no caller SQL. Peers judge those rows exactly like
   that user's manual edits (role at write time, §write-time-role).
2. **Admission is per host.** Admission records, consent records and audit entries are local and
   never replicated. Only the per-verse policy row replicates.
3. **Every write is re-checked at use** by the DB: lease, named capability, pinned petal ancestry,
   and the local DID's role floor (`fe-database/src/handlers/plugin_host.rs::authorize_as`).

## Host switch (R16)

| Switch | Effect |
|---|---|
| plugin_host_revival OD-4's Cargo feature + runtime flag `FE_PLUGIN_HOST_ALLOW=1` (S1 fixed the name: `fe_plugin::PLUGIN_HOST_ENV`; exactly `"1"` opts in) | Off: no package is admitted, install answers `host-off`, the Extensions UI shows the off state. The plugin host thread still runs (it has since `2cc05f27`); the flag gates admission, not the thread. |
| `FE_EXT_DISABLE_ALL=1` at startup, or the local administrator's **Disable all** at runtime (the Extensions panel or `fe-ext admin`; never REST) | Every installation is `disabled` within one tick; activation answers `host-off` until it is cleared. |

Flipping the default to on is a later decision, after plugin_host_revival S6's security review.

## Local identity (R12)

Install, consent, bind (acceptance), unbind, enable, disable, update and uninstall are
administration verbs. They are a **GUI surface** (the Extensions panel), optionally driven from a
terminal by `fe-ext admin` through the **local admin channel** ([admin-channel.md](admin-channel.md)):
a named pipe or Unix socket bound to this OS user, never the network API. Both paths act as the
host's local DID, the identity the extension will write as (user decision 2026-11-09 06:30).

REST and MCP refuse every one of these verbs for every caller, local DID included: a remote
token holder must not be able to make the host write as the host, and a local token must not skip
the consent click. Every capability grant needs the operator's confirmation in the GUI consent
dialog. The local channel can raise that dialog; it can never answer it. Read-only status and audit
routes stay on REST and keep their own role checks.

For a REST or MCP caller this refusal runs before every other precondition, `host-off` included:
a remote DID always sees the uniform not-found, so it cannot learn whether this host admits
extensions (fe-api §existence-oracle; R12's AC holds whatever the host switch says). Before even
that, every extension route applies a remote caller's per-DID request rate (10/s, 429, not
audited), so denied calls cannot be turned into a flood of audit appends ([bus.md](bus.md#4-commands) check 0).

## Trust roots

`trusted_publishers.toml` (plugin_host_revival OD-3's file as S2 built it, `fe-plugin/src/trust_roots.rs`:
loaded at startup, a missing file is an error, no dynamic registration), one `[[publisher]]` table
per key:

```toml
[[publisher]]
key_id = "release-2026"           # free-form label, unique in the file (not a DID)
public_key = "<64 hex chars>"     # the ed25519 public key; weak (small-order) keys refused at load
revocation = false                # true: may sign revocation lists (and only then)
revocation_url = "https://…"      # OD-8 pull source; only meaningful with revocation = true
role = "publisher"                # NEW (E10, additive, default "publisher"): "release" | "publisher" | "dev"
```

- `release` roots sign first-party packages (release_ci_20260429). `dev` roots load only when the
  dev flag is set, and the dev flag is unreachable from release builds.
- A package is **trusted** when its publisher signature verifies and the ed25519 key of
  `publisher_did`, or of a verifying counter-signature's `did:key`, equals a loaded root's
  `public_key`. Roots match by key bytes, never by label ([package-format.md](package-format.md#signatures)).
- Only `revocation = true` roots may sign a revocation list. A key trusted to sign its own
  packages can never revoke someone else's.

## Policy row

One replicated row per verse (spec R18), written by Manager+ at verse scope (the `device_format`
precedent) and judged role-at-write on every replica like any row.

```json
{
  "verse_id": "01J…",
  "mode": "allow-list",
  "allow": [
    { "package_id": "org.example.fleet-sim", "content_hash": "9f2c…" },
    { "package_id": "org.example.device-adapter", "versions": ">=0.1.0, <0.2.0" }
  ],
  "deny": ["org.example.retired"],
  "ceilings": {
    "scene_write": true,
    "readings": true,
    "ingress": "declared",
    "egress": "loopback",
    "bus_external": false,
    "kinds": ["simulation", "device-adapter", "bi-connector", "general"]
  }
}
```

| Field | Values | Default (no row) |
|---|---|---|
| `mode` | `first-party-only` (trusted by a `release` root), `allow-list` (an `allow` entry matches by exact `content_hash` or SemVer range, AND the package is trusted), `trust-root` (trusted by any loaded root) | `first-party-only` |
| `allow` | entries as above; `[]` is an explicit lockdown (nothing matches) | `[]` |
| `deny` | refused in every mode; each entry is a package id, or a 64-hex digest matched like a revocation entry (`content_hash`, `component_digest` or `plaintext_digest`, under any id), so a renamed copy of a denied build stays denied | `[]` |
| `ceilings.scene_write`, `.readings` | `false` refuses packages declaring scene writes / readings | `true` |
| `ceilings.ingress`, `.egress` | `none` < `loopback` < `declared` | `declared` |
| `ceilings.bus_external` | `false` makes every `external: true` stream and command local-only in this verse | `false` |
| `ceilings.kinds` | admitted manifest kinds | all four extension kinds |

A policy never admits an untrusted signer: `allow-list` matches still need a trust root.

**Local policy.** `extensions.toml` beside the trust roots carries the host's own `deny` list and
`ceilings` (same fields). A package must pass BOTH the verse policy and the local policy: denies
union, ceilings intersect (the S-RELAY-ADVERTISE pattern: a verse advertisement under local policy).

**Policy changes.** A policy row change (local write or replicated arrival) re-evaluates every
binding in that verse. A binding the new policy no longer admits deactivates with reason
`policy-denied`, its admission record moves to `policy-denied`, and `host/admission` announces it,
on every peer, within one tick of the row's arrival.

## Activation preconditions

Checked in this order at install, bind, enable, every activation (including restarts and updates)
and on every policy, trust-root or revocation-list change. The first failure is the refusal (R19).

| # | Code | Check | Suggested status (read-only routes; the GUI and the local channel show the code) |
|---|---|---|---|
| 1 | `host-off` | the host switch is on and Disable all is clear | 503 |
| 2 | `not-local-identity` | the actor is the host's local DID (R12) | 404 (uniform) |
| 3 | `package-invalid` | seal, integrity, identity equality (the requested id = the signed `hexon_id` = the manifest v2 `id`; reason `id-mismatch`), component checks, manifest v2 validation (for `kind: web-bundle`, the portal validator, `docs/spec/portal-bundle/README.md` §6, replaces the extension rules) | 422 |
| 4 | `untrusted-signer` | the package is trusted (signed and rooted; dev flag rules) | 422 |
| 5 | `revoked` | no accepted revocation entry's `content_hash` equals the package's `content_hash`, `component_digest` or (licensed, body phase) `plaintext_digest`, whatever `package_id` the entry names ([package-format.md](package-format.md#identity)), unless the verse Owner recorded a [kill-list override](#kill-list-override) for this verse and this build on this host | 409 |
| 6 | `entitlement-required` | a `paid` licence has a valid token through S-ENTITLEMENT's `EntitlementVerifier` (no new verifier code) | 403 |
| 7 | `policy-denied` | the verse policy and the local policy both admit it, within ceilings | 403 |
| 8 | `role-denied` | the local DID holds Editor+ at the petal for a writing extension (one that declares `scene.write` or `readings`), Viewer+ for any other | 403 |
| 9 | `consent-missing` | a consent record exists for this exact capability digest | 409 |

For a non-local actor, row 2 is evaluated first (§Local identity).

After activation every flush is re-checked at use (fact 3). A demotion does not stop a running
instance: its flushes are refused at use and counted as `refusals.role` until the role returns
(R22). An instance stops with `role-lost` only when the identity can no longer read the petal.

## Consent

A local record per installation, never replicated (R20; plugin_host_revival S3's modal, created at
`fe-ui/src/extensions/consent.rs`):

```json
{
  "installation_id": "01J…",
  "package_id": "org.example.fleet-sim",
  "content_hash": "9f2c…",
  "capability_digest": "b71e…",
  "actor_did": "did:key:z6Mk…",
  "at": "2026-11-08T19:50:00Z"
}
```

- `capability_digest` is defined in [manifest-v2.md](manifest-v2.md#capability-digest); for
  `kind: web-bundle` it is the portal contract's digest (`docs/spec/portal-bundle/README.md` §7.2),
  and the same dialog renders the `portal` section (portal A-10).
- The modal renders the binding, scene scopes, readings class, every ingress and egress endpoint
  (with credential slots), bus exposure, consumption and calls, UI contributions, signer and trust
  state, the policy verdict and the licence state. Enable stays disabled until that list has
  rendered once (S3 AC-5).
- **Updates.** Same digest: no prompt. Different digest: re-prompt, and the old version keeps
  running until consent is given.
- **Consent is GUI-only.** There is no REST consent. The dialog records the digest it displayed, and
  a stale digest is refused with `consent-mismatch` wherever it comes from (the panel, or the local
  channel's request for the dialog). A headless host cannot consent: its channel is off
  ([admin-channel.md](admin-channel.md) §1).
- **Bind** is a separate local-DID action per petal; for a `source_class` list it names exactly
  one class, recorded with the binding.
- **Revoking consent** deactivates every instance of the installation (`ended` reason
  `consent-revoked` on its bus subscriptions) and sets the admission record to `disabled`.

## Admission record

Local, keyed by `(package_id, content_hash)`, never replicated; a row or blob arriving from a peer
cannot change it (D-X5). Web bundles (portal_surface_20261107) use the same record.

| State | Meaning |
|---|---|
| `admitted` | passed preconditions 1-9 for at least one binding; may run |
| `disabled` | stopped locally (Disable, Disable all, consent revoked) |
| `policy-denied` | the verse or local policy no longer admits it |
| `revoked` | named on an accepted revocation list |
| `removed` | uninstalled |

Every change publishes one `host/admission` event ([bus.md](bus.md#24-hostadmission)) whose closed
`reason` names the cause (the kill-switch layer, a ceiling, a role loss), so the record keeps that
reason beside its state. The portal track maps these states to its own (`Admitted`,
`StaticSandbox`, `Blocked`, `Revoked`).

## Kill switch layers (R23)

| Layer | Who | Scope | Running instances | Latency |
|---|---|---|---|---|
| (a) Disable | local DID | one installation or one binding | stop (`disabled`); drain; one final flush | next tick |
| (b) Disable all / `FE_EXT_DISABLE_ALL=1` | local DID / operator | the whole host | stop (`disabled`); drain; one final flush | next tick |
| (c) Policy removal | verse Manager+ | that verse, every peer | stop (`policy-denied`); drain; no final flush | one tick after the row arrives |
| (d) Revocation list | a `revocation = true` root | every host trusting that root | stop (`revoked`); drain; no final flush; then the lease is revoked | one OD-8 pull interval |

**Stop order (every layer, OD-E1).** 1. The instance leaves the bus: queued calls answer
`not-available`, subscriptions end with the layer's reason, no new command or subscription is
accepted. 2. No further tick runs. 3. **Drain:** the one commit already in flight (one per instance,
R4) is awaited, bounded by the host's final-flush wait, while the lease is still live, so it lands
whole with its true receipt. A commit with no answer by then is counted `unknown` and is never
retried; once the lease is revoked in step 5 a commit still queued is refused at the DB's use
check, so nothing lands partially. 4. Staged writes get one final flush for (a) and (b); for (c)
and (d) they are discarded. 5. The lease is revoked and the context unbound. Rows already written
stay (§Provenance).

**Revocation list** (plugin_host_revival §4, a public wire contract, `schema_version: 1`):
`{schema_version, issued_at, entries: [{package_id, content_hash, reason, revoked_at}],
signer_key_id, signature}`, ed25519 over the canonical JSON of `entries` + `issued_at`, pulled from
each root's `revocation_url`, the last good list cached. An entry matches by hash under ANY
package id (its `content_hash` against a package's `content_hash`, `component_digest` or licensed
`plaintext_digest`; orchestrator steer 2026-11-08 20:10), so a renamed, re-sealed or re-encrypted
copy of a revoked build is still revoked. The list signs `entries` and `issued_at` only;
`schema_version` is checked before verification and `signer_key_id` selects the root, so neither
needs signing. When a
host accepts a list matching an installed package, it refuses activation AND stops every running
instance within one poll, in the stop order above: the in-flight commit drains, then the lease is
revoked. Revocation is stop, not rollback and not withdrawal: what the build wrote stays (OD-E1).

### Kill-list override

User decision 2026-11-08 (OD-E1): a kill-listed build is refused by default, at install and at every
activation. The verse Owner may override that, for one verse, on the Owner's own host only.

- **Who.** The local DID must be the verse's `owner_did` (`verse.created_by`). The request comes over
  the local-identity surface (`POST /api/v1/extensions/kill-list-overrides`, R12) with a typed
  confirmation: `confirm` must equal the `package_id` exactly, or the answer is `confirmation-mismatch`
  (409). Any other local DID gets `role-denied` (403). Both refusals are audited.
- **Key.** `(verse_id, content_hash)` on that host. With it, row 5 passes there for that build; on every
  other host, and in every other verse, the build stays `revoked` and a binding naming it stays unresolved.
- **Arrival.** An accepted list does not stop an instance the Owner overrode in its verse; every other
  instance of the build stops as above.
- **Record.** One `ExtensionAudit` entry, action `kill-list-override`, through `op_log::commit_operation`.
  The override is that entry: the host rebuilds its override set from the trail at startup. It is never
  replicated, and nothing can make it reach another verse or host. The op log's canonical projection is
  dormant, so the entry is local today.
- **No other override.** Consent, local acceptance and policy can never make a kill-listed build pass row 5.

## Audit

One new op-log type, `OpType::ExtensionAudit`, appended through `op_log::commit_operation` (the
live log-first path, fe-database `src/AGENTS.md` §log-first-commit). Local only (OD-E7): E10 must
keep it out of verse replication. Readable at `GET /api/v1/extensions/{installation}/audit` and in
the Extensions UI.

```json
{
  "action": "command-call",
  "installation": "01J…",
  "package_id": "org.example.fleet-sim",
  "content_hash": "9f2c…",
  "actor_did": "did:key:z6Mk…",
  "target": "org.example.fleet-sim/start@01JPETAL…",
  "outcome": "ok",
  "reason": null,
  "args_digest": "4be0…"
}
```

`action` ∈ `install`, `update`, `consent`, `consent-revoke`, `bind`, `unbind`, `enable`, `disable`,
`uninstall`, `revocation`, `policy-change`, `bus-subscribe`, `bus-unsubscribe`, `command-call`,
`denial`, `kill-list-override`. `outcome` ∈ `ok`, `refused`, `denied`. Disable all and Enable all are
`disable` and `enable` with `target: "*"`; local acceptance in a verse is `bind` with `verse_id` set. Each listed action yields exactly one entry,
denials included (R21 AC). Stream events and query re-runs are not audited ([bus.md](bus.md#7-audit-and-counters)).

- **Cost.** Each entry is one `commit_operation` on the DB thread, the queue whose starvation
  spec §4.10 measured. What bounds it: remote callers are rate-limited per DID before any lookup
  (429, not an action, not audited), commands are rate-limited per caller, and nothing per-event is
  audited. P6 measures audit appends next to `slow DB command` lines.
- **Replay.** The entry is its own fact, not a mutation that "failed after append": the
  precondition-before-append rule (§log-first-commit) governs the audited action's own operation,
  while a `denial` entry records the refusal. Its materializer touches no scene row and is a no-op
  on replay.

## Provenance (R24: stamp and stop, OD-E1 decided 2026-11-08)

User decision 2026-11-08: a revoked build stops after its in-flight writes complete, and the rows it
wrote stay. R24 is therefore stamp-and-stop: no withdrawal, no re-judge, no readings exclusion.

Every scene version and reading written through the extension path carries a host-stamped
`{package_id, content_hash}`: the verified package's identity, minted into the `PluginAdmission`
at activation (`fe_runtime::plugin_host::PackageIdentity`) and copied onto rows by the DB handler.
The guest never names either value, so no WIT call can set or strip it.

| Row | Where the stamp lives | Replicates |
|---|---|---|
| Scene version | the journal fact, `node_log.payload.ext = {package_id, content_hash}`, beside the fact's `hlc_timestamp` (its write clock); the op-log `PluginTransaction` intention carries the same pair as `payload.package` | no: `node_log` and `op_log` are local |
| Reading | additive `option<string>` columns `ext_package_id`, `ext_content_hash` on `iot_reading`, omitted when unstamped (the `payload_id` precedent) | yes, with the row |

- **Why scene provenance stays local.** The spec put it in the replicated `_fe_scene` marker so
  that peers could re-judge withdrawn versions. With no withdrawal, no peer reads it. And the
  marker is strict (exactly four keys): an added key would make every peer deny extension-written
  snapshots. The authoring host's journal is the audit record (OD-E7: audit is local).
- **Revocation is stop, not withdrawal.** A host that accepts a list naming an installed build
  stops its instances in the [stop order](#kill-switch-layers-r23). Nothing is re-judged, restored,
  removed or hidden, on any peer. Readings stay visible at the scope-substitution seam, in
  `/api/v1/query` and in the distributed partials.
- **Filtering.** The stamp is a filter key, not a visibility switch. Readings filter by
  `ext_package_id` / `ext_content_hash` in a guarded query on any upgraded peer; scene versions
  filter through the authoring host's journal. A user who wants a revoked build's rows gone
  deletes them like any rows, under their own role.
- **Inbound shape.** A peer denies a replicated reading that carries only one of the two columns,
  or a pair `PackageIdentity::new` would refuse (strictest reader). A stamp from a peer is that
  author's attestation, judged role-at-write like the rest of the row.
- **Upgrade.** An older peer's SCHEMAFULL `iot_reading` lacks the `ext_*` columns, so it fails to
  create a stamped reading (logged, not applied). Peers upgrade together, as with `payload_id`.
- **Not yet covered.** The window-membership door digest (fe-database `canon_log` §observation)
  does not include the stamp; adding it is a canonical-log contract change, deferred.
- **Portal writes** (portal_surface_20261107 `ipc-v1.md` A-13): an admitted web bundle has an
  installation but no instance, so its `act set_property` rides the same `DbCommand::PluginCommit`
  seam as a one-op commit outside any tick, under the bundle's own host-minted admission (same DB
  re-checks, provenance `{package_id, content_hash}` of the bundle). It never enters an extension
  instance's window, and the portal's `set_property` budget (2/s) bounds its DB-thread cost.

## OD-E1: decided, stop and keep (S-PLUGIN amendment)

User decision 2026-11-08: the kill switch does NOT withdraw rows. A revoked build is refused
activation and its running instances stop after their in-flight writes complete; the rows they
wrote stay, stamped for audit and filtering. Slice D2 applies the amended row below to
`conductor/open-core.md` as a versioned seam change. Slice D1 (the withdrawal variant) does not run.
The row also records D-X1's package shape and the two identity rules from the 2026-11-08 20:10
security steer, which stand regardless of OD-E1.

**Current row** (`conductor/open-core.md`):

> Publisher-signed plugin packages (ed25519 over manifest + payload hash), configurable trust
> roots, a signed revocation list. The list is a package-level kill switch keyed by
> `(package_id, content_hash)` for withdrawn or malicious packages, accepted only from trust roots
> marked for revocation; it never revokes a licence.

**Amended row (S-PLUGIN v2):**

> Publisher-signed plugin packages (a sealed `.hexon` of type `plugin`: ed25519 over the canonical
> manifest, which commits every member and the package id; optional counter-signatures over the
> same bytes; the id an install asks for must equal the signed id), configurable trust roots, a
> signed revocation list. The list is a package-level kill switch whose entries
> `(package_id, content_hash)` match by hash under any package id, for withdrawn or malicious
> packages, accepted only from trust roots marked for revocation; it never revokes a licence. A
> host that accepts a list naming an installed build refuses its activation (unless the verse
> Owner recorded an audited override for that verse on the Owner's own host) and stops its running
> instances after their in-flight writes complete. Rows they wrote stay, carrying host-stamped
> provenance `(package_id, content_hash)` for audit and filtering.

What changed from the proposal: the withdrawal sentences (re-judge, restore or remove, readings
excluded from reads, reversible on trust-root removal) are gone; the drain, the stamp and the
Owner override are in. In this track R24 shrinks to stamp-and-stop: §Provenance above, slice E10W.

## Journeys

- **J6** (P1.6 creates `fe-api/tests/extensions_api_test.rs`; P2.2 extends): drive each
  precondition refusal above through the local admin channel (or the GUI), in order, and assert the
  reason; the other surface shows the same text. REST and MCP are asserted to refuse every
  administration verb uniformly. Then the audit trail holds exactly one entry per action, denials included. Same table:
  a remote DID's install on a host-off host answers the uniform not-found (not `host-off`); its
  11th call inside one second answers 429 and adds no audit entry; a verse `deny` naming a build's
  `component_digest` refuses that build re-sealed under a new id with `policy-denied`.
- **J7 stamp-and-stop** (P2.3, OD-E1):
  - `fe-database/src/op_log.rs` plugin block: a sealed-package batch writes a journal fact whose
    `payload.ext` is the admission's pair, beside a positive write clock. After the lease is
    revoked, the next commit is refused with no op-log row, and the row and its stamped fact stay.
  - `fe-api/tests/iot_ingest_test.rs::guarded_query_reads_iot_rows_scope_filtered`: a stamped
    extension reading reads back through the guarded query beside an unstamped REST-door reading.
    Nothing is excluded.
  - `replicated_row` timeseries journey: a stamped reading joins the union with its stamp; a
    half stamp is denied.
  - `fe-plugin/src/tick.rs` J1: a `revoked` stop with a flush in flight settles that flush
    before the lease goes and submits nothing new.
