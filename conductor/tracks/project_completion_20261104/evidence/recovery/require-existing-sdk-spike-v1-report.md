---
type: ImplementationReport
title: Partial SDK admission spike - not integrable
date: 2026-11-04
---

# Require-existing SDK admission spike v1

**PARTIAL, concrete source components. Not a safe opener, not production-integrable, not approved or executed. Retained roots remain HOLD.** This spike implements the isolated logical admission component and two existing-entry helpers against actual locked source. The protected physical-open and connector dispatch are explicitly incomplete; there is no working retained launch recipe and no claim that ordinary opening became safe.

Only `.omc/research/require-existing-sdk-spike-v1/**` was written. The registry cache, vendor/production files, Cargo files and Git index were untouched. No compiler/parser/formatter/test/build, application/helper, store/DB opening, OS/native/process, API or network operation occurred. Source copies, static editing, hashing and textual diff generation were used. Normal source paths remain selected because no existing production call site is rewired.

## Frozen source packet

Read the approved design SHA256 `93c8a59920867e803ce2e95b677667a1d842754b81b17638eae2297f162cc73c` and independent design review. Inspected the named local SDK connector/config/core/SurrealKV seams first; the inspected Config does not supply the selected pre-bootstrap admission boundary. No exhaustive search claim is made.

Versions: surrealdb3.0.5, surrealdb-core3.0.5, surrealkv0.21.1, with surrealdb-types3.0.5 public values/kinds/variables used by the core source. The first three match the supplied reviewed Cargo.lock facts; this spike does not change dependency resolution.

`manifest.json`: **3983 bytes**, SHA256 **`bb8cf01cc51613e037fb52deccb853290008975b6ef4ae4d8f3311089d7a1dd9`**.

`source.patch`: **11806 bytes**, SHA256 **`289c95ec0d815e6ba669270fcd93f2f2697f56fed6b8034433fc1a39ac0e6c79`**. It is a plain unified source diff using crate-version-relative paths, not an applied Git/vendor patch. Complete original and edited copies are under `baseline/` and `source/`. The manifest also pins nine supporting inspected SDK files.

| Source path under source/ | Bytes | SHA256 |
| --- | ---: | --- |
| surrealdb-core-3.0.5/src/kvs/ds.rs | 90498 | `7f50728c3caaf218f5d72dcccc6ecddb3edee20ddba44c753ba4f0735e88e468` |
| surrealdb-core-3.0.5/src/kvs/mod.rs | 2105 | `58686565a7d83981999fd7fcd2c48b058568ba90b01cfad31ea83f62e4f189f2` |
| surrealdb-core-3.0.5/src/kvs/ds/existing.rs (new) | 5466 | `8645365376901927bfcc828bb2ba3aeb98df351124451946b3b282545f1b837b` |
| surrealdb-core-3.0.5/src/kvs/ds/AGENTS.md (new) | 2759 | `caf786ceb97c948ad6ec4464eaa87d90b5f0131fde9cb1c332f5211310c5df6e` |
| surrealkv-0.21.1/src/lockfile.rs | 11299 | `ba7b5da15d3d7e32c64519f9da0b65079240f0ddeca1d0712d496dcd854971d2` |
| surrealkv-0.21.1/src/levels/mod.rs | 20321 | `a79f20ad4670a961a6743591f84484a3c2c69c78c9301a237b3375ac006decea` |

Original copied source hashes are pinned in the manifest and still match registry bytes exactly. This is not independent verification of the downloaded crate archives. Source has not been parsed/compiled; syntax, type/API fit and behavior require independent review and the authorized build boundary.

## Actual component code

### Logical admission in the same owned Datastore

The new `kvs/ds/existing.rs` supplies real source for proposed core API `Datastore::admit_existing(self, &ExistingAdmission) -> Result<Self>`, exported by the copied ds.rs/kvs mod. **This API is introduced by the patch; it is not represented as an already supported installed SDK option.** It consumes the existing owned Datastore and returns that same object on success, without closing/reopening or cloning a public handle. The caller must already have obtained it through a protected existing-only opener. This precondition is not yet enforced by a sealed physical-open capability, so this method alone is not the final safe API.

The typed expectation supplies namespace/database, owner table and id/owner field identifiers, a bound public identity, actual expected owner, and bounded required field definitions. Identifiers use a closed ASCII grammar; values are bound variables. The owner query is fixed by the SDK component: SELECT VALUE owner-field FROM owner-table WHERE id-field=$retained_identity LIMIT2 TIMEOUT5s. No arbitrary SQL or application callback is accepted. The SDK component does not invent a DID; the eventual host must supply the true DID derived from its load-only existing key.

The method temporarily closes capabilities using actual Capabilities::none, begins one actual TransactionType::Read/Optimistic transaction, and uses typed catalog-provider reads for existing namespace/database/table/field definitions. Required kind/flexible/readonly settings must match; computed fields are rejected. Explicit String-kind requirements for the owner/id fields are mandatory. It reads the existing typed storage-version key directly through the core's own transaction/key abstraction and refuses absent/incompatible version, rather than calling the create-capable get_version path. This is a core metadata read within the sole engine, not a replacement raw-KV forensic reader.

The fixed owner query uses actual execute_with_transaction, that same transaction snapshot, existing Session::owner.with_ns.with_db and public Variables/Value APIs. Exactly one returned string owner must equal the expected real owner; missing/duplicate/malformed results refuse. A root query session is an internal catalog-reading context, not an owner claim or outward authority grant. Closed capabilities prevent an unexpected computed schema from authorizing script/network/functions; backend Read mode remains the write barrier. Required-field reconciliation is a bounded compatibility contract, not proof of every possible index/event/permission invariant.

The transaction is cancelled, never committed. Capabilities are restored for the same object's successful return or immediate teardown. Metadata/owner-read errors are collapsed to fixed refusal codes, and the read body has a five-second timeout. Transaction construction and cancellation/engine teardown are not bounded by that inner timeout; final host lifecycle handling remains required. No connection-ready, SDK bootstrap, API, sync or token producer exists in this component.

Primary API fit was inspected in locked core ds.rs:1712,1776–1845; tx.rs:114,132,191; catalog/providers.rs and tx implementation; dbs/executor.rs:1053–1090 (external transaction is retained rather than committed there); expr/kind.rs:482 onward; public types Variables and Array/Value. No typecheck or executed read-only proof is claimed.

### Existing-entry helpers

Copied LockFile adds acquire_existing through a shared acquire_with_creation path. Normal acquire still passes create=true/truncate=true, preserving its prior source ordering. Existing acquire passes false/false: a missing LOCK cannot be created and a failed contention attempt cannot truncate it before exclusivity. The existing common PID-write code runs only after successful try_lock_exclusive, retaining that actual handle. This is a useful component, **not** proof of canonical path/reparse/identity binding.

Copied LevelManifest adds open_existing, which directly invokes the actual existing load/format/table-validation path at manifest_file_path(0), bypassing new's create-if-missing branch. It still uses a path read and does not retain a caller-pinned catalog handle; therefore it is not a race-resistant catalog opener by itself.

The existing lock acquisition/release test is extended in copied source with missing-LOCK noncreation, existing acquisition, contention byte preservation and acquisition after release. No new registered test is added or run. `acceptance.md` authors the remaining concrete component/integration cases and explicitly labels unimplemented boundaries.

## Exact remaining technical work before a safe seam exists

1. **Protected physical open:** implement actual Windows retained ancestor/root/catalog/lock file identity transfer and nonreparse/no-replacement protection, tied to the final opened handles. The path-based helper is not sufficient. Add an existing-only TreeBuilder/Core/CoreInner path that uses the new lock/catalog helpers, requires existing root and all configuration-required directories/side stores, and does not call ordinary create_directory_structure as a fallback. CoreInner currently opens lock→manifest→WAL (lsm.rs:156–178); Tree::new creates directories before Core (1484–1522). The actual guard must survive this exact handoff and stay continuously owned through promotion.
2. **Recovery/configuration contract:** freeze the existing store's matching versioned/vlog/index policy rather than letting new ambient environment defaults silently create side stores. Wire a source-proven fail-closed corruption/recovery policy, preserving valid WAL replay but not discarding or repairing corrupt logical data. Ordinary physical lock/WAL maintenance is allowed and must be reported separately; zero physical writes was not adopted as a requirement.
3. **SurrealDB adapter propagation:** the current SurrealKV core adapter creates TreeBuilder and calls ordinary build (kvs/surrealkv/mod.rs:50–88). It must receive and consume the actual guarded existing-open capability. Simply adding a query-string boolean or calling the unused helpers after ordinary build would not work. Current SDK/core Config propagation needs an explicit reviewed policy type and ownership semantics.
4. **Connector pre-bootstrap wiring:** local/native.rs:80–98 currently invokes new→check_version→bootstrap→optional initial credentials→connection-ready, then starts router maintenance tasks. Add a separately reviewed retained branch that obtains the strict-existing engine, runs the concrete admission component, and only then bootstraps/promotes the **same** engine. Failure/cancellation must not publish a client/handle or start tasks. Normal path remains unchanged. The spike deliberately does not insert ordinary Datastore::new behind a require-existing label.
5. **Capability sealing and application coverage:** expose the final consumed admitted object only through a guarded existing-open result; reconcile required schema fields/constraints with actual application consumers and select the correct bootstrap Verse. Host load-existing key/secrets, safe root parsing, checked clock recovery, no Seed/reconcile/migration and delayed shared handle/services remain the application work specified in the approved design. No application code is patched here.

These gaps cross physical identity, three crate layers and host lifetime ownership. Completing only a few configuration fields would hide those obligations. This partial spike therefore stops at reviewable source components rather than asserting the full selected seam is finished.

## Minimal production dependency shape and next bounded lane

No new production dependency is installed/requested by this report. If this direction passes exact source review, the final integration requires reviewed pinned upstream/vendor sources for **surrealdb3.0.5, surrealdb-core3.0.5 and surrealkv0.21.1**, with one consistent public types version; Cargo.toml/lock changes belong to the parent integration lane. The current patch contains only core/SurrealKV component edits, not the full necessary dependency patch. Do not modify registry caches or silently substitute a newer crate.

The next concrete lane is physical existing-open capability plus CoreInner/adapter/connector handoff, not another general SDK audit. Its deliverable should add the missing exact files above and demonstrate the source ordering before any build. Then independently review the complete combined SDK patch and host method contracts. Parent owns compilation/integrated execution/disposable filesystem cases. Only completed, reviewed positive and negative disposable cases can justify any separately authorized retained root startup. No physical DB copy, offline second opener, fake DID/provenance or baseline rewrite is an alternative.

No safe startup, unchanged retained records, read-only execution, restart durability, runtime acceptance, CI or release result is claimed. This spike is concrete incomplete engineering work with an explicit integration block, not a refusal-only production replacement.
