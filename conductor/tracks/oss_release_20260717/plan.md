---
type: Implementation Plan
title: "Implementation Plan: OSS Release — phased pre-push checklist"
tags: [oss_release_20260717]
resource: ./spec.md
---

# Implementation Plan: OSS Release Checklist

## Overview

Phases 1–4 are agent-executable now. Phase 5 is the ratification wall — every item is
**BLOCKED-ON-USER** and nothing in Phase 6 may run until Phase 5 is fully resolved.
Ordering follows the audit's blocking analysis: legal scaffolding verification →
truth alignment → community/CI → metadata → ratification → preflight + push.

## Phase 1 — Verify the 2026-07-17 scaffolding (agent)

- [x] 1.1 `LICENSE-APACHE` present at root with correct canonical text
      (D-69 ratified 2026-07-17: Apache-2.0 single; LICENSE-MIT removed).
- [x] 1.2 Root `[workspace.package]` carries `license = "Apache-2.0"`,
      `repository`, `edition`, `rust-version`; all member crates inherit via
      `license.workspace = true` (spec FR-6).
- [ ] 1.3 `deny.toml` allowlist correct (MIT/Apache-2.0/BSD/ISC/MPL-2.0 + BUSL
      exception scoped to `surrealdb*` only); `cargo deny check licenses` wired into
      `build-artifacts.yml` (spec FR-3).
      — AUDIT 2026-10-10: PARTIAL — allowlist + `surrealdb`/`surrealdb-core`-only BUSL exception are correct (`deny.toml:4-32`), but no `cargo deny` step exists in `.github/workflows/build-artifacts.yml` (only the clippy step at :73).
      — 2026-10-11 (cargo-deny 0.20.2): CI WIRED for `advisories bans sources`. The
        lint job has an `EmbarkStudios/cargo-deny-action@v2` step after Clippy, and it
        runs green locally ("advisories ok, bans ok, sources ok"). Changes in deny.toml:
        - The surrealdb/surrealdb-core `license-files` hashes are filled (xxHash32
          `0xbb1191d7`).
        - The BUSL exception now names all five `surrealdb*` engine crates (+protocol,
          types, types-derive, as THIRD-PARTY-LICENSES.md scopes it).
        - `unmaintained = "workspace"`.
        - 34 vulnerability/unmaintained IDs are explicitly `ignore`d, each with a
          reason, and mirrored in SECURITY.md.
        - Three zero-graph-change patch bumps landed in Cargo.lock: anyhow 1.0.103,
          crossbeam-epoch 0.9.20, quinn-proto 0.11.15. The bumps for ammonia, h2,
          rustls and webbrowser were rejected because they would change the graph.

        **`licenses` is held out of CI — BLOCKED_DECISION (allow-list policy).**
        18 crates fail. Widening the allow-list is a user call:
        - Unlicense: async_io_stream 0.3.3, ext-sort 0.1.5, pharos 0.5.3,
          ws_stream_wasm 0.7.5
        - BSL-1.0: clipboard-win 5.4.1, error-code 3.3.2
        - CC0-1.0: constant_time_eq 0.1.5, hexf-parse 0.2.1, tiny-keccak 2.0.2
        - MIT-0: encase, encase_derive, encase_derive_impl 0.12.0
        - OFL-1.1 + Ubuntu-font-1.0: epaint_default_fonts 0.33.3 (egui's bundled fonts)
        - CDLA-Permissive-2.0: webpki-roots and webpki-root-certs 0.26.11 + 1.0.7

        Also: `Unicode-DFS-2016` is never encountered and could be dropped. Once the
        list is ratified, add `licenses` to the CI command and tick this item.
- [x] 1.4 `THIRD-PARTY-LICENSES.md` names SurrealDB BUSL-1.1 + the Additional Use
      Grant terms explicitly; note the regeneration command used.
      — AUDIT 2026-10-10: DONE d5cc361/b555082 — `THIRD-PARTY-LICENSES.md:8-34` (BUSL-1.1, Additional Use Grant DBaaS carve-out, Change Date) + `:36-47` regeneration (`cargo deny check licenses`, `cargo about generate`); surrealdb 3.0.5 still matches `Cargo.lock`.
- [x] 1.5 Re-grep `"00".repeat(64)` and reconcile against the spec FR-2 register
      (13 sites, all fe-database, as of 2026-07-17).
      — AUDIT 2026-10-10: OPEN, DRIFTED — the re-grep now finds 20 sites, still all fe-database (`handlers/crud.rs` 6, `space_manager.rs` 5, `queries.rs` 3, `handlers/entity_property.rs` 2, `role_manager.rs` 2, `handlers/transform.rs` 1, `session_cache.rs` 1); the FR-2 register needs updating. D-CL24(5) fixes these legacy rows as permanently unsigned.
      — DONE 2026-10-11: re-grepped after the auth-policy and mcp-api-surface batches.
        `git grep -n '"00"\.repeat(64)' -- '*.rs'` returns 22 hits in 9 fe-database
        files: 21 production sites plus `op_log.rs:420`, a `#[cfg(test)]` fixture. The
        two sites added since the audit are `handlers/elevated_query.rs:137` and that
        fixture. The spec FR-2 register is rewritten with every site as a linked
        file:line plus its purpose, under a D-CL24(5) header. SECURITY.md mirrors the
        count and the citation.

## Phase 2 — Truth alignment in shipped docs (agent)

- [x] 2.1 README claims audit (spec FR-5.3): spatial-analytics positioning per
      roadmap, MSRV unified with BUILDING.md (1.83+), thread topology corrected to 7
      (add plugin host to the diagram), honest status section (REL-06).
      — AUDIT 2026-10-10: DONE as written — `README.md:5` spatial analytics, `:24` "Status: alpha", `:63` Rust 1.83+, `:111-134` seven-thread diagram incl. T7 Plugin Host. NEW TRUTH GAP: no shipped binary depends on `fe-plugin` (only fe-plugin-test and extensions/iot-bridge do), so T7 does not run in `fractalengine` or `fe-relay`; the diagram must say so (see 6.3).
- [x] 2.2 README security claims amended per spec FR-2 path (b): op-log signing
      stated as not-yet-implemented — unless Phase 5.3 resolves to path (a).
      — AUDIT 2026-10-10: DONE — `README.md:51-53` known limitation (placeholder signatures) + `:253` "per-op ed25519 signing is designed but not yet implemented".
- [x] 2.3 SECURITY.md known-limitations section lists the unsigned-op-log register
      and any open RBAC gaps (spec FR-7).
      — AUDIT 2026-10-10: DONE — `SECURITY.md:28-44`. STALE BULLET: "The sync write gate is permissive" is false since F3 (inbound rows need Editor+ at the DB thread, `fe-database/src/handlers/replicated_row.rs:261`); fix in the 6.3 claims re-run.
- [x] 2.4 BUILDING.md: add RUST_MIN_STACK=134217728 / surrealdb-core deep-recursion
      gotcha (+ poisoned-rmeta `cargo clean -p` note); fix the GHCR org pull URL
      (REL-09).
      — DONE 2026-10-11: `BUILDING.md:128` is now `docker pull ghcr.io/escherbridge/fractalengine-relay:latest`, matching `release.yml:208` `ghcr.io/${{ github.repository_owner }}` (lowercased by GHCR/metadata-action). A note was added that `latest` moves only on stable tags. No other `jadezaher` hits outside conductor history.
      — AUDIT 2026-10-10: PARTIAL — RUST_MIN_STACK + `cargo clean -p surrealdb-core` note present (`BUILDING.md:182-185`); GHCR URL still wrong (`BUILDING.md:128` `ghcr.io/jadezaher/...` vs `release.yml:208` `ghcr.io/${{ github.repository_owner }}`, i.e. Escherbridge).
- [x] 2.5 README license section: Apache-2.0 text + contribution-licensing sentence
      (landed 2026-07-17 with D-69 ratification).

## Phase 3 — Community health + CI gates (agent)

- [x] 3.1 CONTRIBUTING.md (build via BUILDING.md, pre-commit trio
      `cargo fmt && cargo clippy -- -D warnings && cargo test`, licensing-of-
      contributions note) (REL-07). (done 2026-07-17)
- [x] 3.2 CODE_OF_CONDUCT.md (Contributor Covenant 2.1) (REL-07). (done 2026-07-17)
- [x] 3.3 `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml` + PR template
      referencing the test policy (REL-07). (done 2026-07-17)
- [x] 3.4 Fast lint job (fmt --check + clippy -D warnings, Linux-only) in
      `build-artifacts.yml` (spec FR-8). (done 2026-07-17)
- [x] 3.5 `release.yml` build-job timeouts 120→150 (parity with 43655f9).
      (done 2026-07-17)
- [x] 3.6 build-artifacts badge in README; document the scoped 3-crate test sweep in
      the workflow (widening is D-66 — do not silently change scope).
      (done 2026-07-17)
- [x] 3.7a Docs polish, done parts: `#![warn(missing_docs)]` on fe-sdk;
      docs/unwrap-audit.md marked STALE (REL-11). (done 2026-07-17)
- [x] 3.7b Docs polish, remaining: fix root AGENTS.md SQLite→SurrealDB line
      (currently AGENTS.md:20); fill fe-sdk missing-docs gaps (REL-11).
      — AUDIT 2026-10-10: OPEN — `AGENTS.md:20` still says "SQLite (via fe-database)".
      — CONSOLIDATION 2026-10-10: AGENTS.md:20 fixed (SurrealDB/SurrealKV). Remaining: fe-sdk missing-docs gaps (needs a cargo doc/clippy run).
      — DONE 2026-10-11: `cargo +1.99.0 clippy -p fe-sdk --lib -- -W missing_docs` reports 0 missing_docs warnings, because the gaps were already filled by earlier doc passes. `cargo doc -p fe-sdk --no-deps` raised 1 broken intra-doc link (`transaction.rs:4` `PluginContext::transaction()`), which is fixed; it is now 0 warnings.

## Phase 4 — crates.io metadata verification (agent)

- [x] 4.1 Per-crate `description` for fe-sdk + fe-plugin-test at minimum (spec FR-6).
      — AUDIT 2026-10-10: DONE — `fe-sdk/Cargo.toml:8`, `fe-plugin-test/Cargo.toml:8`.
- [x] 4.2 `cargo publish --dry-run` green for fe-sdk and fe-plugin-test.
      (Granted-command exception must be explicit in the executing session; otherwise
      defer to the end-of-session sweep.)
      — DONE 2026-10-11 (cargo 1.99.0). `fe-plugin-test/Cargo.toml` now pins
        `fe-sdk = { version = "0.1.0", path = "../fe-sdk" }`; a path-only dependency
        cannot be published.

        Command: `cargo publish --dry-run -p fe-sdk -p fe-plugin-test --allow-dirty`.
        `--allow-dirty` is needed because the tree carries intentional uncommitted
        edits. Exit 0. Results:
        - fe-sdk: 20 files, 62.7KiB (17.3KiB compressed). Files: Cargo.toml,
          Cargo.lock, src/{AGENTS.md, api, context, events, lib, node, path_asset,
          primitive, property, query, scene, storage, texture, transaction, units,
          ui/mod}.rs.
        - fe-plugin-test: 14 files, 73.8KiB (17.2KiB compressed). Files: Cargo.toml,
          Cargo.lock, src/{AGENTS.md, assertions, fixtures, lib, mock_host,
          mock_storage, rhai_runner, spy}.rs, and
          examples/property-transformer/{manifest.json, plugin.rhai}.
        - Both verified by building from the unpacked tarball, with fe-sdk resolved
          from the local tmp-registry.

        Not checked by `--dry-run`:
        - Whether the crates.io names `fe-sdk` and `fe-plugin-test` are available.
          That is a D-53 publishing-gate concern (5.6).
        - Neither tarball includes a LICENSE file. The SPDX `license` field is
          present, but consider `include`-ing LICENSE-APACHE before a real publish.
- [x] 4.3 Root .glb strays (`camera.glb` 14-byte placeholder, `duck.glb`,
      `info_panel.glb`) relocated under assets/ or examples/; camera.glb replaced.
      — DONE 2026-10-11/12: moved to fractalengine/assets/samples/ (40590e9); camera.glb
      (a saved 404 page) deleted; duck.glb (SCEA, non-OSI) replaced by CC0 Khronos
      "Cameras" as cameras.glb (2 embedded cameras — keeps the camera regression asset);
      info_panel.glb verified CC-BY-4.0 (c) 2017 Cesium and attributed in
      THIRD-PARTY-LICENSES.md. Residual: 20 tracked SCEA Duck copies under
      fractalengine/assets/imported/ (runtime import cache) — user decision.
      — DONE 2026-10-11: `git mv` duck.glb and info_panel.glb into
        `fractalengine/assets/samples/`, the Bevy AssetServer root, so they now load as
        `samples/<name>.glb`. `git rm camera.glb`: its 14 bytes were the text
        "404: Not Found", not a GLB, and nothing referenced it. A replacement camera
        model is content work, not this item.

        `fractalengine/assets/samples/README.md` records provenance: the Khronos
        Duck, with an embedded camera that is the regression asset for
        `deactivate_foreign_cameras`, and the Khronos Box. **The licences are
        UNVERIFIED** (believed to be SCEA Shared Source 1.0 for Duck, which is
        non-OSI, and CC-BY 4.0 for Box). Confirm them, or replace the files, before
        the public release.

        Re-grep checks:
        - `ls *.glb` at the root is empty.
        - The only live-doc mention is the bare name in
          `docs/diagrams/11-user-interaction-flow.md:47` (a file-dialog pick, so no
          path is implied); it is left as-is.
        - Archive history is untouched.

## Phase 5 — Ratification wall (every item BLOCKED-ON-USER)

- [x] 5.1 **RESOLVED 2026-07-17 — D-69:** user ratified **Apache-2.0 single**
      (overrode the dual default; LICENSE-MIT removed, metadata + docs updated).
- [x] 5.2 **RESOLVED 2026-07-17 — D-70:** conductor/ **stays public**;
      conductor/README.md framing added, register marked draft where unratified.
- [ ] 5.3 **BLOCKED-ON-USER — REL-03 path choice:** implement real op-log signing
      (D5-1-adjacent, its own track if chosen) vs ship with disclosure (Phase 2.2/2.3
      text stands). Default posture staged: disclosure.
- [ ] 5.4 **BLOCKED-ON-USER — SECURITY.md contact:** the real private reporting
      channel (GitHub private vulnerability reporting and/or security@ address).
      — AUDIT 2026-10-10: text names GitHub private vulnerability reporting (`SECURITY.md:5-8`); the repo-setting toggle remains the user's action.
- [ ] 5.5 **BLOCKED-ON-USER — D-66:** ratify the scoped CI test sweep or order the
      widening before the badge goes public.
      — AUDIT 2026-10-10: still scoped — `build-artifacts.yml:118` `cargo test --release -p fe-ui -p fe-terrain -p fractalengine`; the RUNBOOK's "full-workspace run is left to CI" assumption does not hold.
- [ ] 5.6 **BLOCKED-ON-USER — D-53:** publishing gate (repo settings, tokens, live
      tag) — required for the eventual push/release but executed by the user.

## Phase 6 — Pre-push preflight + push (agent prepares, user pushes)

- [x] 6.1 Secrets re-scan across full history; record the tool + date in this plan.
      — 2026-10-11: gitleaks v8 (built via `go install`), run as
        `gitleaks detect --log-opts="--all" --redact`. It scanned 266 commits
        (18.4 MB) and raised 111 raw hits. A complementary `git log -p --all` regex
        sweep found 0 hits: PEM private keys, AKIA, ghp_/github_pat_, xox[bp]-,
        sk_live, password assignments (only `${{ secrets.GITHUB_TOKEN }}`), and the
        local Postgres PIN. Triage of the 111 hits:
        - 78 are public `did:key` ids in `scripts/hexon-serve-verify.log`.
        - 20 are test vectors or fixture DIDs: fe-canonical-log fixtures, cbor.rs,
          did_key.rs, key_wrap.rs doc, AGENTS.md, the envelope spec JSON, and the
          fe-hexon, fe-api and registry test fixtures.
        - 1 is a `Cargo.lock` false positive (the `crypto_secretbox` crate name).
        - **12 are 6 real local API bearer JWTs** (EdDSA, localhost:8765, roles
          viewer/manager/editor). They sit in the tracked
          `.claude/settings.local.json`, introduced in d3c5fab and STILL in HEAD as
          Claude Code permission entries.

        The JWTs all expired on 2026-05-31 and are scoped to one local dev petal. No
        signing key leaked. **USER-ESCALATION:** history is not rewritten (per policy).
        Decide whether to untrack `.claude/settings.local.json`; an agent may not edit
        permission files. "Zero findings" therefore holds for live secrets only.
- [ ] 6.2 **BLOCKED-ON-USER** Decision-register review: no public doc states an unratified default as
      settled; Phase 5 items all carry dated resolutions.
- [ ] 6.3 Final README claims audit re-run (spec FR-5.3) against the tree as it will
      push.
      — RE-RUN 2026-10-11 (after the auth-policy, mcp-api-surface and this batch's
        changes). Fixed:
        - The README MCP line now says 29 tools, 24 advertised off a sim-lab host,
          with `sim_*` hidden.
        - The REST create-node row documents the optional `asset_id` placement.
        - The Test/Lint section notes that CI also runs cargo-deny (advisories, bans,
          sources) and a bounded fuzz smoke.

        Re-checked with no change needed: the samples move (README never named the
        root .glbs), the 24-crate count (members = 24; fuzz/ is excluded), the
        signing disclosure (README:54, :258), and the 7-thread diagram note. Keep
        this item open for the final pre-push pass.
      — CONSOLIDATION 2026-10-10: partial re-run done — README Status section corrected (BI egress + IoT export moved to works-today, MCP vocabulary dropped from Planned, release matrix post-macOS, plugin host marked library-only incl. T7 diagram note, 24 crates); SECURITY.md stale "sync write gate is permissive" bullet replaced with the DB-thread Editor+ gate and its residuals. Keep open for the pre-push re-run.
- [x] 6.4 Final `"00".repeat(64)` re-grep matches the disclosed register.
      — 2026-10-11: `git grep -n '"00"\.repeat(64)' -- '*.rs'` = 22 (21 prod + 1 test) == spec FR-2 table. Re-run this exact command before push.
- [ ] 6.5 **BLOCKED-ON-USER:** the push itself (first public push is a user action).
