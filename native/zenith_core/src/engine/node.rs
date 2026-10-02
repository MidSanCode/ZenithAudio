//! The engine's DSP node interface.
//!
//! A node is a unit of processing the engine can run in series (the master
//! chain). It is deliberately narrower than [`crate::effects::EffectProcessor`]:
//! a node has no parameters and no per-slot identity, because the rich,
//! parameterised processors live in the effect suite and are addressed through
//! the mixer.
//!
//! # Why a flat planar buffer
//!
//! Nodes receive one contiguous `f32` block laid out channel-major:
//! channel `c` occupies `planar[c * stride .. c * stride + frames]`. A flat
//! slice plus a stride needs no per-channel `Vec`, so a node can be invoked on
//! the audio thread without the engine building a temporary array of slices,
//! and the same signature works for any channel count.
//!
//! # Real-time discipline
//!
//! [`DspNode::prepare`] runs on the control thread and is the only place a node
//! may allocate. [`DspNode::process`] runs on the audio thread and must not
//! allocate, lock or perform IO (ABI principle P5).

use super::render_context::RenderContext;

/// A real-time processing node.
pub trait DspNode: Send {
    /// A short, stable name for diagnostics.
    fn name(&self) -> &'static str;

    /// Allocates every buffer the node will need.
    ///
    /// `channels` is the number of channel regions passed to
    /// [`Self::process`]; `max_block` the largest `frames` it will ever see.
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);

    /// Processes one planar block in place.
    ///
    /// Channel `c` is `planar[c * stride .. c * stride + frames]`. Real-time
    /// safe: no allocation, no locking, no IO.
    fn process(
        &mut self,
        planar: &mut [f32],
        stride: usize,
        channels: usize,
        frames: usize,
        ctx: &RenderContext,
    );

    /// Clears internal state; call on seek.
    fn reset(&mut self);

    /// Latency this node introduces, in samples, for delay compensation.
    fn latency_samples(&self) -> usize {
        0
    }
}
