//! Delay and echo effects.
//!
//! [`sync_delay`] is the tempo-synchronised delay PLAN §3.S5 asks for: its
//! time is expressed in beats and converted through
//! [`crate::effects::buffer::RenderContext::beats_to_samples`], so a project
//! at 90 BPM and the same project at 140 BPM both produce echoes that land on
//! the grid rather than on the wall clock.

pub mod sync_delay;

pub use sync_delay::SyncDelay;
