//! Bounded quarantine, GC leases, tombstone non-resurrection, crypto-shredding (SPEC-5 §4-§5);
//! see `src/AGENTS.md` §module-ownership and `src/retention/AGENTS.md` for this module's notes.

pub mod crypto_shred;
pub mod leases;
pub mod quarantine;
pub mod tombstone;
