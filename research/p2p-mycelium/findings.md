# Research Findings: P2P Mycelium Architecture

Synthesized from primary-source WebFetch/WebSearch (April 2026). Citations inline.

## 1. iroh's Current State (April 2026)

### Critical version finding
- **iroh-blobs 0.35** is officially the last "production quality" version per the maintainers' own disclaimer on [lib.rs/crates/iroh-blobs](https://lib.rs/crates/iroh-blobs): *"this version of iroh-blobs is not yet considered production quality. For now, if you need production quality, use iroh-blobs 0.35."*
- Current dev version: **0.99.0** (March 17, 2026)
- 29 breaking releases between 0.35 and 0.99 — fractalengine is correctly pinned to 0.35
- **Implication:** We can build on 0.35, but should plan an upgrade path once 1.0 ships

### 1.0 roadmap ([iroh blog "Road to 1.0"](https://www.iroh.computer/blog/road-to-1-0))
- Targeted late 2025, still pending as of March 2026
- Protocols (blobs, docs, gossip) will move to separate repos with **independent version numbers**
- One more round of network protocol breaks planned before 1.0 freeze
- WASM browser support is on the roadmap
- **Implication:** API churn is guaranteed; isolate iroh usage behind a trait boundary

## 2. iroh Architecture Primitives

### iroh-blobs ([docs.rs](https://docs.rs/iroh-blobs/))
- **Content addressing:** 32-byte BLAKE3 `Hash`
- **Primitives:** `Hash`, `BlobsProtocol`, `HashAndFormat`, `BlobFormat` (raw or HashSeq)
- **Store backends:** memory (`mem`) or filesystem (`fs` feature flag)
- **Sharing:** `BlobTicket` = endpoint address + hash + format
- **HashSeq:** blobs containing sequences of hashes (multiples of 32 bytes) — useful for manifests
- **Transport:** QUIC, direct-first with relay fallback for NAT traversal

### iroh-docs ([docs.rs](https://docs.rs/iroh-docs/))
- **Model:** Replica = entries identified by `(namespace, author, key)`
- **Keys:** `Author` (write identity, ed25519), `NamespaceSecret` (write capability for a replica)
- **Sync:** range-based set reconciliation (same algorithmic family as Willow protocol)
- **Conflict resolution:** last-write-wins by timestamp per `(namespace, author, key)` triple
- **Storage:** `redb` embedded KV store
- **Entry = (key, hash, timestamp)** — the value is a BLAKE3 hash referencing an iroh-blobs entry
- **Sharing:** `DocTicket` = namespace key (secret or public) + peer list

### iroh-gossip
- Topic-based pub/sub with probabilistic delivery
- Used by iroh-docs for "live sync" notifications

### Composition
- **iroh-docs depends on iroh-blobs and iroh-gossip** — this is the production CRDT+blob stack
- Architecture quote: *"Docs is a 'meta protocol' that relies on the iroh-blobs and iroh-gossip protocols"*

## 3. SurrealDB / SurrealKV Capabilities

- SurrealKV is **append-only segmented storage** with **CRC32 integrity**, **transaction logs**, **trie-based in-memory index**
- SurrealDB docs claim "the append-only format makes replication procedures straightforward" — but **no replication protocol is shipped** in 3.0
- No multi-peer sync in SurrealDB 3.0 (confirmed via [SurrealDB repo](https://github.com/surrealdb/surrealdb))
- Discussion thread [iroh#1454](https://github.com/n0-computer/iroh/discussions/1454) proposes iroh as a storage backend — **feature proposal, not production**
- **Implication:** We cannot rely on SurrealDB to do its own P2P replication. We must build the bridge ourselves.

## 4. Comparable Systems

### Jazz.tools ([jazz.tools/docs](https://jazz.tools/docs))
- **CoValues** = primitive building block (similar role to our Nodes)
- Types: CoMap (object), CoList, CoFeed, CoText, **FileStream (binary)**, CoVector, ImageDefinition
- Authentication: passkeys, passphrases, Clerk, Better Auth
- E2E encryption, offline-support, version control built-in
- **Direct analog for fractalengine:** `Node` ≈ `CoMap`, `Asset` ≈ `FileStream`, `Petal` ≈ group/scope

### Willow Protocol
- Successor/sibling to iroh-docs' reconciliation algorithm
- **3D range-based set reconciliation** (vs iroh-docs' 1D)
- iroh team is implementing Willow (currently iroh-willow, unreleased) — [HN iroh dev comment](https://news.ycombinator.com/item?id=39028475)
- **Strategic:** iroh-docs may be deprecated in favor of Willow post-1.0. Don't over-invest in iroh-docs-specific schemas.

## 5. The 7 Local-First Ideals ([Ink & Switch 2019](https://www.inkandswitch.com/essay/local-first/))

1. **No spinners** — near-instant response, no server round-trips
2. **Not trapped on one device** — sync across devices
3. **Network is optional** — works fully offline
4. **Seamless collaboration** — no manual conflict resolution
5. **The Long Now** — data survives vendor shutdown
6. **Security & privacy by default** — E2E encryption
7. **You retain ultimate ownership** — bytes on your device

**Implicit tradeoffs called out:**
- Consistency vs availability: CRDTs give availability but eventual consistency
- Centralization vs longevity
- Developer simplicity vs user control

## 6. Architectural Decisions for fractalengine

### Decision: Hash-reference DB, blobs in iroh-blobs store
**Confidence: HIGH**

The base64-in-DB pattern is a scaling anti-pattern. Every client pulls the full DB on sync, every query reads GB of base64. Replace with:

```
asset table:
  asset_id: ULID (for human/stable reference)
  content_hash: BLAKE3 hex string (the blob key)
  name, size, content_type, created_at
  # NO data field
```

Blobs live in a local iroh-blobs `fs` store at `~/.local/share/fractalengine/blobs/`.

### Decision: iroh-docs per-verse namespace for DB sync
**Confidence: MEDIUM**

Use one iroh-docs namespace per **Verse** (tenant). Entries map to SurrealDB rows.

- `namespace_id` ↔ `verse_id`
- `key` = `{table}/{record_id}` (e.g., `node/01K7...`)
- `value` = hash of a JSON blob containing the row data (stored in iroh-blobs)
- `author` = the writing peer's ed25519 key (same as their `NodeKeypair` in `fe-identity`)

**Why namespace-per-verse:** authorization boundary aligns with human-meaningful unit. When a user is invited to a Verse, they get the `NamespaceSecret`; when revoked, they lose write access (but retain what they've already synced — this is an unavoidable property of P2P).

**Risk:** iroh-docs may be deprecated for Willow. **Mitigate** by isolating the bridge behind a trait:
```rust
trait VerseReplicator {
    fn write_row(&self, table: &str, id: &str, data: &[u8]) -> Result<()>;
    fn subscribe(&self) -> impl Stream<Item = RowChange>;
}
```

### Decision: Bridge SurrealDB ops → iroh-docs entries
**Confidence: MEDIUM**

Intercept writes at the `fe-database` handler layer. On every `CREATE`/`UPDATE`/`DELETE`, also write an iroh-docs entry. On iroh-docs `subscribe()` events from peers, apply to SurrealDB.

**Alternative rejected:** file-level replication of `data/fractalengine.db` via iroh-blobs. Simpler but loses per-row CRDT properties, forces full-db transfers, creates write conflicts at file level.

### Decision: Identity unification
**Confidence: HIGH**

Your `fe-identity::NodeKeypair` (ed25519) can serve as:
- iroh `SecretKey` (node identity on the network)
- iroh-docs `Author` key (write identity on replicas)
- DID method key (`did:key:z6Mk...`)
- Verse membership signature key

**But NOT the same keypair for per-verse write caps** — use iroh-docs `NamespaceSecret` separately (capability-based, revokable).

### Decision: Offline-first semantics
**Confidence: HIGH**

- Local writes commit to SurrealDB immediately
- iroh-docs bridge fires-and-forgets (crossbeam channel, never blocks UI)
- Incoming peer updates apply via last-write-wins by timestamp (iroh-docs default)
- **Divergence handling:** for most fields LWW is acceptable (name, position). For structural fields (parent_id), we may need manual "last-edit wins" + user notification
- **No "merge conflict" UI in v1** — accept LWW, document it clearly

### Decision: Bootstrap/discovery
**Confidence: MEDIUM**

Three tiers:
1. **Ticket invite:** User A generates a `DocTicket` (namespace key + their node address) and shares via out-of-band channel (QR, URL, copy-paste). User B applies ticket → joins Verse.
2. **Gossip seeding:** After bootstrap, members discover each other via iroh-gossip topic per-verse.
3. **No global DHT** — we are NOT building a public discovery service. Verses are private by default.

### Decision: Blob cache management
**Confidence: MEDIUM**

- **LRU eviction** by access time, with a user-configurable cap (default 5 GB)
- **Pinning:** any blob referenced by a Node the user has "visited" recently is protected
- **Fetch on demand:** when a Node is about to render, check local store, otherwise fetch from any peer in the Verse gossip
- **Fallback:** if no peer has the blob, fall back to `placeholder.glb` (already wired)

### Risk: asset deletion / right-to-erasure
**Confidence: LOW**

Content-addressed stores can't truly "delete" — once a peer has a blob, they have it. Mitigations:
- Mark blobs as "tombstoned" in the DB (iroh-docs entry with empty hash)
- Honor tombstones locally (don't render)
- **Do not promise GDPR compliance for user-uploaded assets in a Verse** — document this limitation

## 7. Migration Path from Current Base64-in-DB

### Phase A: Schema expand-only (no break)
1. Add `content_hash` column to `asset` table (nullable)
2. Add `fractalengine/assets/blobs/` local directory (separate from `imported/`)
3. On next app start: backfill — for every existing `asset` row with `data`, compute BLAKE3, write bytes to blob store, populate `content_hash`
4. `data` column remains populated (read path still works)

### Phase B: Switch read path
1. Asset loader reads from blob store by `content_hash` first, falls back to `data` field
2. New imports only write to blob store (no more base64 in DB)
3. Existing deployments keep working

### Phase C: Compact
1. Once all assets have `content_hash`, drop `data` column
2. DB file shrinks dramatically
3. Existing deployments upgrade by running migration script

### Phase D: Enable replication
1. Wire iroh-docs bridge for new rows
2. Initial sync: one-time export of existing rows to iroh-docs entries
3. Enable live sync with invite tickets

## 8. Known Unknowns (Research Gaps)

1. **iroh-docs throughput at scale:** no published benchmarks for >100 peers or >100k entries
2. **SurrealDB schema evolution while replicating:** no precedent for CRDT-synced SurrealDB
3. **Handling peer's iroh-docs storage growth over time:** GC story unclear
4. **Private Petals within public Verses:** trust boundary harder than per-Verse
5. **Mobile story:** iroh on mobile is not battle-tested for long-running P2P

## 9. Confidence Summary

| Claim | Confidence | Evidence |
|---|---|---|
| iroh 0.35 is prod-quality, 0.93+ is not | HIGH | Maintainer disclaimer |
| iroh-docs uses range-based set reconciliation | HIGH | Official docs + HN dev comments |
| SurrealDB has no built-in replication in 3.0 | HIGH | Repo inspection, no replication module |
| Jazz.tools architecture is comparable | HIGH | Their docs |
| Hash-ref pattern wins over base64-in-DB | HIGH | Universal across IPFS, git, Hypercore |
| LWW is acceptable for fractalengine v1 | MEDIUM | Fits creative tool semantics; user research needed |
| iroh-docs will be stable enough to depend on | MEDIUM | 1.0 imminent but protocol breaks planned |
| Willow will replace iroh-docs | MEDIUM | Confirmed in development, no timeline |
| Storage costs stay manageable at 100 peers | LOW | No empirical data |
