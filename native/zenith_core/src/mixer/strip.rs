//! Channel-strip DSP: the fixed signal chain of one channel.
//!
//! ```text
//! input → phase → gain → pan → effect chain → (send taps) → output buffer
//! ```
//!
//! The order is deliberate and matches a console:
//!
//! * **Phase before gain** so an inverted channel's fader still reads in the
//!   direction the user expects.
//! * **Gain before pan** so the pan law sees an already-levelled signal and can
//!   be a pure distribution (PLAN §3.S3 item 6). Panning first would make the
//!   fader's effect on the stereo image level-dependent.
//! * **Effects after pan** so a stereo effect sees a stereo image; an effect
//!   placed before pan would be fed a mono-ish signal and then be spread.
//! * **Sends last**, tapped either side of the fader per each send's own setting.
//!
//! This module owns the per-sample loops. It deliberately does *not* know where
//! its input comes from or where its output goes — the graph decides that — so
//! the strip can be unit-tested on plain buffers with no engine present.

use super::channel::Channel;
use super::pan_law::PanLaw;

/// Pre-allocated scratch buffers for one strip.
///
/// Owned by the graph and passed by reference into the process call. Keeping
/// the buffers outside the strip keeps the strip itself plain value state and
/// makes the allocate-once discipline visible in one place.
pub struct StripBuffers {
    /// Post-pan stereo output, interleaved `L, R, …`.
    pub output: Vec<f32>,
    /// Pre-fader stereo tap, used by pre-fader sends.
    pub pre_fader: Vec<f32>,
}

impl StripBuffers {
    /// Allocates buffers for `max_frames` frames.
    ///
    /// Called once during prepare. Nothing in the process path ever resizes
    /// these (ABI P5).
    #[must_use]
    pub fn new(max_frames: usize) -> Self {
        Self {
            output: vec![0.0; max_frames * 2],
            pre_fader: vec![0.0; max_frames * 2],
        }
    }

    /// Number of frames these buffers can hold.
    #[must_use]
    pub fn capacity_frames(&self) -> usize {
        self.output.len() / 2
    }
}

/// A channel strip: one channel's values plus the pan law it renders through.
///
/// Holds no buffers; it is the *algorithm*, and [`StripBuffers`] is its scratch
/// space.
#[derive(Debug, Clone, Copy)]
pub struct ChannelStrip {
    /// The channel's values.
    pub channel: Channel,
    /// Pan law this strip renders through.
    pub pan_law: PanLaw,
    /// Linear gain applied by the most recent process call.
    ///
    /// Exposed so sends and the graph can reuse it rather than recomputing the
    /// dB conversion once per send.
    last_gain: f32,
}

impl ChannelStrip {
    /// Creates a strip for `channel` using `pan_law`.
    #[must_use]
    pub const fn new(channel: Channel, pan_law: PanLaw) -> Self {
        Self {
            channel,
            pan_law,
            last_gain: 0.0,
        }
    }

    /// Linear gain applied by the most recent process call, fader and mute
    /// included.
    #[must_use]
    pub const fn last_gain(&self) -> f32 {
        self.last_gain
    }

    /// Applies phase, fader and pan to `input`, writing stereo to
    /// `buffers.output`.
    ///
    /// `input` is interleaved stereo with `frames * 2` samples. `frames` is
    /// authoritative (ABI P6) and is clamped to what the buffers and the input
    /// actually hold, so a driver handing over an unexpected block size cannot
    /// cause an out-of-bounds write.
    ///
    /// The chain runs phase → gain → pan. Effects are applied by the caller
    /// between this and the send taps, because effect processing belongs to S5
    /// and needs buffers of its own.
    ///
    /// Real-time safe: no allocation, no locking, no IO.
    pub fn process(&mut self, input: &[f32], frames: usize, buffers: &mut StripBuffers) {
        let cap = buffers.capacity_frames();
        let n = frames.min(cap).min(input.len() / 2);
        if n == 0 {
            self.last_gain = 0.0;
            return;
        }

        let gain = self.channel.linear_gain();
        self.last_gain = gain;
        let phase = if self.channel.phase_invert { -1.0 } else { 1.0 };
        let (pan_l, pan_r) = self.pan_law.gains(self.channel.pan);
        let gl = gain * phase * pan_l;
        let gr = gain * phase * pan_r;

        for i in 0..n {
            buffers.output[i * 2] = input[i * 2] * gl;
            buffers.output[i * 2 + 1] = input[i * 2 + 1] * gr;
        }
        // Zero the tail so a shorter block does not leave the previous block's
        // samples visible to whatever reads the whole buffer.
        for i in n..cap {
            buffers.output[i * 2] = 0.0;
            buffers.output[i * 2 + 1] = 0.0;
        }
    }

    /// Captures the pre-fader tap into `buffers.pre_fader`.
    ///
    /// Pre-fader means *before the fader* but after mute, matching
    /// [`SendTap::PreFader`]: a muted channel must not keep feeding a reverb.
    /// Phase is still applied so an inverted channel's send stays in polarity
    /// with its dry signal.
    pub fn capture_pre_fader(&self, input: &[f32], frames: usize, buffers: &mut StripBuffers) {
        let cap = buffers.capacity_frames();
        let n = frames.min(cap).min(input.len() / 2);
        if n == 0 {
            return;
        }

        // Mute silences the tap; the fader does not.
        let audible = if self.channel.audible { 1.0 } else { 0.0 };
        let phase = if self.channel.phase_invert { -1.0 } else { 1.0 };
        let g = audible * phase;

        for i in 0..n {
            buffers.pre_fader[i * 2] = input[i * 2] * g;
            buffers.pre_fader[i * 2 + 1] = input[i * 2 + 1] * g;
        }
        for i in n..cap {
            buffers.pre_fader[i * 2] = 0.0;
            buffers.pre_fader[i * 2 + 1] = 0.0;
        }
    }
}

/// Sums `src` into `dst` with a linear gain applied.
///
/// The accumulation step of the mixer. `frames` is authoritative and clamped to
/// both buffers, so a mismatched caller cannot write past the end.
///
/// Summing order matters for repeatability: channels are always summed in the
/// graph's topological order, never in an arbitrary one, so a project renders
/// identically on every run.
pub fn sum_into(dst: &mut [f32], src: &[f32], frames: usize, gain: f32) {
    let n = frames.min(dst.len() / 2).min(src.len() / 2);
    for i in 0..n {
        dst[i * 2] += src[i * 2] * gain;
        dst[i * 2 + 1] += src[i * 2 + 1] * gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::channel::{ChannelId, ChannelRole};

    fn strip() -> ChannelStrip {
        let mut ch = Channel::new(ChannelId(1), ChannelRole::Insert);
        ch.audible = true;
        ChannelStrip::new(ch, PanLaw::ConstantPower3Db)
    }

    fn buffers() -> StripBuffers {
        StripBuffers::new(64)
    }

    #[test]
    fn unity_gain_with_the_linear_law_passes_the_signal_through() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear; // 1.0 per side at centre
        let mut b = buffers();
        let input = [1.0, 1.0, 0.5, 0.5];
        s.process(&input, 2, &mut b);
        assert!((b.output[0] - 1.0).abs() < 1e-6);
        assert!((b.output[1] - 1.0).abs() < 1e-6);
        assert!((b.output[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_fader_scales_the_output() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear;
        s.channel.set_gain_db(-6.0);
        let mut b = buffers();
        s.process(&[1.0, 1.0], 1, &mut b);
        assert!(
            (b.output[0] - 0.501_187).abs() < 1e-4,
            "-6 dB should halve, got {}",
            b.output[0]
        );
        assert!((s.last_gain() - 0.501_187).abs() < 1e-4);
    }

    #[test]
    fn a_muted_channel_outputs_digital_silence() {
        let mut s = strip();
        s.channel.mute = true;
        let mut b = buffers();
        s.process(&[1.0, 1.0, 1.0, 1.0], 2, &mut b);
        assert_eq!(b.output[0], 0.0, "mute must be exactly zero");
        assert_eq!(b.output[1], 0.0);
        assert_eq!(s.last_gain(), 0.0);
    }

    #[test]
    fn a_solo_excluded_channel_outputs_digital_silence() {
        let mut s = strip();
        s.channel.audible = false;
        let mut b = buffers();
        s.process(&[1.0, 1.0], 1, &mut b);
        assert_eq!(b.output[0], 0.0, "solo-excluded must be exactly zero");
    }

    #[test]
    fn phase_inversion_flips_both_channels() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear;
        s.channel.phase_invert = true;
        let mut b = buffers();
        s.process(&[1.0, 0.5], 1, &mut b);
        assert!((b.output[0] + 1.0).abs() < 1e-6, "left not inverted");
        assert!((b.output[1] + 0.5).abs() < 1e-6, "right not inverted");
    }

    #[test]
    fn hard_left_pan_silences_the_right_output() {
        let mut s = strip();
        s.channel.set_pan(-1.0);
        let mut b = buffers();
        s.process(&[1.0, 1.0], 1, &mut b);
        assert!(b.output[0] > 0.9, "left should carry the signal");
        assert!(
            b.output[1].abs() < 1e-4,
            "right should be silent, got {}",
            b.output[1]
        );
    }

    #[test]
    fn the_tail_beyond_the_block_is_zeroed() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear;
        let mut b = buffers();
        b.output.fill(9.0);
        s.process(&[1.0, 1.0], 1, &mut b);
        assert!((b.output[0] - 1.0).abs() < 1e-6, "first frame written");
        assert_eq!(b.output[2], 0.0, "frame 1 beyond n must be cleared");
        assert_eq!(b.output[126], 0.0, "far tail must be cleared");
    }

    #[test]
    fn a_block_larger_than_the_buffers_is_clamped() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear; // unity per side, so the level is predictable
        let mut b = StripBuffers::new(4);
        let input = vec![1.0; 256];
        // 128 frames requested but the buffers hold 4: must not write past them.
        s.process(&input, 128, &mut b);
        assert_eq!(b.output.len(), 8, "the buffer must not grow");
        assert!((b.output[0] - 1.0).abs() < 1e-3, "first frame {}", b.output[0]);
    }

    #[test]
    fn a_short_input_buffer_is_clamped_to_what_it_holds() {
        let mut s = strip();
        s.pan_law = PanLaw::Linear; // unity per side
        let mut b = buffers();
        s.process(&[1.0, 1.0], 16, &mut b); // claims 16 frames, supplies 1
        assert!((b.output[0] - 1.0).abs() < 1e-3, "first frame {}", b.output[0]);
        assert_eq!(b.output[2], 0.0, "frames past the input must be silent");
    }

    #[test]
    fn zero_frames_is_a_no_op() {
        let mut s = strip();
        let mut b = buffers();
        b.output.fill(5.0);
        s.process(&[1.0, 1.0], 0, &mut b);
        assert_eq!(b.output[0], 5.0, "a zero-frame block must not write");
        assert_eq!(s.last_gain(), 0.0);
    }

    #[test]
    fn the_pre_fader_tap_ignores_the_fader_but_honours_mute() {
        let mut s = strip();
        s.channel.set_gain_db(-20.0);
        let mut b = buffers();
        s.capture_pre_fader(&[1.0, 1.0], 1, &mut b);
        assert!(
            (b.pre_fader[0] - 1.0).abs() < 1e-6,
            "pre-fader must ignore the fader, got {}",
            b.pre_fader[0]
        );

        s.channel.audible = false;
        s.capture_pre_fader(&[1.0, 1.0], 1, &mut b);
        assert_eq!(b.pre_fader[0], 0.0, "pre-fader must honour mute");
    }

    #[test]
    fn the_pre_fader_tap_applies_phase() {
        let mut s = strip();
        s.channel.phase_invert = true;
        let mut b = buffers();
        s.capture_pre_fader(&[1.0, 1.0], 1, &mut b);
        assert!((b.pre_fader[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn summing_accumulates_rather_than_overwriting() {
        let mut dst = [1.0, 1.0, 1.0, 1.0];
        sum_into(&mut dst, &[0.5, 0.25, 0.5, 0.25], 2, 1.0);
        assert!((dst[0] - 1.5).abs() < 1e-6);
        assert!((dst[1] - 1.25).abs() < 1e-6);
    }

    #[test]
    fn summing_applies_a_gain() {
        let mut dst = [0.0, 0.0];
        sum_into(&mut dst, &[1.0, 1.0], 1, 0.5);
        assert!((dst[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn summing_clamps_frames_to_both_buffers() {
        let mut dst = [0.0, 0.0];
        let src = vec![1.0; 64];
        sum_into(&mut dst, &src, 32, 1.0);
        assert!((dst[0] - 1.0).abs() < 1e-6);
        assert_eq!(dst.len(), 2, "the destination must not grow");
    }

    #[test]
    fn opposite_phase_channels_cancel_when_summed() {
        // The mono-compatibility check behind the S3 acceptance criterion
        // "no phase problems": two channels at opposite phase must cancel.
        let mut a = strip();
        a.pan_law = PanLaw::Linear;
        let mut b = strip();
        b.pan_law = PanLaw::Linear;
        b.channel.phase_invert = true;

        let mut ba = buffers();
        let mut bb = buffers();
        a.process(&[1.0, 1.0], 1, &mut ba);
        b.process(&[1.0, 1.0], 1, &mut bb);

        let mut mix = [0.0, 0.0];
        sum_into(&mut mix, &ba.output, 1, 1.0);
        sum_into(&mut mix, &bb.output, 1, 1.0);
        assert!(mix[0].abs() < 1e-6, "opposite phase must cancel, got {}", mix[0]);
        assert!(mix[1].abs() < 1e-6);
    }

    #[test]
    fn a_hard_panned_pair_sums_without_gain_above_either_source() {
        // Panning one channel hard left and another hard right must reconstruct
        // the original level in each side, with no +3 dB from the sum.
        let mut l = strip();
        l.channel.set_pan(-1.0);
        let mut r = strip();
        r.channel.set_pan(1.0);

        let mut bl = buffers();
        let mut br = buffers();
        l.process(&[1.0, 1.0], 1, &mut bl);
        r.process(&[1.0, 1.0], 1, &mut br);

        let mut mix = [0.0, 0.0];
        sum_into(&mut mix, &bl.output, 1, 1.0);
        sum_into(&mut mix, &br.output, 1, 1.0);
        assert!((mix[0] - 1.0).abs() < 1e-3, "left {}", mix[0]);
        assert!((mix[1] - 1.0).abs() < 1e-3, "right {}", mix[1]);
    }
}
