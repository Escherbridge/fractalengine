# Specification: Mycelium Live — Peer Discovery & Node Browsing

## Overview

Wire the existing `build_swarm()` and Kademlia DHT infrastructure into the live network thread, extend the command/event protocol to support peer connections and petal announcements, and deliver a functional Peer Manager and Petal Browser UI so that operators can discover, connect to, and browse other Nodes in the Fractal.

## Background

The foundational networking pieces exist but are disconnected:

- `fe-network/src/swarm.rs` defines `FractalBehaviour` (Kademlia over QUIC) and `build_swarm()`, but the swarm is never instantiated inside `spawn_network_thread`.
- `fe-network/src/lib.rs` only handles `Ping`/`Shutdown` commands — a TODO marks Sprint 3B for swarm wiring.
- `fe-network/src/discovery.rs` has `start_discovery()` calling `kademlia.bootstrap()` but it is unreachable.
- `fe-runtime/src/messages.rs` defines `NetworkCommand`/`NetworkEvent` with only basic variants.
- `fe-ui/src/panels.rs` has a stub `peer_manager_panel` showing placeholder text.
- `fe-identity/src/keypair.rs` provides `NodeKeypair` with ed25519 but no bridge to libp2p identity.

This track closes the gap between "networking code exists" and "two Nodes can find and inspect each other."

## Functional Requirements

### FR-1: Swarm Initialization in Network Thread
**Description:** `spawn_network_thread` must call `build_swarm()` with a libp2p identity derived from the Node's `NodeKeypair`, start listening on a QUIC address, and enter a concurrent event loop that processes both crossbeam commands and swarm events.
**Priority:** P0
**Acceptance Criteria:**
- The swarm starts listening on `/ip4/0.0.0.0/udp/0/quic-v1` (OS-assigned port) by default.
- `NetworkEvent::Started` includes the actual listen address and local `PeerId`.
- The event loop uses `tokio::select!` over `swarm.next()` and `rx.recv_async()` (or polled channel).
- Shutdown command gracefully stops the swarm before thread exit.

### FR-2: NodeKeypair to libp2p Identity Bridge
**Description:** Convert a `NodeKeypair` (ed25519-dalek) into a `libp2p::identity::Keypair` for swarm construction.
**Priority:** P0
**Acceptance Criteria:**
- Conversion uses the raw 32-byte seed from `NodeKeypair::seed_bytes()` to construct a `libp2p::identity::Keypair::ed25519_from_bytes()`.
- Round-trip test: generate `NodeKeypair`, convert to libp2p identity, verify the libp2p `PeerId` matches the expected did:key derivation.
- No private key material logged or exposed beyond the conversion function.

### FR-3: Extended NetworkCommand Variants
**Description:** Add new command variants to support peer lifecycle and petal operations.
**Priority:** P0
**Acceptance Criteria:**
- `Connect { multiaddr: String }` — dial a remote peer by multiaddress.
- `ListPeers` — request the current set of connected peers.
- `DiscoverPetals` — trigger Kademlia lookup for petal records.
- `BroadcastPetalAnnouncement { petal_id: String, name: String }` — publish a Kademlia DHT record announcing a hosted petal.
- All variants implement `Debug + Clone`.
- Existing `Ping`/`Shutdown` behaviour unchanged.

### FR-4: Extended NetworkEvent Variants
**Description:** Add event variants emitted by the network thread in response to swarm activity.
**Priority:** P0
**Acceptance Criteria:**
- `PeerConnected { peer_id: String }` — emitted when a new peer connection is established.
- `PeerDisconnected { peer_id: String }` — emitted when a peer disconnects.
- `PeerList { peers: Vec<PeerInfo> }` — response to `ListPeers`.
- `PetalAnnounced { peer_id: String, petal_id: String, name: String }` — emitted when a petal announcement is discovered via DHT.
- `ListenAddress { address: String, peer_id: String }` — emitted after swarm starts listening.
- All variants implement `Debug + Clone + Message` (Bevy).

### FR-5: PeerInfo Data Structure
**Description:** A shared struct representing a connected peer's metadata.
**Priority:** P1
**Acceptance Criteria:**
- Fields: `peer_id: String`, `multiaddrs: Vec<String>`, `petals: Vec<PetalSummary>`.
- `PetalSummary` fields: `petal_id: String`, `name: String`.
- Both structs derive `Debug, Clone, Serialize, Deserialize`.
- Defined in `fe-network/src/types.rs`.

### FR-6: Petal Announcement via Kademlia DHT
**Description:** Nodes publish their hosted petals as Kademlia provider records so other peers can discover them.
**Priority:** P1
**Acceptance Criteria:**
- On `BroadcastPetalAnnouncement`, the handler serializes `PetalSummary` and calls `kademlia.put_record()`.
- On `DiscoverPetals`, the handler calls `kademlia.get_record()` with the well-known petal registry key.
- Retrieved records are deserialized and emitted as `PetalAnnounced` events.
- All DHT records carry the publishing peer's `PeerId` for attribution.

### FR-7: Peer Manager Panel (Live)
**Description:** Replace the stub `peer_manager_panel` with a live panel driven by `NetworkEvent` data.
**Priority:** P1
**Acceptance Criteria:**
- Displays a scrollable list of connected peers with their `PeerId` (truncated) and address count.
- Shows a "Connect" section with a text input for multiaddress and a "Connect" button.
- "Connect" button sends `NetworkCommand::Connect` through the channel.
- "Refresh" button sends `NetworkCommand::ListPeers`.
- Peer count shown in `node_status_panel` updates from actual data.
- UI state managed via Bevy `Resource` holding `Vec<PeerInfo>`.

### FR-8: Petal Browser Panel
**Description:** A new UI panel listing petals discovered from connected peers.
**Priority:** P2
**Acceptance Criteria:**
- Scrollable list grouped by peer, showing petal name and petal_id.
- Each petal entry shows a "Visit" button (placeholder action for now — logs intent).
- "Discover" button sends `NetworkCommand::DiscoverPetals`.
- Panel added to `gardener_console()` layout.

## Non-Functional Requirements

### NFR-1: Performance
- Swarm event loop must not block the crossbeam command channel for more than 10ms.
- Peer list responses must be served within 50ms for up to 100 peers.
- UI panels must not cause frame drops (render in under 1ms per panel).

### NFR-2: Security
- No private key material logged at any level (even `trace`).
- Multiaddress input must be validated before dialing (reject malformed addresses).
- All gossip payloads remain signed per existing `verify_gossip_message` requirement.

### NFR-3: Thread Safety
- Swarm runs exclusively on the network thread (T2). No swarm handles cross thread boundaries.
- Bevy systems communicate with the network thread only via typed crossbeam channels.
- No `block_on()` in Bevy systems.

### NFR-4: Testability
- All swarm event handling logic is testable without a live network.
- Mock peers using in-process channels per workflow.md.
- Target >80% coverage for new code.

## User Stories

### US-1: Operator Discovers Peers
**As** a Node operator,
**I want** my Node to automatically discover other Nodes on the network via Kademlia DHT,
**So that** I can see who else is part of the Fractal.

**Given** the Node is running and the swarm is initialized,
**When** the Kademlia bootstrap completes and peers are found,
**Then** `PeerConnected` events are emitted and the Peer Manager panel populates.

### US-2: Operator Manually Connects
**As** a Node operator,
**I want** to manually connect to a specific peer by entering their multiaddress,
**So that** I can establish a direct connection to a known Node.

**Given** I have a valid multiaddress for another Node,
**When** I paste it into the Peer Manager connect field and click "Connect",
**Then** the network thread dials the address and a `PeerConnected` event appears if successful.

### US-3: Operator Browses Remote Petals
**As** a Node operator,
**I want** to see which petals are hosted by connected peers,
**So that** I can decide which spaces to visit.

**Given** I am connected to one or more peers that have published petal announcements,
**When** I open the Petal Browser and click "Discover",
**Then** I see a list of petals grouped by peer with names and IDs.

### US-4: Operator Announces Own Petals
**As** a Node operator,
**I want** my Node to announce its hosted petals to the network,
**So that** other operators can discover and visit my spaces.

**Given** I have created petals on my Node,
**When** I broadcast a petal announcement,
**Then** the petal metadata is published as a Kademlia DHT record discoverable by other peers.

## Technical Considerations

- **Identity bridge**: `NodeKeypair::seed_bytes()` returns the raw ed25519 seed. libp2p's `Keypair::ed25519_from_bytes()` accepts a 32-byte seed. This is a direct mapping, no format conversion needed.
- **Event loop architecture**: Use `tokio::select!` with `swarm.select_next_some()` and a channel receiver that implements `Future`. Since `crossbeam::Receiver` is not async, wrap it with `tokio::task::spawn_blocking` or use `tokio::sync::mpsc` internally within the network thread while keeping crossbeam at the boundary.
- **Channel adaptation**: Consider replacing the internal crossbeam usage within the network thread with `tokio::sync::mpsc` for async compatibility, while keeping the crossbeam channel at the Bevy-to-thread boundary.
- **Bevy Resource pattern**: `PeerRegistry` as a Bevy `Resource` holding `Vec<PeerInfo>` and updated by a system that drains `NetworkEvent` from the channel each frame.
- **Kademlia record keys**: Use a well-known prefix `/fractal/petals/<peer_id>` for petal announcement records.
- **mDNS**: Not included in this track. LAN discovery is a future enhancement (mDNS requires adding `libp2p::mdns` to `FractalBehaviour`).

## Out of Scope

- mDNS LAN discovery (future track)
- NAT traversal / relay configuration (separate track)
- Petal data sync / replication (covered by Fractal Mesh track)
- Actually joining/entering a remote petal (requires world loading, auth handshake — separate tracks)
- Gossip-based real-time events (iroh-gossip integration is a separate track)
- Voice/video communication
- Asset transfer between peers

## Open Questions

1. **Bootstrap peers**: Should we hardcode a set of bootstrap multiaddresses for initial DHT seeding, or rely entirely on manual `Connect` for the first connection? Recommend: configurable list in a `network.toml` config file.
2. **Listen port**: Should the QUIC listen port be configurable or always OS-assigned? Recommend: configurable with `0` as default (OS-assigned).
3. **Peer limit**: Should there be a maximum number of simultaneous peer connections? Recommend: configurable, default 50.
4. **Petal announcement TTL**: How long should Kademlia records persist before re-announcement is needed? Recommend: align with Kademlia default record TTL (36 hours) with periodic re-publication.
