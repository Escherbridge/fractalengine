# How NAT traversal works

URL: https://tailscale.com/blog/how-nat-traversal-works
Fetched: 2026-07-11
Query intent: Topic 2 - Tailscale NAT traversal success rate / DERP relay data

## Extracted content (via WebFetch summarization)

### Success Rates & Connection Types

The article states: "If you stopped reading now and implemented just the above, I'd estimate you
could get a direct connection over 90% of the time, and your relays guarantee some connectivity
all the time."

This suggests approximately 90% of peer connections achieve direct paths, with fallback relays
ensuring connectivity for remaining scenarios.

### IPv6 Adoption

The document references: "about 33% IPv6" adoption globally, citing Google's statistics on
internet-wide IPv6 distribution.

### Birthday Paradox Collision Probabilities

The article provides specific success rates for the birthday paradox NAT traversal technique with
256 open ports:
- 174 probes = 50% success chance
- 256 probes = 64% success chance
- 1024 probes = 98% success chance
- 2048 probes = 99.9% success chance

For dual hard NATs, achieving 99.9% success requires approximately 170,000 total probes across
both sides.

### Other Specific Numbers

- Standard UDP firewall timeout: "30 seconds"
- Router session limits mentioned (example): "64,000 active sessions" (Juniper SRX 300)
- Probing rate discussed: "100 packets/sec"

Note: The document contains no figures on DERP relay bandwidth, actual connection setup latency
measurements, or real-world deployment statistics in this particular post (see nat-traversal
part 1/3 series for more operational stats).

(Note: captured via WebFetch's summarizing prompt, not a raw HTML dump.)
