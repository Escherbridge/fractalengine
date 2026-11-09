---
type: reference
title: Extension governance contract
track: extension_platform_20261107
status: draft (P0.5; built in P2.2 / slice E10; D1 finalizes after OD-E1)
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
| plugin_host_revival OD-4's Cargo feature + runtime flag `FE_EXT_ALLOW=1` (name is S1's to fix) | Off: no package is admitted, install answers `host-off`, the Extensions UI shows the off state. The plugin host thread still runs (it has since `2cc05f27`); the flag gates admission, not the thread. |
| `FE_EXT_DISABLE_ALL=1` at startup, or the local administrator's **Disable all** at runtime (REST + Extensions UI, slices E6/E13) | Every installation is `disabled` within one tick; activation answers `host-off` until it is cleared. |

Flipping the default to on is a later decision, after plugin_host_revival S6's security review.

## Local identity (R12)

Install, consent, bind, unbind, enable, disable, update and uninstall are accepted only from the
host's local DID — the identity the extension will write as — over REST or the GUI. Any other DID
gets the uniform not-found and one `denial` audit entry. Otherwise a remote token holder could make
the host write as the host.

## Trust roots

`trusted_publishers.toml` (plugin_host_revival OD-3's file, loaded at startup, no dynamic
registration), one entry per key:

```toml
[[root]]
key_id = "did:key:z6Mk…"          # ed25519 did:key; matches publisher_did or a counter-signature key_id
role = "publisher"                # "release" (first-party, the release pipeline's key) | "publisher" | "dev"
revocation = false                # true: may sign revocation lists (and only then)
revocation_url = "https://…"      # OD-8 pull source; only with revocation = true
```

- `release` roots sign first-party packages (release_ci_20260429). `dev` roots load only when the
  dev flag is set, and the dev flag is unreachable from release builds.
- A package is **trusted** when its publisher signature verifies and `publisher_did`, or the
  `key_id` of a verifying counter-signature, is a loaded root ([package-format.md](package-format.md#signatures)).
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
| `deny` | package ids refused in every mode | `[]` |
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

| # | Code | Check | Suggested REST status |
|---|---|---|---|
| 1 | `host-off` | the host switch is on and Disable all is clear | 503 |
| 2 | `not-local-identity` | the actor is the host's local DID (R12) | 404 (uniform) |
| 3 | `package-invalid` | seal, integrity, identity equality (the requested id = the signed `hexon_id` = the manifest v2 `id`; reason `id-mismatch`), component checks, manifest v2 validation | 422 |
| 4 | `untrusted-signer` | the package is trusted (signed and rooted; dev flag rules) | 422 |
| 5 | `revoked` | no accepted revocation entry's `content_hash` equals the package's `content_hash` or `component_digest`, whatever `package_id` the entry names ([package-format.md](package-format.md#identity)) | 409 |
| 6 | `entitlement-required` | a `paid` licence has a valid token through S-ENTITLEMENT's `EntitlementVerifier` (no new verifier code) | 403 |
| 7 | `policy-denied` | the verse policy and the local policy both admit it, within ceilings | 403 |
| 8 | `role-denied` | the local DID holds Editor+ at the petal for a writing extension, Viewer+ for a read-only one | 403 |
| 9 | `consent-missing` | a consent record exists for this exact capability digest | 409 |

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

- `capability_digest` is defined in [manifest-v2.md](manifest-v2.md#capability-digest).
- The modal renders the binding, scene scopes, readings class, every ingress and egress endpoint
  (with credential slots), bus exposure, consumption and calls, UI contributions, signer and trust
  state, the policy verdict and the licence state. Enable stays disabled until that list has
  rendered once (S3 AC-5).
- **Updates.** Same digest: no prompt. Different digest: re-prompt, and the old version keeps
  running until consent is given.
- **REST consent** (headless hosts, QA) must echo the exact digest; a stale digest is refused with
  `consent-mismatch`.
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

Every change publishes one `host/admission` event ([bus.md](bus.md#24-hostadmission)). The portal
track maps these states to its own (`Admitted`, `StaticSandbox`, `Blocked`, `Revoked`).

## Kill switch layers (R23)

| Layer | Who | Scope | Running instances | Latency |
|---|---|---|---|---|
| (a) Disable | local DID | one installation or one binding | stop (`disabled`); one final flush | next tick |
| (b) Disable all / `FE_EXT_DISABLE_ALL=1` | local DID / operator | the whole host | stop (`disabled`) | next tick |
| (c) Policy removal | verse Manager+ | that verse, every peer | stop (`policy-denied`); no final flush | one tick after the row arrives |
| (d) Revocation list | a `revocation = true` root | every host trusting that root | stop (`revoked`); lease revoked; no final flush | one OD-8 pull interval |

**Revocation list** (plugin_host_revival §4, a public wire contract, `schema_version: 1`):
`{schema_version, issued_at, entries: [{package_id, content_hash, reason, revoked_at}],
signer_key_id, signature}`, ed25519 over the canonical JSON of `entries` + `issued_at`, pulled from
each root's `revocation_url`, the last good list cached. An entry matches by hash under ANY
package id (its `content_hash` against a package's `content_hash` or `component_digest`; orchestrator
steer 2026-11-08 20:10), so a renamed or re-sealed copy of a revoked build is still revoked. When a
host accepts a list matching an installed package, it refuses activation AND revokes the lease of
every running instance within one poll. A commit already admitted completes with its true receipt:
revocation is not rollback.

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
`denial`. `outcome` ∈ `ok`, `refused`, `denied`. Each listed action yields exactly one entry,
denials included (R21 AC). Stream events and query re-runs are not audited ([bus.md](bus.md#7-audit-and-counters)).

## Provenance (R24, waits for OD-E1)

Every scene version and reading written through the extension path carries a host-stamped
`{package_id, content_hash}`. The PluginCommit handler stamps it from the host-minted admission
(slice E4 adds the package identity to `PluginAdmission`; stamping lands with E10W). No WIT call
can set or strip it.

- **Scene versions:** in the versioned snapshot, beside the version's write clock.
- **Readings:** additive optional `iot_reading` fields `ext_package_id`, `ext_content_hash`,
  omitted when absent (the `payload_id` precedent).
- **Withdrawal:** a host that accepts a revocation list re-judges the affected verses (one batch
  per verse, §rejudge-on-demotion discipline). Versions with revoked provenance are inadmissible:
  the newest admissible version is restored, or the node is removed. Readings with revoked
  provenance are excluded at the scope-substitution seam and on the distributed partial path.
- **Reversible:** removing the trust root restores visibility; nothing is physically deleted (OD-E9).

## OD-E1: proposed S-PLUGIN amendment (not applied)

Handed to the orchestrator for ratification with the user; slice D1 applies it to
`conductor/open-core.md` only if OD-E1 is ratified. It also records D-X1's package shape, which
changes the row's "manifest + payload hash" wording regardless of OD-E1.

**Current row** (`conductor/open-core.md`):

> Publisher-signed plugin packages (ed25519 over manifest + payload hash), configurable trust
> roots, a signed revocation list. The list is a package-level kill switch keyed by
> `(package_id, content_hash)` for withdrawn or malicious packages, accepted only from trust roots
> marked for revocation; it never revokes a licence.

**Proposed row (S-PLUGIN v2):**

> Publisher-signed plugin packages (a sealed `.hexon` of type `plugin`: ed25519 over the canonical
> manifest, which commits every member; optional counter-signatures over the same bytes),
> configurable trust roots, a signed revocation list. The list is a package-level kill switch keyed
> by `(package_id, content_hash)` for withdrawn or malicious packages, accepted only from trust
> roots marked for revocation; it never revokes a licence. A host that accepts a list naming an
> installed pair refuses activation, stops running instances, and withdraws what they wrote: scene
> versions and readings written through the extension path carry host-stamped provenance
> `(package_id, content_hash)`, and versions with revoked provenance are inadmissible on re-judge
> (the newest admissible version is restored or the node removed; readings are excluded from
> reads). Withdrawal is reversible: removing the trust root restores them; nothing is physically
> deleted.

If OD-E1 is refused, keep the current row with only the package-shape wording updated, and R24
shrinks to stop-and-refuse.

## Journeys

- **J6** (P1.6 creates `fe-api/tests/extensions_api_test.rs`; P2.2 extends): drive each
  precondition refusal above through REST, in order, and assert the reason; the GUI shows the same
  text. Then the audit trail holds exactly one entry per action, denials included.
- **J7** (P2.3, the op_log re-judge block): seed an extension-provenance version over a manual
  baseline; accept a revocation list (baseline restored, the extension-created node removed, a
  manual property edit survives); remove the trust root (the withdrawn versions return).
