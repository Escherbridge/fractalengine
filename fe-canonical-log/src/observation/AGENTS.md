# observation — SPEC-10 kind 5, operation half (CDL-SPEC10 B1)

`docs/spec/canonical-log/observation-plane.md` (SPEC-10) is authoritative wherever this file or
the campaign plan summaries differ (plan C20). This module owns the parts of the observation plane
that are pure protocol: the `observation_batch` payload codec, its schema hash, the anchor
mapping, the kind-5 reduction, and the kind availability a quarantine re-check asks about. It has
no storage, no Parquet, no network. The columnar hexon codec is B2 (`fe-query`), the sealer,
hydrator and reference table are B3 (`fe-database canon_log::observation`), and the preview codec
lives in `wire/preview.rs` (§preview-kind-registry there).

| File | SPEC-10 | Contents |
| --- | --- | --- |
| `mod.rs` | §4.1 rule 3, §4.5 | `observation_batch_schema_hash`, `node_anchor`, `SpecRegisteredKinds` |
| `payload.rs` | §4.3 | `ObservationBatchPayload` (+`Parts`, `HlcRange`, `SensorTimeRange`), `validate_observation_batch_plaintext` |
| `reduce.rs` | §4.4 | `reduce_observation_batch`, `ObservationBatchMaterializer`, `ObservationBatchReference` |

The kind itself is in `kind.rs`: `OperationKind::ObservationBatch` (wire 5), the §4.1 rule 2
header rule (`StructuralRuleError::ObservationBatch*`), and
`OperationKind::requires_registered_interpretation`.

## §structural-rule — header only, at the one ingress

`kind::validate_structural_rules` runs inside `signing::decode_and_admit`, so every peer byte
string meets it before anything else (`src/AGENTS.md` §ingress): at least one parent, an
encrypted non-empty payload (never the SPEC-1 §3.5 no-payload form), and `scope.petal_id`
non-null with `scope.resource_id` null. A failure is `SigningError::Structural`, which
`verify_candidate` maps to `invalid_envelope` — exactly SPEC-10 §4.1 rule 2. Nothing in the rule
reads the payload: a petal scope and a payload are checkable on the header, which is half
the D-CL31 argument for a dedicated kind over a kind-1 schema.

## §unknown-kind — kind 5 still needs a registry that interprets it

Kind 5 is not an `Application` kind any more, but admission still asks the payload schema
registry whether this build interprets it: `OperationKind::requires_registered_interpretation`
is true for `ObservationBatch` and `Application(_)`, and both `ComposedCandidateVerifier::
verify_envelope` and `fe-database`'s `RegisteredSchemaSet::validate` key on it. So a build whose
registry has not registered the observation schema quarantines kind 5 as `unknown_kind` (its own
G1 budget, SPEC-10 §4.1 rule 4) instead of letting it through to `unknown_schema`. Before B1 the
three sites matched `Application(_)` literally; adding the variant without moving them would have
silently re-routed an unregistered kind 5 from `unknown_kind` to `unknown_schema`, the budget
D-CL31 exists to protect.

B3 registers the schema: `RegisteredSchemaSet::register(OBSERVATION_BATCH_OPERATION_KIND,
observation_batch_schema_hash(), validate_observation_batch_plaintext)`. Any other schema hash
on kind 5 is then `unknown_schema` (§4.1 rule 3) by construction of that registry.

`SpecRegisteredKinds` is the `retention::quarantine::OperationKindAvailability` a quarantine
re-check uses: kinds 1-5 are known to this build, application kinds never are. It is how a kind-5
record an older build quarantined promotes after an upgrade (`unknown_kind_promotion_ready`).

## §payload — the eleven-key map

`ObservationBatchPayload` has private fields; `new`, `from_cbor` and `decode_canonical` are the
only doors, and each enforces every §4.3 rule: exactly keys 0-10, ranges not inverted (HLC by
`(wall_ms, counter)`), `count >= 1`, `anchors` non-empty / strictly ascending / no longer than
`count`, `asset_refs` strictly ascending (may be empty), `sources` non-empty and strictly
ascending by canonical encoding (each re-validated by A2's `ObservationSource::from_cbor`),
`hexon_format_version == 1`, `hexon_artifact_id != stats_hash`, both stored lengths non-zero.
`new` never sorts or repairs; `ObservationBatchPayloadParts::normalized` is the sealer's explicit
sort+dedup step, so a receiver can never "fix" a peer's unsorted list into acceptance.

`count` is attacker-chosen and is compared, never used to size an allocation. `sources` are
encoded once in `new` for the ordering check rather than once per comparison.

Anchors come from `node_anchor(node_id)`, which delegates to `fe_sdk::legacy_identifier(
LegacyTable::Node, id)` — SPEC-10 §4.5 rule 2 forbids a second mapping, so nothing here hashes a
node id itself.

## §reduction — reference only, and what `None` means

`reduce_observation_batch(meta, envelope_bytes, payload_facts)` is pure and storage-free; the
materializer is a unit struct with nothing to fetch with, which is how §4.4 rule 2 (never open,
fetch, presence-test, or measure the hexon, the statistics artifact, or an asset blob) holds by
construction rather than by review.

| Input | Output |
| --- | --- |
| kind other than 5 | `Excluded` — this projection does not project it |
| facts absent, or facts for another `op_id` | `ReferencedArtifactUnavailable { PayloadRef::ciphertext_hash }` of the operation's OWN payload (`meta.op_id` if the bytes are not that operation) |
| `meta`/facts schema ≠ observation schema | `Excluded` (admission would have quarantined it as `unknown_schema`) |
| facts that do not decode as §4.3 | `Excluded` with the codec error (admission validated them, so unreachable through `admit_candidate_with_facts`) |
| otherwise | one `Apply`: `CREATE observation_batch CONTENT $observation_batch` |

**The `None` contract (RUNBOOK §4 AC-33 default, plan B1 "Reduction contract").** SPEC-10 §4.4
rule 3 forbids a kind-5 reduction from returning `referenced_artifact_unavailable` for the hexon,
statistics artifact, or an asset: the signed payload binds their identities and lengths, so the
reference row is always constructible from facts. It does not let the reduction invent facts it
was not given. When the caller holds no decrypted payload, the reduction is pending on the
operation's own payload ciphertext — the general `materialize/AGENTS.md` §payload-facts rule —
and never fakes a `stats_hash`, `count`, or range. That is the legacy `None` wrapper's outcome;
B2b's replay facts source re-derives facts through custody so a durable replay hands this
function `Some`, and B2b's own distinct pending reason covers a shredded scope key. The B2b
Errata scopes §4.4 rule 3 to the hexon/statistics/asset artifacts accordingly.

**The reference row** (`ObservationBatchReference::to_row`) is a flat text-keyed map: `op_id`,
`verse_id`, `petal_id`, `author_public_key`, `hlc_wall_ms`, `hlc_counter`, `hexon_artifact_id`,
`hexon_stored_length`, `hlc_{min,max}_{wall_ms,counter}`, `sensor_time_{min,max}_ms`,
`stats_hash`, `count`, `anchors`, `asset_refs`, `sources` (each a source's canonical signed
bytes, in signed order), `hexon_format_version`, `stats_stored_length`. Byte fields stay
`CborValue::Bytes`; `fe-database`'s `cbor_to_json` renders them as lowercase hex. It is keyed by
`op_id` as a field, not a record id: apply markers already make a re-apply a no-op, and B3's
`observation_batch` table definition owns the unique index on `op_id` (and the schema the row
must satisfy). A row layout change is a new `OBSERVATION_BATCH_MATERIALIZER_VERSION`.

## §append-shard — where SPEC-10 §4.2 rule 2 is enforced (plan B1, AC-36g)

§4.2 rule 2 requires `append/shard` to seal the hexon and statistics artifact. **Decision: the
B3 sealer self-checks it before signing; B1 adds no admission check and does not touch the
capability step of `compose/verifier.rs`.** Reasons: admission authorizes the *operation*
(`append/op` with `allowed_operation_kinds ∋ 5`, already checked by the composed verifier from
the signed envelope); SPEC-4 §4 rule 8 forbids admission from reading the hexon, so a receiver
cannot even see which artifact was sealed by whom; and the sealed artifact's own key/epoch gate
is `seal_artifact`'s, which the sealer passes through anyway. A receiver checks the shard-side
authority where it can — at hydration, which requires `fetch/shard`, `decrypt/shard` and
`materialize/shard` (§5.1 rule 1, B3). B1's only edit to `compose/verifier.rs` is the
`requires_registered_interpretation` routing in §unknown-kind above.

## Hand-off

- **B2** writes the hexon whose `anchor` column uses `fe_sdk::legacy_identifier(LegacyTable::Node,
  ..)` — the same function `node_anchor` wraps — and computes the stats summary that becomes
  `stats_hash`.
- **B3** assembles `ObservationBatchPayloadParts` from its window, calls `.normalized()`, signs
  kind 5 with `observation_batch_schema_hash()`, registers the schema (above), defines the
  `observation_batch` table for the reference row, and wires `ObservationBatchMaterializer`
  (identity `ObservationBatchMaterializer::version()`) into replay/rebuild through B2b's facts
  source.
