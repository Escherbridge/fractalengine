---
type: Review
track: hexon_unification_20260716
gate: R-HEXON-1
reviews: [HEXON-01, HEXON-02, HEXON-03, HEXON-05, HEXON-08, HEXON-19]
security_review: true
date: 2026-11-13
---

# R-HEXON-1 — adversarial review (turn 4)

Diffs read against `7af0a144`; prompted to refute. Security pass covered package open (seal + signature, fail closed), publish client auth and redirects, snapshot import caps and integrity, P2P announcement parsing. No cargo run.

R1-01 FIXED - fe-format/src/manifest.rs:74 - `HexonType` Display looked names up in a 16-row table and fell back to "unknown"; a future variant missing from the table would render (and announce) as "unknown". Now an exhaustive `as_str()` match; FromStr parses snake_case through serde (every variant), legacy spellings after; the table is test-only.
R1-02 FIXED - fe-hexon/src/publisher.rs:445 - `PluginHexonBuilder` used `zip::write::FileOptions::default()`, which under zip 2 (the API the rest of fe-hexon now uses) leaves the extension type unresolved (E0283). Now `SimpleFileOptions`.
R1-03 FIXED - fe-hexon/src/p2p/discover.rs:51 - the kind filter compared the announcement's string to the new snake_case Display ("gpx_collection"), so a legacy peer's "gpx"/"TerrainTileset" never matched despite the doc saying case-insensitive. Now parsed through `HexonType::from_str`; `test_search_by_kind` extended with a legacy "gpx" row.
R1-04 FIXED - fe-format/src/snapshot.rs:185 - entries.json was plain serde output; `AssetEntry.sub_assets` is a HashMap, so the snapshot archive was not byte-stable (FR-9). Now canonical JSON (sorted keys), the same canonicalizer as manifest.json.
R1-05 FIXED - fe-format/src/snapshot.rs:240 - `import_petal_snapshot` ignored `manifest.integrity`: a sealed snapshot with an edited state table imported silently. Now `verify_archive(.., VerifyIfSealed)` runs first (unsealed snapshots read as before); the round-trip journey seals a snapshot, imports it, and refuses a sealed copy with an edited `rooms.json`.
R1-06 FIXED - fe-format/src/snapshot.rs:330 - duplicate `assets/{hash}` member names were returned as duplicate blobs; names are now de-duplicated after sorting.
R1-07 FIXED - fe-hexon/src/handlers/mod.rs:38 - Script/PetalSnapshot "refusal" lived only in the post-install dispatcher, whose error `HexonRegistry::install` merely logs after blobs, DB rows and the record are already written. Added `check_content_installable` (also refuses Plugin, which installs through `verify_plugin_package`); `handle_install` calls it first.
R1-08 HANDED-OFF (R-HEXON-2, owns fe-hexon/src/registry.rs) - fe-hexon/src/registry.rs:186 - `install` must call `crate::handlers::check_content_installable(&package.manifest.hexon_type).map_err(RegistryError::Package)?;` before the blob-check loop, and its journey gets one refused-PetalSnapshot row (nothing stored, no record).
R1-09 HANDED-OFF (I-LIBS-1) - fe-hexon/Cargo.toml:24 - `zip = "0.6"` is still there while package.rs, publisher.rs and partial/p2p.rs use the zip 2 API (`SimpleFileOptions`, `new_append`): replace it with `zip = { version = "2", default-features = false, features = ["deflate"] }` (removal alone does not compile).
R1-10 HANDED-OFF (I-LIBS-1) - fe-hexon/Cargo.toml:41 - HEXON-03's routed "multipart" reqwest feature is stale: `RemoteRegistryClient::publish` posts the raw body the registry route reads (`Bytes`), so do not add `multipart`.
R1-11 HANDED-OFF (HEXON-06) - fe-api/src/hexon.rs:376,571 - still parses `HexonKind`, which HEXON-02 deleted; re-type to `fe_format::HexonType` (its `FromStr` keeps the legacy spellings).
R1-12 HANDED-OFF (HEXON-13 / HEXON-09) - fe-format/src/snapshot.rs:205 - state rows and op-log lines are written as given; FR-9 byte determinism needs the DB exporter to hand rows with a stable key order (or canonicalize them there). Not changed here because the op-log is restored verbatim.
R1-13 DEFERRED (HEXON-10 restore defines row-count semantics) - fe-format/src/snapshot.rs:112 - `meta.table_row_counts` is not checked against the shipped tables; its keys are the exporter's table names, not the member stems.
R1-14 KEPT (dormant P2P inbound path, display only) - fe-hexon/src/p2p/discover.rs:89 - an unparseable announced type still renders as Model in unfiltered results; pre-existing.
R1-15 KEPT (track directive 2: dev-phase breakage allowed) - fe-hexon/src/package.rs:197 - packages signed by the deleted raw-bytes scheme, or unsealed ones, now open with `signature_valid() == false` and `install` refuses them.
R1-16 KEPT (bounded by visible callouts; A12 requires per-frame recompute) - fe-ui/src/actions/dimension_callout.rs:136 - the label rebuilds a small `MeasureState` and a String per callout per frame.
R1-17 DEFERRED (trigger: any identity keyed on signature bytes) - fe-format/src/signature.rs - `verify_manifest` uses `verify`, not `verify_strict`; content_hash blanks the signature, so malleability changes no identity today.
R1-18 KEPT (verified) - fe-hexon/src/remote.rs:128 - publish applies the bearer token like GET and inherits the same-origin redirect policy; the registry route verifies the seal under RequireSealed and caps the body at 1 GiB.
R1-19 KEPT (verified) - fe-hexon/src/package.rs:197 - open is fail closed: fe-format's import verifies the seal over every member (presentation extras included), and `signature_valid` additionally needs seal publisher == manifest publisher and the canonical signature.
SEC-INBOX: no entry names a file this gate owns (entries 1-4 checked).

Verdict: PASS-WITH-FIXES (7 fixed, 4 handed off, compile depends on I-LIBS-1 applying R1-09).
