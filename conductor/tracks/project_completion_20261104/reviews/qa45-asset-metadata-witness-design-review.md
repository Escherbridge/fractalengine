---
type: Review
title: Typed publication asset witness design review
date: 2026-11-04
---

# QA45 asset witness: independent design/readability review

Reviewer `/root/legacy_helper_io_fix`, 2026-11-04.
**APPROVE at the requested design/readability scope; no concrete blocker.**
This reviewer did not author the slice. Full security/correctness review is a
separate lane; no compilation, executed test or runtime field diagnosis is claimed.

Verified manifest SHA-256
`787589d07c4bb0e40a1fe91fac19b715667ee564318e37a2622c5da7dde760ce`
at `.omc/research/qa45-asset-metadata-witness-v1/manifest.json`. All four raw sizes/
hashes match. Read the complete scoped patch/report and actual capture, dependency
and final transaction code. No source/packet/Git change; only this report is written.

- **One explicit projection is the useful extension point.** Capture and fence
  share the exact nine-field PUBLICATION_ASSET_COLUMNS constant. The narrower
  current-reader projection is kept separate. Extending this projection has one
  obvious source location, while final equality continues using the server-native
  encoded witness rather than relying on decoded JSON type fidelity.
- **Pair integrity is present in the actual capture path.** One server SELECT
  returns metadata and its witness from the same projected row. The decoder
  validates both, then appends one metadata object and one witness in the same
  loop, preserving order/cardinality. The sole production consumer immediately
  names the returned `(assets, fingerprints, models)` and installs the witness
  only in its original unique/proven/valid-hash branch. Later dependency stripping
  operates on a metadata clone, not the captured native witness.
- **The three-Vec tuple is acceptable in this narrow API.** Two Vec<Value> return
  positions make the signature less self-describing; a small named capture would
  make those roles explicit. In the current one-consumer path, however, immediate
  named destructuring and the lockstep decoder make the invariant inspectable.
  A named struct would not repair a demonstrated detachment/reordering bug, so
  this review does not require an additional abstraction or block integration.
- **Deferral remains the production contract.** Missing, ambiguous, foreign or
  unproven assets still follow the existing deferred-dependency branch; model/
  schema deferral, authority/provenance checks and post-transaction enqueue order
  remain. In particular, an initially empty asset capture does **not** cause
  production to install a creation fence: that branch still defers. The test that
  binds an empty expected list calls the fence helper directly and proves that
  primitive's list/count behavior, not a stronger missing-asset publication policy.

The existing real-store journey meaningfully exercises the production capture/
fence helpers: unchanged uploaded-style native absent provenance, old JSON-roundtrip
refusal, nine-field/native-type mutations, deletion/duplicates and whole-transaction
rollback after a probe write. Its helper-level empty-capture case has the limit
above. op_log's normalized production prefix is unchanged; registered tests stay
10→10. Test re-exports remain cfg(test); no new test registration or public API.

Root still must compile/run these cases and establish fresh fixture publication
and genuine concurrent-change refusal. QA45's actual differing field/type remains
unidentified; no publication, peer delivery, native, FULL, CI or release credit
follows from this source review. No compiler/parser/test/implementation execution,
application/process/OS/native/API/DB/network or Git mutation occurred.
