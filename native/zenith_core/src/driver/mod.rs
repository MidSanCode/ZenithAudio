//! Audio drivers: the injectable source of the audio callback (PLAN §3.S1).
//!
//! The engine does not own an audio device. It renders when a driver asks it
//! to, and which driver that is, is a configuration choice:
//!
//! * `CpalDriver` drives the engine from a real device callback on
//!   desktop and mobile. It lives behind the non-default `cpal` feature so the
//!   core builds and tests without pulling in an external crate.
//! * `WorkletDriver` is driven by the web `AudioWorklet` (S1.5).
//! * [`OfflineDriver`] drives the engine in a plain loop, for tests and for
//!   offline export (S4).
//!
//! The trait is what makes "whose clock advances the DSP graph" a dependency
//! rather than a fact baked into the engine. It is also the reason the default
//! build can run every engine test on a machine with no audio hardware.

pub mod offline_driver;

#[cfg(feature = "cpal")]
pub mod cpal_driver;

pub use offline_driver::OfflineDriver;

use crate::engine::Engine;

/// The shape a driver needs to know about the engine it is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverFormat {
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Frames per callback.
    pub block_size: usize,
    /// Output channels.
    pub channels: usize,
}

impl DriverFormat {
    /// Reads the format from an engine's configuration.
    #[must_use]
    pub fn from_engine(engine: &Engine) -> Self {
        let config = engine.config();
        Self {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
            channels: 2,
        }
    }
}

/// A source of audio callbacks.
///
/// A concrete driver owns the device (or the offline loop) and calls
/// [`Engine::render_block`] whenever the platform needs more audio. The engine
/// never blocks the driver and the driver never allocates in its callback.
pub trait AudioDriver {
    /// The sample rate the device negotiated.
    fn sample_rate(&self) -> u32;

    /// Frames the device asks for per callback.
    fn block_size(&self) -> usize;

    /// Output channels.
    fn channels(&self) -> usize;

    /// A stable label for diagnostics and the status snapshot.
    fn label(&self) -> &'static str;
}

/// Which driver the engine should use, mirrored from `ZenithDriverKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    /// Pick the platform default.
    Auto,
    /// A real device via `cpal`; unsupported on `wasm32`.
    Cpal,
    /// Driven by the web `AudioWorklet`.
    Worklet,
    /// Offline rendering, no device.
    Offline,
}

impl DriverKind {
    /// Decodes the ABI integer, rejecting an unknown value rather than
    /// coercing it (ABI §2.2).
    #[must_use]
    pub const fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Auto),
            1 => Some(Self::Cpal),
            2 => Some(Self::Worklet),
            3 => Some(Self::Offline),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_kind_round_trips_and_rejects_unknown() {
        for (value, expected) in [
            (0, DriverKind::Auto),
            (1, DriverKind::Cpal),
            (2, DriverKind::Worklet),
            (3, DriverKind::Offline),
        ] {
            assert_eq!(DriverKind::from_abi(value), Some(expected));
        }
        assert_eq!(DriverKind::from_abi(99), None);
    }

    #[test]
    fn the_format_follows_the_engine_configuration() {
        let engine = Engine::new(crate::engine::EngineConfig::default()).unwrap();
        let format = DriverFormat::from_engine(&engine);
        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.block_size, 256);
        assert_eq!(format.channels, 2);
    }
}
