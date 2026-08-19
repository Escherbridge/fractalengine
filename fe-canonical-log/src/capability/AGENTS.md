# capability — SPEC-3 certificates, chains, permissions, revalidation, blinded topics

Normative source: `docs/spec/canonical-log/capabilities-and-revocation.md`. Byte-exact vector:
`fe-canonical-log/tests/fixtures/capability-chain-v1.json`. The fixture is read-only from the
code's point of view; code changes to match it, never the reverse.

## Why certificates live here and not in `fe-policy`

`fe-policy` is the runtime authorization engine: it answers "may this caller do this now" for
the live engine and is depended on by `fe-database`, `fe-hexon`, and the sync write path. A
capability certificate is not a runtime decision — it is a **wire artifact** with canonical
bytes, a content address, and a D-CL3 signature domain, and its bytes have to be produced and
verified identically by a peer that runs none of this engine.

Putting it in `fe-policy` fails twice:

- **Cycle.** `fe-canonical-log` must verify chains during admission, so it would depend on
  `fe-policy`; `fe-policy` would need the log's canonical CBOR codec, `Hash32`, `Scope`, and
  `sign_domain` to define the artifact at all. That is a dependency cycle, and breaking it by
  duplicating the codec would put two encoders behind one content address — which is a
  consensus fork, not a refactor (see `src/AGENTS.md` §cbor).
- **Blast radius.** `src/AGENTS.md` §forbidden-dependencies bars this crate from `fe-policy`,
  `fe-database`, `fe-identity`, and `fe-runtime` precisely so the log's byte-exactness tests
  never become hostage to a bevy/tokio/keyring build. A certificate parser that pulled in the
  policy engine would drag that whole graph into the one crate that must stay cheap to test.

The seam instead runs the other way: this module verifies the artifact and hands back a
[`VerifiedCapability`], and the *decision* inputs it cannot compute — Manager+ authority and
the current epoch — are read through the caller-implemented `ManagerAuthorityView` trait, which
the verifier queries itself. Wave 3 implements that trait over the persistent authorization
view; nothing here opens a database or a socket.

## Why SPEC-1 `Scope` is deliberately not `fe_policy::Scope`

They look alike and mean different things.

`fe_policy::Scope` is a runtime **role-resolution path** — a hierarchical string/identifier
walk that answers "what RoleLevel does this principal hold here", resolved against live
membership state and free to gain fields, string forms, or resolution rules as the product
changes.

`envelope::Scope` is a **signed wire tuple**: three 32-byte identifiers whose canonical CBOR
map is inside the signature preimage and inside `op_id`. Its containment relation (§1.7) is
part of the protocol, not of the product. Changing it — adding a level, accepting a wider
containment, admitting a resource without a petal — changes what past signatures mean and what
a remote peer computes. Because `Scope`'s fields are private and every constructor validates,
an invalid tuple cannot reach `contains` at all (`src/AGENTS.md` §scope).

So containment in this module is always `envelope::Scope::contains`, never a reimplementation
and never a policy lookup. A future need to map one onto the other belongs in the integration
layer that owns both, not in either definition.

## Module map

| File | Spec anchor | Holds |
| --- | --- | --- |
| `verbs.rs` | §3.1, §3.2 | `Verb`/`ObjectClass` and their non-empty, unknown-bit-rejecting bitsets |
| `caveats.rs` | §2.3, §2.4 step 5 | the six-key caveat map and `attenuates` |
| `certificate.rs` | §2.2 | the fifteen-key certificate, `fe-capability-cert-v1` signing, `certificate_id` |
| `chain.rs` | §2.4 | the three-key chain map, the nine ordered verification steps, and `ManagerAuthorityView` |
| `permissions.rs` | §3.3, §4.7 | the effective permission table and its two extras |
| `revalidation.rs` | §5.3 | cache keys, pinned sessions, and epoch-bump invalidation |
| `topic.rs` | §6.1 | the five-key topic label and its keyed BLAKE3 derivation |

## Invariants later slices must not relitigate

- **`fe-capability-cert-v1` is this artifact's only signature domain**, NUL-terminated, and
  distinct from `fe-oplog-v1` (D-CL3). A test signs a certificate body under the envelope
  domain and asserts the chain rejects it; a shared domain would let one artifact type be
  replayed as another.
- **The 24-hour maximum lifetime is a fixed protocol bound, not a policy knob.** It is the
  constant `MAXIMUM_CERTIFICATE_LIFETIME_MS`, always enforced. §2.4 step 7 lets an
  implementation be stricter, so `VerificationOptions::stricter_maximum_lifetime_ms` can lower
  it; nothing can raise it. `UnsignedCertificate::sign` refuses to mint an over-long
  certificate, while decoding stays permissive so a peer-supplied over-long certificate is
  refused at step 7 with the step's own typed error rather than as a parse failure.
- **Verification is ordered and short-circuits.** Every §2.4 step has its own `ChainError`
  variant, so a caller can tell "peer sent junk" from "peer sent a valid chain that does not
  cover this request" from "this chain was revoked". Tests assert the exact variant, never
  `is_err()`.
- **Both the attenuation direction and its diagnostics are one function.**
  `Caveats::attenuates` is `attenuation_violation(..).is_none()`; there is no second copy of
  the rule that could disagree with the error the chain reports.
- **The empty array is a valid maximal narrowing.** A child that lists no schema, resource, or
  operation kind has granted itself nothing, which attenuates. A child that drops a non-null
  parent row to `null` has granted itself everything, which does not.
- **The verifier asks the authorization view; it is never handed an answer.** SPEC-2 still owns
  Manager+ history and epoch state, but this module poses each question itself, keyed by the
  chain's own fields. See §step-3-asks-the-view-itself.
- **Absence is refusal wherever the request must know the answer.** Every allowlist caveat row
  denies a request that names nothing. The two numeric size rows now behave the same way for the
  request shapes that necessarily carry the quantity (`append`/`op` for `max_payload_bytes`,
  `append`/`segment` for `max_segment_bytes`): an unstated size is `PayloadSizeUnstated` /
  `SegmentSizeUnstated`, never an implicit zero that satisfies the bound. For every other shape
  `None` honestly means "this request carries no payload / seals no segment" and is allowed.
  `max_preview_hz` keeps `unwrap_or(0)` and is unreachable: §3.3 dashes the entire preview row,
  so a `Verb::Preview` request never reaches the caveat.
- **A request's two resource identifiers must agree.** `target_scope`'s third slot is
  scope-checked against `grant_scope`; the bare `resource_id` is only allowlist-checked. When
  `target_scope` names a resource, `resource_id` must be `None` or that same resource, or step 8
  returns `ResourceScopeMismatch` — otherwise a request could be authorized against one resource
  while naming another.

## §possession-is-never-authority

Two types in this crate carry authority out of the module, and their names assert something
their fields cannot.

`VerifiedCapability` is now `#[non_exhaustive]`. That is load-bearing, not
forward-compatibility boilerplate: it makes the struct literal writable only inside
`fe-canonical-log`, so `fe-api`, `fe-database`, and every future consumer can obtain one only
from `verify_chain`/`verify_chain_bytes`. Field reads are unaffected and no call site changed.
The same treatment is NOT applied to `materialize::VerifiedEnvelopeMeta`, which `fe-database`
builds with struct literals in a test helper; that type's guarantee remains a documented
obligation and its own doc comment says so rather than implying otherwise.

`PinnedSession` is the weaker of the two and the honest statement is that **it does not carry
the grant it was pinned from**. It records the leaf principal, chain, certificate, epoch scope,
epoch, expiry, and the subscribed scopes — but not `effective_verbs`, not
`effective_object_classes`, not `effective_caveats`, and not `effective_scope`. A handler
holding a `Valid` session therefore cannot tell whether that session may append, only fetch, or
merely seed, and `covers` answers a question about subscription, never about verbs, classes, or
caveats. Until those fields exist, `covers` + `is_still_valid` is NOT an authorization decision
and no call site may treat it as one; the sufficient check is re-verifying the chain for the
specific `AuthorizationRequest`. Adding the fields is a breaking change to a struct literal in
`fe-api/src/canonical_ws/handler.rs` and belongs to whoever owns both files at once.

## §step-3-asks-the-view-itself

§2.4 step 3 requires the root issuer to be a current Manager+ identity *for `epoch_scope`*, as
proven by *that certificate's* `issuer_authority_id`. Wave 2 first shipped that as a
caller-supplied `AuthorizationSnapshot.root_issuer_is_manager_plus` boolean, which is not the
same check: a boolean is bound to no authority id, no issuer key, and no scope, so a caller that
cached one across requests, or computed it from the requester instead of the root issuer, got a
successful step 3 over a chain rooted at a non-Manager identity. The same criticism applied to
`requester_is_manager_plus` and to `current_epoch`.

All three are now questions, not answers. `ManagerAuthorityView` is a required parameter of both
`verify_chain` and `verify_chain_bytes`, so there is no verification path that skips it, and the
verifier supplies every argument from the artifact in front of it:

| Question | Asked with | Refusal |
| --- | --- | --- |
| `authority_is_manager_plus` | the **root** certificate's own `issuer_authority_id`, `issuer`, `epoch_scope`, `scope_epoch` | `RootIssuerNotManagerPlus` / `RootAuthorityRecordUnknown` |
| `principal_is_manager_plus` | the principal step 6 bound (author for `append`, requester otherwise) and the leaf's `epoch_scope` | `PermissionError::ManagerPlusRequired` |
| `current_epoch` (from `AuthorizationView`) | the leaf's `epoch_scope` | `EpochScopeUnknown` / `EpochRevoked` |

`AuthorityState` has three states rather than a `bool` so that "the view holds no such record"
is a distinct, denying outcome instead of collapsing into "not Manager+". A view that cannot
resolve the anchor has not answered the question, and an unanswered question is a refusal.

`ManagerAuthorityView` extends `revalidation::AuthorizationView` rather than restating
`current_epoch`: one durable projection answers epoch and authority questions, and the §5.3
cache key needs its `version()` anyway.

`AuthorizationSnapshot` survives holding exactly one field,
`epoch_causally_valid_for_operation`. That one is genuinely not askable here: it depends on the
operation's position in the epoch bump's transitive parent closure (§5.2 rule 2), which is DAG
state that no field of a chain or an `AuthorizationRequest` identifies. It stays injected, and
it only ever *relaxes* step 9 when `VerificationOptions::allow_causal_replay` is also set.

## §revalidation — the one seam this crate cannot enforce

`AuthorizationView` is no longer dormant: it is the supertrait of `ManagerAuthorityView`, so
chain verification reads `current_epoch` through it on every §2.4 step 9. What remains without a
non-test caller is the *caching* half — `CacheKey`, `RevalidationGate`, and `PinnedSession` —
and that is the single exception to the crate's no-dormant-gates rule (`src/AGENTS.md`). It
cannot have a real caller here: the paths it protects — API request authorization, relay
disclosure, and WebSocket session pinning — live in `fe-api`, `fe-network`, and `fe-database`,
which this crate is forbidden to depend on.

The obligation is therefore explicit rather than implied. A caller MUST, per §5.3:

1. key every authorization cache by `CacheKey` (chain, epoch scope, epoch, expiry, and
   authority-view version) and reach the cache through `RevalidationGate::admitted_now` before
   any cached allow;
2. call `RevalidationGate::on_epoch_bump` on every admitted `scope_epoch_bump`;
3. call `PinnedSession::is_still_valid` on a timer, not only on traffic, and stop commits,
   previews, payload delivery, and snapshots on anything other than `Valid`, and
   `PinnedSession::covers` before serving any scope the handshake did not pin — remembering that
   `covers` is a subscription check and **not** an authorization one (§possession-is-never-authority);
4. read durable epoch state at startup, treating a missed notification as a cache miss.

**Which half of that is now structural.** Obligations 1 and 4 used to be prose only: the
Wave 2 API was `admit(CacheKey)` / `is_admitted(&CacheKey)`, both pure set operations over a key
the caller assembled, including its `authority_view_version`. A caller that stored a key and
compared it back — which is exactly what `fe-api` `state.rs` did — got a permanent allow, because
no code path read the view at question time. The `_now` pair closes that in the type system's
reach:

| Door | Reads the view? | Use |
| --- | --- | --- |
| `RevalidationGate::admitted_now(&AdmittedDecision, now_ms, &view)` | yes: expiry, `current_epoch`, `version` | the only door that may gate an allow |
| `RevalidationGate::admit_verified(&AdmittedDecision, &view)` | yes: `version` | record a fresh full verification |
| `RevalidationGate::is_admitted(&CacheKey)` | no | set membership; the caller owes the view read |
| `RevalidationGate::admit(CacheKey)` | no | set insertion; the caller owes the version |
| `PinnedSession::cache_key(version)` | no | the caller chooses the invalidation dimension |

`AdmittedDecision` exists to make the safe call the short one: it carries the four dimensions a
verification establishes and deliberately omits `authority_view_version`, so there is no
caller-supplied version to get stale. `CacheRefusal` distinguishes `Expired`,
`EpochScopeUnknown`, `EpochMoved`, `AuthorityViewChanged`, and `NeverAdmitted`, and every one of
them denies — an unanswered question is a refusal here as it is at §2.4 step 3.

Obligations 2 and 3 remain conventional: nothing in this crate can force a timer to run or an
epoch bump to reach `on_epoch_bump`. Until those call sites exist, this module is a contract for
them, not an enforcement point, and must not be described as one.

## Provisional wire numbering

D-CL24: where SPEC-3 describes a canonical structure in prose without a normative integer-key
table, the shape below is **provisional** and awaits owner ratification. No cross-implementation
interop is claimed for it. Everything not listed here comes from a normative table in the spec
(§2.2 keys 0-14, §2.3 keys 0-5, §2.4 keys 0-2, §6.1 keys 0-4) and is not provisional.

| Structure | Spec text | Provisional encoding | Reason a choice was needed |
| --- | --- | --- | --- |
| `canonical_scope_key_context` | §6, "the deterministic-CBOR pair of the exact scope map and its epoch" | two-element CBOR array `[scope_map, topic_epoch]`, epoch as an unsigned integer | "pair" names no container; an array is the smaller, order-fixed choice against a two-key map whose keys would themselves need numbering |

The crate-root `src/AGENTS.md` register indexes this row under "Provisional wire numbering";
the row here remains the source of truth for the encoding.

## Conformance tests

§8 names fourteen tests. Implemented here, under their exact spec names:

- `capability_chain_v1_golden_vector_round_trip` (`mod.rs`) — rebuilds both certificates from
  the committed fixture's field values, and asserts the unsigned bytes, the Ed25519 signature,
  the complete bytes, `certificate_id`, the chain bytes, and `chain_id` all reproduce
  byte-for-byte, then verifies the chain.
- `capability_chain_rejects_noncanonical_cbor` (`chain.rs`)
- `capability_chain_rejects_link_or_signature_substitution` (`chain.rs`)
- `capability_delegation_only_attenuates` (`chain.rs`)
- `capability_ttl_and_leaf_binding_are_enforced` (`chain.rs`)
- `relay_seeding_never_implies_decrypt_or_disclosure` (`chain.rs`) — the SPEC-3 half is a leaf
  that grants only `seed`, refusing `decrypt`, `append`, `materialize`, `fetch`, and the
  checkpoint class; the SPEC-6 half drives `segment::relay_policy` from a seed-only view and
  asserts no disclosure and no scope-key wrap. Neither half needs a relay service.
- `blinded_topic_derivation_is_deterministic_and_lane_separated` (`topic.rs`)
- `private_topic_requires_capability_and_rotates_on_epoch_bump` (`topic.rs`) — drives
  `segment::discovery_labels::authorize_lane_subscription` with the real SPEC-3 §6 derivation
  wired in as its `BlindedTopicDerivation`, so it covers the refusal of an unauthorized
  subscribe, the refusal of the pre-bump epoch after a bump, and the rotation of the label
  itself. A pubsub node is not needed: the gate and the derivation are both pure.

Deferred to Wave 3 because each needs durable epoch state across a restart, a live WebSocket
session, or key material this crate does not own: tests 6, 7, 8, 9, 13, and 14.

A test is deferred only when the code it would exercise does not exist here. Tests 10 and 12
were previously listed as deferred while the logic they name was already shipping, which
under-reports coverage and points Wave 3 at finished work — check the delivered modules before
adding a row here.

## Regenerating the fixture

`capability-chain-v1.json` was produced by an out-of-tree generator that reimplements the
canonical CBOR profile, `did:key`, Ed25519, and BLAKE3 independently of this crate — the point
being that the vector is not simply this encoder's own output echoed back. Regeneration is a
deliberate protocol change and requires owner sign-off, because every committed
`certificate_id` and `chain_id` moves with it.
