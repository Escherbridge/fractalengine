# Challenging Tribal Knowledge -- Large Scale Measurement Campaign on Decentralized NAT Traversal

URL: https://arxiv.org/html/2510.27500v1
Fetched: 2026-07-11
Query intent: Topic 2 - libp2p/IPFS hole punching success rate measurement (primary academic source)

## Extracted content (via WebFetch summarization)

### Overall Success Rates

Primary Result: "70%±7.1% for the hole punching stage" across the measurement campaign.

This baseline represents performance in a production IPFS/libp2p network with over 4.4 million
traversal attempts from 85,000+ distinct networks across 167 countries.

### Protocol Comparison (TCP vs QUIC)

The research challenges conventional assumptions about protocol performance:
- TCP Success Rate: ~70%
- QUIC Success Rate: ~70%

The finding refutes "tribal knowledge" that UDP-based protocols inherently outperform TCP. When
transport protocols were unrestricted, approximately 80% of successful connections used QUIC, but
this reflects connection speed/preference rather than success rate differences.

### Efficiency Metrics

First-Attempt Success: 97.6% of successful hole punches succeeded on the initial attempt, with only
2.4% requiring subsequent retries.

### Relay Independence

Success showed minimal dependency on relay characteristics:
- Weak negative correlation between relay RTT and success (failed attempts had slightly higher RTTs)
- Success rate remained largely independent of relay path location
- No discernible impact from relay network positioning

### Sample Composition

- Total traversal attempts: 4.4 million (usable data after filtering)
- Distinct networks: 85,000+
- Client deployment countries: 39
- Remote peer countries: 167
- Data points excluded: ~29% due to prerequisite protocol failures

### Connection Reversal Optimization

Port mapping via UPnP/PMP significantly improved outcomes, showing higher CONNECTION_REVERSED
results when active port mappings existed, validating this fast-path optimization's effectiveness.

### Latency Benefits

Post-successful hole punch, 50% of peers achieved 70% or less of their original relay RTT, with 90%
experiencing reduced latency through direct connectivity.

(Note: captured via WebFetch's summarizing prompt, not a raw HTML dump.)
