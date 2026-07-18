---
type: Implementation Plan
title: "Implementation Plan: Crate Consolidation Round 2"
tags: [crate_consolidation_r2_20260718]
resource: ./spec.md
---

# Implementation Plan: Crate Consolidation Round 2

## Overview

Evidence → gates → execution. Each merge candidate gets a decision gate with
the audit counter-evidence in front of it (spec FR-1..3); DEFER is a valid
outcome and gets recorded like an accept. Merges are structure-only (code
moves verbatim, fe-auth-absorption precedent). Full workspace sweep runs ONCE
after all accepted merges land, per the standing test-execution policy.
Watch for the rustc/surrealdb ICE workaround on big rebuilds
(RUST_MIN_STACK + `cargo clean -p` on phantom rmeta errors).

## Phase 1: Evidence refresh

Goal: current consumer graphs for all three pairs, no stale audit data.

- [ ] Task: FR-3 consumer map — grep all workspace Cargo.tomls for fe-query + use-site check (confirm/refute the preliminary fe-database dependency and what it actually uses); record as an artifact in this folder
- [ ] Task: FR-1/FR-2 dep snapshot — fe-plugin-test consumer list, fe-hexon-registry dep closure vs fe-hexon closure (sizes the docker-image cost of G-2)
- [ ] Task: Cross-reference task — add the format-merge gate to hexon_unification_20260716 (spec note + plan task there; sequencing note both directions)
- [ ] Verification: evidence artifacts complete; gates ready to decide [checkpoint]

## Phase 2: Decision gates

Goal: G-1/G-2/G-3 each decided ACCEPT or DEFER with rationale.

- [ ] Task: G-1 (fe-plugin-test→fe-plugin test-utils feature) — decide against the F8 counter-evidence (OSS plugin-author closure cost); record in decision register
- [ ] Task: G-2 (fe-hexon-registry→fe-hexon feature-gated bin) — decide against the F7 counter-evidence (docker closure, foundry extraction); record
- [ ] Task: G-3 (fe-query→fe-api) — decide from the Phase-1 consumer map (sole-consumer ⇒ mechanical; fe-database consumer ⇒ layering inversion, pick option a/b/defer); record
- [ ] Verification: three register entries exist, user-visible [checkpoint: gate outcomes recorded]

## Phase 3: Execute accepted merges

Goal: each ACCEPT lands as a verbatim code move + re-pointed consumers.

- [ ] Task: Execute G-1 if accepted — move sources under feature gate, re-point dev-deps, delete crate, preserve tests (TDD: consumer test suites unchanged-green)
- [ ] Task: Execute G-2 if accepted — registry as feature-gated bin, Dockerfile.hexon-registry re-pointed, docker build verified locally
- [ ] Task: Execute G-3 if accepted — fe-query into fe-api/src/query/ with features hoisted (or the scoped option chosen at the gate)
- [ ] Task: FR-4 bookkeeping per merge — workspace members, lock regen, crates.io metadata, AGENTS.md rationale, placeholder-sig register addresses if moved

## Phase 4: Close-out

- [ ] Task: Single end-of-track workspace sweep (test/clippy/fmt); docker registry image build if G-2 executed
- [ ] Task: Update oss_release_20260717 checklist + tracks.md board entry with final crate count
- [ ] Task: Retro + archive per track-per-feature workflow
