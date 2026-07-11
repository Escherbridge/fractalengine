# Source: Two Trautwein et al. NAT traversal measurement papers (arXiv)

Fetch date: 2026-07-11
Query intent: published hole-punching success rates at scale, DCUtR/libp2p, contradiction check vs iroh's own claims

## Paper 1
URL: https://arxiv.org/abs/2604.12484 (also pdf: https://arxiv.org/pdf/2604.12484)
Title: "Large-Scale Measurement of NAT Traversal for the Decentralized Web: A Case Study of DCUtR in IPFS"
Authors: Dennis Trautwein, Cornelius Ihle, Moritz Schubotz, Corinna Breitinger, Bela Gipp
Published: April 15, 2026; to appear ACM IMC '26 (Oct 12-16, 2026, Karlsruhe)

Key numbers:
- Scale: 4.4 million+ traversal attempts, 85,000+ distinct networks, 167 countries
- Conditional hole-punch success rate: 70% ± 7.1% (conditional on relay reservation + public address discovery both succeeding)
- TCP and QUIC: statistically equivalent, both ~70% — the paper explicitly says UDP has NO inherent advantage over TCP/QUIC when high-precision RTT-based synchronization is used (challenges "tribal knowledge" that UDP hole-punches better)
- 97.6% of successful connections happen on the FIRST attempt (i.e., retries add little)

## Paper 2 (companion/preprint)
URL: https://arxiv.org/pdf/2510.27500
Title: "Challenging Tribal Knowledge -- Large Scale Measurement Campaign on Decentralized NAT Traversal"
Authors: Dennis Trautwein, Cornelius Ihle, Moritz Schubotz, Bela Gipp
Published: November 3, 2025 (earlier preprint of the same research program)
Scope: libp2p/IPFS DCUtR, 25 pages, CC BY 4.0. Full numeric breakdown not extracted (PDF binary parse failed) but keywords/abstract consistent with Paper 1.

## Contradiction flagged (IMPORTANT)
- These papers measure **unconditional real-world libp2p DCUtR success at ~70%** across a huge, geographically diverse sample.
- iroh's own blog (comparing-iroh-and-libp2p) and several secondary sources (byteiota, TechTimes-style aggregator posts) claim iroh itself achieves "~90-95% hole-punch success" — but cite the SAME 70% figure as "libp2p's" number for the comparison, and do NOT cite an equivalent large-scale independent study for iroh's own number.
- No independent (non-n0) large-scale measurement of iroh hole-punch success rate was found. The 90-95% figures trace back to n0/iroh's own blog posts and pricing/marketing-adjacent pages, not a peer-reviewed or third-party measurement campaign comparable in scale to the Trautwein et al. work.
- **Confidence on iroh's own hole-punch % : LOW** (vendor-reported, no methodology disclosed, small-N implied by "perf.iroh.com" self-monitoring)
- **Confidence on libp2p DCUtR ~70% : HIGH** (peer-reviewed-track, 4.4M attempts, 85k networks, methodology disclosed)
