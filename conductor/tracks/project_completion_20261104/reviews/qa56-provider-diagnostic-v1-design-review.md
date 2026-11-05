---
type: Review
title: Provider diagnostic coverage correction requested
date: 2026-11-04
---

# QA56 isolated provider diagnostic v1: independent design review

Verdict: **changes requested for DIAG-T1, one authored-coverage gap.** The production diagnostic design and implementation have no identified readability, ownership or protocol blocker. Do not treat the unmodified v1 scenario as complete coverage of its ordinary-stage allowlist/type fence. Root agreed to preserve this freeze and use a separate author for candidate v2. This report grants no integration, build, native provider, Invoke or capture acceptance.

Applied root/local guides and C:/Users/atooz/.gemini/UI-UX-guidance.md. Only this report is written. The reviewed code is the isolated raw candidate, not the unchanged production files or an assumed clean Git checkout.

## Exact reviewed candidate

Manifest: `.omc/research/qa56-provider-diagnostic-v1/manifest.json`, 3127 bytes, SHA256 `a9e802958b033c53e5b9a8248a715699559ba08d552e9be5cf3f4d807b4b5b38`.
Patch: 29646 bytes, SHA256 `7fddaf18865e0f05f8269335afa596ac10f5e4a2abddb864bdb40b86201fe344`.
Source baseline identity: `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`. The packet explicitly pins actual before bytes and does not claim a clean checkout. Both approved-plan anchors match their recorded bytes/hashes. All four before/raw snapshots match the manifest; production still matched all four before copies when checked.

| Canonical native_observer suffix | Candidate bytes | SHA256 |
| --- | ---: | --- |
| worker.cpp | 51181 | `f2ee668d464f349fb7fdff996578ca3a9cab4fc62ad8dfec4391abf86f4b6b42` |
| controller.py | 20121 | `9b3eb778c98bf1f4f9f3464f248ad0162dd4b72d1113fcf9dcb2a8089ead2c87` |
| scenario.py | 23314 | `7a0bc60dc2e783733a4c2cb3c8f0080cf709ae8aa4a691d5dd0f0a7a80d0e4b3` |
| AGENTS.md | 35755 | `f330612d8754930329c5dbd7bb781b4b4d25075d82ac22dda0d612f64e68d490` |

## DIAG-T1: single-fault contract cases ? severity: low

**Location:** candidate `tools/qa/native_observer/scenario.py:289-292` (with the stage20 sample at281-283).

**Current:** invalid stage cases replace stage_id on provider_sample but retain property_id30019. Invoke-only stages13/14/15, unknown22 and booleanTrue consequently also have a forbidden fourth key for an ordinary-stage shape. They still refuse if the ordinary-stage allowlist is accidentally widened. True also fails the property-key branch even if the exact-int check is removed, so that case does not isolate Python bool-as-int behavior.

**Suggestion:** retain the conditional-property cases, and add ordinary three-key diagnostics whose only invalid field is stage_id. Include13/14/15,22 andTrue at minimum, with the valid schema, failed hr and base result gates. The existing invalid_providers runner can handle them; no new harness or test registration is needed.

**Example:**

```python
for stage in (13, 14, 15, 22, True):
    diagnostic = {
        "schema": "fe-native-provider-failure-v1",
        "stage_id": stage,
        "hr": "80004005",
    }
    changed = dict(provider_result, provider_failure=diagnostic)
    invalid_providers.append(("ordinary-stage-" + repr(stage), changed, True, 2))
```

**Impact:** these cases meaningfully pin the closed ordinary-stage allowlist and exact integer fence. The current production validator already implements both correctly; this request concerns authored regression coverage, not an observed protocol bypass. Cases remain unexecuted until root's authorized verification boundary.

## Sound production patterns and preserved boundaries

**Typed failure/value object:** worker-local ProviderStage and ProviderFailureFacts carry one fixed method category, original HRESULT and conditional fixed property ID. They replace ambiguous default checked origins without mutable ambient context or arbitrary diagnostic strings. The mandatory-stage checked_provider wrapper evaluates each existing call once. Source inventory is32 old checked occurrences to2 remaining explicit COM calls, plus30 wrapper callers; the named replacements match their actual methods. The two property failures save the original status across the original VariantClear and retain existing success/type predicates.

**Catch-compatible specialization:** ProviderFailure derives from Refusal. The existing admission catch still rethrows every refusal except property_unavailable, and the post-Invoke generic catch still converts successful Invoke followed by refusal to submitted_state_changed. The dedicated top-level catch precedes the generic catch and copies facts only for a parsed observe request. Base receipt.method/hr remains the Invoke result; it is not overwritten by provider status.

**Closed serialization/validation boundary:** the emitter repeats mode/raw-flag/base-hr/code and fixed stage/property/FAILED checks, then uses canonical scalar atoms and the existing output-only append seam, one-write rule and total bound. Controller validation uses explicit observe-stage/property sets, exact int types, failed lowercase eight-digit HRESULT and exact nested keys. It runs on raw bytes before independent ledger-spent replacement. Optional extensions remain exclusive; old base/scalar/service branches remain. No generic raw JSON atom, request parser, ledger format, provider read, retry, action, discovery fallback, proof or admission predicate changes appear in the four-file patch.

The remaining scenario additions are useful: valid stages/property combinations, failed-HRESULT boundaries, conditional property shapes, mixed/duplicate extensions and raw mode/code/flag/hr/exit gates. Independent spent-ledger assertions stay, but parser cases do not execute the C++ throw/catch path or prove live controller/native behavior. Fixed stage IDs identify methods, not elements, occurrences or phases. QA56's original failing call/HRESULT remain unknown; only future fresh evidence can identify a later execution.

No implementation parser, tests, formatter, compiler/build, app/native/process/API/DB/network action or Git mutation occurred. Verification was limited to candidate/source/guide reads, artifact JSON/hash checks and textual call-site inventory. Production files, GUI57 and frozen QA56/v1 evidence were not changed.
