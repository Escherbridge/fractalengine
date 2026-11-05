---
type: Review
title: Provider diagnostic final source review
date: 2026-11-04
---

# QA56 provider diagnostic v2 independent source review

Verdict: **approved for later exact integration and verification**. V2 adds the six requested independent invalid-stage cases without changing the approved production candidate. No source blocker remains from this correction. This is source approval only; no parser, formatter, compiler, test, runtime/native, API, DB or network execution or Git mutation was performed. Only this report was written; root production files, isolated packets and live GUI57 were not changed.

The reviewed packet is `.omc/research/qa56-provider-diagnostic-v2`, baseline `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`:

- Manifest: 4,539 bytes, SHA256 `eafab1ccf9f830c875cebba7f5e1227482dddb786ef61b08f91825f749d3673b`.
- Full patch: 29,936 bytes, SHA256 `b5c9cece78eb7f0886705678ef6f8b0096a7e407c6fcbb0354769b36447486cd`.
- V1-to-v2 delta: 757 bytes, SHA256 `93cb00b5f0cc9ac69d6dd4fa90d2b1e3891a6719f1cf6d5c2d7249a140d9d18c`.
- Candidate scenario: 23,600 bytes, SHA256 `43f8d3a90d3355e1001a0d24b6b10eb454d97ded8cc94dffd9447b7d385a39c5`.

All four raw and before files match their frozen sizes/hashes. Candidate worker, controller and directory documentation are byte-identical to approved v1: worker `f2ee668d464f349fb7fdff996578ca3a9cab4fc62ad8dfec4391abf86f4b6b42`, controller `9b3eb778c98bf1f4f9f3464f248ad0162dd4b72d1113fcf9dcb2a8089ead2c87`, AGENTS `f330612d8754930329c5dbd7bb781b4b4d25075d82ac22dda0d612f64e68d490`. Removing exactly the single new four-line block from v2 scenario reproduces v1 scenario raw bytes. The three copied v1 predecessor artifacts are exact in size/hash and byte-identical to their retained originals. All four canonical production files still match frozen before bytes, so the live root has not adopted either candidate.

The six new cases construct an ordinary three-key `provider_failure` with fixed correct schema, valid failed HRESULT `80004005`, and stage13/14/15/22/True/False. They contain no property_id, retain the otherwise valid provider-refusal base record, and pass actual observe=True/exit2 into the unchanged rejection loop. Thus stages13–15 exercise the Invoke-only exclusion and22 exercises the unknown-stage exclusion without an extraneous property independently causing rejection. True is especially useful because Python bool otherwise aliases integer1 in membership; its refusal independently exercises the exact-int guard. False also exercises exact-int rejection, although zero would separately be outside the stage allowlist.

This improves upon the earlier property-stage-derived malformed samples, whose forbidden property field could mask some ordinary-stage allowlist failures. It changes no acceptance policy. The source loop catches actual `Refused` from the same controller validator and records the rejection, so these are meaningful authored boundary checks rather than assertions of a copied predicate. They remain unexecuted, and do not establish native C++ behavior.

The full v1 source review remains applicable unchanged: all30 default HRESULT call replacements and two untouched explicit COM checks; fixed stage/property/failed-HRESULT output; observe-only raw flags/base hr/exit2 validation before independent ledger override; exclusive closed extensions and recursive duplicate rejection; generic admission/post-Invoke propagation; no extra reads or admission/proof/lifetime changes. The approved plan and shared/local guidance remain the basis of this bounded review.

Root may proceed after the live-source freeze with exact integration, matching build and planned verification. QA56's original failed call/HRESULT/cause remain unknown and its external clarification remains mandatory. No successful provider/root/witness, Invoke/capture, peer, retained restart, CI/release or FULL acceptance is claimed.
