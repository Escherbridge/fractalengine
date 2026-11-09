---
type: reference
title: Scoped extension bindings
track: extension_platform_20261107
status: draft (contract for the bindings turn after the current swarm; user decisions 2026-11-08 22:00)
consumers: [extension_platform_20261107, portal_surface_20261107]
---

# Scoped extension bindings

A **binding** says which installed extension serves a **hook point** (a built-in seam such as a
petal's map, a petal's extra API routes, or a device format's decoder) at a verse, fractal or
petal. Bindings are replicated rows written by a Manager+ at their scope, and the nearest scope
wins. Every peer resolves them for itself: a peer that lacks the bound extension, or may not run
it, keeps the built-in behaviour, says so, and offers the install. A binding grants nothing. It
only chooses among what a peer has installed, consented to and accepted.

Contracts this builds on: [manifest-v2.md](manifest-v2.md) (`provides`, V17-V18),
[governance.md](governance.md) (consent, policy, kill switches, audit), [bus.md](bus.md) §12
(how bindings meet the broker), [package-format.md](package-format.md) (identity, kill-list
matching).

## 1. Decisions implemented (user, 2026-11-08 22:00)

- A scope (verse / fractal / petal) binds an installed extension to a hook point. Nearest scope
  wins: petal > fractal > verse.
- A Manager+ at that scope writes bindings, by subtree authority (role propagation R7). Bindings
  are replicated rows judged by the author's role at write time.
- Resolution is per peer. A peer missing the extension, or forbidden by policy, keeps the
  built-in behaviour, shows an unresolved state naming the extension, and is offered an install
  through the normal consent dialog under the verse install policy. Nothing auto-installs.
- OD-E1: no withdrawal on revocation (stop after in-flight writes complete; rows stay). A
  kill-listed package is refused at install by default; the verse Owner may override, audited,
  for the Owner's own peer. A kill-listed bound extension resolves as unresolved and never runs.

## 2. Terms

| Term | Meaning |
|---|---|
| Hook point (hook) | A built-in seam an extension can serve, named `<family>.<point>` (§6). Not the WIT `*-hooks` exports, which are how the host delivers calls into a guest. |
| Key | Tells apart several points of one keyed hook: a device format id for `ingest.decode`, a MIME type for `asset.import`, a name for `api.routes`. `""` for a keyless hook. |
| Point | The pair `(hook, key)`. |
| Scope | A verse, fractal or petal, as fe-policy's scope string `VERSE#v[-FRACTAL#f[-PETAL#p]]` (`fe-policy/src/scope.rs:68`). |
| Binding | The replicated row selecting what serves one point at one scope (§3). |
| Resolution | One peer's answer for one point at one petal: `resolved`, `builtin`, `elsewhere` or `unresolved` with a reason (§5). |
| Instance | Unchanged: one installation active at one petal on one host (OD-E3, [bus.md](bus.md#1-addressing) §1). Bindings start instances. They never widen one. |
| Local acceptance | This host's local-DID record that an installation may run in a verse (§5.3). Never replicated. |

## 3. The binding record

**Doc key:** `binding/{scope}/{hook}/{key}/{author_did}`. There is one live version per (scope,
point, author), the role-statement shape (`role/{scope}/{peer}/{grantor}`, fe-database
`src/AGENTS.md` §role-propagation). `{key}` is percent-encoded (every byte outside
`[a-z0-9._+-]`); a keyless hook writes `_`.

```json
{
  "schema_version": 1,
  "scope": "VERSE#01JV…-FRACTAL#01JF…",
  "hook": "petal.map",
  "key": "",
  "mode": "extension",
  "package_id": "org.example.city-map",
  "version_req": ">=1.2.0, <2.0.0",
  "config": { "district": "harbour" },
  "runs_on": null,
  "author_did": "did:key:z6Mk…",
  "written_at": "1762631400000000123"
}
```

| Field | Rule | Meaning |
|---|---|---|
| `schema_version` | `1` | Record shape version. |
| `scope` | a scope string in this verse | Where the binding applies: this scope and everything below it. |
| `hook` | `^[a-z][a-z0-9-]{0,31}\.[a-z][a-z0-9-]{0,31}$` | A hook id. An id this build does not know is stored and resolves `unknown-hook`. |
| `key` | `""`, or `^[a-z0-9][a-z0-9._+@/-]{0,127}$` | The point's key. Keyless hooks take `""`. Format ids and MIME types are written lowercase. |
| `mode` | `extension` \| `builtin` \| `inherit` | `extension`: the package serves the point. `builtin`: the built-in serves it here, which masks outer bindings. `inherit`: cleared, as if no row existed. |
| `package_id` | manifest v2 `id` grammar; `extension` only | The package. Never an installation id or a content hash: the row names what, each peer decides whether. |
| `version_req` | Cargo SemVer requirement; `extension` only; absent = any | The builds a peer may use. |
| `config` | JSON object ≤ 16 KiB; `{}` when absent | Input to the serving implementation. For `extension`, validated per peer against `provides[].config_schema` (§5.2 row 9). For `builtin`, only hooks whose built-in takes config accept one (§6). |
| `runs_on` | `null` or a DID | Placement (§5.4). `null` = every resolving peer, except one-peer hooks (§6). |
| `author_did` | the entry's attested author | The writer. Stamped by the host. |
| `written_at` | HLC, decimal string | The write clock (the op's HLC). Stamped by the host. It is a string because the wire range is u64. |

- The payload's `scope`, `hook`, `key` and `author_did` must equal the doc key's components and
  the attested author, else the row is Denied (§4.3).
- The canonical JSON payload is ≤ 32 KiB.
- `config` is **not** validated at admission. Peers may run different builds of the package, and
  admission must not depend on which build a peer holds. The writer's UI validates against the
  manifest it can see (installed, or the registry header) and warns when it cannot.
- No credential, grant, installation id or local state ever enters the record.

## 4. Writing, replication and judging

### 4.1 Who may write

- **Manager+ at the binding's scope, by subtree authority:** `authority(author, scope) ≥
  Manager` (`fe-policy/src/role_grants.rs:418`), the R7 rule
  (`conductor/tracks/role_propagation_20261013/plan.md` §2.2).
  - A petal Manager binds in that petal. A fractal Manager binds in the fractal and its petals.
    A verse Manager, and the owner, bind anywhere in the verse.
  - An owner pin cuts the walk: a verse Manager pinned to `viewer` in petal P cannot bind at P.
  - Authority never reaches up or sideways.
- **Write, replace and clear are one act:** a newer version at the same (scope, point).
  - Clearing writes `mode: inherit`, never a `Doc::del` (an empty entry is `NotApplicable`,
    fe-database `src/AGENTS.md` §inbound-admissions).
  - Any Manager+ at the scope may replace or clear any author's binding there.
- **Overrides follow nearest-scope-wins.** A petal Manager overrides an inherited binding inside
  their petal (another package, or `builtin`). A verse Manager can override back at the petal.
  The owner's tool against a verse Manager's verse-wide binding in one petal is a petal-scope
  `builtin`, the binding analogue of an owner pin.

### 4.2 Local write path

The steps run in this order, under the §log-first-commit discipline (`fe-database/src/op_log.rs:179`
`commit_operation`):

1. **Shape.** `schema_version` 1, a known scope in this verse, the hook and key grammar (a keyless
   hook takes `""`), `mode`/`package_id`/`version_req` consistency, and size caps. A failure is
   `invalid-binding` (422). A hook this build does not know is `unknown-hook` (422): you cannot
   configure what your build cannot name.
2. **Role.** The caller holds Manager+ at the scope: the local DID on the GUI, the token's DID on
   REST/MCP. The local DID, which authors the row, holds it too: the `SetPetalTerrain { grant }`
   pattern. A failure is `role-denied` (403). An unknown or foreign scope gets the uniform 404
   (fe-api §existence-oracle).
3. **Verse policy, package level** (R18 AC: "cannot be bound in that verse"). The write is
   refused with `policy-denied` (403) when the replicated verse policy:
   - denies the package id, or
   - is `allow-list` with no entry for it, or
   - (when the manifest is known) excludes its kind.

   Checks that depend on a build or a host (trust roots, content hashes, local policy) wait for
   resolution.
4. **Commit.** Op-log append, durable write, then publish `binding/{…}`. `written_at` is the op's
   HLC. A local write never defers.

### 4.3 Inbound admission (`DbCommand::ApplyReplicatedRow`)

| Verdict | When |
|---|---|
| `Denied` (permanent) | Any structural failure: <ul><li>the key or payload does not parse;</li><li>the payload differs from the key components or the attested author;</li><li>the scope lies outside this verse, or is an interim `VERSE#<hex>` scope;</li><li>`schema_version` ≠ 1;</li><li>`mode` and `package_id` are inconsistent;</li><li>the row is over a size cap.</li></ul> |
| `Deferred(Manifest)` / `Deferred(Parent)` | The row arrives before the verse manifest, or names a fractal or petal not known yet. |
| `Deferred(AuthorRole)` | The author is below Manager at the scope NOW: `Floor::ManageScope` at the binding's scope (`fe-database/src/handlers/replicated_row.rs:516`, gate `role_gate` at `:719`). It is retried when the author's authority rises (the existing `InboundAdmissions` worklist). |
| `Applied` | Otherwise. The row is stored even when its hook id or package is unknown here. |

### 4.4 Effective version and admissibility

For one (scope, point), the **effective version** is the newest admissible version across
authors. Order by HLC, then by canonical payload bytes (fe-policy `latest_per_grantor`'s order).
An effective `inherit` means no binding at that scope.

A version is **admissible** iff both hold:

- **Write time:** the author held Manager+ at the scope at `written_at`. This is
  `written_while_authorized` (`fe-database/src/role_grants.rs:350`), generalised from its
  hard-coded Editor to a required level, over `fe_policy::materialize_at`
  (`fe-policy/src/role_grants.rs:618`).
- **Now:** the author holds Manager+ at the scope under the current materialization.

**Convergence.** Both checks read only replicated role statements and stored versions. So every
peer holding the same statements and rows computes the same effective binding, whatever the
arrival order. This is the fe-database `src/AGENTS.md` §write-time-role argument.

### 4.5 Demotion (default, OQ-B1)

- **What happens.** A demoted author's versions become inadmissible on every peer. Nothing is
  deleted; the versions stay stored for audit.
  - The point falls back to the newest admissible version by another author at the same scope,
    else to `inherit`.
  - Instances that served it stop as `unbound` after in-flight writes complete.
- **Re-grant does not revive.** Re-granting the author does not bring back pre-demotion versions
  (continuity, §write-time-role).
  - The author, or any current Manager, re-publishes.
  - The Bindings view offers **Keep binding** on a withdrawn version: one click re-authors it as
    the clicking Manager, copying its effective `runs_on` so placement does not move.
- **Clears are withdrawn too.** A demoted Manager's clear (`inherit`) stops masking an older
  binding by someone else, which applies again.
- **Why both checks.** The decision names write time. The current check is added because an
  author stamps their own HLC: with write time alone, a demoted Manager could backdate a new
  binding into their authorized window. It also matches the scene-row rule.

## 5. Resolution on one peer

### 5.1 Algorithm

```
resolve(peer, petal P, point (H, K)):
    for S in [P, fractal(P), verse(P)]:          # nearest first; one scope per level
        v = effective(S, H, K)                   # §4.4; a hook's implicit petal row counts at P (§6)
        if v is none: continue                   # no row, or inherit: look one level out
        if v.mode == "builtin": return builtin(S, v.config)
        return check_local(peer, P, S, v)        # mode == "extension"
    return builtin(none)

check_local(peer, P, S, v):
    if placement(v) is a DID other than peer.local_did: return elsewhere(placement(v))   # §5.4
    for row in §5.2, in order:
        if row fails: return unresolved(row.state, row.offer)   # the built-in serves P meanwhile
    return resolved(instance(P, v.package_id))
```

**No fall-through.** An unresolved `extension` binding falls back to the built-in, never to the
next outer binding. Fall-through would make the outcome depend on which outer binding happens to
resolve on which peer, and a Manager could not predict what a petal does. The decision says the
peer "keeps the built-in behaviour".

**Tie rules**

- **One effective version per (scope, point)** (§4.4). Each level has exactly one scope, so
  levels never tie.
- **Several installed builds** may satisfy `version_req` and pass every check. The highest SemVer
  wins. During an update, the running build serves until the new one is active (governance
  §Consent).
- **Keyed points are independent.** Two decoders for two formats, or two connectors with two
  names, coexist at one petal.
- **One instance per (petal, package) per host.** Every resolved point of a package at P, a local
  bind of it at P, and `petal.activate` share that one instance.

### 5.2 Local checks

Checked in order. The first failure is the state, and its offer is the only action shown. Rows 1
and 5-7 and 10-11 are governance's activation preconditions, re-evaluated per verse.

| # | State | Check | Offer (local DID only) |
|---|---|---|---|
| 1 | `host-off` | The host switch is on and Disable all is clear ([governance.md](governance.md#host-switch-r16)). | — |
| 2 | `unknown-hook` | This build serves the hook (§6). Stretch hooks are unknown until built. | — |
| 3 | `not-installed` | An installation of `package_id` exists. | **Install** (§5.5) |
| 4 | `version-unsatisfied` | An installed build satisfies `version_req`. | **Update**, when a source has a satisfying build |
| 5 | `package-invalid` / `untrusted-signer` / `entitlement-required` | Governance preconditions 3, 4 and 6 still hold for that build. Trust roots and tokens change after install. | — |
| 6 | `revoked` | No accepted kill-list entry matches the build ([package-format.md](package-format.md#identity): `content_hash`, `component_digest` or `plaintext_digest`, under any id), unless the verse Owner overrode it for this verse on this host (§5.7). | The Owner's own host only: **Override…** |
| 7 | `policy-denied` | The verse policy and the local policy both admit the build within ceilings (precondition 7). The state names which policy. | — |
| 8 | `not-provided` | The build's manifest `provides` the hook, and its `keys` (if declared) match the key. | **Update**, when a source has a build that provides it |
| 9 | `invalid-config` | `config` validates against that `provides` entry's `config_schema` (no schema: `{}` only). | Manager+: **Edit binding** |
| 10 | `role-denied` | The local DID holds Editor+ at P for a writing extension, Viewer+ otherwise (precondition 8). | — |
| 11 | `consent-missing` | A consent record exists for the build's capability digest (precondition 9). | **Review permissions** |
| 12 | `not-accepted` | A local acceptance exists for this installation in P's verse (§5.3). | **Allow in this verse** |
| 13 | `disabled` | Neither the installation nor this petal's instance is locally disabled (kill layer a). | **Enable** |
| 14 | `degraded` / `capacity` | The instance is healthy, and the host's binding-instance cap has room (§5.6). | — |

### 5.3 Local acceptance

- **Why.** An extension writes as the host's local DID (governance fact 1). A replicated row that
  someone else wrote must not make this host act as itself in a verse it never chose. That is the
  R12 confused deputy, and D-X5's "replicated rows never auto-execute".
- **Record.** `{installation_id, verse_id, source_class?, actor_did, at}`:
  - local, never replicated;
  - local DID only (R12);
  - audited as `bind` (governance §Audit).

  When a manifest lists several readings classes, the one-class pick moves here (governance
  §Consent "Bind").
- **Scope of one acceptance.** It covers every binding of that installation in the verse: all
  hooks, all scopes. Revoking it stops them all (instances deactivate `unbound`).
- **The existing per-petal local bind** keeps working and implies acceptance in that petal's
  verse.

### 5.4 Placement (`runs_on`)

- `runs_on: null` runs on every resolving peer.
- **One-peer hooks** (`egress.connector`, `blob.mirror`, §6) read `null` as the author's DID. A
  verse connector therefore never pushes the same rows from every peer.
- `runs_on: <did>` runs only on the peer whose local DID equals it. Every other peer shows
  `elsewhere` ("Runs on <name>"): built-in locally, with no offer.
  - Whether the named peer actually resolves it is that peer's local fact; others cannot see it.
- `scene.*` and `ingest.decode` run only where the triggering write is accepted (§6). `runs_on`
  can narrow that further, never widen it.

### 5.5 Fallback, the unresolved state and the install offer

- **Fallback.** The built-in behaviour in §6's "Built-in default" column. An unresolved point
  never half-applies.
- **Where the state shows.** Every surface names the package (its manifest name when known, else
  the id), the scope level the binding comes from, and the state, with at most one action:
  - at the feature itself: the Map column, the API reply, the egress card, the inspector;
  - in the Extensions panel's Bindings view;
  - in `GET /api/v1/petals/{petal_id}/bindings/effective` (§8).

  Local states are local facts. Only the local DID sees them, so a remote caller cannot use them
  to learn whether this host admits extensions (governance §Local identity).
- **The install offer.**
  1. **When.** Shown only when both hold:
     - the replicated verse policy and this host's local policy would admit the package at
       package level;
     - a configured `RegistrySource` (R29) has a build satisfying `version_req`.

     Otherwise the state shows without an action, so there is never a dead control.
  2. **Install.** Runs the header phase ([package-format.md](package-format.md#header-phase-then-body)),
     then the normal consent dialog (governance §Consent). The dialog renders manifest v2,
     including `provides`, plus one acceptance line: "Allow in <verse name>".
  3. **Confirm.** One confirmation records install, consent and acceptance (three audit entries:
     `install`, `consent`, `bind`).
  4. **Re-resolve.** The point re-resolves, and the instance starts by §5.6.
- **Never automatic.** No install, update, consent or acceptance happens without a local-DID
  action. On REST these are the local-DID-only routes (R12).
- **Dismiss** hides the offer for that binding version on this host. That is local, never
  replicated.

### 5.6 Instances, budgets and re-resolution

- **Start and stop.**
  - *Eager:* `petal.activate` and `egress.connector` start as soon as they resolve.
  - *Lazy (every other hook):* start on first need, meaning P becomes the active petal or a call
    targets P. Stop after 10 minutes without a call while P is not active (OQ-B9).
- **Cap.** At most 64 binding-started instances per installation per host. Past the cap, lazy
  points resolve `capacity` until one stops. An eager instance is never evicted.
- **Shared egress budget.** The instances one binding starts on a host share one token bucket per
  `egress` endpoint (`rate_hz`). A 200-petal verse binding cannot multiply an endpoint's rate by
  200. Fuel stays per call.
- **Re-resolution triggers.** Each one re-runs the resolver once, for the affected verse only.
  Nothing reads the DB per frame.
  - binding row changes: a row applied or withdrawn;
  - role and policy changes: a role statement change in the verse; a verse or local policy change;
    a trust-root change;
  - lifecycle actions: install, update, uninstall, consent, acceptance, enable, disable;
  - kill-list changes: a kill-list accept or an Owner override;
  - navigation, for lazy starts.
- **Stopping one point.** When a point stops resolving to package K at P, K@P stops only if
  nothing else keeps it (another point, a local bind). It stops with `on-deactivate(unbound)` and
  one final flush (the existing lifecycle).

### 5.7 Revocation and the kill list (OD-E1, decided)

- **Accepting a kill list that names a running build.**
  - Its instances stop after in-flight writes complete: the current commit drains, and no new
    ticks, commands, subscriptions or hook calls start.
  - Rows they wrote stay, with provenance `{package_id, content_hash}`. There is no withdrawal and
    no re-judge.
  - The binding row is untouched. Every peer that trusts that root resolves it `revoked`: the
    built-in serves, and the state shows.
- **At install, including the offer.**
  - A kill-listed build is refused (`revoked`).
  - The verse Owner, on the Owner's own host (local DID = the verse `owner_did`), may override for
    that verse. The override needs an explicit typed confirmation and is audited:
    `ExtensionAudit` action `kill-list-override` through `op_log::commit_operation`, projected to
    the canonical log as E10 defines.
  - The override is keyed `(verse_id, content_hash)` on that host. With it, row 6 passes there
    only; everywhere else the binding stays `revoked`.
  - It is never replicated and never reaches another verse.
- **No other override.** Consent and acceptance can never make a kill-listed build resolve.

## 6. Hook catalogue

**Call** column:
- **waits**: the triggering operation waits for the answer, up to the deadline.
- **cached**: computed off the frame and reused until an input changes.
- **after-commit**: runs after the triggering write is durable.
- **service**: the binding switches on surfaces the instance already has, configured by one
  `configure` call.

No hook call ever runs on the DB thread or the render loop.

**Needs** column: what a provided hook must declare in its manifest (V18).

| Hook | Family | Key | Built-in default | Seam it wraps | Call | Extension receives → returns | Needs | Tier |
|---|---|---|---|---|---|---|---|---|
| `petal.activate` | lifecycle | name | Does not run here | `fe-plugin/src/scene_host.rs:62` (`PluginContexts::bind`, today's local bind); `fe-plugin/src/lib.rs:296` (`plugin_thread_loop`; this file is moving under the swarm, so cite by symbol) | service, eager | `configure` → `{}`; then its own ticks, bus, ingress and UI | — | v1 |
| `petal.map` | map/terrain | — | The petal's own `terrain` column, else no map | `fe-ui/src/terrain_map/mod.rs:142` (nav-change load), `:37` (`PetalMapState`); `fractalengine/src/terrain_bridge.rs:151` (`bridge_petal_terrain`); `fe-terrain/src/petal_binding.rs:17` (strictest reader); `fe-database/src/handlers/petal_terrain.rs:223` (stored doc) | cached, ≤ 5 s | `{petal_id, stored_terrain, installed_tilesets}` → `{terrain: TerrainConfig \| null}` | — | v1 |
| `api.routes` | API | mount name | Uniform 404 | `fe-api/src/server.rs:487` (auth-layer merge), `:527` (`mount_extension_handles`, first-party precedent); `fe-api/src/auth.rs:15`; `fe-api/src/rest.rs:50` (`authorized_petal_scope`) | waits, ≤ 5 s | the request as an `ingress-frame` (existing `on-ingress`) → `ingress-reply` | `uses` ≥ 1 `http-server` or `websocket-accept` id | v1 at petal paths; verse and fractal paths stretch |
| `scene.create` | scene ops | — | The write proceeds | `fe-database/src/lib.rs:749` (`CreateNode`); issuers `fe-ui/src/actions/node.rs:17`, `fe-api/src/rest.rs:262` | waits, ≤ 250 ms | `{petal_id, proposed: {name, position}, caller}` → `{verdict: "allow" \| "refuse", reason?, augment?}` | `scene.read`; augment also needs `scene.write` with `set_property`, plus `property_scope` and `tick` | v1 |
| `scene.transform` | scene ops | — | The write proceeds | `fe-database/src/lib.rs:920` (`UpdateNodeTransform`); issuer `fe-ui/src/node_manager/transform_broadcast.rs:16` (on commit, never per frame) | waits, ≤ 250 ms | `{node, proposed: {position, rotation, scale}, caller}` → as above | as above | v1 |
| `scene.property` | scene ops | — | The write proceeds | `fe-database/src/lib.rs:1246` (`SetNodeProperty`) | waits, ≤ 250 ms | `{node, key, value, caller}` → as above. Only keys inside `property_scope` reach the guest; others pass untouched. | as above | v1 |
| `ingest.decode` | ingest | device format id | The payload stays opaque | `fe-api/src/device.rs:185` → `fe-database/src/handlers/device_payload.rs:280` (`insert_device_payloads`, after the durable insert); readings with `fe-database/src/handlers/iot_reading.rs:109` semantics | after-commit, ≤ 5 s | `{payload_id, format_id, mime, node_id, recorded_at_ms, files: [{asset_id, content_type, size}]}` + bytes (≤ 1 MiB) → `{staged: n}`, with readings staged through `readings.stage` | `readings`, `tick` | v1 |
| `ui.panel` | UI | panel name | No extension UI at the scope | `fe-sdk/src/ui/mod.rs:88` (`UiExtensionRegistry`); `fe-ui/src/plugin.rs:46` (`UiExtensions`); `fe-ui/src/panels/inspector.rs:251` | service; rendering reads the resolution cache | No call. The `uses` contributions render while the active petal resolves the binding. | `uses` ≥ 1 `ui` id | v1 |
| `egress.connector` | egress | connector name | No push; pull egress unchanged | R33 push connector; `fe-api/src/query_guard.rs:90` and `:226` (the `host/query` core). Pull egress stays core: `fe-api/src/rest.rs:1245`, `fe-api/src/export.rs:284`, `fe-api/src/share.rs:123`. | service, eager, one peer | `configure`; then its consumes and egress | `uses` ≥ 1 `egress` id; a `host/query` or `host/readings` consume | v1 |
| `egress.snippet` | egress | target name | The compiled-in Copy-for-BI targets only | `fe-ui/src/panels/egress_card.rs:16`; built-ins `fe-ui/src/gis/egress_strings.rs:164` (never revocable, R33) | cached, ≤ 1 s | `{sql, query_url, export_urls: {parquet, csv}}` → `{label, snippet}` (text ≤ 16 KiB, shown for copying, never run) | — | v1 |
| `asset.import` | files | MIME type | Refused unless GLB | `fe-api/src/upload.rs:201` (`validate_glb`), `:609` (`upload_petal_asset`); GUI import `fe-database/src/lib.rs:780` | waits, ≤ 30 s | `{petal_id, file_name, mime}` + bytes (≤ 16 MiB) → GLB bytes (≤ 16 MiB) + `{name?}`; the host re-runs `validate_glb` and the normal store path | `keys` (the MIME types) | v1 |
| `blob.mirror` | storage | mirror name | Blobs stay local and on peers | `fe-runtime/src/blob_store.rs:42` (`add_blob`, at the asset-creating callers) | after-commit, one peer | `{hash, size, content_type, asset_id}` + bytes (≤ 8 MiB; larger skipped and counted) → `{}`; it pushes through egress | `uses` ≥ 1 `egress` id | v1 |
| `blob.source` | storage / network | source name | Peers only | `fe-sync/src/sync_thread.rs:2101` (`handle_fetch_blob`); `fe-runtime/src/bevy_blob_reader.rs:45` (`with_on_miss`) | after a peer miss, ≤ 30 s | `{hash, expected_size?, asset_id?}` → bytes, kept only if BLAKE3 and size match | `uses` ≥ 1 `egress` id | v1 |
| `sync.tileset-source` | network | source name | Peers only | `fe-sync/src/tileset_transfer.rs:1089` (`fetch_chunk`), `:1120` (`download`); `fe-runtime/src/blob_transport.rs:105` | per child, ≤ 30 s | `{tileset_id, root, child_hash, size}` → bytes, verified per child hash as the HashSeq already is | `uses` ≥ 1 `egress` id | v1 |
| `portal.open` | portal / web | portal name | Node `webpage_url` portals only | `fe-ui/src/actions/mod.rs:1058` (`open_portal`); `fe-webview/src/petal_portal.rs:51` (`ActivePortal`); portal contract `README.md` §9.5 | service, explicit Open | No call: the bound bundle is offered with an **Open** action | an admitted `kind: web-bundle` (no `provides`) | v1 |
| `asset.export` | files | format name | `.hexon` export only | `fe-api/src/format.rs:19` (`export_petal`) | waits | petal snapshot → file bytes | — | stretch |
| `render.asset-source` | renderer | path prefix | `blob://` paths only | `fe-runtime/src/bevy_blob_reader.rs:78` (`AssetReader::read`); fe-database `src/AGENTS.md` §asset-resolution | cached | `{path}` → bytes, verified against a pinned hash when the node names one | `keys` | stretch |

**Per-hook rules**

- **`petal.map`**
  - **The stored column counts as a petal-scope binding.** A petal's own non-null `terrain`
    column is an implicit `builtin` binding at that petal, so it beats inherited bindings (OQ-B6).
    "Use the fractal's map" clears the column (`DELETE /api/v1/petals/{p}/terrain`).
  - **`builtin` takes config.** A `builtin` binding's `config` is a `TerrainConfig`, read by the
    strictest reader. A fractal-wide `builtin` binding therefore gives every petal the same map
    with no extension at all. This is how the hook generalises today's per-petal binding (root
    `AGENTS.md` §petal-map).
  - **An extension's output is presentation on that peer.**
    - It is validated by `terrain_config_from_petal_json`, then sent as `TerrainAssignmentMsg`
      (`fe-terrain/src/petal_binding.rs:185`).
    - It is never written to the petal row. `GET /api/v1/petals/{p}/terrain` still returns the
      stored column.
    - `PetalMapState` records the effective source.
    - The tilesets it names must be installed locally, as for the built-in.
- **`api.routes`**
  - **Alias path.** The alias is `/api/v1/petals/{petal_id}/apis/{key}/{endpoint}[/{sub_path}]`.
    It sits beside the package mount `…/extensions/{package_id}/ingress/{id}`
    ([manifest-v2.md](manifest-v2.md#ingress--endpoints-that-receive-traffic-spec-r8)).
  - **Same checks.** It applies the same bearer auth, `role_floor`, rate and size limits, and the
    host-side credential-header strip (V16 plus the host denylist).
  - **Stable URL.** Rebinding the key to another package keeps the URL.
  - **Unresolved.** The alias answers 503 `binding-unresolved` `{package_id, reason}` to callers
    with Viewer+ at the petal, and the uniform 404 to everyone else (OQ-B7).
  - **Wider scopes.** A fractal or verse binding mounts under every covered petal's path. The
    verse and fractal paths themselves need a non-petal instance (stretch, OQ-B2).
- **`scene.*`**
  - **Where they run.** Only on the authoring peer, on the sending side of the DB channel: a gate
    in front of `DbCommandSender` (`fe-runtime/src/app.rs:19`), never on the DB thread (the
    self-drain rule).
    - Never for `ApplyReplicatedRow`.
    - Never for `PluginCommit` (`fe-database/src/lib.rs:699`), so hooks cannot recurse.
  - **They are authoring aids, not access control.** A peer without the extension writes freely,
    and its rows are judged by role alone.
  - **Faults fail open** (OQ-B4). Input over the 64 KiB call cap is not a fault: the caller sized it, so a
    bound write is refused (`input-too-large`, REST 413; steer 2026-11-09 02:45).
  - **A refusal** answers REST 409 `refused` with the guest's reason. The GUI rolls back the
    optimistic transform the way `transform_failed` does.
  - **Augment ops** are staged in the instance's window and flush with its next tick as one
    `PluginCommit` with provenance (R5). The original write is never delayed by an augment.
- **`ingest.decode`**
  - **Where it runs.** Only on the peer whose door accepted the payload, after the durable insert.
    It never runs on replicated payload rows, where every peer would decode and duplicate the
    readings.
  - **Stamped readings.** Readings staged inside the call are host-stamped:
    - `payload_id` = the call's payload;
    - `source_class: decoder` (`fe-sdk/src/source_class.rs:17`).

    Neither is guest-settable, and `decoder` stays undeclarable in a manifest.
  - **Unresolved at ingest.** A payload accepted while the decoder is unresolved stays opaque
    (OQ-B10).
- **`egress.connector`**
  - **Placement.** It runs on one peer (§5.4).
  - **Per-petal batches.** Each covered petal's instance reads only its own petal, so a verse
    connector delivers per-petal batches through the shared bucket (§5.6).
  - **The pull API is untouched.** `/api/v1/query`, exports and share URLs stay core and are never
    replaced (R33).
- **`egress.snippet`**
  - **Text only.** The snippet is shown in a read-only copy field.
  - **Built-ins come first.** The compiled-in targets render first and no binding hides them.
- **`blob.*` and `sync.*`**
  - **Content addressing is the safety property.** No byte an extension provides is used unless
    its BLAKE3 matches.
  - **No transport replacement.** These hooks never replace iroh transport, relays or peer
    selection; P2P capability for extensions is a spec §5 non-goal.
  - **`blob.source` comes last.** It is tried only after the peer fetch fails or times out.
- **`portal.open`**
  - **D-X5 holds.** A replicated row never mounts a bundle. The bound bundle appears as an offer
    at the scope and mounts only on a local Open.
  - **Admission** is the portal track's (`admission.md`).

## 7. Capabilities, identity and the hook call

### 7.1 What a binding needs to take effect

All of these must hold on the peer:

1. The build's manifest `provides` the hook, and its `keys` match ([manifest-v2.md](manifest-v2.md#provides--hook-points-bindingsmd)).
2. The hook's needs are declared (`uses`, grants; V18 at install).
3. Consent covers the build's capability digest, which includes `provides`.
4. The verse and local policy ceilings admit the build.
5. The installation is locally accepted for the verse.

### 7.2 Rules

- **A binding grants nothing.**
  - `config` is input, validated by the extension's own `config_schema`. It cannot add an
    endpoint, a scope, a metric, a property key or a role.
  - A config-chosen path still goes only under a declared egress base.
- **Confinement is unchanged.** An instance at P reads and writes only P, however wide the
  binding's scope. A verse binding means every petal gets its own instance, never one instance
  that sees the verse (OQ-B2).
- **Identity is unchanged.**
  - Every write a hook causes is authored by the host's local DID through the typed bridge (R22),
    and judged role-at-write on every peer like the local user's own edits.
  - Augmentations and decoded readings flush through `PluginCommit` with host-stamped provenance.

### 7.3 Hook calls (proposed WIT delta, not applied; OQ-B11)

WIT 2.0 has no export the host can call for a hook point. `bus-hooks.on-command` does not fit:
`caller-kind` has no `host`, and commands are bus traffic. The WIT slice adds:

```wit
/// Hook-point calls selected by scoped bindings (docs/extensions/bindings.md).
interface hooks {
    use types.{json};

    record hook-call {
        hook: string,          // catalogue id, e.g. "petal.map"
        key: string,           // "" for keyless hooks
        binding-scope: string, // the scope string of the binding row that selected this call
        config: json,          // validated against provides[].config_schema
        input: json,           // hook-specific (§6); {"event": "configure"} for service hooks
        body: list<u8>,        // ingest.decode, asset.import, blob.mirror; else empty
    }
    record hook-output { output: json, body: list<u8> }
    variant hook-error { refused(string), invalid-input(string), failed(string) }

    on-hook: func(call: hook-call) -> result<hook-output, hook-error>;
}

world extension-with-hooks {
    include extension;
    export hooks;
}
```

**The export is required.** A manifest with `provides` needs a component that exports `hooks`.
The component check is `missing-export hooks`
([package-format.md](package-format.md#component-checks-at-install)). Plain 2.0 components stay
valid.

**Call shapes.**
- *Service hooks* get one `configure` call when they start and one per config change.
- *Call hooks* get one call per use.

**Limits per call.**
- its own fuel refill (`tick.fuel`);
- JSON in and out ≤ 64 KiB;
- `body` ≤ the hook's cap in §6;
- the §6 deadline.

**Faults.** A trap, fuel exhaustion or a missed deadline is a fault: the built-in serves that use,
and the fault is counted. Fuel exhaustion also degrades the instance (R4).

**Counters** (extension status, R11):
- `hooks.calls.<hook>`, `hooks.refused.<hook>`, `hooks.faults.<hook>`, `hooks.timeouts.<hook>`;
- `bindings.resolved`, `bindings.unresolved.<state>`.

## 8. Examples

### 8.1 Petal map provider (fractal-wide, one petal opted out)

`org.example.city-map` (kind `general`) declares:

```json
"provides": [
  { "hook": "petal.map",
    "config_schema": { "type": "object", "required": ["district"], "additionalProperties": false,
                       "properties": { "district": { "type": "string", "maxLength": 64 } } } }
]
```

Fractal F's Manager writes, at `VERSE#V-FRACTAL#F`: `hook: "petal.map"`, `mode: "extension"`,
`package_id: "org.example.city-map"`, `version_req: ">=1.2.0, <2.0.0"`,
`config: {"district": "harbour"}`.

| Petal, peer | Result |
|---|---|
| P1 (no stored terrain), peer A (1.2.3 installed, consented, accepted) | `resolved`. `petal.map` runs with `{petal_id: P1, stored_terrain: null, …}`. The returned config passes the strictest reader and becomes P1's terrain on A. The Map column reads "City Map · fractal". |
| P1, peer B (not installed; the policies admit it; B's source has 1.2.3) | `builtin` (no map), state "City Map: not installed", action **Install**. Consent plus "Allow in <verse>" is one confirm, then `resolved`. |
| P1, peer C (1.2.3 installed, then its local `extensions.toml` denies the package) | `policy-denied`, "Blocked by this device's extension policy", no action. Not installed, the same peer reads `not-installed` with no action: §5.2 row 3 comes first, and the Install offer needs both policies to admit the package (§5.5). |
| P2 (its petal Manager set a stored terrain) | The stored column wins on every peer (implicit petal-scope `builtin`) until a Manager chooses "Use the fractal's map". |
| P1 on A, provider faults | The built-in serves P1 on A; `hooks.faults.petal.map` counts. |

### 8.2 Petal API routes

The device-adapter example (`org.example.device-adapter`) adds
`"provides": [{"hook": "api.routes", "uses": ["telemetry"]}]`. Petal P's Manager binds
`api.routes`, key `weather`, to it (`version_req: ">=0.1.0, <0.2.0"`).

- **Peer A.** A `POST /api/v1/petals/P/apis/weather/telemetry` request runs these steps in order:
  1. the bearer `auth_middleware`;
  2. `authorized_petal_scope` (uniform 404 for an unknown or foreign petal);
  3. resolution, which is `resolved`;
  4. the `telemetry` endpoint's `role_floor: editor`, rate, size and content-type checks;
  5. the credential-header strip;
  6. `on-ingress` with endpoint `telemetry`.

  Staged readings flush with the next tick, authored by A's local DID.
- **Peer B (not installed).** The same request answers a Viewer+ caller with 503
  `{"error": "binding-unresolved", "package_id": "org.example.device-adapter", "reason": "not-installed"}`.
  A caller with no role at P gets the uniform 404.
- **Rebinding.** Rebinding `weather` to another package changes nothing for clients. The package
  mount `…/extensions/org.example.device-adapter/ingress/telemetry` keeps working beside it.

### 8.3 Fractal-wide ingest decoder

- **Setup.**
  - The verse Manager declares device format `acme.thermo/frame@1` (existing
    `POST /api/v1/verses/{v}/device-formats`).
  - `org.example.acme-thermo` (kind `device-adapter`) declares
    `readings: {source_class: "device", metrics: ["temperature_c", "humidity_pct"]}`,
    `tick: {cadence_ms: 1000}` and
    `provides: [{"hook": "ingest.decode", "keys": ["acme.thermo/*"]}]`.
  - Fractal F's Manager binds `ingest.decode`, key `acme.thermo/frame@1`, at F.
- **Decoding on peer A.**
  1. Peer A accepts `POST /api/v1/petals/P3/iot/payloads` (P3 is in F). The durable insert and
     publish are unchanged.
  2. The decoder instance at P3 starts lazily, receives the bytes, and stages 12 readings.
  3. The next tick flushes them in one `PluginCommit`, authored by A's local DID, with:
     - `payload_id` = the payload;
     - `source_class: decoder`;
     - provenance `{package_id, content_hash}`.
  4. They replicate like any readings.
- **Other peers.** Peers B and C receive the payload row and A's readings. They never decode the
  payload themselves.
- **Opting a petal out.** P4's Manager binds the same point at P4 with `mode: builtin`. P4's
  payloads then stay opaque on every peer.

### 8.4 Verse egress connector

- **Setup.**
  - `org.example.bi-push` adds `"provides": [{"hook": "egress.connector", "uses": ["sink", "latest"],
    "config_schema": {"type": "object", "properties": {"format": {"enum": ["json", "csv"]}},
    "additionalProperties": false}}]`.
  - Verse Manager M binds `egress.connector`, key `warehouse`, at `VERSE#V` with `runs_on: null`.
    That is M's DID, because this is a one-peer hook.
- **M's host.**
  - Each petal of V gets an instance (eager). Each instance consumes `latest` scoped to its own
    petal and pushes through `sink`.
  - The instances share `sink`'s `rate_hz: 1` bucket, so the sink sees at most one request per
    second whatever the petal count.
  - Idle petals cost zero query runs ([bus.md](bus.md#23-hostquery--change-triggered-re-runs-od-3) §2.3).
- **Other peers** show "Runs on M" (`elsewhere`), keep the built-in (no push), and get no offer.
- **M is demoted below Manager at V.**
  - The binding is withdrawn on every peer (§4.5), and M's instances stop after in-flight
    requests.
  - A current Manager's **Keep binding** re-authors it with `runs_on` = M's DID, so placement
    does not move silently.
- **Unchanged for everyone:** `/api/v1/query`, exports and share URLs.

## 9. Surfaces

**REST:**

| Route | Rule |
|---|---|
| `GET /api/v1/bindings?verse_id=…[&fractal_id=…[&petal_id=…]]` | The rows at that exact scope: the effective version per point, plus withdrawn versions (author, `written_at`, admissible). Viewer+ at the scope; uniform 404 otherwise. |
| `GET /api/v1/petals/{petal_id}/bindings/effective` | Per point: the winning scope, `mode`, `package_id` and `version_req`. Only the local DID also gets this host's state and offer (§5.5). |
| `PUT /api/v1/bindings` | Body `{verse_id, fractal_id?, petal_id?, hook, key, mode, package_id?, version_req?, config?, runs_on?}`. Runs §4.2 and replies with the stored row. |
| `DELETE /api/v1/bindings` | Body `{verse_id, fractal_id?, petal_id?, hook, key}`. Writes `inherit`. |

Bound routes live under `/api/v1/petals/{petal_id}/apis/{key}/…` (§6).

**MCP:** `list_bindings`, `set_binding` and `clear_binding`, with the same checks.

**GUI:**
- **Accessible names.** `binding:<hook>` or `binding:<hook>:<key>` names a state row; the same
  name plus `:action` names its one action.
- **The Bindings view.** It sits in E13's Extensions panel and lists the points at the active
  petal, grouped by family, each with its winning scope and state.
- **Editing.** A Manager+ edits through a form generated from `config_schema`. The args_schema
  subset is renderable.
- **Feature-level states.** The feature surfaces carry the state too: the Map column, the egress
  card, the inspector.

## 10. Deltas to other contracts (owners apply them; not applied here except where noted)

- **WIT** (`fe-plugin/wit/fractal-plugin-2.wit`): the `hooks` interface and the
  `extension-with-hooks` world (§7.3).
- **manifest-v2.md and the schema:** `provides`, V17 `unknown-hook`, and V18
  `hook-capability-missing`. Applied in this change.
- **governance.md:**
  - *Replicated rows and records:*
    - binding rows become a replicated row family (Manager+ at scope, §4);
    - local acceptance (§5.3);
    - audit action `kill-list-override` (acceptance reuses `bind`).
  - *Kill list:* the Owner kill-list override amends precondition 5 (§5.7).
  - *OD-E1:* decided as no withdrawal. governance.md now reads stamp-and-stop (applied), and spec
    R24 matches (D-EXT).
- **bus.md:** §12. Applied in this change.
- **spec:**
  - OD-E3 holds (instances stay petal-anchored).
  - R18's AC holds at write time (§4.2 step 3).
  - R24 shrank to stamp-and-stop (OD-E1), applied.
  - Bindings have their own requirement, spec R37 (applied).
- **Portal contract:** `portal.open` binds admitted web bundles with no `provides`. Their manifest
  stays the portal track's.
- **manifest-v2.md "`manual` and `decoder` are not extension classes"** stands. `decoder` is
  host-stamped, and only inside `ingest.decode` calls.
- **`fe-terrain/src/extension.rs:10`** `TERRAIN_EXTENSION_ID` is now `org.fractalengine.terrain`, a
  valid v2 package id (E18, applied). The built-in still needs no install (`mode: builtin`); its
  manifest declares `provides: [{ "hook": "petal.map" }]`, so a binding can name the native
  provider as a package.

## 11. Open questions (defaults in force until ratified)

| # | Question | Default | Reversal cost |
|---|---|---|---|
| OQ-B1 | What happens to a binding when its author is demoted? | Withdrawn: admissible needs both write time and now; the point falls back to the next admissible version; **Keep binding** re-authors it. | Low. Dropping the current check is a relaxation, but it reopens backdating. |
| OQ-B2 | Where are instances anchored? | Petals only (OD-E3). Verse and fractal bindings fan out per petal; verse and fractal API paths are stretch. | Medium. Scope anchors need a WIT 2.x binding-record field and a manifest `binding` value; both are additive. |
| OQ-B3 | Does an installed package need local acceptance before a binding runs it? | Yes, per (installation, verse), for every hook. | Low to remove later; high to add later, because it would stop running bindings. |
| OQ-B4 | What does a scene hook do on a fault or timeout? | Fail open, counted. | Low. A per-binding `on_fault` arrives with `schema_version` 2. |
| OQ-B5 | Can an outer scope lock its binding against overrides? | No. Nearest always wins. | Medium. A `locked` field needs `schema_version` 2, which older peers deny. |
| OQ-B6 | Does a petal's stored terrain beat an inherited `petal.map` binding? | Yes. It counts as a petal-scope `builtin`. | Medium. Flipping it changes which map existing petals show. |
| OQ-B7 | What does an unresolved bound route answer? | 503 to Viewer+ at the petal; the uniform 404 otherwise. | Low. |
| OQ-B8 | Does the bus carry a `host/bindings` source? | No. The state is in REST and the UI. | Low. A new host source is an additive bus change. |
| OQ-B9 | How long do binding-started instances live? | Lazy start; 10 minutes idle stop; 64 per installation per host. | Low (host configuration). |
| OQ-B10 | Are payloads accepted while a decoder was unresolved decoded later? | No. | Low. A later `decode-backlog` command can add it. |
| OQ-B11 | What is the hook-call ABI? | A `hooks` export in a world that includes `extension`. | Medium (the WIT owner). |
| OQ-B12 | How far does a kill-list override reach? | `(verse_id, content_hash)` on the Owner's own host only. | Low. |

## 12. Implementation slices (next swarm turn)

Rules:
- New files belong to one slice.
- Shared files are serialized in the listed wave order.
- Per-slice gate: `cargo check -p`.
- Tests extend existing journeys or register one small batch in `conductor/test-batches.md`.

| Id | Does | Owns (new) | Shared (serialize) | Depends on | Model |
|---|---|---|---|---|---|
| BND-0 | Record, point, mode and state types; doc-key codec; the hook catalogue as a constant table | `fe-runtime/src/extension_binding.rs` | `fe-runtime/src/lib.rs`, `fe-runtime/src/AGENTS.md` | — | sonnet |
| BND-1 | DB plane, in four parts: <ul><li>the table;</li><li>the local write path (§4.2);</li><li>inbound admission (§4.3), with `ManageScope` at the binding scope;</li><li>admissibility (§4.4), with `written_while_authorized` taking a level.</li></ul> One journey covers LWW across authors, demotion withdraw, re-grant staying out, and Keep binding. | `fe-database/src/handlers/extension_binding.rs`, `fe-database/tests/extension_binding_test.rs` | `fe-database/src/{schema.rs, lib.rs, admin.rs, role_grants.rs, handlers/replicated_row.rs, handlers/mod.rs, AGENTS.md}`, `fe-runtime/src/messages.rs`. Extends `role_propagation_inbound_test.rs::a_not_yet_managers_statement_is_deferred_then_admitted_on_promotion` with a binding row. | BND-0; E10 merged | opus |
| BND-2 | The pure resolver (§5.1-5.2) over a snapshot (rows, `EffectiveRoles`, local facts); a table-driven journey: every §5.2 row, nearest wins, no fall-through | `fe-plugin/src/bindings/{mod.rs, resolver.rs, AGENTS.md}` | `fe-plugin/src/lib.rs` (mod line) | BND-0 | sonnet |
| BND-3 | The `hooks` WIT export and component binding; `provides` serde plus V17/V18; guest SDK helper | `fe-plugin/src/wasm/component/hooks.rs` | `fe-plugin/wit/{fractal-plugin-2.wit, AGENTS.md}`, `fe-sdk/src/manifest_v2.rs`, `tools/fe-ext/guest/` | E1, E2 merged | sonnet |
| BND-4 | Activation from bindings (eager, lazy, cap, idle stop); placement; shared egress buckets; re-resolution triggers; revocation drain | `fe-plugin/src/bindings/activation.rs` | `fe-plugin/src/{lifecycle.rs, scene_host.rs}`, `fractalengine/src/plugin_bridge.rs` | BND-1, BND-2; E7, E10 | opus |
| BND-5 | `petal.map`: terrain as the first bound hook; implicit stored-column row; `builtin` config; effective source on `PetalMapState` | — | `fractalengine/src/terrain_bridge.rs`, `fe-ui/src/terrain_map/mod.rs`, `fe-terrain/src/extension.rs` | BND-3, BND-4; E18 | sonnet |
| BND-6 | `api.routes` alias mount, exported as a router | `fe-api/src/bound_routes.rs` | — (BND-8 merges it) | BND-2, BND-4; E6 | sonnet |
| BND-7 | Install offer, acceptance in the consent flow, and the kill-list Owner override record and confirm | `fe-ui/src/extensions/install_offer.rs` | `fe-ui/src/extensions/consent.rs`, `fe-database/src/handlers/extension_policy.rs` | BND-2; E10, E11 | sonnet |
| BND-8 | REST and MCP bindings surface; `server.rs` wiring | `fe-api/src/bindings.rs`, `fe-api/tests/bindings_api_test.rs` | `fe-api/src/server.rs`, `fe-api/src/mcp/` (tools plus inventory), `fe-api/AGENTS.md` | BND-1, BND-6 | sonnet |
| BND-9 | Bindings view: effective rows, states, offers, the Manager edit form; applies the UI-UX guidance | `fe-ui/src/extensions/bindings.rs` | `fe-ui/src/extensions/panel.rs` | BND-7, BND-8; E13 | sonnet |
| BND-10a | `scene.*` gate on the send side; GUI rollback on refuse | `fe-runtime/src/scene_hook_gate.rs` | `fractalengine/src/main.rs`, `fe-ui/src/node_manager/transform_broadcast.rs` | BND-3, BND-4 | opus |
| BND-10b | `ingest.decode` after-commit call and stamping | `fe-plugin/src/bindings/decode.rs` | `fe-database/src/handlers/device_payload.rs`, `fe-database/src/lib.rs` (after BND-1) | BND-3, BND-4 | sonnet |
| BND-10c | `ui.panel` and `egress.snippet` | `fe-ui/src/extensions/bound_slots.rs` | `fe-ui/src/plugin.rs`, `fe-ui/src/panels/egress_card.rs` | BND-3, BND-4 | sonnet |
| BND-10d | `egress.connector`, `blob.mirror`, `blob.source`, `sync.tileset-source` | `fe-plugin/src/bindings/{egress.rs, blob_source.rs}` | `fe-sync/src/{sync_thread.rs, tileset_transfer.rs}`, `fe-runtime/src/bevy_blob_reader.rs` | BND-3, BND-4 | opus |
| BND-10e | `asset.import` | `fe-api/src/import_hooks.rs` | `fe-api/src/upload.rs` | BND-3, BND-4 | sonnet |
| BND-10f | `portal.open` (EXT-PORTAL session, their files) | per that track | `fe-webview/src/petal_portal.rs`, the portal inspector section | BND-2, BND-8 | sonnet |
| BND-11 | Simulator scenario `ext-bindings` (per-peer resolution, offer, demotion, kill list) and batch registration | `tools/qa/sim/scenarios/ext-bindings.json` | `conductor/test-batches.md`, sim driver ops (`set_binding`, `accept_extension`) | all | sonnet (batch bookkeeping: haiku) |

**Waves:**

1. BND-0
2. BND-1 ∥ BND-2 ∥ BND-3
3. BND-4
4. BND-5 ∥ BND-6 ∥ BND-7 ∥ BND-10b ∥ BND-10c ∥ BND-10d ∥ BND-10e
5. BND-8 ∥ BND-10a
6. BND-9 ∥ BND-10f
7. BND-11, then advocate → critic, serially

Within a wave no two slices share a file. `fe-database/src/lib.rs` and `fe-runtime/src/messages.rs`
go to BND-1 first; `server.rs` goes only to BND-8.

**Not scheduled (stretch):**
- `asset.export`;
- `render.asset-source`;
- verse and fractal API paths, and scope-anchored instances (OQ-B2).
