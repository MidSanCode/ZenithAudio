//! The C ABI surface (ABI principle P1).
//!
//! Everything Dart can call lives here or in a sibling module of this one.
//! Nothing outside `src/ffi/` is `#[no_mangle]`, so the set of exported symbols
//! is auditable by reading one directory.
//!
//! # Ownership
//!
//! This directory is **shared** (`docs/COORDINATION.md`). Each agent owns one
//! submodule:
//!
//! * `types.rs` — split by labelled section (S0/S1, S2, S3)
//! * `param_api.rs` — Agent-C (S2)
//! * `engine_api.rs` — Agent-A (S1)
//! * `graph_api.rs` — Agent-D (S3)
//!
//! Adding a file here requires a coordination entry, because `lib.rs` must
//! declare it.

pub mod param_api;
pub mod types;
