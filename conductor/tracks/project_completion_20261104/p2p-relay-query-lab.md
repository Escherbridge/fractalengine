---
type: Plan
status: proposed
date: 2026-11-04
title: Disposable Podman lab for peer, relay and query acceptance
---

# Proposed peer, relay and query lab

Persisted in response to the user's feasibility and fixture questions. This is
a proposed implementation/acceptance aid for existing peer and rich-data scope.
It authorizes no installation or deployment and adds no mandatory Kubernetes
release gate. No lab, cluster or passing acceptance is claimed. Existing track
requirements, retained-store HOLD and release gates remain unchanged.

## Evidence and current feasibility

[The exact source inventory](./evidence/deployment-fixtures/fixture-inventory.txt)
records repository paths, commands, fixtures and limitations.
[Root's read-only runtime observation](./evidence/deployment-fixtures/runtime-state.json)
records reachable Podman5.7.1 client/server, a running WSL machine reporting6CPU
and2GiB, a rootless default connection and a separately available rootful
connection. No identifiable FractalEngine lab container was listed on the default
connection. The rootful connection and remote clusters were not inventoried.
kubectl/kind/k3d/minikube/helm were not found on PATH and no user kubeconfig was
present; that does not prove absence of every cluster. Exact copy hashes and
source paths are in [the receipt](./evidence/deployment-fixtures/copy-receipt.json).

The repository provides Dockerfiles for the **fe-relay application peer** and
Hexon registry, plus `docker/compose.dev.yml`. The application peer is not the
iroh transport relay. No canonical Kubernetes manifests were found. The existing
Compose host port8765 conflicts with live QA60; do not start it unchanged or
touch that app. Repository GLBs, rich-fleet JSON, generated0.35 migration metadata
and three built Hexon packages are present. Presence is neither a live fixture
service nor completed fixture acceptance.

## Podman first

1. Review capacity and use an explicitly owned, disposable QA context. The
   reported2GiB is not a demonstrated capacity guarantee for concurrent builds,
   three peers, registry and a cluster. Choose resources before starting the lab;
   do not change the user's global default connection, unrelated containers or
   existing machine configuration as a side effect.
2. Author a small separate lab definition: one digest-pinned **iroh-relay**, three
   **fe-relay application peers**, one registry, and ordinary Windows GUI clients
   as actual external participants. Reuse the current image/fixture contracts.
   The existing transport test pins
   `n0computer/iroh-relay:v1.3.0@sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57`.
   Reserve isolated loopback host ports after an ownership/conflict check;
   proposed peer18865–18867, registry18790 and transport13340 are unverified
   candidates, not permission to reclaim occupied ports. Do not expose lab APIs
   publicly or reuse QA60's ports, run paths, tokens or stores.
3. Give each peer a new independent DB, P2P and blob directory, stable lab node
   identity and namespace secrets, genuine membership/roles and scoped API
   tokens. Persist these deliberately for restart tests; a volume alone does not
   persist EnvBackend namespace overrides. Provision Owner/Editor/Manager and
   recipient Viewer through actual supported doors. Use real reachable
   EndpointAddr/bootstrap data and custom relay configuration, with HTTPS for a
   non-loopback relay receiving an access token. No fake identity, broad Owner
   token, DB copy or raw-store opener substitutes for membership.
4. Keep three evidence lanes distinct: **18 direct-loopback headless scenarios**,
   **four required relay-container tests**, and a **new composed real-fleet query
   exercise**. The current harness binary and GUI QA launcher explicitly disable
   relays. Their outcomes cannot establish relay-routed application queries.

The supported Podman override for the existing CI cargo command is:

```powershell
$env:FE_CONTAINER_RUNTIME = 'podman'
$env:FE_REQUIRE_RELAY_CONTAINER = '1'
cargo test --release -p fe-ui -p fe-terrain -p fractalengine -p fe-sync --test relay_assisted_test -- --color never
```

This command is recorded, not executed. Require all four named tests to run and
pass, retain logs and verify exact owned-container cleanup. An optional skip or
zero matching tests earns no acceptance. The workspace-qualified headless runner
is `cargo run --release -p fractalengine-test-harness`; its substring filter must
be cleared for the full18. Schedule execution under the existing final integrated
test workflow rather than disrupting the current build/GUI run.

## Composed acceptance

Before querying, prove each declared shard host actually holds its expected rows
and that the aggregate truth is spread across peers. Compare actual REST query,
MCP query_timeseries and analytics results against an independent expected union,
including exact counts/sums/min/max, latest-per-anchor, half-open windows,
out-of-order appends, deduplication and covered/missing/answered metadata. A
single-peer distributed request or fake sync seam is not this proof.

Cover role/scope denial without remote work, Viewer write refusal, real role
loss/delegation and revoked/expired tokens; compatible schema evolution and
rejected invalid values; exact large integers; transforms plus unchanged earlier
history; malformed/unavailable history, pagination and rollback. Include fresh
recipient asset misses with actual byte transfer/hash verification, cancellation
and rendering/picking on the normal GUI. Older fixtures that manually copy blob
bytes do not satisfy that network leg.

Prove relay-only application traffic separately: relay URLs can later hole-punch
a direct path. Enforce actual peer-to-peer isolation while allowing only the
intended relay and required control traffic. Preserve direct-path negative probes
and actual connection-path/relay receipts. Test relay interruption/recovery,
exclusive R=1 host absence, R=2 mirror survival, bounded timeouts and honest
partial results. A declared policy or configured relay URL is insufficient.

Restart each disposable peer with the same real identity/state while another
peer is offline; read back exact schema, rows, historical entries, asset bytes
and query outcomes. Record distinct process lifetimes, convergence, negative
cases, full logs and cleanup. This does not waive retained campaign-store startup
restrictions. GUI native selection, rendering/material/intersection reports,
Query/History/chart UI and screenshot judgment remain desktop acceptance.

## Optional Kubernetes overlay

After a useful Podman lab, optionally package the same service/identity/state and
fixture contracts as an overlay: transport relay Service, three independently
owned application-peer state volumes and credentials, API/readiness services,
registry and bounded teardown. It remains a deployment option, not a new
mandatory release gate or a claimed existing fixture.

Choose an explicit dedicated provider/context after capacity review. Podman
Desktop's [Kind documentation](https://podman-desktop.io/docs/kind) directs
Windows users to rootful mode; the separate [Kind rootless
requirements](https://kind.sigs.k8s.io/docs/user/rootless/) include cgroupv2 and
delegation. Do not silently mutate the global Podman default or assume the
reported machine already meets those requirements.

Relay-only Kubernetes acceptance needs an enforcing network solution and measured
negative probes. [Kubernetes NetworkPolicy
documentation](https://kubernetes.io/docs/concepts/services-networking/network-policies/)
states that the plugin must implement enforcement; creating a policy resource
alone has no effect. Cluster installation, provider configuration and the overlay
need their own review before action. The four current Podman relay tests do not
require Kubernetes.

## Next reviewable deliverable

Independently review this persisted proposal and its exact evidence copies. Then
author the isolated Podman lab configuration, credential/state lifecycle and
scenario driver with explicit ports and measurable relay-only enforcement. No
implementation, deployment, goal replacement or acceptance credit is implied by
this persistence step.
