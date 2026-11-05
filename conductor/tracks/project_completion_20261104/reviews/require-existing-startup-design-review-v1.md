---
type: Review
title: Require-existing startup independent design review
date: 2026-11-04
---

# Require-existing startup v1: independent source-design review

**APPROVED AS A BOUNDED DESIGN**, for `.omc/research/require-existing-startup-design-v1.md`, SHA-256 **`93c8a59920867e803ce2e95b677667a1d842754b81b17638eae2297f162cc73c`**. No blocking contract/design defect was found. The unresolved same-engine SDK admission seam is correctly identified as an engineering dependency, not an implemented API. This approves the next bounded source work, **not a retained store open/restart**, dependency patch or live preservation result.

Only this review was authored. Applied root/local instructions and shared UI guidance. Read-only design, existing implementation, retained hold and installed primary SDK inspection; no production/registry/source/Git mutation, implementation/parser/compiler/tests/build, apps/helpers, store/DB open, OS/process/native/API/network action. Other source/evidence work was preserved.

## Contract and practical alignment

The selected typed Normal/RequireExisting branch fits the completion plan's live retained-restart requirement and the existing explicit hold. It avoids scattered initialization booleans, another DB owner, an offline opener, a fresh fixture substitute and an invented owner. Loading the actual existing key and matching exact persisted bootstrap Verse.created_by with cardinality/format checks is the correct identity boundary. A client-token subject, current PID or missing historical owner witness is not substituted. Missing/malformed/foreign data refuses rather than healing identity, granting ownership or seeding replacement content.

The ordering is materially stronger than launcher fixture suppression: capability/policy refusal precedes stopping old lifetimes; key/secret/path admission precedes create-capable helpers; the sole continuously owned engine verifies scope/schema/version/owner before logical initialization or handle/service readiness; RequireExisting skips application DDL/reconcile/migration/backfill/role initialization and rejects later Seed. Normal startup compatibility remains explicit. Fresh-mode policy clearing, immutable failed attempts, exact pair reseal/outcome ownership and refusal without automatic fresh fallback are appropriate.

The design correctly **does not require zero physical I/O or unchanged DB file hashes**. Valid lock bookkeeping, WAL replay and ordinary maintenance are distinct from replacement logical stores, namespace/schema creation, corruption repair, format migration or discarded recovery data. Positive evidence must compare actual logical/identity/history/blob invariants and report physical maintenance separately. No extra immutable-files blocker is imposed by this review. Deferring SDK logical bootstrap until owner acceptance is the design's explicit selected v1 boundary; a supported weaker internal-maintenance contract may be brought back with its concrete effects for a bounded decision rather than silently asserted equivalent.

## Source feasibility checked

- Actual `fe-database/src/lib.rs:517-616` ordinary-opens, applies schemas, tolerates some clock errors as zero, publishes a shared clone and then reconciles/migrates/starts role handling. Thus skipping main's Seed alone cannot implement the retained contract, and the existing HLC fallback must not hide retained read errors.
- Installed `surrealdb-3.0.5/src/engine/local/native.rs:80-106` calls Datastore::new, check_version, bootstrap and optional configured-root credential initialization before sending connection-ready. An owner query after normal Surreal::new is later than that chain. Existing Config exposes timeout/capability/maintenance-interval choices in the inspected file, not a proved pre-bootstrap existing-store admission mechanism. Adjusting maintenance intervals or using later read-only transactions does not establish the missing opening/order guarantee. This is a finding about the inspected path, not exhaustive absence of every SDK option.
- Installed `surrealkv-0.21.1/src/lockfile.rs:69-95` uses create/truncate before exclusive-lock acquisition. A safe retained branch must address actual open/lock/catalog behavior; a path-existence check followed by this call is not an existing-only admission proof. Normal lock writes themselves are not alleged logical data loss.
- The earlier independently reviewed store-open trace establishes missing-manifest/create-directory paths. Existing-only catalog/version checks must occur at a protected boundary, with actual object/engine identity retained across admission-to-serving. A generic canonical-string check or close/reopen is insufficient.

No SDK cache mutation, new raw KV forensic reader or unsupported router bypass is approved. The design honestly leaves callback signatures, read-only execution capability and actual Windows identity handoff unproved for exact source review. Required schema/version compatibility should be an explicit supported contract, not a brittle comparison to every incidental SDK table.

## Minimal next engineering task

Take **one isolated admission-seam implementation spike** against the locked SDK, on source/disposable fixtures only. Its deliverable should be a concrete patch and source call-order/ownership report, not another exhaustive audit:

1. Trace only the already named connector/core/SurrealKV opening points to determine whether an existing supported configuration supplies existing catalog opening plus pre-logical-bootstrap read-only admission. If it does, wire that exact supported seam. Do not search indefinitely for unrelated SDK options.
2. Otherwise present the smallest explicitly pinned vendor/upstream patch: default Normal behavior unchanged; RequireExisting refuses missing directories/catalog/version; existing lock is acquired without create/truncate-before-ownership; actual opened identities remain bound; one read-only admission phase precedes version creation/bootstrap/readiness and promotes the **same engine/session** after success. Specify how the admission query cannot perform writes and how cancellation/error drops ownership without publishing a handle. Dependency/vendor scope and the concrete API need their own review before integration; no registry edits.
3. Demonstrate the proposed source ordering with authored disposable missing/duplicate/wrong-owner, missing catalog/version, lock contention/replacement and interrupted-admission cases. Root owns execution at the agreed integrated boundary. If a concrete SDK limitation prevents that seam, report the exact limitation and smallest alternative contract for a decision; do not convert ordinary housekeeping into an invented zero-write blocker or release the hold.

Application policy parsing/load-existing identity helpers can be prepared in parallel within assigned ownership, but no retained-enabled launcher path should ship as accepted before the seam exists. Subsequent application wiring should stay in the existing launcher/main/identity/sole-worker modules described by the design, with early refusal, checked clocks, no Seed and delayed external services. No broad new framework is required merely to express those phases.

## Acceptance boundary

The proposed negative/positive disposable matrix and sole-live-owner readback strategy are appropriate. Source authorship and generic initialization tests cannot prove unchanged retained histories, real owner/provenance, asset bytes, a checked post-resume edit or restart durability. Preserve facts through version106 and the unavailable historical prefix; no new baseline, adoption or owner field may be fabricated. Exact source review and successful disposable cases must precede any separately authorized retained restart. Native/peer/rendering, full campaign, final sweep, CI and release gates remain independent. Current retained roots remain **HOLD**.
