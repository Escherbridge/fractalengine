# Implementation Plan: Mycelium Live — Peer Discovery & Node Browsing

## Overview

This track wires the existing libp2p swarm into the live network thread, extends the command/event protocol for peer management and petal announcements, and delivers interactive Peer Manager and Petal Browser UI panels. Work is divided into four phases: identity bridge and message types, swarm integration, DHT-based petal announcements, and UI panels.

**Estimated total effort:** 8-12 hours across 4 phases.

---

## Phase 1: Identity Bridge & Message Types

Goal: Establish the foundational types and the NodeKeypair-to-libp2p conversion so all subsequent phases have stable interfaces.

Tasks:

- [ ] Task 1.1: Add `PeerInfo` and `PetalSummary` structs to `fe-network/src/types.rs` (TDD: Write tests for serialization round-trip and `Debug`/`Clone` derives, then implement structs with `Serialize`/`Deserialize`)

- [ ] Task 1.2: Extend `NetworkCommand` with new variants `Connect { multiaddr: String }`, `ListPeers`, `DiscoverPetals`, `BroadcastPetalAnnouncement { petal_id: String, name: String }` in `fe-runtime/src/messages.rs` (TDD: Write tests for `Debug`/`Clone` on new variants and crossbeam channel round-trip, then add variants)

- [ ] Task 1.3: Extend `NetworkEvent` with new variants `PeerConnected { peer_id: String }`, `PeerDisconnected { peer_id: String }`, `PeerList { peers: Vec<PeerInfo> }`, `PetalAnnounced { peer_id: String, petal_id: String, name: String }`, `ListenAddress { address: String, peer_id: String }` in `fe-runtime/src/messages.rs` (TDD: Write tests for `Debug`/`Clone`/`Message` on new variants and channel round-trip, then add variants. Note: `PeerInfo` import from `fe-network` may require adding the dependency or duplicating the struct — prefer re-export.)

- [ ] Task 1.4: Add `to_libp2p_keypair(&self) -> libp2p::identity::Keypair` method to `NodeKeypair` in `fe-identity/src/keypair.rs` (TDD: Write test that generates `NodeKeypair`, converts to libp2p keypair, verifies `PeerId` is deterministic across conversions and that the libp2p keypair can sign/verify. Then implement using `seed_bytes()` and `libp2p::identity::Keypair::ed25519_from_bytes()`. Add `libp2p` dependency to `fe-identity/Cargo.toml`.)

- [ ] Verification: Run `cargo test -p fe-network -p fe-runtime -p fe-identity`, `cargo clippy -- -D warnings`, `cargo fmt --check` [checkpoint marker]

---

## Phase 2: Swarm Integration into Network Thread

Goal: Replace the stub event loop in `spawn_network_thread` with a live swarm that processes both commands and swarm events concurrently.

Tasks:

- [ ] Task 2.1: Refactor `spawn_network_thread` signature to accept `NodeKeypair` (or the libp2p `Keypair` derived from it) as a parameter, so the swarm can be initialized with the node's identity (TDD: Write test that spawns the network thread with a keypair and receives `Started` + `ListenAddress` events, then modify function signature and initialization)

- [ ] Task 2.2: Initialize `SwarmHandle` via `build_swarm()` inside `spawn_network_thread` and start listening on `/ip4/0.0.0.0/udp/0/quic-v1` (TDD: Write test that spawns thread, asserts `ListenAddress` event is received with a non-empty address string, then implement swarm init and listen call)

- [ ] Task 2.3: Implement async event loop using `tokio::select!` over `swarm.select_next_some()` and an async command receiver (TDD: Write test that sends `Ping` and `Shutdown` commands and verifies correct responses still work alongside swarm polling. Consider using `tokio::sync::mpsc` internally bridged from crossbeam. Then implement the select loop.)

- [ ] Task 2.4: Handle `Connect { multiaddr }` command — parse multiaddr, dial the peer, emit `PeerConnected` on success or log error (TDD: Write test with an invalid multiaddr asserting no crash and a valid loopback scenario if feasible. Then implement `Multiaddr::from_str()` parsing and `swarm.dial()` call.)

- [ ] Task 2.5: Handle swarm `ConnectionEstablished` and `ConnectionClosed` events — emit `PeerConnected` / `PeerDisconnected` (TDD: Write test using mock swarm events or by connecting two in-process swarms and verifying events on the channel. Then implement match arms in the event loop.)

- [ ] Task 2.6: Handle `ListPeers` command — gather connected peer info from `swarm.connected_peers()` and emit `PeerList` (TDD: Write test that sends `ListPeers` and receives `PeerList` with empty vec when no peers connected. Then implement.)

- [ ] Task 2.7: Handle `Shutdown` — call `swarm.close()` or drop the swarm, emit `Stopped`, exit the loop cleanly (TDD: Write test that sends `Shutdown` and asserts `Stopped` is received and thread joins. Then implement graceful shutdown.)

- [ ] Verification: Run `cargo test -p fe-network`, verify swarm starts and responds to all commands. Manual test: run the binary and observe "Network thread started" + listen address in logs. [checkpoint marker]

---

## Phase 3: Petal Announcement via Kademlia DHT

Goal: Enable nodes to publish and discover petal metadata through Kademlia DHT records.

Tasks:

- [ ] Task 3.1: Define Kademlia record key convention — implement helper functions `petal_record_key(peer_id: &str) -> kad::RecordKey` and `parse_petal_record(record: &kad::Record) -> Option<Vec<PetalSummary>>` in a new file `fe-network/src/petal_registry.rs` (TDD: Write tests for key formatting and record serialization/deserialization round-trip. Then implement.)

- [ ] Task 3.2: Handle `BroadcastPetalAnnouncement` command — serialize `PetalSummary`, call `kademlia.put_record()` with the constructed key (TDD: Write test that sends the command and verifies no errors. Test record serialization matches expected format. Then implement in the event loop.)

- [ ] Task 3.3: Handle `DiscoverPetals` command — call `kademlia.get_record()` with the petal registry key prefix, process `KademliaEvent::OutboundQueryProgressed` results and emit `PetalAnnounced` events (TDD: Write test verifying the command triggers a Kademlia query. Then implement the query dispatch and result handling in the swarm event match.)

- [ ] Task 3.4: Wire `start_discovery()` into swarm initialization — call `kademlia.bootstrap()` after the swarm starts listening, handle bootstrap results (TDD: Write test that verifies bootstrap is called during init without error. Then add the call to the init sequence in `spawn_network_thread`.)

- [ ] Verification: Run `cargo test -p fe-network`, verify petal announcement and discovery logic. Two-node manual test if possible: start two instances, announce a petal on one, discover on the other. [checkpoint marker]

---

## Phase 4: UI Panels — Peer Manager & Petal Browser

Goal: Deliver live egui panels that display peer and petal data from the network thread and allow operator interaction.

Tasks:

- [ ] Task 4.1: Create `PeerRegistry` Bevy `Resource` in `fe-ui/src/state.rs` (or equivalent) holding `Vec<PeerInfo>`, local `PeerId`, listen address, and connection count (TDD: Write test for `PeerRegistry::default()`, adding/removing peers, and lookup by peer_id. Then implement.)

- [ ] Task 4.2: Create `network_event_system` Bevy system that drains `NetworkEvent` from the channel receiver each frame and updates `PeerRegistry` (TDD: Write test with a mock channel that sends `PeerConnected`, `PeerDisconnected`, `PeerList`, `PetalAnnounced` events and assert `PeerRegistry` state is correct after system runs. Then implement.)

- [ ] Task 4.3: Rewrite `peer_manager_panel` to read from `PeerRegistry` — show scrollable peer list with truncated PeerId and address count, connect input field, refresh button (TDD: Write test that constructs a `PeerRegistry` with sample data and verifies the panel function does not panic when called with an egui test context. Then implement the UI.)

- [ ] Task 4.4: Update `node_status_panel` to show actual peer count from `PeerRegistry` and the local listen address (TDD: Write test with `PeerRegistry` containing known count, verify label text updates. Then modify the panel.)

- [ ] Task 4.5: Create `petal_browser_panel` function — show petals grouped by peer with "Visit" button (logs intent) and "Discover" button (sends `DiscoverPetals`) (TDD: Write test with sample `PeerRegistry` data containing petals, verify no panic. Then implement and add to `gardener_console()` call list.)

- [ ] Task 4.6: Wire UI command buttons — "Connect" button sends `NetworkCommand::Connect`, "Refresh" sends `ListPeers`, "Discover" sends `DiscoverPetals` through the command channel (TDD: Write test that simulates button clicks via egui test harness and asserts commands appear on the channel. Then implement.)

- [ ] Verification: Run `cargo test -p fe-ui`, `cargo test --workspace`, `cargo clippy -- -D warnings`. Manual test: launch the app, observe Peer Manager panel with connect UI, Petal Browser panel with discover button. [checkpoint marker]
