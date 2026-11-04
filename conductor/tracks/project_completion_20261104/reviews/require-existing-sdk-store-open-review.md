---
type: Review
title: Strict-existing SDK opening source findings
date: 2026-11-04
status: approved_at_source_scope
---

# Require-existing SDK source followup: independent review

2026-11-04. Reviewed `.omc/research/require-existing-sdk-store-open-followup.json` against the exact local registry source and Cargo.lock. **APPROVED for its bounded positive source findings. Strict-existing design remains unresolved; retained campaign roots remain on HOLD.**

This review read source/lock/note files and independently computed source-file byte lengths and SHA256. It did not open a store/DB, start an app/helper, inspect native OS/process state, call an API/network, execute a parser/compiler/test, change production source, or mutate Git.

## Exact source identity

Registry root: `C:/Users/atooz/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

| Source relative to registry root | Bytes | Independently matching SHA256 |
| --- | ---: | --- |
| `surrealdb-3.0.5/src/opt/endpoint/surrealkv.rs` | 1113 | `413a278368db467a7f4427dc0b8a038e979aab5f00153acde3108203bf5570c5` |
| `surrealdb-3.0.5/src/opt/config.rs` | 4864 | `abcade070566af679d1cdac8106a3d2bc62e3005b8ac3d58131192125c23a645` |
| `surrealdb-core-3.0.5/src/kvs/surrealkv/mod.rs` | 31195 | `5f5a0630121c9f6899abc98da1be8dbb36c62b0dc543690406b3320d5761c528` |
| `surrealkv-0.21.1/src/lsm.rs` | 70100 | `4e73ce26c32d944c59df23d5c458aaf37845924cbdff847bc2094621d1fb4570` |

Cargo.lock independently pins `surrealdb` 3.0.5 at lines 13427-13430, `surrealdb-core` 3.0.5 at 13465-13468, and `surrealkv` 0.21.1 at 13617-13620. Registry package paths and versions agree. This compares inspected local source bytes to the note; it is not a fresh download or independent crate-archive checksum verification.

## Positive call evidence

- SDK `opt/endpoint/surrealkv.rs` builds the surrealkv endpoint, supplies the converted path, and carries the supplied `Config` for the tuple form. No store is opened by that conversion itself.
- Core `kvs/surrealkv/mod.rs:50-88` has `Datastore::new(path, config)`, constructs `TreeBuilder::new()` at line 52, supplies the path at line 86 and calls `builder.build()` at line 88.
- SurrealKV `lsm.rs:1918-1919` implements `TreeBuilder::build` by calling `Tree::new`.
- `Tree::new` at line 1484 validates options, calls `create_directory_structure` at line 1489 **before** `Core::new` at line 1492, and later syncs directory structure.
- `create_directory_structure` at line 1506 unconditionally calls `create_dir_all` for the root at 1508 and SST/WAL/manifest directories at 1511-1513. **Qualification:** the optional vlog and versioned-index directory calls at 1517 and 1521 are conditional on `enable_vlog` and `enable_versioning`, respectively. They are not unconditional calls in every configuration. This makes precise the followup note's wording about optional subdirectories.
- Core `kvs/surrealkv/mod.rs:142-145` selects `Mode::ReadOnly` when beginning a transaction on the already constructed datastore. That is transaction mode, distinct from a strict-existing/no-create store-opening contract.

These positive calls support the followup's concern that ordinary opening is create-capable and cannot, merely by selecting read-only transactions later, prove strict-existing startup. They do not establish that every option or alternate supported opening mechanism has been searched, that directory calls always create new entries, or that every retained open changes bytes.

No exhaustive absence claim is approved. No proposed live restart, SDK confinement protection, namespace/schema admission, original-owner validation or logical no-init/no-seed guarantee is established by this note. The unresolved supported strict-existing design and original retained-root hold from the startup trace remain in force. No offline opener, fake DID, reseed, baseline/provenance rewrite or retained restart is authorized or performed here.
