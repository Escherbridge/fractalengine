# Source: IPFS/Bitswap performance literature (aggregated search findings)

Fetch date: 2026-07-11
Query intent: why IPFS is slow, content discovery latency, bitswap vs bittorrent

## Key sources
- "Design and Evaluation of IPFS: A Storage Layer for the Decentralized Web" (arXiv 2208.05877, ar5iv.labs.arxiv.org/html/2208.05877)
- "A Closer Look into IPFS: Accessibility, Content, and Performance" (par.nsf.gov/servlets/purl/10547038)
- ipfs/go-bitswap issue #166 "Improving Latency Measurement"
- Protocol Labs research: "Accelerating Content Routing with Bitswap: A multi-path file transfer protocol in IPFS and Filecoin" (delarocha2021.pdf)

## Numbers / findings
- Majority of IPFS content retrieval takes "at least 4x as long as the equivalent HTTPS request" — the quoted "cost of decentralization."
- Bitswap's discovery mechanism broadcasts WANT messages to ALL connected neighbors ("queries all neighbors for the content") — this leaks interest/privacy AND is the main source of message amplification, distinct from BitTorrent's tracker/DHT-scoped swarm discovery.
- The IPFS "Discover" step in benchmarking papers is measured as adding a fixed ~1s Bitswap timeout floor in experimental setups (methodological artifact of the test harness, but reflects a real default timeout tuning problem).
- Retrieval latency is markedly worse for LOW-POPULARITY content — i.e., IPFS-style broadcast discovery degrades specifically for the long tail, which is the common case for a "commons" of many small, rarely-accessed petals/hexons rather than a few viral objects.
- Protocol Labs' own research framed the fix as "multi-path" retrieval — pulling chunks from multiple peers concurrently — as the main latency lever, analogous to BitTorrent's piece-rarity/multi-peer swarming.

## Relevance to fractalengine
- iroh-blobs uses direct content-addressed fetch (BLAKE3) with a known provider (ticket) rather than DHT-style broadcast discovery — this sidesteps Bitswap's core "ask everyone" amplification problem BUT only if fractalengine has a way to know WHICH peer(s) hold a given hexon blob (i.e., discovery is pushed to the application/registry layer, not solved by the P2P layer itself). This validates the existing hexon-registry / relay-as-federation-seam design.

## Confidence: HIGH for the "4x slower than HTTPS" and "asks all neighbors" claims (multiple peer-reviewed/arXiv sources agree); MEDIUM for the low-popularity-degradation generalization (based on secondary summary, not independently re-derived from primary data in this session)
