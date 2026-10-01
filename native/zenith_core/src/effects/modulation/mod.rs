//! Modulation effects: chorus, flanger and phaser.
//!
//! The three share one idea -- an LFO sweeping something -- but differ in what
//! it sweeps and therefore in what they sound like:
//!
//! * [`chorus`] sweeps a *long* delay (5-40 ms) with little or no feedback, so
//!   the result is a detuned copy of the input rather than a comb.
//! * [`flanger`] sweeps a *short* delay (0.5-10 ms) with heavy feedback, so the
//!   moving comb notches are the effect.
//! * [`phaser`] sweeps the break frequencies of a cascade of first-order
//!   all-pass sections -- no delay line at all -- so it produces a handful of
//!   moving notches instead of the harmonic series a comb gives.
//!
//! They are deliberately separate modules rather than one parameterised
//! processor: a chorus's stability guard and a flanger's are different, and
//! merging them would mean one set of compromises for two different sounds.

pub mod chorus;
pub mod flanger;
pub mod phaser;

pub use chorus::Chorus;
pub use flanger::Flanger;
pub use phaser::Phaser;
