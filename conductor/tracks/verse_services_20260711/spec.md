---
type: Track Spec
title: Verse Services — Opt-In Per-Verse Centralization as Accelerator-Only Plugins
tags: [spike, spec-only, verse_services_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
decisions: ../../decisions/hexon-p2p-commons-20260711.md
---

# Specification: Verse Services (Accelerator-Only)

**Track ID:** `verse_services_20260711`
**Type:** Spec / design (spec-only; implementation after auth + delta foundations)
**Status:** Draft — queued. 2026-07-14 alignment: FOUNDRY-CANDIDATE — deferred out of this repo to the separate hexon-foundry project; kept here as reference only.
**Decision basis:** decisions §D3 (accelerator-only), §D4 (serverless operating mode)

## Overview

A verse may **opt in** to centralized services — seeding, caching, presence hosting,
order-hinting, serverless materialization — delivered as a plugin service class. The
governing invariant (§D3, non-negotiable): **the signed op-log remains the state of
record, and any member can reconstruct the verse without the service.** Services
accelerate; they never become the sole authority. This is the "Hubs survival condition":
prior-art 3D worlds that kept authority in a server died with their operator (Third Room);
the one whose users held their bytes survived its sponsor's death (research report,
stage 4 §1.1).

The relay (`fractalengine-relay`) and hexon registry (`fe-hexon-registry`) are re-framed
as the **first two instances** of this class — currently special-cased infrastructure,
eventually expressible through the same service contract.

## Design sketch

### Service class contract

- A **verse service** is a long-running process bound to a verse (or petal) scope,
  declared via an extended `CapabilityManifest` — new capability family alongside the
  existing `storage.read`/`storage.write`/`query.select`: e.g. `service.host`,
  `service.seed`, `service.presence`, `service.materialize`.
- Fail-closed, per the existing `fe-plugin` capability posture (the best-designed of the
  auth surfaces per `auth_policy_pattern_20260710`'s survey). Granting `service.*` is a
  verse-owner decision recorded as a signed op in the op-log (so the grant itself is
  auditable and replicated).
- **Reconstruct-without-service invariant, made testable:** for every service type, a
  conformance test must demonstrate a member materializing correct verse state from
  op-log + blobs alone with the service offline.

### Initial service types (priority order)

1. **Seeder** (§D2/§D4): always-on holder of the verse's hexon deltas + bucket blobs;
   what the relay container already is in practice. Quota/GC per
   `hexon_p2p_bucket_20260710` FR-6 with a larger budget.
2. **Presence host**: authoritative-ish liveness aggregation (the `VersePeers` consumer),
   strictly advisory data — safe to centralize because it is reconstructible and
   non-durable.
3. **Serverless materializer** (§D4): stateless worker that pulls content-addressed
   deltas, materializes a queryable view (SurrealDB instance or static export), serves
   T2 consumers (dashboards, analytics), and can be killed/restarted with zero durable
   state of its own.
4. **Order-hinter**: publishes a suggested total order for recent ops to reduce merge
   ambiguity. *Hint only* — materialization must not require it. (A binding sequencer was
   considered and **deferred**, not rejected — decisions §D3.)

### Security surface (why this waits for auth)

A long-running service with network reach is the largest capability grant in the system.
Prerequisites before implementation:
- `auth_policy_pattern_20260710` policy engine (service actions go through
  `evaluate(service_did, action, scope)` like any other subject — a service is a subject,
  not a superuser).
- Real per-op signing (decisions §D5-1) so service-authored ops (e.g. compaction
  checkpoints) are attributable and the packager-vs-author distinction from the delta
  spec holds.
- Production `HostEnv` wiring (open residual from `analytics_extension_api_20260710`).

## Out of Scope

- Binding sequencer authority (deferred — revisit after accelerator-only services are
  proven in production).
- Payment/billing for service operation (relay-as-*paid*-seeder economics — future track).
- Any implementation this round.

## Open Questions

1. Does a verse service run in-process (fe-plugin Wasmtime/Rhai sandbox) or as a separate
   binary speaking the same protocol (like the relay today)? The relay precedent suggests
   separate-binary for network-facing services, sandbox for compute-only ones.
2. How does a verse *elect/revoke* a service? Natural answer: the same signed
   grant/revoke ops as membership (auth_policy causal DAG) — confirm no special-casing
   is needed.
3. Service discovery: registry entry per service? Fits the registry's existing
   "single load-bearing discovery path" role.
