//! fe-sim — the full-lab simulation mode (F8/M3, decision D3: full lab
//! simulation, in-process, on the same `VerseReplicator` trait as prod).
//!
//! Nothing on the production path depends on this crate (the only dependent
//! is fractalengine-relay's default-off `sim-control` feature); the one
//! production-code concession it needs lives behind explicit seams:
//!
//! * [`clock::SimClock`] — the accelerable virtual clock every simulated
//!   peer reads. Sensor timestamps, the virtual network's delivery
//!   schedule, and HLC stamping (through
//!   `fe_database::op_log::set_wall_clock_source`, installed via
//!   [`clock::install_hlc_source`]) all derive from it, never from wall
//!   time, so a stepped run is deterministic.
//! * [`net::SimNet`] — the deterministic in-process network hub: scripted
//!   membership, latency, partitions, and churn, with latest-per-key doc
//!   convergence, plus the virtual gossip plane the distributed-query
//!   fan-out rides ([`net::SimGossipTopic`], F9/A21). [`net::SimVerseReplicator`] implements the SAME
//!   `fe_sync::VerseReplicator` + open-lifecycle contract as
//!   `IrohDocsReplicator` (fe-sync `virtual_transport.rs`), so prod and sim
//!   cannot drift by construction; a sim sync thread binds no iroh
//!   endpoint at all (pinned by `fe_sync::bound_endpoint_count`).
//! * [`sensors`] — pure sensor models (sine, weather-like, random-walk):
//!   a reading is a pure function of (model config, tick), never of hidden
//!   state or wall time.
//! * [`fleet`] — the declarative fleet config and its runner: synthetic
//!   fleets ingest real `iot_reading` rows through the F5 emission seam
//!   (`DbCommand::InsertIotReadings` →
//!   `fe_database::handlers::iot_reading::insert_readings_with_replication`),
//!   so sim readings replicate exactly like production ones.
//! * [`peer::SimPeer`] / [`scenario`] — the simulated peer (a harness
//!   `TestPeer` on the virtual transport) and the scripted-scenario runner
//!   (faults at sim times + fleet ticks + settle), producing a canonical
//!   fingerprint for determinism assertions.
//! * [`session::ScenarioSession`] / [`control`] — the long-lived driver
//!   (start / step / inject_fault / status / stop; `run_scenario` is built
//!   on it) and the bridge thread that serves the REST/MCP sim control
//!   surface (F9/A20).
//!
//! See `fe-sim/src/AGENTS.md` for the module rationale.

pub mod clock;
pub mod control;
pub mod fleet;
pub mod net;
pub mod peer;
pub mod scenario;
pub mod sensors;
pub mod session;
