---
type: review
track: hexon_entitlements_20261031
gate: S6
date: 2026-11-14
---

# S6 review record (HEXON-17)

Verdict: PASS-WITH-FIXES. The full adversarial pass is recorded in `conductor/tracks/hexon_unification_20260716/review-turn4/R-HEXON-3.md` (the S6 verdict section there).

- Fixed in round 4: F1 (refused members read before the gate), F2 (crate asset route headers), F3 (licence link validation), F4 (doc comment placement).
- F5 (refusals logged at warn): fixed by HEXON5-07. Code at fe-terrain/src/tiles/entitlement.rs:24, composite.rs:131,166, registry.rs:247.
- F6 (non-free tileset members not sealed on export): HANDED-OFF to tileset export tooling. Trigger for re-review: any export path that writes a Paid tileset.
- F7 (streamed bundle reads as unlicensed): DEFERRED. Trigger: a streamed licensed tileset, or the I-LIBS-2 bundle work.
- F8 (cache hits not re-checked against token expiry): DEFERRED. Trigger: an in-app tileset token source lands.
