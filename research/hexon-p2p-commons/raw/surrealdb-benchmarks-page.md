# Source: SurrealDB official benchmarks page + surrealkv README

URL: https://surrealdb.com/benchmarks ; https://github.com/surrealdb/surrealkv/blob/main/README.md
Fetch date: 2026-07-11
Query intent: SurrealKV write amplification / compaction numbers, embedded DB performance under replication-like load

## surrealdb.com/benchmarks
- Last run: 28 May 2026
- Hardware: AMD Ryzen Threadripper 9970X, 128GB RAM, Lexar EQ790 4TB NVMe (high-end dedicated hardware — NOT representative of a typical P2P peer/laptop)
- Dataset: 15,000,000 records; 128 clients x 48 concurrent queries
- SurrealDB (using its own KV layer, unspecified whether SurrealKV or RocksDB backend in this run) vs Redis/KeyDB single-record CRUD:
  - Create: 300.8k ops/s (3.5x faster than Redis)
  - Read: 288.1k ops/s (slightly behind Redis's 367.9k)
  - Update: 300.6k ops/s (3.4x faster than Redis)
  - Delete: 279.3k ops/s (2.8x faster than Redis)
- No RocksDB/LMDB-vs-SurrealKV specific comparison found on this page (crud-bench tool supports it, but the published headline numbers on the page itself are vs Redis/Postgres/Mongo/etc., not backend-vs-backend)

## surrealkv README (github)
- Migrated from VART (versioned adaptive radix trie, all-in-memory index) to an LSM-tree architecture specifically because "the entire index must fit in memory, making it unsuitable for datasets larger than available RAM" — i.e., the OLD design could not scale past RAM-sized working sets.
- Old design's stated write amplification problem: "Each update created new versions, leading to memory pressure."
- New LSM design: "leveled compaction" with "score-based compaction strategy" — no quantified before/after numbers published.
- Explicitly still labeled "beta," intended for "new use cases such as versioning/versioned queries," NOT positioned as a RocksDB/LMDB replacement.
- Platform caveats: Windows = "basic functionality" only, file ops "not thread safe (TODO)"; WASM unsupported; full support only on Linux/macOS.

## Confidence: MEDIUM — official numbers exist but (a) hardware is top-tier workstation-class, not representative of P2P peer devices, (b) no write-amplification/compaction numbers are actually quantified anywhere published, only architectural narrative.
