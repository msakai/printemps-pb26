//! Library shared by the `exact-printemps` and `scip-printemps` binaries.
//!
//! Each binary drives a two-phase hybrid: an exact / branch-and-bound style
//! solver (Exact or SCIP) followed by the PRINTEMPS local-search heuristic.
//! Phase-to-phase information is funnelled through the [`handoff::SolverHandoff`]
//! type so future bidirectional or alternating compositions remain feasible.

pub mod exact;
pub mod handoff;
pub mod opb;
pub mod printemps;
#[cfg(feature = "scip")]
pub mod scip;
pub mod signals;
pub mod verify;
