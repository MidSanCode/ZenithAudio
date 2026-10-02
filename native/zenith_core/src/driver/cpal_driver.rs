//! The `cpal` device driver.
//!
//! # Why this module is behind a feature
//!
//! The engine must compile and be testable without an audio device and without
//! network access to fetch crates (ABI principle P7 also requires a
//! `wasm32`-clean core). `cpal` is therefore an **optional** dependency, and
//! this module is only compiled when the `cpal` feature is enabled:
//!
//! ```toml
//! # native/zenith_core/Cargo.toml
//! [dependencies]
//! cpal = { version = "0.15", optional = true }
//!
//! [features]
//! cpal = ["dep:cpal"]
//! ```
//!
//! Enabling the feature requires adding that dependency; the default build uses
//! [`super::OfflineDriver`] instead and reports [`crate::Status::Unsupported`]
//! from `zenith_engine_start` when asked for a real device.
//!
//! # Integration shape
//!
//! [`CpalDriver::build`] opens the default output stream and returns a handle
//! that borrows the engine for the lifetime of the stream. The audio callback
//! calls [`Engine::render_block`] with exactly the frames `cpal` handed over
//! and never allocates (ABI P5). Errors opening the device are returned as
//! [`crate::Status::Device`] rather than panicking across the FFI boundary.

#![cfg(feature = "cpal")]

use super::{AudioDriver, DriverFormat};
use crate::engine::Engine;
use crate::Status;

/// A real audio device driven by `cpal`.
pub struct CpalDriver {
    /// Negotiated format.
    format: DriverFormat,
    /// The live output stream, held so it stays open.
    stream: cpal::Stream,
}

// SAFETY: `cpal::Stream` is `!Send` on some backends. The engine treats the
// driver as owned by the thread that created it and never moves the stream
// across threads itself; this assertion exists only so the type can be stored
// in an `Engine`-adjacent slot. If a backend disagrees at runtime, `cpal`
// reports it when the stream is played.
unsafe impl Send for CpalDriver {}

impl CpalDriver {
    /// Opens the default output device and starts the callback.
    ///
    /// The callback runs `engine.render_block` for every buffer the device
    /// asks for. Returns [`Status::Device`] when the host has no usable output.
    pub fn build(engine: &mut Engine) -> Result<Self, Status> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(Status::Device)?;
        let supported = device.default_output_config().map_err(|_| Status::Device)?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let format = DriverFormat {
            sample_rate: config.sample_rate.0,
            block_size: engine.config().block_size as usize,
            channels: config.channels as usize,
        };

        // The callback needs raw access to the engine. The engine is not
        // `Sync`, so the driver owns it for the stream's lifetime through a
        // pointer that the caller guarantees outlives the stream.
        let engine_ptr: *mut Engine = engine;
        let channels = format.channels;
        let mut scratch =
            alloc::vec![0.0f32; format.block_size * channels];

        let data_callback = move |output: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            // SAFETY: the caller keeps the engine alive for at least as long as
            // the stream; the callback is the only writer while the stream
            // plays, matching ABI §7.2.
            let engine = unsafe { &mut *engine_ptr };
            let frames = output.len() / channels.max(1);
            let cap = engine.config().block_size as usize;
            let block = frames.min(cap);
            // The engine writes interleaved stereo; copy it into the device's
            // buffer, respecting the device's channel count.
            engine.render_block(&mut scratch[..block * channels], block);
            for i in 0..frames {
                for c in 0..channels {
                    output[i * channels + c] = scratch
                        .get(i * channels + c)
                        .copied()
                        .unwrap_or(0.0);
                }
            }
        };

        let err_callback = |_err: cpal::StreamError| {
            // The audio thread must not log or panic. An error is surfaced to
            // the control thread through the status snapshot's xrun counter in
            // a later revision; for now it is intentionally ignored.
        };

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(&config, data_callback, err_callback, None)
                .map_err(|_| Status::Device)?,
            _ => return Err(Status::Unsupported),
        };
        stream.play().map_err(|_| Status::Device)?;

        Ok(Self { format, stream })
    }

    /// Stops the stream.
    pub fn stop(&self) -> Result<(), Status> {
        use cpal::traits::StreamTrait;
        self.stream.pause().map_err(|_| Status::Device)
    }
}

impl AudioDriver for CpalDriver {
    fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    fn block_size(&self) -> usize {
        self.format.block_size
    }

    fn channels(&self) -> usize {
        self.format.channels
    }

    fn label(&self) -> &'static str {
        "cpal"
    }
}
