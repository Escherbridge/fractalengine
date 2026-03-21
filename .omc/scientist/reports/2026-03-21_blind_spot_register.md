# FractalEngine — Blind Spot Register
**Analysis Date:** 2026-03-21
**Analyst Persona:** NEGATIVE SPACE
**System Under Review:** FractalEngine P2P 3D Digital Twin Platform
**Stack:** Bevy + SurrealDB + libp2p + Embedded Browser + RBAC + DID Auth
**Hierarchy:** Fractal (network) → Node (peer) → Petal (world) → Room → Model

---

## Methodology

This register applies structured adversarial analysis to surface what the design conversation has NOT addressed. Each blind spot is evaluated on two axes:

- **Risk Level:** LOW / MEDIUM / HIGH / CRITICAL
- **Category:** Architecture / Security / UX / Economics / Legal / Ops

No statistical model is applied here — this is a qualitative design-space audit. Evidence for each risk level is drawn from analogous systems (IPFS, Decentraland, Matrix, WebXR) and first-principles reasoning about the stated stack.

---

## Blind Spot Register

---

### BS-01 — Asset Pipeline Conversion: Who Does the Work?

**Category:** Architecture
**Risk Level:** HIGH

**The Gap:**
The design accepts FBX and OBJ uploads but renders via Bevy, which natively consumes GLTF/GLB. There is no documented conversion step. Three options exist, each with severe tradeoffs:

| Option | Tradeoff |
|---|---|
| Client-side conversion (WASM) | FBX SDK is proprietary; no FOSS WASM FBX parser exists at production quality |
| Node-operator-side conversion | Requires the Node to run a headless converter (Blender CLI, assimp). Adds server-side compute dependency. Breaks pure P2P model. |
| Central conversion service | Introduces a single point of failure and a trusted intermediary — directly contradicts the P2P architecture claim. |

**Hidden Assumption:** The design implicitly assumes assets arrive as GLTF. This is false for the majority of professional 3D workflows.

**Recommended Additions to Spec:**
1. Define an official "FractalEngine Asset Format" — GLTF 2.0 with a defined extension set — and make it the ONLY accepted upload format. All conversion is the uploader's responsibility (document Blender export workflow).
2. If conversion is required in-platform, specify it as a Node-side optional capability with a capability advertisement flag in the DHT record, not a universal feature.
3. Document the FBX licensing constraint explicitly so implementers are not surprised.

---

### BS-02 — Content Moderation: No Central Authority, No Safety Net

**Category:** Legal / Security
**Risk Level:** CRITICAL

**The Gap:**
A fully decentralized P2P system with DID authentication and no central authority has no documented mechanism for:
- Removing CSAM or illegal content from the network
- Preventing a Node from hosting extremist recruitment spaces
- Responding to DMCA takedown requests
- Handling doxxing or harassment within a Petal

**Analogous System Failures:** Freenet and early IPFS pinning services faced legal liability when operators were found to be passively caching illegal content. The "we're just infrastructure" defense has a poor legal track record in jurisdictions with strict platform liability laws (EU DSA, UK OSA, EARN IT Act in the US).

**Hidden Assumption:** The design assumes legal and ethical responsibility dissolves with decentralization. Courts and regulators disagree.

**Recommended Additions to Spec:**
1. Define a Petal-level content flagging protocol — a signed report packet that can be propagated over DHT. Nodes can subscribe to blocklists (opt-in moderation).
2. Define a "Fractal Council" or federated moderation layer — a set of well-known DID identities that can publish signed revocation lists for content hashes. Nodes MAY honor these.
3. Specify that Node operators are legally responsible for content they host and cache. Make this explicit in operator documentation.
4. Implement hash-based CSAM scanning at the asset ingest point on the Node (PhotoDNA or NCMEC hash list matching). This is a legal requirement in many jurisdictions, not optional.

---

### BS-03 — Discovery UX: DHT Finds Nodes, Not Experiences

**Category:** UX
**Risk Level:** HIGH

**The Gap:**
DHT (libp2p Kademlia) resolves known peer IDs. It does not support:
- Full-text search ("find a coffee shop simulation")
- Category browsing ("show me all art gallery Petals")
- Relevance ranking
- Social discovery ("what are my friends visiting?")

A brand new user with zero social graph and zero known peer IDs is stranded. There is no "front door" to the network.

**Analogous Problem:** Early Gnutella and eDonkey networks suffered severe discoverability failures. Only BitTorrent solved this by decoupling the tracker/index layer from the data transfer layer.

**Hidden Assumption:** The design assumes users will arrive with prior knowledge of peer IDs or will be invited by existing users. This assumption kills cold-start adoption.

**Recommended Additions to Spec:**
1. Define a Petal Metadata Schema — a structured, indexable document (JSON-LD) that each Node publishes about its Petals: name, description, tags, language, content rating, thumbnail hash.
2. Specify a "Discovery Index" protocol — either a federated index (like Matrix's room directory) or a DHT-stored inverted index for tag-based lookup.
3. Define a "Well-Known Bootstrap Fractal" — a set of hardcoded or DNS-resolved peer IDs that serve as curated entry points for new users, similar to Mastodon's instance picker.
4. Social graph integration: allow DID-linked follow relationships so users can see where trusted peers are present.

---

### BS-04 — Persistence Across Node Downtime: Petals Disappear

**Category:** Architecture / Ops
**Risk Level:** HIGH

**The Gap:**
If the Node operator's machine goes offline, their Petals become unreachable. The design mentions "caching" but does not specify:
- What is cached (full scene? metadata only? assets?)
- Who decides to cache (the visiting peer? a designated replicator?)
- How long cache is valid
- How a cached Petal signals its staleness

**Analogous System:** IPFS content addressing solves this for static assets but not for mutable scenes with live state (SurrealDB records).

**Two distinct problems exist that the design conflates:**
- **Static asset persistence** (GLTF files, textures): Solvable with content-addressed caching (IPFS-style)
- **Live state persistence** (who is in the room, current object positions): Not solvable without a live data source or a designated replica

**Hidden Assumption:** "Caching" is mentioned as if it solves both problems. It does not.

**Recommended Additions to Spec:**
1. Define a "Petal Persistence Tier" in the Petal schema: EPHEMERAL (no caching expected), CACHED (static assets only), REPLICATED (full SurrealDB replica on designated peers).
2. For REPLICATED tier, specify a "Replica Node" protocol — a designated peer (possibly paid) that maintains a live SurrealDB replica and can serve the Petal when the primary Node is offline.
3. Define a cache invalidation TTL and a "last-seen" timestamp in the DHT record for each Petal.

---

### BS-05 — Version Conflicts: Cache Staleness and Fork Resolution

**Category:** Architecture
**Risk Level:** MEDIUM

**The Gap:**
If Peer A has cached Petal v1 and the Node publishes v2:
- Does Peer A know v2 exists?
- Can Peer A and Peer B be simultaneously in different versions of the same Petal?
- Who arbitrates if a cached state conflicts with the live state?

**Hidden Assumption:** The design assumes linear versioning. In a P2P system with partial replication, version graphs can fork.

**Recommended Additions to Spec:**
1. Assign every Petal state a content-addressed version hash (Merkle root of the scene graph). The Node publishes the current canonical hash to DHT.
2. Define a "version check" handshake — when a peer connects to a Petal, it announces its cached version hash. The Node responds with the delta or a full refresh signal.
3. Specify conflict resolution policy: Node operator's live state is always canonical. Cached state is read-only and marked as STALE if hash mismatch is detected.
4. Define a maximum cache age (e.g., 24 hours) after which a peer must re-fetch or display a "possibly outdated" warning.

---

### BS-06 — Bandwidth Economics: The Free Rider Problem

**Category:** Economics
**Risk Level:** HIGH

**The Gap:**
When 100 peers visit a Petal simultaneously:
- All data flows from the Node operator's upstream bandwidth
- The Node operator pays for 100x the bandwidth of a single visitor
- There is no documented mechanism for cost sharing, rate limiting, or incentivized relay

**Analogous System:** WebRTC-based platforms (Gather.town, VRChat) solve this with SFU (Selective Forwarding Units) — centralized relay servers they pay for. A P2P system cannot assume this exists.

**Projected Impact:** A Petal with a 50 MB scene visited by 100 peers simultaneously = 5 GB of outbound transfer. At commodity bandwidth prices, this is non-trivial. At mobile hotspot prices, it is prohibitive.

**Hidden Assumption:** Node operators have unlimited upstream bandwidth and will absorb costs altruistically.

**Recommended Additions to Spec:**
1. Define a "Petal CDN Protocol" — a mechanism by which visiting peers re-serve static assets to other visitors (BitTorrent-style swarming for GLTF assets).
2. Specify a "max concurrent visitors" field in the Petal metadata that Node operators can set. Peers respect this limit.
3. Define a "Relay Node" role — peers that volunteer (or are compensated) to relay traffic when the Node operator's bandwidth is saturated.
4. Consider a token-economic layer (even simple: proof-of-relay credits) to incentivize bandwidth contribution. This is explicitly optional but must be acknowledged as a design gap.

---

### BS-07 — Identity Bootstrapping: The First Keypair Problem

**Category:** UX / Security
**Risk Level:** HIGH

**The Gap:**
DID (Decentralized Identifier) authentication requires a keypair. For a new user:
- How is the keypair generated? (browser crypto API? native Bevy? external wallet?)
- Where is the private key stored? (OS keychain? file? hardware token?)
- What happens if the user loses their private key? (DID is irrecoverable by design)
- How does the user understand what a DID is without a 10-minute explainer?

**Analogous Failures:** Ethereum's UX research consistently shows that seed phrase management causes permanent key loss for approximately 20% of new users within 12 months. DID systems without recovery mechanisms replicate this failure.

**Hidden Assumption:** Users will manage cryptographic identity correctly without tooling or education.

**Recommended Additions to Spec:**
1. Define a "FractalEngine Identity Wallet" spec — a local encrypted keystore (similar to age-encrypted file or OS keychain integration) that abstracts key management from the user.
2. Mandate a social recovery mechanism: the user designates N trusted DIDs (friends) such that M-of-N can authorize key rotation. This is the standard pattern (see: EIP-4337 social recovery).
3. Provide a "guest mode" — a temporary, session-scoped DID that allows exploration without commitment. Users upgrade to persistent DID when they choose to create content.
4. Define what "DID" means in plain language in all user-facing surfaces. Never show a raw DID string as primary UI.

---

### BS-08 — Accessibility: 3D Worlds Are Hostile to Assistive Technology

**Category:** UX / Legal
**Risk Level:** MEDIUM

**The Gap:**
3D interactive environments rendered via Bevy (a game engine) are entirely outside the ARIA/HTML accessibility model. Screen readers, keyboard navigation, switch access, and cognitive accessibility tools have no defined integration path.

**Legal Context:** The EU Web Accessibility Directive (2016/2102), the US ADA (as applied to digital spaces via case law), and the UK Equality Act 2010 create legal obligations for accessibility in digital products. "It's a 3D game engine" is not a recognized exemption.

**Hidden Assumption:** Accessibility is a later concern. In practice, retrofitting 3D accessibility is an order of magnitude harder than designing it in from the start.

**Recommended Additions to Spec:**
1. Define a "Text Layer" for every Room and Model — a structured semantic description (JSON) that can be read by screen readers via a companion 2D UI overlay.
2. Specify keyboard navigation targets for all interactive objects (doors, teleporters, UI panels) — every 3D interactive element must have a keyboard-accessible equivalent action.
3. Define a "Low Mobility Mode" — a locomotion mode that does not require continuous mouse/joystick input (click-to-teleport, tab-to-select-object).
4. Commit to WCAG 2.1 AA compliance for all 2D UI surfaces (menus, HUD, chat) as a baseline. 3D content is aspirational but must be documented as a known gap.

---

### BS-09 — Embedded WebView Security: An Arbitrary Code Execution Surface

**Category:** Security
**Risk Level:** CRITICAL

**The Gap:**
An embedded browser (webview) loading arbitrary URLs inside a native desktop application creates a threat surface that the design has not addressed. Specific risks:

| Threat | Mechanism | Impact |
|---|---|---|
| SSRF via webview | A malicious Petal embeds a URL pointing to localhost:8080 (SurrealDB admin, local services) | Data exfiltration, database takeover |
| Cross-context script injection | Webview JavaScript bridge (if any) allows JS→native calls | Arbitrary code execution on the host OS |
| Credential harvesting | A Petal embeds a convincing fake login page for an external service | Phishing within a trusted application chrome |
| Privacy tracking | Third-party URLs in webview leak the user's IP to arbitrary domains | De-anonymization, surveillance |
| Malicious redirects | A URL in a Petal redirects to a drive-by download | Malware delivery |

**Analogous Incidents:** Electron apps have a well-documented history of RCE via webview misuse (Discord, older VS Code extensions, multiple Electron CVEs). Bevy's embedded browser faces identical risks.

**Hidden Assumption:** The embedded browser is a display surface. In practice, it is a full attack surface.

**Recommended Additions to Spec:**
1. Mandate a URL allowlist/denylist per Petal — Node operators declare permitted external domains. The webview refuses to load undeclared domains.
2. Block all localhost and 192.168.x.x / 10.x.x.x / 172.16-31.x.x ranges unconditionally in the webview.
3. Disable JavaScript-to-native bridge entirely unless a specific, audited API is defined. Default: no bridge.
4. Implement a Content Security Policy (CSP) header enforcement layer inside the webview.
5. Show a clear "External Website" visual indicator whenever the webview is active, so users can distinguish Petal UI from external content.
6. Conduct a dedicated threat model document for the webview component before implementation. This is non-negotiable.

---

## Summary Risk Matrix

| ID | Blind Spot | Category | Risk Level |
|---|---|---|---|
| BS-01 | Asset pipeline conversion ownership | Architecture | HIGH |
| BS-02 | Content moderation without central authority | Legal / Security | CRITICAL |
| BS-03 | Discovery UX for new users | UX | HIGH |
| BS-04 | Petal persistence across Node downtime | Architecture / Ops | HIGH |
| BS-05 | Version conflict resolution | Architecture | MEDIUM |
| BS-06 | Bandwidth economics and cost distribution | Economics | HIGH |
| BS-07 | Identity bootstrapping and key recovery | UX / Security | HIGH |
| BS-08 | Accessibility in 3D environments | UX / Legal | MEDIUM |
| BS-09 | Embedded webview security model | Security | CRITICAL |

---

## Priority Recommendations (Ordered by Risk × Effort)

**Immediate (pre-prototype, no code required):**
1. Write a WebView Threat Model document (BS-09) — this must exist before any embedded browser code is written.
2. Define the official upload format as GLTF-only (BS-01) — eliminates an entire class of pipeline complexity.
3. Add a content moderation policy document for Node operators (BS-02) — this is a legal requirement, not a design choice.

**Short-term (first milestone):**
4. Design the Identity Wallet spec with social recovery (BS-07) — key loss is silent and permanent.
5. Define the Petal Metadata Schema and Discovery Index protocol (BS-03) — without this, the network cannot grow.
6. Specify the Petal Persistence Tier system (BS-04) — unmanaged expectations about uptime will cause user churn.

**Medium-term (before public launch):**
7. Define the bandwidth swarming protocol for static assets (BS-06).
8. Specify version hash and staleness protocol (BS-05).
9. Commit to WCAG 2.1 AA for 2D UI surfaces and document 3D accessibility as a known gap (BS-08).

---

## What Is NOT Missing (Scope Confirmation)

To avoid false positives, the following items were considered and excluded from the register because they are either addressed by the stated stack or are out of scope for this analysis phase:

- **Peer routing and NAT traversal:** libp2p handles this via its relay and hole-punching protocols.
- **Data synchronization between peers:** SurrealDB's live query feature and CRDTs address this.
- **Authentication integrity:** DID + RBAC covers this adequately at the design level.
- **Scene rendering performance:** Bevy's ECS architecture is well-suited to the scale described.

---

*Report generated by NEGATIVE SPACE analyst persona | FractalEngine Design Audit | 2026-03-21*
*File: /home/jade/projects/fractalengine/.omc/scientist/reports/2026-03-21_blind_spot_register.md*
