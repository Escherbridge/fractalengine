//! Canonical Fractal Data Log: deterministic CBOR envelopes, signing, admission, and
//! materialization for Workstream G. Rationale, dependency rules, and the module
//! ownership map live in `src/AGENTS.md`.

pub mod author_key;
pub mod branch;
pub mod capability;
pub mod cbor;
pub mod checkpoint;
pub mod compose;
pub mod crypto;
pub mod did_key;
pub mod envelope;
pub mod frontier;
pub mod kind;
pub mod materialize;
pub mod payload_aad;
pub mod retention;
pub mod segment;
pub mod signing;
pub mod units_codec;
pub mod wire;
