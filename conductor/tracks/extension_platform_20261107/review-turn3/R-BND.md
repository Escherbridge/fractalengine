---
type: review
track: extension_platform_20261107
gate: R-BND
reviewed: [X-BIND, X-REPL, X-DECODE, I-DB]
date: 2026-11-13
security_review: true
---

# R-BND: binding authority and provenance (turn 3)

Adversarial review, prompted to refute: resolver rows, the all-authors binding snapshot, host-only reading
provenance and the policy actor seam. No builds or tests were run (code-first directive).

1. FIXED (steer 2026-11-13, security): the readings flush wrote one `insert_readings_with_provenance` call per source class and stopped at the first error, so a refused later group left earlier groups written. `plugin_host::door_readings` now checks every stamped `payload_id` against `device_payload` rows of the bound petal, plus the empty-metric rule, before `commit`/`authorize_readings`. Every rule the insert enforces now runs first. db_test J3 gains a mixed-class flush (a plain reading plus a decoded reading with an unknown payload) that must be refused with no rows landed.
2. FIXED (security, I-DB seam): `set_policy_as` audited the caller's `actor_did` but never checked that actor's role. When the actor differs from `local_did`, it must now hold Manager+ at the verse; a refusal is audited under that actor. The DB no longer trusts the API layer for this check.
3. HANDED-OFF (I-BIN, compile break): `fractalengine/src/plugin_bridge.rs:2256` builds `LocalFacts` without the two X-BIND fields `default_access` (the verse's replicated default access) and `admitted_bundles` (`PortalResolver::admitted_set()`). As written it does not compile, and portal.open can never resolve.
4. KEPT (X-BIND): `check_portal` matches an admitted bundle by `package_id` alone. A binding names a package and never a content hash. P-RES `admitted_set()` already removes kill-listed hashes and disabled pairs, and no offer is made.
5. KEPT (X-BIND): `LocalFacts::local_role` lets the verse default lend Viewer to a DID whose Owner pin is the explicit `none` role. fe-policy `role_at` documents that `default_access` fallback on `None` is the §2.2 contract, and the default never lends more than Viewer.
6. KEPT (X-BIND): kill-listed registry builds are dropped before any Install or Update offer (`gated_offer` uses `!build.revoked`). An override counts only on the verse Owner's own host (`check_build`), and the table tests have rows for both cases.
7. KEPT (X-REPL): the iroh `snapshot()` reads `binding/` from all authors (`ALL_AUTHOR_TABLES`), and the mock does the same. Inbound rows are then confined: key author == attested author, payload == doc key, Manager+ now at the row's scope (`replicated_row::confine_binding` and `extension_binding::confine_inbound`). The loopback test covers a forged author, the real author and two-sided convergence.
8. KEPT (X-DECODE): `StagedReading.provenance` is `skip_deserializing`. It is set only in `imports.rs` from `InstanceState::decode_stamp`, which `call_hook` sets and clears around one decode call. `DecodeStamp::for_payload` always uses `SourceClass::Decoder`, so a guest cannot pick its class.
9. KEPT (I-DB): `commit_local` stores `written_at = hlc` inside the transaction and publishes `lamport`. These are the same value, because `commit_operation_atomic_with` passes the packed HLC as `lamport`.
10. DEFERRED (trigger: device_payload retention or deletion lands): a payload or anchor deleted between the door check and the per-class inserts still makes a flush partial. That case answers `unknown-outcome`, never a silent success; closing it needs one transaction across all class groups in `iot_reading.rs`, which this slice does not own.
