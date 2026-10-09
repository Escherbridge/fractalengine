//! Library surface: reusable API-integration harness (see src/AGENTS.md
//! §api-harness) and the in-process P2P peer model (`peer::TestPeer`,
//! §peer-model — exported since F8/M3 so the sim lab (fe-sim) builds
//! `SimPeer` on the same machinery). The P2P scenario runner stays
//! binary-only (`main.rs`).

pub mod api;
pub mod peer;
