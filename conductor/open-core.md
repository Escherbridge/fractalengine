---
type: Boundary
title: Open-core boundary — what this repository owns
tags: [open-core, boundary, seams, licensing]
timestamp: 2026-10-31T00:00:00Z
resource: ./product.md
---

# Open-core boundary

FractalEngine is open-core. **This repository is the open core**, Apache-2.0, and everything a self-hoster needs lives here.
The maintainers also run hosted services and a commercial distribution. Those are built **on top of** this repository's public interfaces, in a separate codebase.
This page states where the line is, so contributors and operators know what to expect.
It realises decision D-12, which keeps the format and engine open: the
[decision register](./tracks/outstanding_decisions_20260715/spec.md), ratified in
[RUNBOOK.md](./RUNBOOK.md) §3, and [roadmap.md](./roadmap.md) §2.

## The rule

> Anything a self-hoster needs to **run, verify, or extend** FractalEngine is open and lives here.
> Operating services for other people (multi-tenant hosting, billing, marketplace curation) is outside this repository.

Consequences:
- **No crippleware.** Nothing a single operator needs is held back. You can run your own relay, your own registry, and your own plugins, with no license keys.
- **Open interfaces, not open operations.** This repository defines the formats, protocols and extension seams. Anyone, the maintainers included, can build services against them.
- **One-way dependency.** Code here never depends on, names, or feature-flags any external service.

## What this repository owns

| Area | Open capability | Track |
|---|---|---|
| Engine, GUI, headless relay, DB, sync, HTTP/MCP API | Everything | (existing) |
| Hexon map-package format | Spec + `fe-format` / `fe-hexon` | (existing) |
| Registry | Reference single-tenant registry + **registry protocol v1** + `PackageStore` trait + default-deny auth | [hexon_entitlements_20261031](./tracks/hexon_entitlements_20261031/) |
| Licensed packages | `EntitlementVerifier`: verify a signed entitlement token, decrypt licensed entries, load `free_entries` as a preview | [hexon_entitlements_20261031](./tracks/hexon_entitlements_20261031/) |
| Relay operations | OpenMetrics `/metrics`, `UsageEvent` sink trait, TLS/backup/self-hosted-iroh-relay docs, verse-advertised relays | [relay_operability_20261031](./tracks/relay_operability_20261031/) |
| Plugins | Plugin host, publisher-signed package format, install from any protocol-v1 registry, signed revocation lists | [plugin_host_revival_20261031](./tracks/plugin_host_revival_20261031/) |
| Agent automation | Deny-by-default synthetic input surface beside the screenshot surface, for QA and accessibility automation | [agent_input_surface_20261031](./tracks/agent_input_surface_20261031/) |
| Distribution trust | Signed release artifacts, first `v*` release | [release_ci_20260429](./tracks/release_ci_20260429/) |

## Seams (stable extension points)

These names are canonical. External services build against them, so a change here is a versioned, documented change.

| Seam | Contract |
|---|---|
| **S-RELAY** | Opt-in `/metrics`. A `UsageEvent` + `UsageSink` trait (caller DID, scope, route class, bytes in/out, duration, status) with a no-op default. Also `/api/v1/health`, `/ready`. |
| **S-RELAY-ADVERTISE** | A verse can advertise relay URLs, which peers add or remove at runtime (iroh `Endpoint::insert_relay`/`remove_relay`). Policy-gated, and peers may pin or deny. |
| **S-REGISTRY** | Registry protocol v1, versioned; a `PackageStore` storage trait; default-deny authentication. |
| **S-ENTITLEMENT** | An ed25519 entitlement token (package id + content hash, licensee DID, expiry, issuer key id) verified against pinned issuer keys. It gives effect to the `LicenseType::Paid`, `encrypted_key` and `free_entries` fields already defined in `fe-format/src/license.rs`. Any operator can run an issuer. |
| **S-PLUGIN** | Publisher-signed plugin packages (ed25519 over manifest + payload hash), configurable trust roots, a signed revocation list. |
| **S-AGENT-GUI** | Synthetic input and screenshot surfaces. Both are off by default, Owner-only and rate-limited. |

## Contributing

Contributions are accepted under Apache-2.0, inbound = outbound. Anything that matches the rule above belongs here, and that includes features that the maintainers' hosted services also use.
