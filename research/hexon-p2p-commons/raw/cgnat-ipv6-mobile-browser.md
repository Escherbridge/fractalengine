# Source: CGNAT/IPv6 trend pieces, mobile P2P, iroh browser/WASM status

Fetch date: 2026-07-11
Query intent: NAT reality on the ground, mobile battery/background constraints, browser P2P ceiling

## CGNAT / IPv6 (APNIC blog, jazenetworks, coronium.io, brandergroup)
- Projections (as cited in 2025-dated secondary sources): global IPv6 adoption ~50-60% by 2025, mobile networks 95%+, major ISPs 70%+, enterprise 35-40%. Caveat: figures are projections/aggregated from marketing-adjacent sites, not a single authoritative measurement (e.g., not Google's IPv6 stats page or APNIC Labs' own measurement directly quoted).
- Carriers increasingly run IPv6-only mobile cores with 464XLAT translation to keep IPv4-only destinations reachable — meaning "IPv6 traffic flows natively, no carrier NAT in the path" for mobile in many markets, which is a MEANINGFUL improvement for P2P reachability from phones specifically (mobile NAT reachability may now be BETTER than residential broadband CGNAT in some regions).
- A Cornell study on NAT64 (cited via brandergroup piece) found NAT64 paths are 23.13% longer (route length) and have 17.47% higher RTT than native IPv4 paths — a concrete latency tax where NAT64/464XLAT is in the path.
- Overall theme across sources: IPv6 deployment is uneven and even where deployed often retains "IPv4-era" operational practices (i.e., don't assume IPv6 = NAT-free in practice even when the network claims IPv6 support).

## Mobile P2P (patent filings + Iroh/libp2p comparison pieces — LOW-value sources, no real measurement)
- Only concrete claim found: direct P2P uses less battery than routing through service-provider infrastructure/relay (intuitive, sourced from old patent filings, not a modern measurement).
- No published data found on iOS/Android background execution limits specifically killing long-running iroh/libp2p connections, or on real battery-drain measurements for a QUIC-based P2P stack running in the background. This remains an open gap (see p2p-mycelium §8 unknown #5 — NOT closed by this research pass).
- Platform-specific alternatives exist for LAN-local P2P (Multipeer Connectivity on iOS, Nearby Connections on Android) but these are NOT iroh/QUIC-compatible — they're separate proprietary proximity protocols, not usable as-is for fractalengine's iroh-based architecture.

## Browser / WASM (docs.iroh.computer/deployment/wasm-browser-support, iroh blog "Iroh & the Web", GitHub #2671/#2799)
- Iroh compiles to WASM (since ~v0.32-0.33) and runs in browsers, but CANNOT do real hole-punching there: "Browser sandboxes don't support sending UDP packets to IP addresses from inside the browser."
- Effective browser mode = "relay only": traffic is E2E encrypted but always relayed via WebSocket to an iroh relay, never direct.
- Future direction (not yet shipped as of the fetched sources): using WebTransport with serverCertificateHashes or WebRTC to attempt direct browser connections.
- WebTransport reached "Baseline" (works in Chrome/Firefox/Edge/Safari) as of Safari 26.4 (~March 2026) — the browser-API precondition for a future non-relay-only browser mode now exists industry-wide, but iroh has not yet built on it per the sources fetched.

## Confidence: MEDIUM-HIGH for CGNAT/IPv6 aggregate trend claims (multiple consistent secondary sources, no single authoritative primary dataset found this pass); HIGH for iroh browser "relay-only" limitation (official iroh docs, unambiguous); LOW for mobile battery specifics (no real measurement data found — gap remains open).
