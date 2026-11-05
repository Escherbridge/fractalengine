---
type: ImplementationReport
title: Provider diagnostic with isolated stage coverage
date: 2026-11-04
---

# QA56 provider diagnostic — isolated candidate v2 coverage correction

Authored for independent review, **not executed or approved**. Ownership was only `.omc/research/qa56-provider-diagnostic-v2/**`. The frozen v1 packet and all four canonical production files remain unchanged. Root's GUI57 continues against its unchanged6ca source; this packet supplies no live coverage or native-provider repair credit.

## Exact change

Only `raw/tools/qa/native_observer/scenario.py` differs from v1: four added source lines append six cases to the existing `invalid_providers` list. Each creates an ordinary **three-key** `provider_failure` object with actual fields `schema`, `stage_id`, `hr`, no `property_id`, exact `fe-native-provider-failure-v1`, and failed lowercase HRESULT `80004005`. It inherits otherwise-valid base output from `provider_result` and supplies actual observe True/helper exit integer2. The existing common refusal loop calls `controller.worker_output` and records whether it raised `Refused`.

Stages13,14,15 independently test observe rejection of Invoke-only stages. Stage22 independently tests the unknown stage boundary. True independently tests the exact-int guard because Python True otherwise aliases accepted ordinary integer1. False is also included for boolean completeness, but aliases invalid integer0; that case alone cannot distinguish type rejection from membership rejection. No extra-property failure can mask any of these six ordinary-shape cases. Existing malformed stage/property/type cases and all original positive, ledger, CLI, duplicate, signature and lifecycle cases remain byte-preserved around this insertion.

Case record suffixes are `provider-ordinary-stage-13-refused`, `14`, `15`, `22`, `True`, and `False` under the existing `native-contract-` prefix. These are authored assertions, not observed passing results. No new exercise/test registration, parser fork, production predicate, payload, C++ call or action was added.

## Frozen provenance

The v1 baseline remains `6ca9f49bf65bb344ff5bdb0fa9794208c54c4a9a`; source-dirty caveats and original plan/review anchors remain in the full manifest. Copied every `before/` and `raw/` file byte-for-byte first, then inserted only the four scenario lines. The predecessor metadata is preserved verbatim as:

| Original v1 artifact / v2 archival name | Bytes | SHA256 |
| --- | ---: | --- |
| manifest.json / v1-manifest.json | 3127 | a9e802958b033c53e5b9a8248a715699559ba08d552e9be5cf3f4d807b4b5b38 |
| source.patch / v1-source.patch | 29646 | 7fddaf18865e0f05f8269335afa596ac10f5e4a2abddb864bdb40b86201fe344 |

`v1-source-report.md` is also an exact original metadata copy, separately pinned by the v2 manifest. Before/raw hashes from v1 were checked before modification; the full v2 manifest preserves all four original before hashes. The three other raw production candidate files are exact v1 copies: worker.cpp `f2ee668d464f349fb7fdff996578ca3a9cab4fc62ad8dfec4391abf86f4b6b42`; controller.py `9b3eb778c98bf1f4f9f3464f248ad0162dd4b72d1113fcf9dcb2a8089ead2c87`; AGENTS.md `f330612d8754930329c5dbd7bb781b4b4d25075d82ac22dda0d612f64e68d490`.

| V2 artifact | Bytes | SHA256 |
| --- | ---: | --- |
| manifest.json | 4539 | eafab1ccf9f830c875cebba7f5e1227482dddb786ef61b08f91825f749d3673b |
| source.patch (full before→v2 four-file diff) | 29936 | b5c9cece78eb7f0886705678ef6f8b0096a7e407c6fcbb0354769b36447486cd |
| v1-to-v2.patch (scenario-only delta) | 757 | 93cb00b5f0cc9ac69d6dd4fa90d2b1e3891a6719f1cf6d5c2d7249a140d9d18c |
| raw/tools/qa/native_observer/scenario.py | 23600 | 43f8d3a90d3355e1001a0d24b6b10eb454d97ded8cc94dffd9447b7d385a39c5 |

Static byte/text/hash inspection confirms only scenario differs among raw candidates, every before copy remains identical to v1, and each current canonical production file still matches its before copy. No implementation import/parser, tests, formatter/compiler/build, native/helper/app/runtime/OS/process, API/DB/store/network or Git mutation occurred. Independent review must assess this correction; no self-approval is claimed. Root alone integrates reviewed exact bytes after the live run and owns all execution. Frozen QA56's original failed call/HRESULT/cause remain unknown; no later diagnostic may rewrite that evidence.
