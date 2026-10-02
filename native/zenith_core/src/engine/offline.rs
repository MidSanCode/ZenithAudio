//! Offline rendering (PLAN §3.S4).
//!
//! # One DSP graph, two clocks
//!
//! The offline renderer calls the **same** [`Engine::render_block`] a device
//! callback would. There is no second DSP implementation, which is what makes
//! "real-time and offline agree sample for sample" (PLAN §3.S4 item 5) a
//! structural property rather than a promise: the only difference is who asks
//! for each block.
//!
//! # Seeking is the caller's job
//!
//! The renderer does not rewind the transport itself; the caller positions the
//! engine (transport seek, loop, tempo) before calling [`render_range`]. That
//! keeps the renderer a pure function of engine state and start/end, which is
//! what a test can assert against.
//!
//! # Tail handling
//!
//! A reverb tail or a delay continues past the last note. [`render_range`]
//! renders exactly `end` frames; callers that want the tail render past the
//! project end (`EffectProcessor::tail_seconds` reports how much each effect
//! needs).

use alloc::vec::Vec;

use crate::engine::Engine;

/// Renders `frames` frames of interleaved stereo audio from `engine`.
///
/// Uses the engine's own block size, and the final partial block is a real
/// `render_block` call clamped to the remaining frames, so the engine never
/// assumes a constant block size (ABI P6).
///
/// `out` must hold at least `frames * 2` samples. Returns the number of frames
/// actually rendered (less than `frames` only if `out` is too small).
pub fn render_range(engine: &mut Engine, out: &mut [f32], frames: usize) -> usize {
    let block = engine.config().block_size as usize;
    let mut done = 0usize;
    while done < frames {
        let n = (frames - done).min(block);
        let start = done * 2;
        if start + n * 2 > out.len() {
            break;
        }
        engine.render_block(&mut out[start..start + n * 2], n);
        done += n;
    }
    done
}

/// Renders a project range to a fresh interleaved stereo buffer.
///
/// A convenience over [`render_range`] for callers that want an owned buffer;
/// the FFI `zenith_render_offline` uses this and hands ownership to the caller.
///
/// Control thread only: it allocates the whole output buffer, which for a long
/// project is large. A streaming export should use [`render_range`] into a
/// fixed buffer instead.
#[must_use]
pub fn render_to_buffer(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let mut buffer = alloc::vec![0.0f32; frames * 2];
    let rendered = render_range(engine, &mut buffer, frames);
    buffer.truncate(rendered * 2);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig;

    fn engine() -> Engine {
        let mut engine = Engine::new(EngineConfig::default()).unwrap();
        engine.sequencer_mut().push_note(60, 1.0, 0, 4_800);
        engine.transport_mut().play();
        engine
    }

    #[test]
    fn rendering_a_range_advances_the_transport_by_that_many_frames() {
        let mut engine = engine();
        let mut out = alloc::vec![0.0f32; 1_000 * 2];
        let rendered = render_range(&mut engine, &mut out, 1_000);
        assert_eq!(rendered, 1_000);
        assert_eq!(engine.transport().position_frames(), 1_000);
    }

    #[test]
    fn rendering_produces_sound_for_a_placed_note() {
        let mut engine = engine();
        let mut out = alloc::vec![0.0f32; 4_800 * 2];
        render_range(&mut engine, &mut out, 4_800);
        let energy: f32 = out.iter().map(|s| s * s).sum();
        assert!(energy > 0.0, "a placed note must render audible audio");
    }

    #[test]
    fn rendering_is_deterministic_across_two_runs() {
        // The offline path must be reproducible: same project, same samples.
        let mut a = engine();
        let mut b = engine();
        let mut out_a = alloc::vec![0.0f32; 2_000 * 2];
        let mut out_b = alloc::vec![0.0f32; 2_000 * 2];
        render_range(&mut a, &mut out_a, 2_000);
        render_range(&mut b, &mut out_b, 2_000);
        assert_eq!(out_a, out_b, "offline rendering must be deterministic");
    }

    #[test]
    fn a_short_output_buffer_stops_rather_than_overrunning() {
        let mut engine = engine();
        let mut out = alloc::vec![0.0f32; 256 * 2];
        let rendered = render_range(&mut engine, &mut out, 10_000);
        assert_eq!(rendered, 256, "the renderer must stop at the buffer end");
    }

    #[test]
    fn render_to_buffer_returns_the_rendered_frames() {
        let mut engine = engine();
        let buffer = render_to_buffer(&mut engine, 512);
        assert_eq!(buffer.len(), 512 * 2);
    }

    #[test]
    fn offline_and_a_manual_block_loop_agree() {
        // Rendering through `render_range` must equal stepping `render_block`
        // by hand — this is the "one graph" guarantee expressed as a test.
        let mut a = engine();
        let mut b = engine();
        let frames = 1_536; // six 256-frame blocks
        let mut ranged = alloc::vec![0.0f32; frames * 2];
        render_range(&mut a, &mut ranged, frames);

        let mut manual = alloc::vec![0.0f32; frames * 2];
        for block in 0..6 {
            b.render_block(&mut manual[block * 256 * 2..(block + 1) * 256 * 2], 256);
        }
        assert_eq!(ranged, manual);
    }
}
