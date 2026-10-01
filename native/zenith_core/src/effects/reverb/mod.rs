//! Reverb: algorithmic and convolution.
//!
//! Two processors that answer the same musical question in opposite ways.
//!
//! * [`algorithmic`] *generates* a decaying, diffusing tail from a feedback
//!   delay network. It costs a fixed, small amount of CPU per sample, its
//!   decay time is a parameter the user can sweep, and it never needs a file.
//! * [`convolution`] *reproduces* a measured space by convolving the input with
//!   a stored impulse response. It cannot be swept (the room is the room) but
//!   it is the only way to sound like a specific hall, plate or spring.
//!
//! # Why both
//!
//! An algorithmic tail is always slightly synthetic and a convolved tail is
//! always the same. A session normally wants the first while tracking, because
//! a longer decay is a knob turn rather than a different IR, and the second
//! while mixing, when the character is the point. Publishing only one of them
//! would make the other impossible to reach from a project file.
//!
//! # Shared real-time discipline
//!
//! Both effects size every delay line and every FFT scratch region in
//! `prepare` and allocate nothing in `process` (see
//! [`crate::effects::EffectProcessor`]). A reverb is the effect most likely to
//! break that rule, because its natural implementation — "keep the last N
//! seconds of audio" — is a growing buffer if it is written carelessly.

pub mod algorithmic;
pub mod convolution;
