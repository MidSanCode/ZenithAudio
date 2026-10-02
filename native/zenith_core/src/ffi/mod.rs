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
//! * `types.rs` — split by labelled section (S0/S1, S2, S3, S5)
//! * `engine_api.rs` — Agent-A (S1)
//! * `param_api.rs` — Agent-C (S2)
//! * `effect_api.rs` — Agent-C (S5)
//! * `mixer_api.rs` — Agent-D (S3)
//!
//! Adding a file here requires a coordination entry, because `lib.rs` must
//! declare it. The S5 effect surface was registered as entry C-011 in
//! `docs/COORDINATION.md`; the S1 engine surface was registered as C-013.

pub mod effect_api;
pub mod engine_api;
pub mod mixer_api;
pub mod param_api;
pub mod types;
