//! Mixer: the summing and routing layer of the DSP core.
//!
//! # Scope
//!
//! This module owns the *console* structure — channels, buses, sends, effect
//! slots, pan law and metering — and the topology that connects them. It does
//! **not** own the audio device, the transport or the plugin host: those live in
//! `driver/`, `transport/` and the future `plugins/` module respectively. The
//! graph here is deliberately self-contained so the mixer can be built and
//! tested before the real-time engine lands, and so the engine can adopt it by
//! calling [`MixerGraph::process_block`] at a block boundary.
//!
//! # Real-time discipline
//!
//! Everything the audio thread touches is pre-allocated in [`MixerGraph::prepare`]
//! and then never resized: the process path contains no `Vec::push`, no
//! `Box::new`, no `String`, no `Mutex` and no printing (PLAN §0.2, ABI P5).
//! Topology edits (adding channels, changing routing) are **not** real-time
//! safe and are documented as control-thread-only, matching ABI §7.3.
//!
//! # Layout
//!
//! ```text
//! mixer/
//! ├── channel.rs        channel state (gain / pan / mute / solo / phase)
//! ├── strip.rs          channel-strip DSP: gain → pan → effects → sends
//! ├── bus.rs            bus and return-bus routing
//! ├── send.rs          per-channel sends (pre/post fader)
//! ├── effect_chain.rs  the ten effect slots
//! ├── meter.rs         peak + RMS metering with peak hold
//! ├── pan_law.rs       selectable pan laws
//! └── graph.rs         topology → topology validation, cycle rejection
//! ```

pub mod channel;
pub mod pan_law;

pub use channel::{Channel, ChannelId, ChannelRole};
pub use pan_law::PanLaw;

/// Insert channels a project gets by default (PLAN §3.S3 item 1).
pub const DEFAULT_INSERT_CHANNELS: usize = 64;

/// Return buses a project gets by default (PLAN §3.S3 item 1).
pub const DEFAULT_RETURN_BUSES: usize = 8;

/// Maximum gain a fader can apply, in decibels (PLAN §3.S3 item 6).
pub const MAX_GAIN_DB: f32 = 12.0;

/// Minimum gain a fader can apply, in decibels.
///
/// Treated as silence: [`crate::mixer::channel::db_to_gain`] maps it to exactly
/// `0.0`, so a fader pulled fully down is muted rather than merely very quiet.
pub const MIN_GAIN_DB: f32 = -96.0;
