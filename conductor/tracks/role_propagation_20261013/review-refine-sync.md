---
type: Security Review
track: role_propagation_20261013
phase: refine R1 (sync plane)
reviewed_range: 4d3a1a6..5ef9996
date: 2026-10-25
verdict: FIXED
---

# Refine R1: sync-plane security review and fix (blob-gate binding redesign, OF-8 and OF-9)

**Verdict: FIXED.** Every finding below was fixed in this commit. No CRITICAL or HIGH finding is left open.
The review ran in a fresh context and tried to refute the claims rather than confirm them. It covered the W5s blob gate,
`sharding.rs` (RP-5/6/7), the compute plane and the W6-8 relay config, checked against threats T12–T15 and T22–T28.
Gate: `cargo check -j4 -p fe-sync --tests` (env `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728`), exit 0, no
warnings. Tests were written but not run; the R5 sweep runs them. Only fe-sync was touched: `VerseFabric`,
`BlobAccessGate` and `note_petal_row` have no callers outside fe-sync, so fe-test-harness needed no change.

## The blob-gate redesign (directive in OF-8 AMENDED)

W5s had already patched the gate twice. Instead of a third patch, the binding model now rests on one invariant:
**a binding is set only by an authoritative local event, and gossip never mutates it.**

| Piece | Where | What |
|---|---|---|
| Fixed fetch context | `fe-sync/src/docs_engine.rs:268` `bind_fetched_root` | `learned` is set at this node's FIRST own fetch decision. A later fetch context is ignored, which means no remap and no narrowing. |
| No gossip entry point | `fe-sync/src/sync_thread.rs:840` | An inbound ad feeds only the provider map. `note_advertised_root` and `learned_overflow` were deleted. |
| Claimant rule | `docs_engine.rs:343` `admits_hash` | A hash is admitted if some claimant is an own PUBLISHED root that admits, OR if EVERY claimant admits on its learned path. The decision does not depend on the order roots were learned. |
| Child-range requests | `docs_engine.rs:382` `get_request_hashes`, `:529` | A `GetRequest` that reaches into HashSeq children is answerable only for a tracked root with recorded children. |
| Format upgrade | `docs_engine.rs:65,142` | A format-1 file's `learned` sets are dropped on load. This fails closed, and the next own fetch re-learns them. |
| Fetch-context integrity | `sync_thread.rs:2491` (eviction), `:2559` `verses_for`, `:2574`/`:2992` provenance | Eviction preserves verses. The fetch context covers the tileset id OR the root. An unvouched provider sets the `UNVOUCHED_FETCH_CONTEXT` marker. |

What the redesign keeps working: an untracked raw hash is still admitted (asset/doc sync, `FetchBlob`). A publisher
still serves its own tilesets to open-verse Viewers. The intent of all 10 W5s tests is preserved. The one gossip-API
test was rewritten to the new model as `a_root_bound_to_its_first_fetch_context_is_never_rebound_or_narrowed`.

## Findings

| # | Sev | Finding | Disposition | Flow test |
|---|---|---|---|---|
| OF-8a | HIGH | Any-parent admission let a confused deputy through. `set_children` indexed every child of a FETCHED (attacker-shaped) HashSeq, and `.any(..)` over parents gave the attacker's viewers the victim's bundles. | **FIXED**: the claimant AND rule (an own publish is the only override), and `set_children` is set-once. | `a_fetched_hash_seq_listing_a_victims_children_never_lends_them_its_viewers` (both learn orders) |
| OF-8b | MED (escalated) | Gossip narrowing plus a persisted `learned_overflow` gave any attacker a durable refuse-everyone state. | **FIXED**: gossip no longer reaches the gate, the binding is fixed at first learn, and format-1 narrowing is dropped on load (reversible). | `a_root_bound_to_its_first_fetch_context_is_never_rebound_or_narrowed`, `a_pre_r1_state_file_drops_its_gossip_narrowed_bindings_and_relearns_on_the_next_fetch` |
| OF-9 | HIGH | `note_petal_row` accepted and OVERWROTE the petal→fractal map from any replicated row, bending every `role_at` scope chain. | **FIXED** at `sharding.rs:598`. The gate mirrors the DB plane's A3 `confine_petal`: ids must be scope-safe, the author needs Editor+ at the target petal scope AND at the current scope when the row moves a petal, failing rows are quarantined, own writes are admitted locally, and a move re-verdicts holders. Editor+ was chosen over Manager+ because the DB plane lets Editors create petals; a stricter sync gate would strand those petals' `__shards` rows forever. The squat residual (the first authorized mapping wins) matches the DB plane by construction. | `a_petal_row_without_authority_over_its_scopes_never_bends_the_scope_chain` |
| R1-1 | HIGH (new) | Child-range laundering. iroh-blobs streams every child that a root blob lists. An UNTRACKED HashSeq this node stores (for example a replicated row's content blob listing a protected bundle hash) was admitted as "untracked", so a `GetRequest::all` on it streamed tracked tileset bytes to anyone. | **FIXED**: `get_request_hashes` makes a child-range request answerable only for a tracked root with known children, and every streamed hash must admit. No client in the workspace sends child-range requests. | `a_child_range_request_through_an_untracked_hash_seq_cannot_launder_a_tracked_bundle` (real loopback) |
| R1-2 | HIGH (new) | The fetch context could be shrunk. (a) `verses_for` filtered on the root under ONE tileset id, so a victim root re-advertised under a fresh id on the attacker's verse was bound to that verse alone. (b) Per-tileset eviction dropped the oldest hint, so a flood on one verse evicted the victim verse's last entry. Both are OF-6a remaps by another route. | **FIXED**: the context now covers the tileset id OR the root, and eviction prefers an entry whose verse another entry still represents. | `an_attacker_cannot_shrink_a_fetched_roots_bound_verse_set` (drives the real gate) |
| R1-3 | MED (new) | Third-party provider laundering. A relayed hint names a provider that serves THIS node because of this node's membership in some other verse; re-serving those bytes to the hint's verse launders them one hop. The attacker can self-vouch as a provider to stay in the candidate list. | **FIXED**: the provenance rule. If any candidate provider never vouched (no authenticated entry) for this tileset or root, the context gets `UNVOUCHED_FETCH_CONTEXT`: the node fetches for local use but never seeds onward. | `a_root_fetched_through_an_unvouched_provider_is_never_secondary_seeded` (+ vouched control) |
| R1-4 | HIGH (new; OF-6d residual) | Sync-plane owner takeover on joiners. `seed_reconciliation` replays the replica BEFORE pending local writes flush, and any member holding the namespace secret can sign a `verse/` entry naming itself creator. First-self-signed-wins therefore pinned the attacker, and this node's own DB-admitted manifest could never correct it. After a restart the latest-per-key snapshot could hide our own entry entirely. | **FIXED**: `repin_owner_from_local` (`sharding.rs:551`) lets a local write or an own replay replace a false pin; the false owner's settings are dropped and the demote cascade drops its statements. Own-signed entries are `RowOrigin::OwnReplay` (corrective, but stale-safe: settings and petal moves still pass the gates). `reassert_own_rows` (`sync_thread.rs:1574,1743`) replays our own `verse/` and `petal/` entries by author after the snapshot. Remaining D-23 residual: a node that never writes a manifest (a relay that opened without an invite). | `this_nodes_own_manifest_corrects_an_owner_pinned_by_a_forged_self_signed_manifest`, `a_replayed_own_manifest_corrects_a_forged_owner_pin_without_overriding_newer_settings` |
| R1-5 | LOW (new) | Gating petal rows means a legitimate petal row can be quarantined and then evicted by a flood (T12). Separately, a later foreign entry on the same `petal/` key hides the genuine row from latest-per-key snapshots. | **FIXED**: `petal/` was added to the re-read-on-empowerment prefix set (`sync_thread.rs:1436`). Because re-reads forward to the DB thread, the DB plane recovers the shadowed row too. | `an_evicted_editors_petal_row_is_reread_from_the_replica_on_empowerment` |
| S-1 | INFO | Relayed gossip attribution is claim-based for requester and responder `from_did`. | **COVERED-ELSEWHERE**: pre-campaign residual recorded at `fe-sync/src/AGENTS.md` §transport-admission-control item 2 ("Residual (honest)… not closable without per-message signing"). Not in this diff. | — |
| S-2 | INFO | T28 (blob reach by known hash for non-tileset blobs). | **COVERED-ELSEWHERE**: plan.md §6 T28 row + ledger BLOB-HASH-CAPABILITY. R1-1 closes the tileset-laundering variant that T28 does not cover. | — |
| S-3 | INFO | Publish path: joining any `default_access: viewer` verse opens every published tileset to everyone. | **NOT-REAL (by design)**: a publish advertises to every joined verse, as documented in `fe-sync/src/AGENTS.md` §tileset-security. | — |

Threat sweep with no new finding:
- **T13**: settings are adopted only from the owner or a verse Manager+, at `sharding.rs` `note_verse_row_from`.
- **T14**: the all-authors `role/` snapshot is in `replicator.rs`.
- **T15**: the OF-7c merge excludes Unverified answerers, at `distributed_query.rs:310-340`.
- **T12**: role statements do not use the quarantine. Every quarantined row comes back through the re-read by author, and petal rows now do too.
- **T22**: the re-read by author.
- **T23**: `request_local_role_reemit` runs on open.
- **W6-8 relay token**: it attaches only to custom https or loopback relays (`relay_config.rs` `token_transport_is_secure`).
- **OF-4 verification**: belongs to R2 (DB plane).

## Watch items for R5 (sweep)
- `two_peer_tileset_stream`: the publisher path is unchanged. Bob's provider is authenticated (a direct ad), so no unvouched marker applies.
- An upgraded node with a format-1 `blob-access-gate.json` stops secondary-seeding previously downloaded roots until it re-fetches them. This is intended and fails closed.
- Fixtures that build a petal map from non-owner rows before any grant now see the row quarantined, not mapped. All in-tree fixtures use owner-authored rows after the manifest.
