# Relay custody and the durability floor v1 (SPEC-9, relay scope)

**Status:** Drafted 2026-11-20 from owner decisions D-CL36, D-CL38, D-CL39,
D-CL43, D-CL45 and D-CL48 (track `relay_record_warehouse_20261120`, slice S2).
Every wire value here is provisional under D-CL24(3) and joins the single
`CDL-PROV` register in `fe-canonical-log/src/AGENTS.md` §Provisional wire
numbering (registered by track slice I8). No interop claim is made until that
batch is ratified. Wave-0 critic verdict: pass with fixes (see Errata).

This document is the relay-scoped part of SPEC-9. It defines how a record relay
acknowledges custody of sealed segments, when a segment counts as recorded, and
what that placement information discloses. General peer N-of-M placement,
under-replication repair and range-to-holder resolution remain outside it
(track non-goal NG-6). It is read with
[segment-shard-relay.md](segment-shard-relay.md) (SPEC-6),
[capabilities-and-revocation.md](capabilities-and-revocation.md) (SPEC-3) and
[branches-checkpoints-retention.md](branches-checkpoints-retention.md)
(SPEC-5).

## 1. Conformance vocabulary and boundaries

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are
   normative.
2. A **record relay** is a relay running the record-relay profile for exactly
   one verse (section 5). It is the only relay role that signs custody
   receipts. A connectivity relay and a desktop relay are not record relays.
3. The **custody set** of a segment manifest `M` is the set of stored
   artifacts made of: `M` itself; every HashSeq node reachable from each header
   and payload root that `M` declares, walked to and including `M`'s declared
   availability boundary (SPEC-6 section 4.1 rule 4); and every header segment
   and payload shard those nodes list. A bulk artifact (SPEC-6 section 3.4) is
   never in a custody set, because no manifest indexes one.
4. A **custody receipt** is a record relay's signed statement that it durably
   holds every artifact of one custody set (section 3).
5. A segment is **recorded** when it belongs to the custody set of at least one
   manifest whose durability floor is met (section 4).
6. The **assessor** is whoever evaluates a floor: a peer deciding whether to
   prune, or the warehouse status read.
7. A custody receipt attests possession only. It never attests validity,
   authorization, completeness of history, or a checkpoint, and it grants no
   authority (SPEC-6 section 5 rule 6 and section 6 rule 1).
8. The warehouse settings named here (`warehouse_mode`, `relay_key_custody`,
   `record_relays`, `peer_prune_after_floor`) are the per-verse settings that
   the verse admin (Manager+ at verse scope) sets under D-CL43. R is not one of
   them: it is the verse row's existing `ts_replication_factor`.

## 2. Custody set identity

1. `custody_set_id` is:

   ```text
   custody_set_id = BLAKE3(
       ASCII("fe-custody-set-v1") || 00 || canonical_custody_set
   )
   ```

   `canonical_custody_set` is a deterministic-CBOR array of two-entry arrays
   `[artifact_id, stored_length]`, where `artifact_id` is a 32-byte byte string
   and `stored_length` an unsigned `u64`. The array is strictly ascending by
   `artifact_id` byte order and has no duplicate (SPEC-3 section 2.1 rule 4).
2. A peer that holds the verse-wide header scope key computes the custody set
   by decrypting `M` and walking its HashSeq roots under SPEC-6 section 4. It
   MUST NOT compute it from a relay's statement or a seed commitment.
3. A seed commitment names `M`'s `artifact_id` and the `canonical_custody_set`
   the seeder computed. Its transport encoding belongs to the segment
   transport; it carries opaque artifacts and never a parsed manifest.
4. A blind relay cannot read `M`. It computes `custody_set_id` only from the
   seed commitment's list, after it has received and re-hashed every listed
   artifact (section 3.3).

## 3. Custody receipt

### 3.1 Wire form

A custody receipt is one deterministic-CBOR map with exactly eight keys in
numeric order, under SPEC-1 section 2 restrictions. It is a signed attestation,
not a SPEC-6 stored artifact, so SPEC-6 section 2.1 rule 2 does not apply to
it; its disclosure is limited by section 7 instead.

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `receipt_version` | unsigned integer | MUST be `1`. |
| 1 | `verse_id` | `Identifier32` | The SPEC-1 scope `verse_id` of the record relay's bound verse. |
| 2 | `manifest_id` | `Hash32` | The `artifact_id` of manifest `M`. |
| 3 | `custody_set_id` | `Hash32` | Section 2 rule 1 over the set the relay holds. |
| 4 | `relay_principal` | principal map | SPEC-3 section 2.1 rule 2 `{0: did, 1: public_key}`. |
| 5 | `held_at_ms` | unsigned `u64` | The relay's Unix millisecond time when the set became durable. |
| 6 | `warehouse_format_epoch` | unsigned `u64` | The relay's record format epoch (section 6). |
| 7 | `signature` | 64-byte byte string | Ed25519 over the preimage below. |

The unsigned receipt is the exact canonical map of keys 0 through 6. Its
signature preimage is:

```text
ASCII("fe-custody-receipt-v1") || 00 || unsigned_receipt
```

`signature` is deterministic Ed25519 signing of that preimage by
`relay_principal.public_key`. `receipt_id = BLAKE3(complete_receipt)`, where
`complete_receipt` includes key 7.

### 3.2 Verification

A verifier MUST, in order:

1. decode and re-encode the receipt byte-for-byte, rejecting a non-canonical
   encoding, a missing, duplicate or unknown key, or a version other than `1`;
2. verify the `relay_principal` DID-to-key binding; and
3. verify `signature` over the section 3.1 preimage.

A receipt that passes is **verified**. Verification is not qualification:
section 4 decides whether a verified receipt counts.

### 3.3 Issuance by a record relay

1. A record relay MUST sign a receipt only after it has received every artifact
   in the seed commitment's list, re-hashed each one to its `artifact_id`,
   checked each exact `stored_length` (SPEC-6 section 5 rules 1 and 2), and
   stored all of them durably. `M`'s own `artifact_id` MUST be in the list.
2. It MUST NOT sign a receipt naming any verse other than its bound verse
   (section 5), or carrying a `warehouse_format_epoch` other than its own.
3. It MUST accept the seed only under SPEC-6 section 6 rule 2: the seeder
   presents a currently valid `seed` capability for the verse. A refused seed
   produces no receipt and stores nothing.
4. A signed receipt is a durable custody acknowledgement in the SPEC-5 section
   5.3 sense, with no expiry in v1: retention durations are owner policy, not
   set here. It ends only by a section 6 reset, by the relay leaving its
   verse binding, or by an explicit custody-failure report. Silent eviction is
   prohibited (SPEC-5 section 5.2 rule 2).
5. Re-seeding a set the relay already holds MAY return a fresh receipt for the
   same `(manifest_id, custody_set_id)`. Assessors count it once (section 4
   rule 3).

## 4. Durability floor

1. **R.** For verse `V`, `R` is `V`'s current `ts_replication_factor`, read
   at assessment time. A usable R is at least `1`; a missing, zero or
   unreadable value makes the floor unknown (rule 5), never met, and is never
   replaced by a default. For the floor, R is a hard
   requirement and is never clamped to the relays available. Timeseries
   placement keeps its own clamping; this rule does not change it.
2. **Qualifying receipt.** For an assessment of manifest `M` in verse `V`, a
   receipt qualifies only when every one of these holds at assessment time:

   | # | Condition |
   | ---: | --- |
   | Q1 | It is verified under section 3.2. |
   | Q2 | Its `verse_id` equals `V`. |
   | Q3 | Its `manifest_id` equals `M`. |
   | Q4 | Its `custody_set_id` equals the value the assessor computed for `M` under section 2 rule 2. |
   | Q5 | Its `warehouse_format_epoch` equals the assessor's. |
   | Q6 | Its `relay_principal.did` is on `V`'s current `record_relays` list. |
   | Q7 | The persistent SPEC-3 authorization view shows `relay_principal` currently holding a valid `seed` capability for class `segment` at `V`'s verse-wide scope and current epoch. |

3. **Distinct relays.** Qualifying receipts are counted by distinct
   `relay_principal.public_key`. Several receipts from one relay count once.
4. **Floor met.** The floor for `M` is met when the count is at least R.
5. **Unknown.** An assessor that cannot compute the custody set of `M` (it has
   no header scope key, or `M` or a required HashSeq node is unavailable) MUST
   report the floor for `M` as unknown, never as met.
6. **Fresh evaluation.** Q6 and Q7 are evaluated against current durable
   state on every assessment. A stored receipt is evidence, not a cached
   verdict: a relay removed from the list, or whose `seed` lapsed or was
   revoked, stops counting at once. A cache miss in the authorization view is
   a non-qualifying receipt (SPEC-3 section 5.3 rule 5).
7. **`held_at_ms` is advisory.** It MAY be shown. It MUST NOT be used to
   qualify or disqualify a receipt.
8. **Not met with too few relays.** When `record_relays` lists fewer than R
   principals, no manifest of `V` can meet the floor. The status read MUST
   report this; the assessor MUST NOT lower R.

### 4.1 Pruning behind the floor

1. A peer MAY release a locally held artifact only when all of these hold:
   SPEC-5 permits the release (checkpoint, replay-tail and tombstone
   conditions); `V`'s `peer_prune_after_floor` is on; and the artifact is in
   the custody set of a manifest whose floor is met at the moment of release.
2. Floor evidence is a required input to the one prune decision. An
   implementation MUST NOT provide a prune path that can run without it, and
   MUST NOT represent "no evidence" as a default that passes.
3. A tombstone and the evidence SPEC-5 section 5.4 rules 1 and 2 require for
   no-resurrection are never released by this section, whatever the floor.
4. A bulk artifact is in no custody set (section 1 rule 3), so rule 1 never
   permits its release (SPEC-5 section 5.2 rule 6.5).
5. After release, the peer no longer holds those bytes and MUST NOT seed,
   advertise or receipt them (SPEC-5 section 5.1 rule 4).
6. Repair after a relay later drops out is outside this document. The status
   read reports the current qualifying count per recent manifest so an operator
   can see a manifest fall below R.

## 5. One record relay, one verse

1. A record relay is configured with exactly one verse at start. It MUST refuse
   to start with zero verses or with more than one, and name the reason.
2. It MUST refuse a seed for any other verse and store nothing. It MUST answer
   a fetch for any other verse exactly as it answers a fetch for an absent
   artifact.
3. Its custody ledger MUST refuse any record that names another verse.
4. A record relay principal serves one verse for its whole life. Moving a
   relay to another verse requires a new principal key; a principal MUST NOT
   be listed as a record relay of two verses.
5. A connectivity relay and a desktop relay keep serving many verses as
   transports (D-CL48). They MUST NOT sign custody receipts. A receipt from a
   principal not running the record-relay profile is a protocol violation;
   assessors cannot detect the profile directly, so Q6 and Q7 together are the
   control. Q6 alone is not enough: `record_relays` is a verse-row column that
   rides legacy row replication, so Q7, a `seed` chain a Manager+ issuer
   signed (D-CL46), is the cryptographic half and is never skipped because Q6
   passed.

## 6. Pre-GA reset

1. Every binary carries one `warehouse_format_epoch` constant. A record relay
   stamps its held records and custody ledger with it.
2. On start, a record relay whose stamp differs from its binary's constant MUST
   wipe every held artifact and its custody ledger, log one line naming the old
   and new epoch, and then accept seeds again. It MUST NOT migrate held
   records.
3. A wiped relay answers a custody query for a wiped manifest as not held.
   Receipts it signed under the old epoch stop qualifying for any assessor
   already on the new epoch (Q5).
4. A peer that still holds a manifest whose qualifying count is below R SHOULD
   re-seed it to the listed relays. The floor recovers as peers re-seed.
5. A reset never touches legacy rows or any peer's local verified log.
6. This section expires at `CDL-PROV` ratification (D-CL38). After that, a
   format change needs a migration specification, and this section is
   withdrawn by an Errata entry.

## 7. Placement-privacy analysis (NFR-4)

Custody receipts and the record-relay list together form a placement index:
which relays hold which manifests of which verse, and when. This section is the
SPEC-9 hard gate, scoped to record relays, measured against SPEC-3 blinded
discovery (SPEC-3 sections 6 and 7).

### 7.1 Disclosure table

| Surface | Reveals | To whom | Against SPEC-3 blinded discovery | Mitigation | Residual |
| --- | --- | --- | --- | --- | --- |
| Custody receipt | Raw `verse_id`, `manifest_id`, `custody_set_id`, relay DID, `held_at_ms`, format epoch | The seeding peer; verse members via sealed announcements; Viewer+ via status | Raw verse and manifest IDs are exactly what SPEC-3 section 6 bars from discovery | P-1, P-2, P-3 | Verse members learn relay placement and custody timing |
| `record_relays` list | Record-relay DIDs of the verse | Anyone who can read the verse's legacy replica rows | Comparable to a verse-wide header-lane fact, but on the legacy row plane, which has no blinded discovery | Holds DIDs only; dialing data comes from the existing signed relay adverts | Any reader of the verse replica learns which principals keep its record |
| Relay-to-verse binding (D-CL45) | A record relay endpoint identifies one verse | The relay operator; a network observer who learns the endpoint's binding | Blinding cannot help: the endpoint itself is the identifier. A concentrated form of SPEC-3 section 7's relay/IP row | P-4, P-5 | Seeing a peer connect to a known record relay reveals that the peer belongs to that verse |
| Seed commitment | Artifact IDs, stored lengths, and their grouping per manifest | The record relay | The relay already sees ciphertext sizes and request timing; grouping adds per-manifest structure | Ciphertext only; blind by default | Per-manifest size and seeding cadence |
| Canonical announcements | Manifest IDs and receipts | Subscribers of the verse sync topic | The legacy topic is `BLAKE3("verse:" + verse_id)`, unkeyed and computable by anyone who knows the verse ID; SPEC-3 section 6 would forbid it for private discovery | P-2 seals the body | Topic existence, message timing and frame size, as for all legacy gossip today |
| Warehouse status read | Counts, manifest IDs, relay DIDs | Viewer+ of that verse | Authenticated and verse-scoped; no discovery surface | P-3 | Viewers without payload keys see header-lane facts only |
| Keyholder relay | Plaintext of every scope it is wrapped for | The relay operator | Outside blinding: the relay is an authorized key holder | Blind default; admin opt-in; keyholder → blind bumps the epoch (SPEC-6 section 6 rule 3) | The operator keeps whatever it already decrypted or copied |

### 7.2 Rules

1. **P-1.** A record relay delivers a custody receipt only in the response to
   the authenticated seed exchange that produced it, or to an authenticated
   requester holding `fetch` for its verse. It MUST NOT publish receipts in a
   relay advert, a public topic, or any unauthenticated response.
2. **P-2.** A peer re-shares receipts and manifest announcements only inside a
   frame body sealed under the verse-wide header scope key with
   XChaCha20-Poly1305, a fresh 24-byte nonce and the AAD domain
   `fe-canonical-announce-v1\0` (provisional), in addition to gossip signing.
   It MUST NOT send them in the clear on the legacy verse topic.
3. **P-3.** The warehouse status read requires Viewer+ at verse scope. A
   foreign or unauthorized verse gets the same bytes as a verse that was never
   registered.
4. **P-4.** No advertisement published outside the verse's own replica may
   name the verse a record relay serves, or mark the relay as a record relay.
   The binding is disclosed only through that verse's `record_relays` list.
5. **P-5.** Section 5 rule 4 holds: one principal, one verse, so a relay key
   never links two verses.
6. **P-6.** User and operator documentation MUST state the section 7.1
   residuals, as SPEC-3 section 7 requires for its own.

### 7.3 Gate result

With P-1 to P-6 in force, the placement index discloses nothing to a non-member
beyond the residuals SPEC-3 section 7 already documents, with one exception:
the relay-to-verse binding. That row is a new, concentrated form of SPEC-3's
relay/IP residual. It follows directly from the owner's D-CL45 and is accepted
here only as a documented residual, not mitigated.

## 8. Required conformance tests

An implementation MUST provide deterministic fixtures and at least these named
cases. They are table rows in existing journey tests where one fits, not
necessarily separate test functions.

1. **`custody_receipt_v1_round_trip_and_domain`** — a fixture receipt encodes
   byte-exactly, its signature verifies under `fe-custody-receipt-v1\0` only,
   and `receipt_id` is BLAKE3 of the complete bytes.
2. **`custody_receipt_rejects_noncanonical_or_tampered`** — unordered,
   duplicate, missing or unknown keys, version not `1`, a DID/key mismatch, and
   any changed signed field all fail verification.
3. **`durability_floor_counts_distinct_qualifying_relays`** — with R=2, one
   qualifying receipt leaves the floor unmet and prune refuses; a second
   distinct relay meets it and prune releases (AC-5.1).
4. **`durability_floor_excludes_non_qualifying_receipts`** — one table: a
   duplicate from the same relay, a relay without `seed`, an unlisted relay, a
   foreign verse, a wrong manifest, a wrong custody set, a foreign format
   epoch, and a bad signature each add nothing to the count (AC-5.2).
5. **`floor_is_unknown_without_custody_set`** — an assessor without the header
   scope key reports unknown, never met.
6. **`prune_requires_floor_evidence_and_keeps_tombstones`** — no prune path
   runs without floor evidence, and a no-resurrection tombstone survives a met
   floor (AC-5.3).
7. **`relay_signs_receipt_only_after_full_custody_set_rehash`** — a missing,
   altered or wrong-length set member yields no receipt.
8. **`record_relay_binds_exactly_one_verse`** — zero or two configured verses
   fail start with a named reason; a foreign-verse seed is refused; a
   foreign-verse fetch matches the absent answer (AC-14.1).
9. **`format_epoch_mismatch_wipes_and_invalidates_receipts`** — a relay with a
   stale stamp wipes, logs one line, and its old receipts stop qualifying
   (AC-11.1).
10. **`receipts_travel_only_on_authenticated_or_sealed_channels`** — an
    unauthenticated custody query gets nothing, and an announcement frame
    carries no receipt bytes outside its sealed body.

## 9. Design notes

- **Why the receipt binds the custody set, not just the manifest:** a blind
  relay cannot read a manifest, so on its own it cannot know whether it holds
  everything the manifest needs. Binding `custody_set_id` turns the receipt
  into a precise claim that any keyed assessor can check against its own walk
  (Q4). A seeder bug that lists too few artifacts then produces receipts that
  never count, instead of a floor that is met on paper and missing bytes in
  fact.
- **Why the format epoch is in the signed body:** D-CL38 lets a relay wipe its
  records. A receipt with no epoch would keep counting after the wipe, and a
  peer could prune data that no relay holds any more.
- **Why qualification is re-evaluated every time:** the list and the
  capability are admin-controlled and revocable. Caching a "qualified" verdict
  would let a removed or revoked relay keep a verse's floor met.
- **Why R is not clamped here:** timeseries placement clamps R to the peers it
  can reach, because placement is best effort. The floor is a safety gate for
  deleting data, so falling short must refuse rather than quietly accept less.
- **Seed chains are short-lived:** SPEC-3 caps a certificate at 24 hours, so a
  relay whose `seed` chain is not renewed stops qualifying within a day. The
  failure is "no pruning", never "data lost".
- **Why announcements are sealed:** the legacy verse gossip topic is an
  unkeyed hash of the verse ID. Anyone who knows the ID can compute it, which
  is the enumeration SPEC-3 section 6 exists to stop. Sealing the frame body
  keeps the placement index readable only by holders of the header scope key,
  without having to move the existing topic first.

## 10. Open items for the owner

Listed, not assumed. None blocks drafting; each needs an owner answer before
the behaviour it names is relied on.

1. **Prune before ratification.** D-CL38 lets relays wipe records, and D-CL39
   lets peers prune once the floor is met. Together they can lose data that
   was pruned while recorded only on relays that later reset. Rule 6.3 makes
   old receipts stop counting, but it cannot restore bytes already released.
   The default `peer_prune_after_floor = off` (track FR-8) is the only
   mitigation. Should pruning stay off until `CDL-PROV` ratification?
2. **Relay-to-verse binding residual.** Section 7.3 accepts it as documented.
   Confirm that it is acceptable, or ask for a mitigation, such as co-locating
   record and connectivity roles on one endpoint.
3. **Bulk artifacts.** glTF asset bytes and other bulk artifacts are outside
   every custody set (section 1 rule 3), so the floor does not cover them.
   Their durability stays with the existing blob paths, and section 4.1 rule
   4 forbids pruning them, so the gap costs disk, never data. Should a later
   version give bulk artifacts their own receipts?

### Errata

- **2026-11-20 (advocate review, wave 0):** section 4.1 gained rule 4 (bulk
  artifacts are never released under this section), section 10 item 3 was
  updated to match, and R is named by its column, `ts_replication_factor`.
- **2026-11-20 (critic review, wave 0):** section 4 rule 1 now makes a
  missing, zero or unreadable R an unknown floor rather than an implied `1`,
  matching SPEC-5 section 5.2 rule 6.2. Section 5 rule 5 no longer names the
  replicated `record_relays` list as the sole control; Q7's Manager+-issued
  `seed` chain is the signed half.
