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
> Operating services for other people (multi-tenant hosting, account management, catalogue curation) is outside this repository.

Consequences:
- **No crippleware.** Nothing a single operator needs is held back. You can run your own relay, your own registry, and your own plugins, with no license keys.
- **Open interfaces, not open operations.** This repository defines the formats, protocols and extension seams. Anyone, the maintainers included, can build services against them.
- **One-way dependency.** Code here never depends on, names, or feature-flags any external service.

## What this repository owns

| Area | Open capability | Track |
|---|---|---|
| Engine, GUI, headless relay, DB, sync, HTTP/MCP API | Everything | (existing) |
| Hexon map-package format | Spec + `fe-format` / `fe-hexon` | (existing) |
| Registry | Reference self-hosted registry + **registry protocol v1** + `PackageStore` trait + default-deny auth | [hexon_entitlements_20261031](./tracks/hexon_entitlements_20261031/) |
| Licensed packages | `EntitlementVerifier`: verify a signed entitlement token, decrypt licensed entries, load `free_entries` as a preview | [hexon_entitlements_20261031](./tracks/hexon_entitlements_20261031/) |
| Relay operations | OpenMetrics `/metrics`, `UsageEvent` sink trait, TLS/backup/self-hosted-iroh-relay docs, verse-advertised relays | [relay_operability_20261031](./tracks/relay_operability_20261031/) |
| Peer networking | Embedded members-only relay in `fe-relay`, relay-capable desktop mode, signed relay advertisements and gossip, network diagnostics | [peer_networking_support_20261031](./tracks/peer_networking_support_20261031/) |
| Plugins | Plugin host, publisher-signed package format, install from any protocol-v1 registry, signed revocation lists | [plugin_host_revival_20261031](./tracks/plugin_host_revival_20261031/) |
| Agent automation | Deny-by-default synthetic input surface beside the screenshot surface, for QA and accessibility automation | [agent_input_surface_20261031](./tracks/agent_input_surface_20261031/) |
| Distribution trust | Signed release artifacts, first `v*` release | [release_ci_20260429](./tracks/release_ci_20260429/) |

## Seams (stable extension points)

These names are canonical. External services build against them, so a change here is a versioned, documented change.

| Seam | Contract |
|---|---|
| **S-RELAY** | Opt-in `/metrics`. A `UsageEvent` + `UsageSink` trait (caller DID, scope, route class, bytes in/out, duration, status) with a no-op default. Also `/api/v1/health`, `/ready`. |
| **S-RELAY-ADVERTISE** | A verse advertises relay URLs in its own doc (`__relays`, versioned rows, Owner/Manager only, role-checked at read time on every replica, author-signed; the next phase adds an inner signature, expiry, bounds and health checks). Each peer keeps a local policy: `DialOnly` by default (the URL is listed and dialable, never inserted into the relay map), `Pinned`, or `Deny`. A relay becomes home-eligible only by a local pin or under the coverage rule (every verse the peer is in advertises it). Rows never carry credentials, advertised relays never receive the configured relay token, and a configured relay is never removed by advertisement processing. |
| **S-RELAY-SERVE** | An embedded iroh-relay server in `fe-relay` (opt-in at build time and run time, TLS only, refuses to start when misconfigured) with a members-only `AccessControl`: a client is admitted only if the endpoint id it proves in the relay handshake maps to a DID holding an explicit Viewer+ role in a served verse; `default_access`/`open_to_all` are never consulted; revocation evicts live sessions; per-endpoint and global connection caps plus per-client bandwidth limits; rate-limited abuse log without IPs. Any verse operator can run one for exactly their members, with no shared token. |
| **S-GOSSIP-SIGN** | Every gossip frame (`FEG1`) is signed by its author's identity key under a dedicated domain, bound to the verse and an HLC, replay-guarded per author; a frame that fails verification is dropped and never parsed as legacy; a `compat` → `require` rollout with a per-author latch. |
| **S-REGISTRY** | Registry protocol v1, versioned by an `X-Hexon-Registry-Protocol: 1` response header plus a `protocol_version` field on `/health` (no route prefix); a `PackageStore` storage trait; default-deny authentication. |
| **S-ENTITLEMENT** | An ed25519 entitlement token (package id + content hash, licensee DID, expiry, issuer key id) verified against pinned issuer keys. It gives effect to the `LicenseType::Paid`, `encrypted_key` and `free_entries` fields already defined in `fe-format/src/license.rs`. Any operator can run an issuer. Licence revocation is short token expiry plus refused reissuance; there is no per-token revocation list. Licensed plugins use this seam unchanged. |
| **S-PLUGIN** | Publisher-signed plugin packages (ed25519 over manifest + payload hash), configurable trust roots, a signed revocation list. The list is a package-level kill switch keyed by `(package_id, content_hash)` for withdrawn or malicious packages, accepted only from trust roots marked for revocation; it never revokes a licence. |
| **S-AGENT-GUI** | Screenshot, camera read-back and synthetic input surfaces. All are Owner-only and rate-limited. **Screenshots (and camera read-back) are on by default** on GUI hosts (`FE_API_SCREENSHOT_ALLOW=0` disables; headless hosts never have them). Synthetic input stays off by default (`FE_API_INPUT_ALLOW=1` opts in). Captures can be posed (camera / orbit / geo / slippy tile / node focus) in window or off-screen scene mode. User directive 2026-10-31; track `agent_vision_20261031`. |

## Contributing

Contributions are accepted under Apache-2.0, inbound = outbound. Anything that matches the rule above belongs here, and that includes features that the maintainers' hosted services also use.
