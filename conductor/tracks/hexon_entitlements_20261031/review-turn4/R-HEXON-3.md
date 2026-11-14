---
type: Review
track: hexon_entitlements_20261031
gate: R-HEXON-3
reviews: [HEXON-14, HEXON-15, HEXON-16]
carries: HEXON-17 (S6 code review high + mandatory security review)
date: 2026-11-13
---

# R-HEXON-3: entitlement wiring review (S6)

Scope: diffs since 7af0a144 of HEXON-14 (89b2f2f5), HEXON-15 (a538f8a4) and HEXON-16 (3ec69177), read against
fe-format/src/entitlement.rs and fe-hexon/src/entitlement_gate.rs. Fresh context, prompted to refute. No cargo run.
Security inbox: no entry names a file this gate owns (entry 6 is fe-api/src/petal_snapshot.rs, not ours).

## Findings

- F1 FIXED - fe-terrain/src/tiles/hexon_source.rs:441,590,610 - a refused member was still read from disk (indexed) or its whole chunk decoded (container) before the gate said no, on every request because refusals are not cached; `refused_before_read` now refuses a licensed non-free member with no entitled gate (none, preview, or expired) before any I/O, after the membership check so an absent tile stays `Ok(None)`.
- F2 FIXED - fe-api/src/hexon.rs:575-580 - the crate asset route now serves publisher bytes, but without `X-Content-Type-Options: nosniff` / `attachment`, which fe-api/src/assets.rs requires on every blob response (stored-XSS surface); both headers added.
- F3 FIXED - fe-ui/src/dialogs/hexon_manager.rs:307 - licence link check accepted embedded whitespace/control characters and passed the untrimmed string; `web_url` now trims, requires http/https (case-insensitive), rejects whitespace/control chars and URLs over 2048 bytes, and returns the cleaned URL; link is `ui.hyperlink_to` only, never fetched or auto-opened.
- F4 FIXED - fe-ui/src/dialogs/hexon_manager.rs:294,340 - HEXON-16 inserted the two helpers between `render_installed_map_card`'s doc comment and the fn, so the card's doc attached to `access_chip`; moved. The link now carries a per-row accessible name ("Licence details for <region>").
- F5 HANDED-OFF (fe-terrain tiles owner, I-LIBS-2 or round 5) - fe-terrain/src/tiles/composite.rs:134,161 and fe-terrain/src/tiles/registry.rs:244 - `EntitlementRequired` is a state, not a fault, but the loader logs it as "hexon tile read failed" at warn for every non-free tile it asks for: downcast `EntitlementRequired` and treat it as a silent miss (debug at most) so a preview tileset does not flood the log.
- F6 HANDED-OFF (tileset export / build tooling, round 5) - fe-format export_tileset - no production path seals non-free tileset members (`seal_paid_entry` is called only in tests); a Paid tileset's non-free tiles sit on disk in plaintext, so preview gating is advisory and an entitled gate cannot open them (OpenFailed, tile missing). Trigger: before any Paid tileset is published.
- F7 DEFERRED (trigger: a streamed licensed tileset, or the I-LIBS-2 bundle work) - fe-terrain/src/tiles/hexon_source.rs:263 - `from_streamed_bundle` builds a source from a P2P bundle that carries no license.json, so it reads as unlicensed; safe only while non-free members are sealed (F6). Bundles need the parent tileset's terms passed in.
- F8 DEFERRED (trigger: an in-app tileset token source lands) - fe-terrain/src/tiles/hexon_source.rs:584,605 - cache hits are not re-checked against token expiry, so opened tiles stay readable from the LRU after `exp`; today no gate is ever entitled in-app (callers pass token None), so nothing is exposed.
- F9 HANDED-OFF (owners of fe-ui/src/terrain_map/mod.rs and fe-ui/src/verse_manager/db_results/terrain.rs, per HEXON-16 NEEDS) - fe-ui/src/terrain_map/mod.rs:288, fe-ui/src/verse_manager/db_results/terrain.rs:280 - test fixtures still build `InstalledTilesetDto` without `access`/`verification_url`; add `access: TilesetAccess::Installed, verification_url: None` or the fe-ui test build fails.
- F10 KEPT (AEAD binds package and entry) - fe-hexon/src/registry.rs:482, fe-terrain/src/tiles/hexon_source.rs:424 - a gate is matched to an install by licence terms only, so an entitled gate from another package with identical terms is accepted; `open_paid_entry` binds the key to `claims.package_id` and the entry id, so its sealed entries fail with OpenFailed and never yield plaintext or ciphertext.
- F11 KEPT (verified) - fe-hexon/src/registry.rs:443 - NFR-2 holds: `gate_for`, `tileset_entitlement` and the REST tokenless gate all go through `EntitlementGate::open`, which returns NotRequired before touching the verifier for unlicensed/Free/Attribution; an install without an archive-recomputed content hash drops any token (preview only, fail closed).
- F12 KEPT (verified) - fe-format/src/entitlement.rs:570-620 - forgery/replay/expiry: alg pinned to EdDSA, kid pinned to the keyring, header kid equals claim kid, TTL capped, exp/iat with a fixed leeway, package/content_hash/licensee checked, content key unwrapped under `WrapBinding::Device {package_id, content_hash}`; a token is useless without the device secret. The registry test flips one signature byte and gets preview, never plaintext.
- F13 KEPT (verified) - fe-hexon/src/registry.rs:472 - plaintext of a sealed entry is returned in memory only; the test asserts the stored blob is still the sealed bytes. Other in-process blob readers (asset_bridge, plugin_bridge, portal resolver, /api/v1/assets) get the stored ciphertext only, never plaintext.
- F14 KEPT (efficiency, owner-only route) - fe-api/src/hexon.rs:295 - `bind_published_archive` reopens and re-verifies the archive while holding the registry mutex in an async handler; publish is rare and owner-gated.
- F15 KEPT (by design) - fe-api/src/hexon.rs:556 - the asset route now reads the whole blob into memory (needed to open sealed entries in memory) on a blocking task; publish multipart size is the bound.
- F16 KEPT (verified) - leak and NFR-1 scan of all eight files' added lines: no checkout, purchase or price wording, no forbidden names; `payment_verification_url` is the pinned format field and the UI says only "Licence details".
- F17 KEPT (seam note) - fe-terrain/src/tiles/entitlement.rs:47, fe-ui/src/terrain_map/dto.rs - two `TilesetAccess` enums exist (fe-terrain `from_label -> Option<Self>`, fe-ui `from_label -> Self`, unknown = Installed) because fe-ui has no fe-terrain dependency; the pinned seam is satisfied on both sides and terrain_bridge maps one to the other via the label.

## S6 verdict (HEXON-17)

EntitlementGate is the only entry on every in-process load path reviewed, fails closed (no token, refused token, missing content hash, mismatched gate, sealed bytes that do not open), the REST route answers 403 `entitlement_required`, tileset member reads are gated per member, and the licence link is a plain hyperlink. Open items: F5, F6, F7, F8, F9.

Verdict: PASS-WITH-FIXES
