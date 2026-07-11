---
type: Track Spec
title: iroh 1.0 Upgrade — 0.35 → 1.0 Behind the VerseReplicator Seam
tags: [chore, iroh_1_0_upgrade_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
decisions: ../../decisions/hexon-p2p-commons-20260711.md
---

# Specification: iroh 1.0 Upgrade

**Track ID:** `iroh_1_0_upgrade_20260711`
**Type:** Chore / dependency migration
**Status:** Queued — **hard external deadline: December 31, 2026**
**Decision basis:** decisions §D5-2

## Forcing function

iroh shipped **stable 1.0 on June 15, 2026**, committing to wire-protocol stability —
and **n0's hosted relay support for the 0.35 wire protocol ends December 31, 2026**
(research report §6, stage 3 §1.1). FractalEngine is pinned to iroh 0.35 across
`fe-sync`/`fe-network` (the April 2026 pin was correct at the time — 0.35 was the last
production-quality release — but is now superseded). After the EOL, 0.35 peers lose
relay-assisted connectivity, which per the measured NAT numbers (~70% hole-punch success;
browser peers 100% relay) means a large fraction of peer pairs simply stop connecting.

## Scope

- Migrate `fe-sync` + `fe-network` from iroh/iroh-blobs/iroh-gossip/iroh-docs 0.35 to the
  1.x line. The April guidance "isolate iroh behind a trait boundary" was followed — the
  `VerseReplicator` trait is the seam; migration should not leak past it.
- Coordinate with `p2p_mycelium_completion_20260701`: if the real iroh-docs Engine wiring
  is still in flight when this starts, wire it against 1.x directly rather than porting
  0.35 work twice. Note iroh-docs' post-1.0 status (protocols versioned independently;
  Willow successor still unreleased) — re-verify iroh-docs 1.x-compat before committing.
- Re-verify BBR congestion-control selection on the 1.x API
  (`p2p_unblock_now_20260711` FR-4 may land the 0.35 version or defer to here).
- Self-hosted relay evaluation: EOL applies to n0's hosted 0.35 relays; running our own
  relay container (the deliberate §D3 seam) on 1.x is the resilient default.

## Acceptance criteria

- Workspace builds and the in-scope sweep passes on iroh 1.x.
- Relay-assisted connection verified against a 1.x relay (hosted or self-hosted docker
  relay container).
- No `VerseReplicator` consumer changes outside `fe-sync`/`fe-network`.

## Out of Scope

- WebTransport browser mode (iroh hasn't shipped it; track separately when it exists).
- Willow/iroh-docs successor migration (new unknown per report §7-2; revisit at
  implementation time).
