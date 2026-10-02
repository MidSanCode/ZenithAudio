//! The instrument voices (PLAN §3.S1 `voice/`).
//!
//! A voice is one sounding note. [`SynthVoice`] is the built-in subtractive
//! instrument: an oscillator through an envelope and a state-variable filter.
//! [`SamplerVoice`] plays a preloaded sample buffer with pitch resampling.
//! [`VoiceAllocator`] owns the fixed pool the engine rents voices from.
//!
//! # Real-time discipline
//!
//! Every buffer a voice needs is allocated when the pool is created. The audio
//! path (`VoiceAllocator::render`, `SynthVoice::next_sample`) allocates nothing
//! and never locks.

pub mod sampler;
pub mod synth_voice;
pub mod voice_allocator;

pub use sampler::SamplerVoice;
pub use synth_voice::{AdsrSettings, Oscillator, SynthVoice};
pub use voice_allocator::VoiceAllocator;
