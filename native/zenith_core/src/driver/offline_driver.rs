//! The offline driver: a plain loop that advances the engine's clock.
//!
//! This is the default driver and the one every engine test uses. It has no
//! device, so it runs on any machine (and on `wasm32`), and it is deterministic:
//! the same project renders the same samples every time, which is exactly what
//! S4's "real-time and offline agree sample for sample" requirement needs.
//!
//! # Why this is not a second DSP path
//!
//! The offline driver calls the *same* [`Engine::render_block`] the device
//! driver would. There is one DSP implementation; only the clock differs
//! (PLAN §3.S4 item 5).
//!
//! # Real-time discipline
//!
//! [`OfflineDriver::render_into`] allocates nothing once the driver is built;
//! the output vector it fills belongs to the caller.

use super::{AudioDriver, DriverFormat};
use crate::engine::Engine;

/// Drives an engine from an explicit render loop.
pub struct OfflineDriver {
    /// Negotiated format.
    format: DriverFormat,
    /// How many frames have been rendered.
    rendered_frames: u64,
}

impl OfflineDriver {
    /// Creates an offline driver matching `engine`'s configuration.
    #[must_use]
    pub fn new(engine: &Engine) -> Self {
        Self {
            format: DriverFormat::from_engine(engine),
            rendered_frames: 0,
        }
    }

    /// Frames rendered so far.
    #[must_use]
    pub const fn rendered_frames(&self) -> u64 {
        self.rendered_frames
    }

    /// Renders `blocks` blocks into `out`, which must hold at least
    /// `blocks * block_size * channels` samples.
    ///
    /// Returns the number of frames actually rendered. `out` is filled
    /// progressively; the engine clamps each block to what it was given, so a
    /// short `out` is a hard stop rather than an out-of-bounds write.
    pub fn render_into(&mut self, engine: &mut Engine, out: &mut [f32], blocks: usize) -> usize {
        let block = self.format.block_size;
        let channels = self.format.channels;
        let mut frame = 0usize;
        for _ in 0..blocks {
            let start = frame * channels;
            if start + block * channels > out.len() {
                break;
            }
            engine.render_block(&mut out[start..start + block * channels], block);
            frame += block;
        }
        self.rendered_frames += frame as u64;
        frame
    }

    /// Renders exactly `frames` frames, using as many blocks as needed.
    ///
    /// The final partial block is still a full `render_block` call clamped to
    /// the remaining frames, so the engine never assumes a constant block size
    /// (P6). Returns the number of frames rendered.
    pub fn render_frames(&mut self, engine: &mut Engine, out: &mut [f32], frames: usize) -> usize {
        let block = self.format.block_size;
        let channels = self.format.channels;
        let mut done = 0usize;
        while done < frames {
            let n = (frames - done).min(block);
            let start = done * channels;
            if start + n * channels > out.len() {
                break;
            }
            engine.render_block(&mut out[start..start + n * channels], n);
            done += n;
        }
        self.rendered_frames += done as u64;
        done
    }
}

impl AudioDriver for OfflineDriver {
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
        "offline"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, EngineConfig};

    #[test]
    fn rendering_advances_the_transport() {
        let mut engine = Engine::new(EngineConfig::default()).unwrap();
        engine.transport_mut().play();
        let driver_format = DriverFormat::from_engine(&engine);
        let mut driver = OfflineDriver::new(&engine);
        let mut out = alloc::vec![0.0f32; driver_format.block_size * 2 * 4];
        let frames = driver.render_into(&mut engine, &mut out, 4);
        assert_eq!(frames, driver_format.block_size * 4);
        assert_eq!(engine.transport().position_frames(), frames as i64);
    }

    #[test]
    fn a_short_output_buffer_stops_rather_than_overruns() {
        let mut engine = Engine::new(EngineConfig::default()).unwrap();
        let mut driver = OfflineDriver::new(&engine);
        // Room for one block only.
        let mut out = alloc::vec![0.0f32; 256 * 2];
        let frames = driver.render_into(&mut engine, &mut out, 8);
        assert_eq!(frames, 256, "the driver must stop at the buffer end");
    }

    #[test]
    fn render_frames_handles_a_partial_final_block() {
        let mut engine = Engine::new(EngineConfig::default()).unwrap();
        let mut driver = OfflineDriver::new(&engine);
        let mut out = alloc::vec![0.0f32; 300 * 2];
        let frames = driver.render_frames(&mut engine, &mut out, 300);
        assert_eq!(frames, 300, "one full block plus a 44-frame tail");
        assert_eq!(driver.rendered_frames(), 300);
    }

    #[test]
    fn the_label_is_stable() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        let driver = OfflineDriver::new(&engine);
        assert_eq!(driver.label(), "offline");
    }
}
