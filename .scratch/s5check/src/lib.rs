//! Standalone harness mirroring `zenith_core`'s effect tree.
//!
//! It includes by `#[path]` exactly the real source files this session is
//! responsible for, plus their shared dependencies. Nothing is copied: editing
//! the real file changes what this harness compiles.

#![allow(clippy::all, missing_docs, dead_code)]

extern crate alloc;

#[path = "automation_mod.rs"]
pub mod automation;

#[path = "effects_shim.rs"]
pub mod effects;
