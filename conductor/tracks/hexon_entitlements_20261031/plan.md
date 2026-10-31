---
type: plan
track: hexon_entitlements_20261031
title: "Implementation Plan: registry protocol v1 + PackageStore + EntitlementVerifier"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: f55ca21
partitions_confidence: hypothesis
---

# Implementation plan: hexon_entitlements_20261031

> Citations computed @`f55ca21`. Partitions are a hypothesis — run `/slice` to collision-proof
> before launching a swarm; S1/S2 touch disjoint crates and are safe to parallelize as drafted,
> but S3's exact file list may shift once `fe-format::entitlement` has a real shape.

## 0. Assumptions (proceeding on spec §6's recommendations unless ratified otherwise)

- **D-ENT-1** (OD-1) `FE_REGISTRY_ALLOW_OPEN=1` hard-required escape hatch. *Reversal: low.*
- **D-ENT-2** (OD-2) protocol version via response header + `/health` field, no route prefix
  change. *Reversal: low.*
- **D-ENT-3** (OD-3) `PackageStore::open_path` returns a local path; non-filesystem impls pay a
  tempfile download cost, `fe_format::HexonArchiveIndex` unchanged. *Reversal: medium — revisiting
  means touching `fe_format`'s reader, a cross-cutting change.*
- **D-ENT-4** (OD-4) recipient-wrap (X25519 + XChaCha20-Poly1305) for `encrypted_key`, wrapped to
  the issuer's key and re-wrapped per licensee in the token. *Reversal: medium — token shape and
  `entitlement_gate` both assume this.*
- **D-ENT-5** (OD-5) EdDSA JWT token mirroring `ApiClaims`. *Reversal: low (one module).*
- **D-ENT-6** (OD-6) static pinned-key config list for issuers. *Reversal: low.*
- **D-ENT-7** (OD-7) plain outward hyperlink for `payment_verification_url`. *Reversal: trivial.*
- **D-ENT-8** (OD-8) ship `LocalIssuer` in the open crate. *Reversal: low — it is additive; can be
  hidden behind a feature flag later without breaking verification.*
- **D-ENT-9** (OD-9) format-spec bump to 1.1.0 now. *Reversal: n/a (docs).*

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** No slice runs cargo build/test/clippy itself; the
  orchestrator gates once per wave (`-p fe-format -p fe-hexon-registry`, then
  `-p fe-hexon -p fe-ui`, then workspace), full sweep once at track end.
- **NFR-1 is non-negotiable.** No payment, pricing, revenue, or payment-provider code or wording
  in any file this track touches. A reviewer finding this is a hard fail.
- **NFR-2 (backward compat).** Every existing test fixture with no `license.json`, or
  `license_type: Free`/`Attribution`, must keep passing unmodified — `EntitlementVerifier` is
  never invoked for them.
- **NFR-3 (registry stays dumb).** `fe-hexon-registry` gains no dependency on `fe-hexon` or on
  `entitlement.rs`. If a slice finds itself importing either into the registry crate, stop — that
  is scope creep into server-side gating, explicitly deferred (spec §3).
- **Docs convention:** terse one-line doc comments in code; rationale in the directory
  `AGENTS.md` section named per slice.
- **Tests model real flows**, table-driven for pure crypto/claims logic. No tautologies: every
  assertion must go red if the covered line were inverted.
- **No read-only reviewers:** S6 fixes its own findings.

## 2. Phase / slice table

| Phase | Slice | Wave | Model | Task | Owns (files) | Depends |
|---|---|---|---|---|---|---|
| Format | **S1** | W1 | sonnet | `fe-format::entitlement`: `EntitlementClaims` (EdDSA JWT, mirrors `ApiClaims`), `IssuerKeyring` (pinned `(kid, pubkey)` pairs), `wrap_content_key`/`unwrap_content_key` (X25519 ECDH, API shaped after `fe_identity::x25519::DeviceKeypair`), `seal_paid_entry`/`open_paid_entry` (XChaCha20-Poly1305, API shaped after `fe_canonical_log::crypto::aead::SealingKey` — no new dependency on that crate), the `EntitlementVerifier` trait + reference impl, `LocalIssuer::mint` (D-ENT-8) — all behind a new opt-in `entitlement` feature | `fe-format/src/entitlement.rs` (new), `fe-format/src/lib.rs` (`#[cfg(feature = "entitlement")] pub mod entitlement;`), `fe-format/src/license.rs` (doc pointer only, fields unchanged), `fe-format/src/AGENTS.md` (§entitlement), `fe-format/Cargo.toml` (`entitlement` feature gating optional `jsonwebtoken`, `x25519-dalek`, `chacha20poly1305`) | none |
| Registry | **S2** | W1 | sonnet | `PackageStore` trait (`async_trait`, dyn-safe) + `FsPackageStore` (behavior-preserving wrap of today's `dir`/`index`/`publish_lock`); `RegistryState` holds `Arc<dyn PackageStore>`; default-deny in `main.rs`/`config.rs` (`FE_REGISTRY_ALLOW_OPEN`); `protocol_version: 1` on `/health` + `X-Hexon-Registry-Protocol` header on every `/api/v1/*` response | `fe-hexon-registry/src/store.rs` (new), `fe-hexon-registry/src/routes.rs`, `fe-hexon-registry/src/config.rs`, `fe-hexon-registry/src/main.rs`, `fe-hexon-registry/src/lib.rs`, `fe-hexon-registry/src/index.rs`, `fe-hexon-registry/Cargo.toml` (adds `async-trait`), `fe-hexon-registry/AGENTS.md` (§store, §auth model) | none |
| **gate W1** | orchestrator | n/a | n/a | Serial check `-p fe-format -p fe-hexon-registry` once | n/a | S1, S2 |
| Client | **S3** | W2 | sonnet | `fe_hexon::entitlement_gate`: free-entries-only load with no/invalid token, `EntitlementRequired` state, decrypt path for a verified token; `remote.rs` and `package.rs`/`manifest.rs` call it; `InstalledCrateDto`-equivalent gains an `EntitlementState` enum | `fe-hexon/src/entitlement_gate.rs` (new), `fe-hexon/src/remote.rs`, `fe-hexon/src/package.rs`, `fe-hexon/src/manifest.rs`, `fe-hexon/src/AGENTS.md` (§entitlement-gate), `fe-hexon/Cargo.toml` (enables `fe-format`'s `entitlement` feature on the existing dependency edge) | S1 |
| **gate W2** | orchestrator | n/a | n/a | Serial check `-p fe-hexon` once | n/a | S3 |
| UI + docs | **S4** | W3 | sonnet | Hexon Manager row states (`installed-preview`/`entitlement-required`/`entitled`); `payment_verification_url` as a plain `ui.hyperlink`, never fetched inline | `fe-ui/src/dialogs/hexon_manager.rs`, `fe-ui/src/dialogs/AGENTS.md` | S3 |
| UI + docs | **S5** | W3 | haiku | `docs/hexon-format-spec.md` → 1.1.0: registry protocol v1 section, entitlement token + encrypted-entry wire format, fix the `## API Endpoints` RBAC-column drift noted in spec §2.2, backward-compat note | `docs/hexon-format-spec.md` | S1, S2 |
| **gate W3** | orchestrator | n/a | n/a | Serial check `-p fe-ui`, then full workspace check once | n/a | S4, S5 |
| Close | **S6** | W4 | opus | Review-and-fix (fresh context): `/code-review high` + mandatory `/security-review` (crypto: token forgery, replay, wrap-key confusion; auth: default-deny bypass paths; user input: `payment_verification_url` rendering, registry publish body parsing). Fixes own findings | `conductor/tracks/hexon_entitlements_20261031/review-s6.md` (new) + fix-only edits to S1-S5 files | S4, S5 |
| Close | **S7** | W5 | haiku | Docs consolidation: root `AGENTS.md` pointer, `conductor/tracks.md` row, plan ticks | `AGENTS.md`, `conductor/tracks.md`, this `plan.md` | S6 |
| **sweep** | orchestrator | n/a | n/a | Full workspace test + clippy `-D warnings` once | n/a | S7 |

## 3. Flow tests per slice

Each name is the test to write. Each asserts an observable outcome, not an implementation detail.

**S1 (`fe-format::entitlement`, table-driven pure crypto)**
- `mint_verify_round_trip_recovers_the_exact_content_key`: the AC-3 core proof.
- `tampered_signature_byte_is_rejected`.
- `expired_claims_are_rejected`.
- `content_hash_mismatch_is_rejected` (token for archive A does not verify against archive B).
- `unknown_kid_is_rejected` (not signed by any pinned issuer key).
- `wrap_unwrap_round_trip_recovers_the_original_key` (X25519, independent of the token layer).
- `seal_open_round_trip_recovers_original_plaintext` (AEAD, independent of the wrap layer).
- `tampered_ciphertext_byte_fails_aead_tag_check`.

**S2 (`fe-hexon-registry`, Axum `Router` integration, real temp dirs)**
- `fs_package_store_behind_the_trait_answers_every_route_identically_to_before`: AC-6, the
  behavior-preservation regression guard — run the existing route test suite unchanged against
  the refactored state.
- `registry_refuses_to_start_with_no_token_and_no_allow_open`: AC-5.
- `allow_open_still_starts_with_a_warning`.
- `health_and_api_responses_advertise_protocol_version_1`: AC-7.

**S3 (`fe-hexon::entitlement_gate`, real archive fixtures)**
- `free_entries_load_with_no_token_present`: AC-2 (half).
- `non_free_entry_refuses_without_entitlement_not_crashes`: AC-2 (half) — asserts a typed
  `EntitlementRequired`, never a panic or silent empty result.
- `valid_token_decrypts_every_non_free_entry_to_original_bytes`: AC-3 end-to-end through the
  real install path, not just the pure S1 round-trip.
- `free_or_untyped_license_archives_are_unaffected`: AC-1, the backward-compat regression guard.
- `tampered_token_never_falls_back_to_plaintext`: AC-4 through the client path.

**S4 (fe-ui)**
- `hexon_manager_row_shows_installed_preview_for_licensed_archive_with_no_token`.
- `hexon_manager_row_shows_entitled_after_verification`.
- `payment_verification_url_renders_as_hyperlink_never_fetched`: asserts no network call is made
  for the URL during render (a regression guard against an SSRF-shaped mistake).

**Cut / declined:** no tests for JWT library internals, egui layout pixel positions, or getter/
setter wrappers around `EntitlementClaims` fields.

## 4. Task checklist

### Phase: Format + registry seam (W1)
- [ ] S1: `fe-format::entitlement` primitives + `LocalIssuer`
- [ ] S2: `PackageStore` trait + `FsPackageStore` + default-deny + protocol version
- [ ] Gate W1 (orchestrator)

### Phase: Client enforcement (W2)
- [ ] S3: `entitlement_gate` + `remote.rs`/`package.rs` wiring
- [ ] Gate W2 (orchestrator)

### Phase: UI + docs (W3)
- [ ] S4: Hexon Manager states
- [ ] S5: format-spec version bump
- [ ] Gate W3 (orchestrator)

### Close
- [ ] S6: review-and-fix (opus), mandatory security review, recorded verdict
- [ ] S7: docs consolidation
- [ ] Full sweep (orchestrator, once)
- [ ] Retro + archive

## 5. Tripwires

- Never import `fe-hexon` or `entitlement.rs` into `fe-hexon-registry` (NFR-3) — the registry
  verifies nothing; it only serves bytes.
- `EntitlementVerifier` must never be invoked for a `Free`/`Attribution`/absent license (NFR-2) —
  gate this at the top of `entitlement_gate`'s entry point, not scattered across call sites.
- Keep `fe_format::license::License` and `fe_hexon::manifest::License` lexically distinct in
  every S3 import (`use fe_format::license::License as ArchiveLicense` or equivalent) — the
  naming collision (spec §2.1, §7) is exactly the kind of mistake that compiles silently wrong.
- `PackageStore::open_path`'s tempfile contract (D-ENT-3) is a public seam contract — changing
  its signature is a versioned, documented change (`conductor/open-core.md` §Seams).
- No payment-provider name, price, or "buy"/"purchase" string anywhere in S1-S5 (NFR-1) — grep
  for `checkout|purchase|pric(e|ing)|\$\d` across the diff before S6 signs off (hits on the
  existing `payment_*` field names are expected).
- `fe-format::entitlement` stays behind its opt-in feature; `cargo tree -p fe-hexon-registry`
  must not gain `jsonwebtoken`/`x25519-dalek`/`chacha20poly1305` from this track (NFR-3).
- Decrypted entry bytes are never written back to disk (spec §4.5/§4.6); otherwise token expiry
  bounds nothing.

## 6. Commit strategy

One commit per slice after its wave gate is green: `feat(fe-format): S1 entitlement primitives …`,
etc. S6 lands as `fix(...)` commits plus `review-s6.md`. S7 is a `docs(...)` commit. Pushes held
for user approval per standing directive.
