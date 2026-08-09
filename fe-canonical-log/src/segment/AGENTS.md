# segment — sealed delivery artifacts, lanes, HashSeq, relay policy (SPEC-6)

Normative source: `docs/spec/canonical-log/segment-shard-relay.md`. Read it with SPEC-1
(`operation-envelope.md`) and SPEC-3 (`capabilities-and-revocation.md`).

This module owns the *delivery* layer: immutable BLAKE3-addressed sealed artifacts, the two
content lanes, the manifest that indexes them, the reachability proof over them, the receipt
ordering that admits them, and the policy a relay would apply. It owns no causality — SPEC-1
parent links alone establish that — and it owns no cryptography: the AEAD arrives in wave 3
behind `receipt::SealedBodyOpener`.

## No network, and no path to one

`relay_policy` and `discovery_labels` contain no socket, listener, connection, iroh, or libp2p
code. Nothing here references `fe-network`, `fe-sync`, or `fe-runtime`, nothing opens a file or
a port, and nothing can change `fe-sync`'s `IrohDocsEngineHolder::is_available()`, which stays
`false`. `decide_seed` and `decide_fetch` answer *whether* a transport would be permitted to
act; the transport itself remains owner-gated and unbuilt. Relay work here is local artifact
handling only.

### One seal entry point, for all four lanes

`artifact::seal_artifact` is the only way to obtain a `SealedArtifact`; `SealedArtifact::seal` is
`pub(crate)` so no Wave-3 caller can reach the ungated constructor. In order it refuses a
lane/body-class mismatch, then the production suite and the lane's current scope epoch and key
through `relay_policy::assert_seals_under_current_key`, then a nonce already seen under that key
through `artifact::NonceLedger::record_fresh`.

That ordering is the whole point. Nonce reuse under XChaCha20-Poly1305 is a keystream-recovery
break, and sealing under a retired epoch key defeats the D-CL17 revocation story, so neither
check may be reachable only by opt-in. Before the R-unify pass `assert_seals_under_current_key`
had **no caller of any kind**, and only the payload shard was checked at all — the header
segment, HashSeq nodes and the manifest went straight through the bare constructor, whose
`validate` covers only the format version, a non-empty ciphertext, and length agreement.

`payload_shard::seal_payload_shard` now takes the `RelayAuthorizationView` and derives its lane
and epoch from the shard's own topic, so it cannot be asked about a lane the shard does not
belong to. It deliberately no longer records the nonce: the nonce belongs to the sealed artifact,
so packing a candidate twice must not burn a nonce the AEAD never used. `seal_artifact` records
it exactly once, at the seal.

The lane/class table `SealRequest::assert_lane_carries_class` enforces (§4.1.3): a header lane
carries verse-wide header segments, its own HashSeq nodes, and the branch manifest; a payload
lane carries only petal-affine shards and its own HashSeq nodes.

`store::SealedArtifactStore` is deliberately a trait with an in-memory implementation and no
binding to `fe_runtime::blob_store::BlobStore`: `fe-runtime` pulls bevy and tokio, which must
stay out of this leaf crate. Wave 3 implements the trait in `fe-database`.

## Ordering is the contract

Three orderings in this module are load-bearing, and each is easy to lose in a refactor.

- **Receipt (§5).** `ReceiptPipeline::receive` re-hashes the complete artifact *before*
  recording it present or serving it, validates the sealed outer form *before* decryption,
  resolves the scope key and authenticates *before* trusting any inner lane, scope,
  predecessor, index, or record field, and cross-checks header and shard references in both
  directions *before* admitting. A per-range checksum never substitutes for the final
  `artifact_id` check, which is why `RangeReassembly` hands back only a complete buffer and
  the digest check happens in `receive` over the reassembled bytes.
- **`put_verified` has exactly one library caller**, `ReceiptPipeline::receive`. That is the
  mechanism behind §5.6: a `have`, an announcement, or a cache hit can never be mistaken for
  validity, because there is no other code path into the verified store. (Unit tests in
  `store.rs` call it directly, since the store's own contract is what they test.)
- **Proof (§4.2).** `verify_checkpoint` verifies the Manager+ signature first and then treats
  it as a *claim*: it independently re-hashes the manifest and every node and segment it
  fetches through `ArtifactLookup::fetch_verified`, decrypts and canonically validates, walks
  the header lane to the declared boundary, walks the SPEC-1 parent closure of the selected
  frontier, and only then matches payloads. Every shortfall returns `Unresolved`; absent
  decrypt authority for a required petal returns `HeaderReachableOnly` and never `Verified`.

## What the proof deliberately does not do

The parent walk starts at the selected frontier and stops at genesis or the declared replay
base. An unrelated concurrent operation is not a missing ancestor (§4.2.6), so the walk never
expands into the rest of the verse; a multi-parent merge inside the closure makes all of its
parents required, which falls out of walking the header's own `parents` list.

## Header lane carries payload-BEARING headers

§3.1.3 excludes payload *ciphertext* from the header lane, not operations that reference one.
Excluding payload-bearing headers would make §4.2.3 step 5 — "for every payload-bearing
reachable header … find a matching payload-shard record" — unsatisfiable. The exclusion is
structural rather than a predicate: `HeaderSegmentBody`'s grammar holds nothing but complete
SPEC-1 envelope byte strings, so ciphertext, capability-chain bytes, and key material have no
slot to occupy, and an extra map key is rejected as an unknown field.

## Author equivocation is refused, never adjudicated

`HeaderSegmentBody::from_admitted` and `checkpoint_proof::assert_no_equivocation` both refuse
when two distinct `op_id`s share one `envelope::EquivocationKey`. The rule (D-CL25, SPEC-1
§3.4) is quarantine BOTH candidates and materialize NEITHER; `store::QuarantineReason` — which is
now a `pub use` of the crate-wide `compose::QuarantineReason`, see `src/AGENTS.md`
§unified-vocabularies — carries the key and both operation IDs so a receiver retains the evidence
without picking a winner. Picking one is precisely the fork the author attempted.

## Scope travels with every record

`EnvelopeView::scope` is on the trait, not derived at the call site, and
`payload_shard::PayloadTopicScope` carries `verse_id` alongside `petal_id`. Four downstream
MUSTs need the scope: SPEC-1 §6.2 same-verse parent checking, SPEC-2 disavow scope matching,
SPEC-2 scope-propagated lineage resolution, and SPEC-3 permission cells. Petal affinity is
checked with `envelope::Scope::contains`, never with a second containment implementation.

## Policy numbers stay with the caller

Under D-CL24 no number in this module has a default. `Quarantine::with_capacity`,
`artifact::NonceLedger::with_capacity`, `payload_shard::SegmentSizeCaveat`, and
`checkpoint_proof::ProofBounds` all take explicit values and none implements `Default`, because
a default here would silently become the deployed policy.

## Fixture suite 65535

`artifact::admit_sealed` calls `EncryptionParams::assert_production_suite` unconditionally.
There is no `fixture` cargo feature and no `#[cfg]` around the check: a compile-time gate would
delete the rejection from exactly the build that needs it. This crate has no `[features]`
table.

## §discovery-lanes — four kinds of traffic, three normative lanes

SPEC-6 §7.1 says the derivation, the topic epoch and the lane labels are normative in SPEC-3 §6
and MUST NOT be reimplemented with a different MAC. SPEC-3 §6.1 key 1 defines exactly THREE
lanes: `0` header, `1` payload, `2` availability. This module therefore names no lanes and
invents no label strings. `discovery_labels::authorize_lane_subscription` takes a
`capability::topic::TopicLane` and a `capability::verbs::ObjectClass`, builds the normative
five-key §6.1 `TopicLabel`, and hands it to `BlindedTopicDerivation` — which Wave 3 implements
with `capability::topic::derive_topic_name` and nothing else.

The four traffic kinds §7.1 lists map onto the three lanes like this, per §7.2 ("Manifest and
availability traffic uses the authorized lane selected by SPEC-3"):

| §7 traffic | §6.1 lane | Scope (`LaneKey::topic_scope`) | Typical `object_class` |
| --- | --- | --- | --- |
| header segments | `0` header | verse-wide | `Operation` |
| payload shards | `1` payload | petal-affine | `Shard` |
| segment manifest | `0` header | verse-wide | `Segment` |
| availability (`have`, announcements) | `2` availability | the announced artifact's lane scope | class of the announced artifact |

The manifest rides the **header** lane because §7.2 binds the header lane to the verse-wide
topic/key scope and a manifest is verse- and branch-wide rather than petal-affine; `object_class`
key 2 is what separates it from raw header traffic, so no fourth lane code is needed. Availability
is a distinct lane code and stays advisory — §5.6, a `have` is never a proof.

The deleted `DiscoveryLane` enum invented a fourth `Manifest` variant with no lane code to encode
and four `fe-lane-*-v1` label strings that appear in no spec, and its trait signature supplied no
`object_class` at all, so §6.1 key 2 was unsatisfiable. The seam is now structurally implementable
from `derive_topic_name` alone.

## Provisional wire numbering

Every number this module assigned now lives in the crate-root register,
`src/AGENTS.md` §"Provisional wire numbering" — the single surface the owner ratifies. It is
deliberately **not** duplicated here: two copies of a key table drift exactly the way two
canonical encoders drift. The sealed outer map, lane classes, header body, payload topic scope,
shard record, shard body, HashSeq node, lane key, segment manifest, and the per-lane AEAD AAD
domains are all recorded there, with the reasons a choice was needed.

## Conformance tests

The §8 named tests implemented here, with their homes:

| Test | File |
| --- | --- |
| `segment_id_hashes_exact_stored_ciphertext` | `artifact.rs` |
| `uniform_encryption_has_no_plaintext_segment_fallback` | `artifact.rs` |
| `segment_uses_scope_key_and_fresh_xchacha_nonce` | `artifact.rs` |
| `immutable_segment_never_overwrites_prior_bytes` | `store.rs` |
| `header_lane_is_verse_wide_and_payload_free` | `header_lane.rs` |
| `payload_shard_is_petal_affine_and_scope_pure` | `payload_shard.rs` |
| `payload_shard_rehashes_each_record_against_header` | `payload_shard.rs` |
| `hashseq_reachability_requires_complete_predecessor_walk` | `hashseq.rs` |
| `checkpoint_proof_covers_selected_parent_closure` | `checkpoint_proof.rs` |
| `checkpoint_signature_is_not_history_authority` | `checkpoint_proof.rs` |
| `receipt_rehashes_reassembled_range_before_serving` | `receipt.rs` |
| `relay_seed_and_fetch_capabilities_are_independent` | `relay_policy.rs` |
| `scope_epoch_bump_stops_old_lane_service` | `relay_policy.rs` |
| `key_wrap_rotation_blocks_old_epoch_segment_service` | `relay_policy.rs` |
| `private_discovery_uses_blinded_lane_separation` | `discovery_labels.rs` |
