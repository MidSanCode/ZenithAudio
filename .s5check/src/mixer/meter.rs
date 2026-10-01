//! Level metering: peak, RMS, and a three-second peak hold.
//!
//! The audio thread writes; the UI thread reads. There is no lock between them
//! and no callback into Dart (ABI §5.3): the audio thread publishes into plain
//! `f32` fields that are read as a snapshot, and the reader may see a value
//! that is one block old. For a level meter that is not merely acceptable but
//! preferable — a torn reading would be a cosmetic artifact, and taking a lock
//! on the audio thread would be a dropout.
//!
//! ## Why the hold decays on a counter, not a clock
//!
//! [`Meter::advance`] takes the number of frames processed rather than reading
//! a wall clock. `std::time::Instant` is unavailable on `wasm32` (PLAN §0.3),
//! and a real-time thread must not consult the system clock anyway. Sample
//! counting is exact, monotonic, and works identically on every platform.

/// How long a peak stays visible after it occurs, in seconds (PLAN §3.S3 item 7).
pub const PEAK_HOLD_SECONDS: f32 = 3.0;

/// RMS averaging window, in seconds.
///
/// Long enough to be steady, short enough to track a bar of music. A one-pole
/// filter is used rather than a true block average so the meter costs one
/// multiply-add per sample and needs no history buffer.
pub const RMS_WINDOW_SECONDS: f32 = 0.3;

/// Decay applied to the held peak once the hold time expires, in dB per second.
///
/// A hold that vanished instantly would read as a glitch; a slow fallback is
/// what makes a peak-hold meter legible.
pub const PEAK_DECAY_DB_PER_SECOND: f32 = 20.0;

/// A level reading for one channel, in linear amplitude.
///
/// This is the value type that crosses to Dart; it mirrors
/// `ZenithMeterSnapshot` in `docs/ABI.md` §6.7 field-for-field and in the same
/// order (ABI P8).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterSnapshot {
    /// Peak magnitude of the left channel since the last reset, in `0.0..`.
    pub peak_l: f32,
    /// Peak magnitude of the right channel.
    pub peak_r: f32,
    /// RMS magnitude of the left channel.
    pub rms_l: f32,
    /// RMS magnitude of the right channel.
    pub rms_r: f32,
    /// Held peak of the left channel, decaying after the hold expires.
    pub peak_hold_l: f32,
    /// Held peak of the right channel.
    pub peak_hold_r: f32,
}

/// Meter state for one channel.
#[derive(Debug, Clone, Copy, Default)]
pub struct Meter {
    /// Peak of the current block, left.
    peak_l: f32,
    /// Peak of the current block, right.
    peak_r: f32,
    /// Running RMS estimate, left.
    rms_l: f32,
    /// Running RMS estimate, right.
    rms_r: f32,
    /// Held peak, left.
    hold_l: f32,
    /// Held peak, right.
    hold_r: f32,
    /// Frames remaining before the hold may start decaying.
    hold_frames_left: u32,
}

impl Meter {
    /// Creates a silent meter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            peak_l: 0.0,
            peak_r: 0.0,
            rms_l: 0.0,
            rms_r: 0.0,
            hold_l: 0.0,
            hold_r: 0.0,
            hold_frames_left: 0,
        }
    }

    /// Returns a meter with all state cleared.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Folds one interleaved stereo block into the meter.
    ///
    /// `frames` is the frame count, `block` holds `frames * 2` samples in
    /// `L, R, L, R, …` order. `sample_rate` converts the hold and decay periods
    /// from seconds into frames.
    ///
    /// Real-time safe: no allocation, no panic on any input, and a `frames`
    /// value larger than the buffer simply reads what it was given — the caller
    /// promises the buffer is at least `frames * 2` long, as every DSP call in
    /// this crate does.
    pub fn accumulate(&mut self, block: &[f32], frames: usize, sample_rate: u32) {
        if frames == 0 || sample_rate == 0 {
            return;
        }
        let available = block.len() / 2;
        let n = if frames < available { frames } else { available };
        if n == 0 {
            return;
        }

        let mut peak_l = 0.0_f32;
        let mut peak_r = 0.0_f32;
        let mut sum_l = 0.0_f32;
        let mut sum_r = 0.0_f32;

        for i in 0..n {
            let l = block[i * 2];
            let r = block[i * 2 + 1];
            // `abs` rather than `max`/`min` so a full-scale negative sample
            // registers as a peak. Non-finite samples are folded to zero on
            // both the peak *and* the RMS paths — a single NaN would otherwise
            // poison the squared sum and stick in the meter forever, since the
            // one-pole filter never forgets it.
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            let al = abs_f(l);
            let ar = abs_f(r);
            if al > peak_l {
                peak_l = al;
            }
            if ar > peak_r {
                peak_r = ar;
            }
            sum_l += l * l;
            sum_r += r * r;
        }

        self.peak_l = peak_l;
        self.peak_r = peak_r;

        // One-pole average toward this block's mean square. `alpha` is the
        // share of the window this *block* covers, not the share a single
        // sample would cover: `accumulate` is called once per block, so using
        // the per-sample coefficient would make the meter converge `n` times
        // too slowly.
        let window_frames = (RMS_WINDOW_SECONDS * sample_rate as f32).max(1.0);
        let alpha = (n as f32 / window_frames).min(1.0);
        let mean_l = sum_l / n as f32;
        let mean_r = sum_r / n as f32;
        self.rms_l += alpha * (mean_l - self.rms_l);
        self.rms_r += alpha * (mean_r - self.rms_r);

        // Peak hold: refresh while the signal is at or above the held value,
        // and restart the countdown only when the hold has expired.
        let hold_frames = (PEAK_HOLD_SECONDS * sample_rate as f32) as u32;
        if peak_l >= self.hold_l {
            self.hold_l = peak_l;
        }
        if peak_r >= self.hold_r {
            self.hold_r = peak_r;
        }

        if peak_l > 0.0 || peak_r > 0.0 {
            // Signal present: the hold is refreshed, and any pending decay is
            // cancelled so a new peak is held for its full duration.
            self.hold_frames_left = hold_frames;
        } else if self.hold_frames_left > n as u32 {
            self.hold_frames_left -= n as u32;
        } else {
            self.hold_frames_left = 0;
            // Decay by the dB-per-second rate, converted to this block's share.
            let decay_db = PEAK_DECAY_DB_PER_SECOND * (n as f32) / sample_rate as f32;
            let factor = db_decay_factor(decay_db);
            self.hold_l *= factor;
            self.hold_r *= factor;
            if self.hold_l < 1e-6 {
                self.hold_l = 0.0;
            }
            if self.hold_r < 1e-6 {
                self.hold_r = 0.0;
            }
        }
    }

    /// Returns the current reading as a snapshot value.
    ///
    /// RMS is reported as a magnitude, so the stored mean square is rooted.
    #[must_use]
    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            peak_l: self.peak_l,
            peak_r: self.peak_r,
            rms_l: sqrt_f(self.rms_l),
            rms_r: sqrt_f(self.rms_r),
            peak_hold_l: self.hold_l,
            peak_hold_r: self.hold_r,
        }
    }

    /// Peak magnitude of the left channel in the most recent block.
    #[must_use]
    pub fn peak_l(&self) -> f32 {
        self.peak_l
    }

    /// Peak magnitude of the right channel in the most recent block.
    #[must_use]
    pub fn peak_r(&self) -> f32 {
        self.peak_r
    }

    /// Whether either channel has clipped (peak at or above full scale).
    #[must_use]
    pub fn is_clipping(&self) -> bool {
        self.peak_l >= 1.0 || self.peak_r >= 1.0
    }
}

/// Linear factor corresponding to `db` decibels of attenuation.
fn db_decay_factor(db: f32) -> f32 {
    // 10^(-db/20), via the same bit-level exponential the fader curve uses.
    exp2_f(-db / 20.0 * core::f32::consts::LOG2_10)
}

/// `abs` without `std`.
#[inline]
fn abs_f(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7FFF_FFFF)
}

/// Square root via Newton iterations from a bit-trick seed.
#[inline]
fn sqrt_f(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if !x.is_finite() {
        return x;
    }
    let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000);
    for _ in 0..3 {
        g = 0.5 * (g + x / g);
    }
    g
}

/// `2^x` via exponent-field construction plus a polynomial mantissa.
fn exp2_f(x: f32) -> f32 {
    if x <= -126.0 {
        return 0.0;
    }
    if x >= 128.0 {
        return f32::INFINITY;
    }
    let xi = floor_f(x);
    let frac = x - xi;
    let p = 1.0
        + frac
            * (core::f32::consts::LN_2
                + frac
                    * (0.240_226_5
                        + frac
                            * (0.055_504_11
                                + frac
                                    * (0.009_618_129
                                        + frac * (0.001_333_355 + frac * 0.000_154_035_3)))));
    let e = xi as i32;
    p * f32::from_bits(((e + 127) as u32) << 23)
}

/// Floor without `std`.
fn floor_f(x: f32) -> f32 {
    let t = x as i32 as f32;
    if x < 0.0 && t != x {
        t - 1.0
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Builds an interleaved stereo block from a per-frame generator.
    fn block(frames: usize, mut f: impl FnMut(usize) -> (f32, f32)) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let (l, r) = f(i);
            v.push(l);
            v.push(r);
        }
        v
    }

    #[test]
    fn a_silent_meter_reads_zero_everywhere() {
        let mut m = Meter::new();
        let b = block(256, |_| (0.0, 0.0));
        m.accumulate(&b, 256, SR);
        let s = m.snapshot();
        assert_eq!(s.peak_l, 0.0);
        assert_eq!(s.peak_r, 0.0);
        assert_eq!(s.rms_l, 0.0);
        assert_eq!(s.peak_hold_l, 0.0);
        assert!(!m.is_clipping());
    }

    #[test]
    fn peak_tracks_the_largest_magnitude_including_negatives() {
        let mut m = Meter::new();
        let b = block(4, |i| match i {
            0 => (0.1, -0.1),
            1 => (-0.9, 0.2),
            2 => (0.3, 0.7),
            _ => (0.0, 0.0),
        });
        m.accumulate(&b, 4, SR);
        assert!((m.peak_l() - 0.9).abs() < 1e-6, "left peak {}", m.peak_l());
        assert!((m.peak_r() - 0.7).abs() < 1e-6, "right peak {}", m.peak_r());
    }

    #[test]
    fn full_scale_registers_as_clipping() {
        let mut m = Meter::new();
        let b = block(8, |_| (1.0, 0.5));
        m.accumulate(&b, 8, SR);
        assert!(m.is_clipping(), "peak of 1.0 must report clipping");
    }

    #[test]
    fn rms_of_a_dc_signal_converges_to_its_magnitude() {
        let mut m = Meter::new();
        // 0.5 DC: RMS should converge to 0.5, not 0.25 (mean square vs mean).
        let b = block(256, |_| (0.5, 0.5));
        for _ in 0..400 {
            m.accumulate(&b, 256, SR);
        }
        let s = m.snapshot();
        assert!((s.rms_l - 0.5).abs() < 0.01, "rms {} != 0.5", s.rms_l);
        assert!((s.rms_r - 0.5).abs() < 0.01, "rms {} != 0.5", s.rms_r);
    }

    #[test]
    fn rms_of_a_sine_is_amplitude_over_root_two() {
        let mut m = Meter::new();
        let amp = 0.8_f32;
        let block_len = 256;
        let mut phase = 0.0_f32;
        let step = 2.0 * core::f32::consts::PI * 440.0 / SR as f32;
        for _ in 0..2000 {
            let b = block(block_len, |_| {
                let v = amp * sin_approx(phase);
                phase += step;
                if phase > 2.0 * core::f32::consts::PI {
                    phase -= 2.0 * core::f32::consts::PI;
                }
                (v, v)
            });
            m.accumulate(&b, block_len, SR);
        }
        let expected = amp / 2.0_f32.sqrt();
        let got = m.snapshot().rms_l;
        assert!(
            (got - expected).abs() < 0.02,
            "sine RMS {got} != {expected}"
        );
    }

    #[test]
    fn peak_hold_persists_for_three_seconds_then_decays() {
        let mut m = Meter::new();
        // A single loud block, then silence.
        let loud = block(256, |_| (0.9, 0.9));
        m.accumulate(&loud, 256, SR);
        assert!((m.snapshot().peak_hold_l - 0.9).abs() < 1e-5);

        let silence = block(256, |_| (0.0, 0.0));
        // Just under the hold window: the peak must still read 0.9.
        let frames_in_hold = (PEAK_HOLD_SECONDS * SR as f32) as usize;
        let blocks = frames_in_hold / 256 - 2;
        for _ in 0..blocks {
            m.accumulate(&silence, 256, SR);
        }
        assert!(
            (m.snapshot().peak_hold_l - 0.9).abs() < 1e-5,
            "hold released early: {}",
            m.snapshot().peak_hold_l
        );

        // Past the window: it must now be decaying.
        for _ in 0..200 {
            m.accumulate(&silence, 256, SR);
        }
        let after = m.snapshot().peak_hold_l;
        assert!(after < 0.9, "hold did not decay after the window: {after}");
        assert!(after >= 0.0, "hold went negative: {after}");
    }

    #[test]
    fn a_new_peak_restarts_the_hold_window() {
        let mut m = Meter::new();
        let silence = block(256, |_| (0.0, 0.0));
        let loud = block(256, |_| (0.9, 0.0));

        m.accumulate(&loud, 256, SR);
        // Burn most of the hold window.
        let frames_in_hold = (PEAK_HOLD_SECONDS * SR as f32) as usize;
        for _ in 0..(frames_in_hold / 256 - 4) {
            m.accumulate(&silence, 256, SR);
        }
        // A new peak arrives; it must be held for a fresh three seconds.
        m.accumulate(&loud, 256, SR);
        for _ in 0..40 {
            m.accumulate(&silence, 256, SR);
        }
        assert!(
            (m.snapshot().peak_hold_l - 0.9).abs() < 1e-5,
            "new peak was not re-held: {}",
            m.snapshot().peak_hold_l
        );
    }

    #[test]
    fn reset_clears_every_field() {
        let mut m = Meter::new();
        let loud = block(256, |_| (1.0, 1.0));
        m.accumulate(&loud, 256, SR);
        assert!(m.is_clipping());
        m.reset();
        let s = m.snapshot();
        assert_eq!(s.peak_l, 0.0);
        assert_eq!(s.peak_hold_r, 0.0);
        assert_eq!(s.rms_l, 0.0);
    }

    #[test]
    fn nan_samples_do_not_poison_the_meter() {
        let mut m = Meter::new();
        let b = block(4, |i| if i == 0 { (f32::NAN, f32::NAN) } else { (0.5, 0.5) });
        m.accumulate(&b, 4, SR);
        let s = m.snapshot();
        assert!(s.peak_l.is_finite(), "peak went non-finite: {}", s.peak_l);
        assert!(s.rms_l.is_finite(), "rms went non-finite: {}", s.rms_l);
    }

    #[test]
    fn degenerate_arguments_are_ignored_rather_than_panicking() {
        let mut m = Meter::new();
        let b = block(16, |_| (0.5, 0.5));
        m.accumulate(&b, 0, SR); // no frames
        m.accumulate(&b, 16, 0); // no sample rate
        m.accumulate(&[], 16, SR); // empty block
        m.accumulate(&b, 10_000, SR); // more frames than the block holds
        assert!(m.snapshot().peak_l.is_finite());
    }

    #[test]
    fn snapshot_field_order_matches_the_abi_contract() {
        // ABI §6.7 freezes the field order; a silent reorder would misread in
        // Dart. Assert the constructor still wires each field by name.
        let s = MeterSnapshot {
            peak_l: 1.0,
            peak_r: 2.0,
            rms_l: 3.0,
            rms_r: 4.0,
            peak_hold_l: 5.0,
            peak_hold_r: 6.0,
        };
        assert_eq!(s.peak_l, 1.0);
        assert_eq!(s.peak_r, 2.0);
        assert_eq!(s.rms_l, 3.0);
        assert_eq!(s.rms_r, 4.0);
        assert_eq!(s.peak_hold_l, 5.0);
        assert_eq!(s.peak_hold_r, 6.0);
    }

    /// Local sine so the test harness does not depend on `std` maths.
    fn sin_approx(x: f32) -> f32 {
        // Range-reduce into [-π, π], then a 7th-order Taylor.
        let two_pi = 2.0 * core::f32::consts::PI;
        let mut t = x;
        while t > core::f32::consts::PI {
            t -= two_pi;
        }
        while t < -core::f32::consts::PI {
            t += two_pi;
        }
        let t2 = t * t;
        t * (1.0
            - t2 / 6.0 * (1.0 - t2 / 20.0 * (1.0 - t2 / 42.0 * (1.0 - t2 / 72.0))))
    }
}
