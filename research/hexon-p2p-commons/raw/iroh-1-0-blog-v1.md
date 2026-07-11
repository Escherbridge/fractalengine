# Source: Iroh 1.0 Blog Post "Dial Keys, Not IPs"

URL: https://www.iroh.computer/blog/v1
Fetch date: 2026-07-11
Query intent: iroh 1.0 release status, adoption numbers, roadmap for docs/gossip/CRDT

## Extracted content

- Iroh 1.0 shipped June 15, 2026, after 65 pre-release versions over 4+ years development.
- Adoption: "public relays we run have seen more than 200 million endpoints created, in the last 30 days alone" (self-reported by n0, not independently verified — this counts endpoint creation, not concurrent active peers).
- Direct-connection data ratio: "It's normal to see 95% of data transferred in a connection pass directly between devices" (i.e., only ~5% of *bytes* go over relay, once a connection is established — distinct from hole-punch *success rate*).
- Wire protocol stability commitment: any breaking change to wire protocol will require a major version bump.
- Technical foundation: QUIC multipath routing, NAT traversal (draft-seemann IETF draft), local-first/offline discovery.
- New language bindings: Python, Node.js, Kotlin, Swift (in addition to Rust).
- Public relay support schedule: v1.0 relays supported until EOL; v0.35x relays supported until Dec 31, 2026; v0.9x/1.0.0-rcX until Sep 30, 2026.
- No mention of iroh-docs, iroh-gossip, or CRDT/sync roadmap in this post — sync protocols are being spun into separate repos/versions per the "Road to 1.0" post, decoupled from the core 1.0 announcement.

## Confidence: HIGH for adoption/version facts (official announcement); MEDIUM for interpreting "95% direct" as a success-rate proxy (it's a byte-ratio, not a connection-attempt success rate — see contradiction note in findings doc)
