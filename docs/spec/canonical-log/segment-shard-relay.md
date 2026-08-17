# Canonical log segment, shard, and relay trust model v1

**Status:** Owner-approved 2026-08-09. Implementation (Workstream G) is unlocked; network rollout, relay seeding, and inbound P2P remain owner-gated.

This document defines immutable BLAKE3-addressed delivery artifacts for the
Canonical Fractal Data Log. It implements the segment and relay parts of
D-CL1, D-CL2, D-CL3, and D-CL4. It is read with
[operation-envelope.md](operation-envelope.md) (SPEC-1) and
[capabilities-and-revocation.md](capabilities-and-revocation.md) (SPEC-3).

Section 9 applies the D-CL17 V1 AEAD, nonce, and scope-key delivery contract
to stored artifacts. This document authorizes neither its implementation nor
network wiring or a change to the currently disabled iroh paths.

## 1. Conformance vocabulary and boundaries

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are
   normative.
2. A **stored artifact** is the exact byte sequence retained, fetched, or
   seeded. Its `artifact_id` is `BLAKE3(stored_artifact_bytes)`. It is a
   32-byte binary value and is never a text URL in a private discovery path.
3. A **segment** is a sealed stored artifact whose decrypted body holds one
   lane of delivery records. A segment is immutable: changing one byte creates
   a distinct artifact and MUST NOT overwrite the old artifact at its ID.
4. A **header segment** contains complete canonical SPEC-1 headers and no
   payload ciphertext. It is scoped to exactly one verse and is available to
   authorized verse members.
5. A **payload shard** is a sealed payload segment containing payload
   ciphertext artifacts for exactly one authorized payload topic scope. It is
   petal-affine under D-CL2.
6. A **segment manifest** is a sealed immutable index that commits to one
   collection of header HashSeq roots and payload-shard HashSeq roots. Its
   `segment_manifest_id` is its `artifact_id`.
7. A **HashSeq** is an immutable linked sequence of segment references. It is
   a delivery/reachability index, not a replacement for the operation DAG:
   parent references in SPEC-1 alone establish operation causality.
8. An artifact being present at a peer or relay is not admission, authority,
   or materialization. Admission remains the SPEC-1/SPEC-3 and SPEC-4 process.

## 2. Sealed artifact identity and immutability

### 2.1 Stored form

1. A segment, HashSeq node, and segment manifest MUST each be a deterministic
   CBOR outer map using the restrictions in SPEC-1 section 2. Its encrypted
   body is an opaque byte string in that outer map.
2. The outer map MUST carry only: format version, lane class, ciphertext
   length, the encryption descriptor, and ciphertext. It MUST NOT carry a raw
   verse ID, petal ID, resource ID, Hexon URI, capability chain, or unblinded
   topic name.
3. The encryption descriptor identifies the V1 scope-key encryption contract:
   `suite_id = 1` (XChaCha20-Poly1305), a `key_id`, and a fresh random 24-byte
   nonce. It is subject to the fixed-suite rule in SPEC-1 section 3.5.
4. `artifact_id` MUST hash the complete stored outer-map bytes, including its
   encryption descriptor and ciphertext. A transport MUST compare that digest
   and the declared stored byte length with the requested values before it
   treats a receipt as complete.
5. The decrypted inner body MUST also use deterministic CBOR. A receiver MUST
   reject non-canonical encoding, unknown required fields, duplicate fields,
   or a lane/body mismatch.
6. A content-addressed store MAY retain multiple physical copies of identical
   artifact bytes. It MUST NOT mutate an existing copy in place, alias a
   different byte sequence to an existing ID, or make a mutable lookup key
   authoritative for history.

### 2.2 Uniform encryption

1. Every non-empty operation payload is encrypted as required by SPEC-1.
   Every segment, HashSeq node, and segment manifest is also encrypted under
   the applicable scope-key contract. This is the one encrypted pipeline in
   D-CL1; there is no plaintext segment or "trusted relay" exception.
2. Header segments and their HashSeq/manifest records use the verse-wide
   header scope. Payload shards and their HashSeq records use their one
   payload-topic scope. A public scope publishes the applicable key material
   through its authorized manifest; it still uses the same sealed artifact
   format.
3. Header visibility does not imply payload access. `fetch`, `decrypt`,
   `materialize`, and `seed` remain distinct SPEC-3 authorities even when an
   authorized member can decrypt the verse-wide header lane.
4. The applicable scope key seals operation payloads, payload shards, header
   segments, manifests, and scope-affine checkpoint artifacts. Its delivery,
   rotation, topic-key derivation, and destruction follow SPEC-3 section 10;
   implementations MUST NOT use any undocumented key convention.

## 3. Lanes and shard packing

### 3.1 Header lane

1. A header segment body contains an ordered set of exact complete SPEC-1
   envelope bytes. Each record is indexed by its derived `op_id`.
2. Each enclosed header MUST have the same `scope.verse_id` as the header
   segment body. Its `scope.petal_id` MAY differ from another header record;
   this is how one per-verse DAG remains causally complete.
3. A header segment MUST NOT include an operation payload ciphertext, decoded
   payload, capability-chain bytes, or private key material.
4. Receipt validation MUST recalculate every enclosed `op_id`, verify its
   canonical bytes and signature, and reject a duplicate `op_id` whose bytes
   differ. A valid header alone remains opaque and non-materializable until
   its payload and all normal admission conditions verify.

### 3.2 Payload-shard lane

1. A payload shard body contains records of the form
   `(op_id, ciphertext_hash, ciphertext_length, ciphertext_bytes)`. The hash
   and length MUST exactly equal the referenced SPEC-1 payload fields in the
   corresponding header.
2. A shard MUST have exactly one canonical payload-topic scope. The scope MUST
   identify one petal. Resource-scoped records MAY share a shard only when all
   resolve to that same petal and use that payload-topic scope/key contract.
   A shard MUST NOT mix petals, verses, scope epochs, or key identifiers.
3. Each `op_id` occurs at most once in a shard. A record's
   `ciphertext_hash = BLAKE3(ciphertext_bytes)` and its exact byte length MUST
   verify before the record is indexed. A receiver with `decrypt/op` then
   verifies the operation payload AEAD associated data from SPEC-1 section
   5.2 before any materialization attempt.
4. A payload shard is a packing and availability optimization only. Splitting
   or repacking an already-valid payload ciphertext creates different shard
   artifacts but MUST NOT change the header, payload artifact hash, operation
   ID, selected frontier, or materialized result.
5. Shard packing MUST obey the effective capability's `max_segment_bytes`
   caveat. A sealer MUST reject an oversized candidate rather than create a
   special oversized lane or silently omit records.
6. An operation whose payload logically spans scopes is not representable by a
   mixed-scope shard. It MUST be decomposed into scope-local intent operations
   whose SPEC-1 parent links express their cross-petal causality.

### 3.3 Segment manifest

1. A segment manifest body MUST bind: `protocol_version`, verse scope,
   `branch_id`, a sorted set of header HashSeq roots, a sorted mapping of
   payload-topic scope to HashSeq roots, the declared availability boundary,
   the clear statistics block of rule 5, and the sealed statistics mapping of
   rule 6. IDs and maps are canonical byte-sorted.
2. A manifest MUST reference artifact IDs and exact stored byte lengths. It
   MUST NOT restate mutable storage locations as history facts.
3. A payload-root mapping MUST use a petal-affine payload-topic scope. The
   mapping is an index for authorized fetches; it does not authorize decryption
   or prove that a receiver possesses the relevant key.
4. A manifest MAY have multiple roots for a lane because publishers can create
   immutable sequences concurrently. The root set is canonical and deduped by
   artifact ID. No relay or publisher may hide a required root by changing a
   signed checkpoint's bound manifest.
5. **Statistics are tiered.** A manifest body MUST carry a clear statistics
   block containing exactly: the inclusive minimum and maximum author HLC over
   the operations it indexes, the canonically sorted set of petal identifiers
   those operations name, and the operation count. It MUST be present, not
   optional: a manifest without a range is one no peer can skip, so an omitted
   block silently degrades selective fetch to fetch-everything. The block MUST
   be internally consistent — minimum not after maximum, and a zero count if
   and only if the manifest claims no lane — and its petal set MUST be strictly
   ascending. Every value in it is derivable from signed header fields alone;
   nothing in it may be derived from payload plaintext.
6. **Fine-grained statistics MUST NOT appear in the clear.** Per-column minima
   and maxima, histograms, distinct counts, bloom filters, and any other
   statistic derived from payload contents MUST be carried in a separate sealed
   artifact under the lane's own scope key, referenced from the manifest by
   `artifact_id` and exact stored byte length only. A manifest MAY carry at most
   one such reference per lane it claims, and MUST NOT reference a lane it does
   not claim. A manifest is sealed under the verse-wide header scope (§2.2.2),
   so anything placed in its clear body is legible to every authorized verse
   member including one with no payload capability for the petals indexed; a
   minimum and maximum on a position column would disclose a project's
   real-world location verse-wide. This specification fixes the tier boundary
   and the reference shape, not the sealed artifact's interior format.
7. The clear block of rule 5 is the only statistic a peer may use to skip a
   segment without a capability check. Resolving a rule 6 reference is a normal
   authorized `fetch` plus `decrypt` under SPEC-3; possession of the manifest
   confers neither.

## 4. HashSeq and reachability proofs

### 4.1 HashSeq node

1. A HashSeq node is a sealed artifact with an inner body containing its lane,
   exact scope binding, one `predecessor_id` or `null`, and a non-empty ordered
   list of `(artifact_id, stored_length)` entries.
2. Entries are ordered by the order in which this immutable node was sealed;
   they MUST NOT be presented as causal order. Each entry ID is unique within a
   node. An identical artifact in separate nodes is deduplicated by ID when a
   proof is evaluated.
3. A node's predecessor, when non-null, MUST be another HashSeq node with the
   same lane and exact scope binding. A cycle, scope change, lane change, or
   missing required predecessor invalidates that HashSeq path.
4. A HashSeq root set in a manifest defines all paths reachable by walking
   each root to `null`. The manifest's availability boundary identifies the
   oldest node(s) that must be present for its claim; a receiver MUST NOT
   report complete coverage when that boundary is unavailable.

### 4.2 Checkpoint reachability proof

1. A signed checkpoint under D-CL4 MUST bind its exact
   `segment_manifest_id` in addition to the branch ID, sorted-frontier hash,
   materializer identity/version, and projection-root hash required by SPEC-4.
2. A checkpoint verifier MUST first verify the Manager+ signature and the
   signed checkpoint bytes. That signature makes a claim; it does not make the
   manifest or enclosed history valid.
3. To accept the checkpoint as an accelerator, the verifier MUST:

   1. re-hash the manifest and all fetched HashSeq nodes and segments;
   2. decrypt and canonical-validate the manifest and each required HashSeq
      node under the applicable authorized scope;
   3. walk every declared header HashSeq root to the manifest boundary and
      collect exact headers after recalculating their `op_id`s;
    4. walk SPEC-1 parents from every member of the checkpoint's selected
       sorted frontier to genesis or the checkpoint's declared replay base,
       requiring every reachable header in that closure to be present and
       admitted; and
   5. for every payload-bearing reachable header that the projection needs,
      find a matching payload-shard record, re-hash it, and perform the
      applicable SPEC-1/SPEC-3 decryption and admission checks.

4. A member without the capability or key for a petal payload MAY verify the
   verse-wide header closure, but MUST describe the result as
   `header-reachable-only`. It MUST NOT claim a verified projection root,
   complete materialization, or full checkpoint validity for content it could
   not authenticate.
5. A missing predecessor, missing required header, absent payload artifact,
   malformed shard, failed hash, unavailable scope key, unknown schema, or
   invalid authorization makes the affected head unresolved. A verifier MUST
   not silently skip it and then call the checkpoint complete.
6. The proof covers the selected-frontier parent closure, not every concurrent
   operation ever observed in the verse. An unrelated concurrent operation is
   not a missing ancestor; every selected frontier member and every selected
   multi-parent merge makes all of its parents required.

## 5. Receipt, validation, and availability behavior

1. A peer, relay, cache, or import path MUST re-hash every complete artifact
   it receives before recording that artifact as present, serving it, or using
   it in a HashSeq proof. Streaming receipts remain incomplete until the final
   byte count and BLAKE3 digest verify.
2. A range or chunk transport MAY retry missing bytes, but it MUST re-hash the
   reassembled complete stored artifact. A per-chunk checksum never replaces
   the final `artifact_id` check.
3. A receiver MUST validate the sealed outer form before decryption, then
   resolve the current authorized scope key and authenticate/decrypt the inner
   body before trusting lane, scope, predecessor, index, or record fields.
4. Header and payload references MUST be cross-checked in both directions:
   shard records may not introduce an unreferenced payload as a committed
   operation, and a projected payload-bearing header must resolve to exactly
   matching ciphertext bytes.
5. A failed receipt is untrusted input. It MAY be retained in a bounded
   quarantine for diagnostics or retry, but MUST NOT be seeded as verified,
   appended to the verified log, included in a checkpoint proof, or
   materialized. Quarantine size and retention are SPEC-5 policy.
6. Artifact availability is advisory. A `have`, announcement, cache hit, or
   relay response MUST NOT be treated as a proof of validity, authorization,
   completeness, or durable retention.

## 6. Relay and seeder constraints

1. A relay is a transport and ciphertext seeder only. It MAY retain opaque
   immutable artifacts and answer authorized fetches; it is not an operation
   author, capability issuer, admission service, branch authority, or
   checkpoint authority.
2. A relay MUST require a currently valid `seed` capability before accepting a
   seed commitment and a currently valid `fetch` capability before disclosing
   an artifact. Possession of an `artifact_id`, a previous cache entry, or a
   relay URL is never sufficient authority.
3. A relay MAY operate without `decrypt` authority. It MUST NOT decrypt,
   inspect, transform, repack, re-encrypt, or fabricate payloads, headers,
   HashSeq nodes, manifests, checkpoints, capabilities, or signatures.
4. A relay MUST consult the persistent SPEC-3 authorization view after restart
   and on epoch invalidation. Missed notification traffic is a cache miss, not
   permission to disclose. Revocation prevents future service; retention and
   cryptographic erasure obligations are governed by SPEC-5 and D-CL17.
5. A relay MAY report opaque availability only to an authorized requester. It
   MUST NOT publish raw private scope IDs, raw hash inventories, or membership
   data to global discovery, and MUST NOT claim a checkpoint is complete just
   because it has stored referenced bytes.
6. Relay transport failure, retention expiry, and a refusal to seed are normal
   availability outcomes. They MUST return an explicit unavailable result; no
   peer may substitute mutable row replication, a different selected frontier,
   or unverified local state as a recovery path.

## 7. Blinded discovery and selective fetch

1. Private header, payload, manifest, and availability traffic MUST use the
   lane-separated blinded topic derivation in SPEC-3 section 6. The exact
   BLAKE3-keyed topic derivation, topic epoch, and lane labels are normative
   there and MUST NOT be reimplemented with raw IDs or a different MAC.
2. A header lane uses the verse-wide topic/key scope. A payload lane uses the
   relevant petal-affine payload-topic scope. Manifest and availability traffic
   uses the authorized lane selected by SPEC-3; it MUST NOT become public just
   because it lists content-addressed artifacts.
3. A peer MUST validate an applicable capability before subscribing,
   announcing, requesting, or responding on a private lane. It MUST rotate or
   leave affected topics on a scope-epoch bump.
4. Segment IDs and HashSeq roots may travel inside an authorized sealed lane
   or authenticated request. They MUST NOT be used as global advertisements
   for private content. Public data uses the same encrypted artifact pipeline
   with deliberately published key material.
5. Blinding limits identifier enumeration; it is not anonymity or protection
   against traffic analysis. Implementations MUST NOT claim that it conceals
   peer IP addresses, timing, ciphertext length, or an authorized member's
   ability to correlate its own scope activity.

## 8. Required conformance tests

A future implementation MUST provide deterministic fixtures and at least these
named tests:

1. **`segment_id_hashes_exact_stored_ciphertext`** — changing outer metadata,
   encryption descriptor, ciphertext, or length changes the BLAKE3 artifact ID
   and is rejected for the old ID.
2. **`immutable_segment_never_overwrites_prior_bytes`** — a conflicting write
   for an existing artifact ID is rejected and cannot alter a verified proof.
3. **`header_lane_is_verse_wide_and_payload_free`** — valid cross-petal
   headers share one verse header lane, while payload ciphertext and capability
   bytes are rejected from it.
4. **`payload_shard_is_petal_affine_and_scope_pure`** — mixed verse, petal,
   scope epoch, or key identifier records are rejected; resource records from
   one allowed petal lane are accepted.
5. **`payload_shard_rehashes_each_record_against_header`** — altered bytes,
   wrong length, wrong hash, duplicate `op_id`, and an unreferenced record all
   fail receipt validation.
6. **`hashseq_reachability_requires_complete_predecessor_walk`** — a missing,
   cyclic, cross-scope, or cross-lane predecessor prevents a complete proof.
7. **`checkpoint_proof_covers_selected_parent_closure`** — a signed checkpoint
   fails when any selected-head ancestor header or required authorized payload
   is omitted, but does not require unrelated concurrent history.
8. **`checkpoint_signature_is_not_history_authority`** — a valid Manager+
   signature cannot make malformed, unauthorized, or hash-mismatched segment
   data valid.
9. **`receipt_rehashes_reassembled_range_before_serving`** — valid per-range
   checksums with a wrong final stored digest never enter the available set.
10. **`relay_seed_and_fetch_capabilities_are_independent`** — a relay accepts
    seed only with `seed`, discloses only with `fetch`, and never gains
    `decrypt`, `append`, or checkpoint authority by storing bytes.
11. **`private_discovery_uses_blinded_lane_separation`** — raw verse/petal/
    resource IDs and raw private hashes do not appear in topic names; header,
    payload, and availability labels differ for the same scope epoch.
12. **`scope_epoch_bump_stops_old_lane_service`** — a relay and peer reject
    old-topic requests and revalidate capabilities after the persistent epoch
    view advances.
13. **`uniform_encryption_has_no_plaintext_segment_fallback`** — no non-empty
    header segment, payload shard, HashSeq node, or manifest is accepted on a
    plaintext path, including public scopes.
14. **`segment_uses_scope_key_and_fresh_xchacha_nonce`** — every non-empty
     stored artifact uses `suite_id = 1`, a current scope/epoch key ID, a fresh
     24-byte nonce, and the correct scope-bound AAD; a fixture-only suite or
     old-epoch key is rejected.
15. **`key_wrap_rotation_blocks_old_epoch_segment_service`** — after an epoch
     bump, no old-key artifact is newly sealed or served as current, and only
     current authorized recipient-device wraps can obtain the e+1 key.
16. **`manifest_carries_coarse_clear_statistics_and_only_a_sealed_reference_for_fine_ones`**
     — a decoded manifest exposes the §3.3.5 HLC range, petal set, and count in
     the clear and exposes fine-grained statistics only as an `artifact_id` and
     stored length; no per-column value appears anywhere in the manifest's
     plaintext body.
17. **`manifest_refuses_inconsistent_or_foreign_statistics`** — an inverted HLC
     range, a non-zero count with no claimed lane, a zero count with claimed
     roots, a non-ascending petal set, or a sealed statistics reference naming
     an unclaimed lane is rejected rather than normalized.

## 9. D-CL17 sealed-artifact key lifecycle

1. Every non-empty stored artifact uses XChaCha20-Poly1305 with the applicable
   32-byte current scope key, `suite_id = 1`, a fresh CSPRNG-generated 24-byte
   nonce, and a format-specific domain-separated AAD that binds its canonical
   outer metadata. `suite_id = 65535` is fixture-only and rejected in
   production.
2. A scope key is generated, recipient-device wrapped, renewed on reconnect,
   rotated on epoch bump, and destroyed for crypto-shredding exactly as
   specified by SPEC-3 section 10. Segment, shard, manifest, and snapshot code
   never invents a separate group key or distributes a raw scope key.
3. A new authorized recipient obtains a fresh X25519 HPKE-style device wrap
   only after Manager+ issuer, recipient device, capability, and current epoch
   validation. After removal, the epoch bump creates an e+1 key and prevents
   future e-key wraps; old ciphertext may remain immutable evidence.
4. A relay may retain and serve opaque sealed artifacts only under the existing
   `fetch`/`seed` checks. It receives neither scope keys nor decryption
   authority. Workstream G remains blocked: this contract does not enable Iroh,
   relay replicas, inbound P2P handlers, or network transport.

## 10. Design notes

- **One DAG, sparse payloads:** Verse-wide encrypted headers preserve causal
  visibility. Petal-affine encrypted payload shards keep a mobile peer from
  needing every payload to understand the verse's DAG.
- **HashSeq is delivery structure:** It proves which immutable artifacts a
  manifest claims to cover, while SPEC-1 parent walks prove which operations a
  selected frontier actually requires. Neither a relay nor a HashSeq order can
  invent causality.
- **Hash proves bytes, not access:** BLAKE3 detects substitution after receipt.
  It never grants fetch, decryption, materialization, or seeding authority.
- **No erase promise from storage:** Immutable ciphertext can outlive access.
  The strongest future erasure property is crypto-shredding under the ratified
  scope-key lifecycle, not physical deletion from every historical peer.
- **Skipping is a privilege boundary, not just an index:** Every statistic that
  makes a segment skippable is also a statistic that leaks. §3.3 answers that by
  splitting on *derivation*, not on usefulness: what a signed header already
  discloses to a verse member may be clear, and what payload plaintext would
  disclose must be sealed — even when the sealed tier is the more useful one.

### Errata

Owner-ratified under D-CL28, 2026-08-16.

- **G4 (2026-08-16):** §3.3 gained rules 5, 6, and 7, and rule 1's binding list
  now names both statistics tiers. The manifest previously bound roots, lengths,
  and an availability boundary but carried no time range and no statistics at
  all, so a peer could not decide whether a segment was worth fetching without
  fetching it — while the obvious fix, putting column statistics in the manifest,
  would have published payload-derived values under the verse-wide header scope.
  The rules land before Wave 3 because this is a wire format: adding the slots
  now is an addition, and adding them afterwards is a migration.
