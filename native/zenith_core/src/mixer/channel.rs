//! Channel state: the parameters of one console strip.
//!
//! A [`Channel`] holds *values*, not buffers — the audio buffers live in the
//! strip and in the graph, so a channel can be copied and inspected freely on
//! the control thread. Values are stored in the units the user thinks in
//! (decibels, pan position, booleans) and converted to linear gain only where
//! the audio path needs it.

use super::{MAX_GAIN_DB, MIN_GAIN_DB};

/// Index of a channel inside a [`super::MixerGraph`].
///
/// Indices are never reused after a removal, so a stale id fails loudly with
/// `NOT_FOUND` instead of silently addressing a different channel (ABI Q3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub u32);

impl ChannelId {
    /// The master channel's index. Master always exists.
    pub const MASTER: Self = Self(0);

    /// Returns the channel index as a plain integer, for FFI use.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// What role a channel plays in the routing topology.
///
/// The role decides which optional stages run: only insert channels host
/// instrument input, and only the master reaches the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    /// A normal insert channel fed by a track.
    Insert,
    /// A return channel fed by sends from other channels.
    Return,
    /// A group channel that sums other channels and forwards to a bus.
    Group,
    /// The single master channel that reaches the audio output.
    Master,
}

impl ChannelRole {
    /// Whether this role may host sends.
    ///
    /// Sending from master or a return into itself is degenerate, so those
    /// roles are refused a send array entirely.
    #[must_use]
    pub const fn hosts_sends(self) -> bool {
        matches!(self, Self::Insert | Self::Group | Self::Return)
    }
}

/// Converts decibels to a linear amplitude multiplier.
///
/// The fader curve is defined in decibels rather than as a linear 0..1
/// (PLAN §3.S3 item 6): a linear fader spends most of its travel in a range the
/// ear reads as "almost full", which is why consoles use dB. At or below
/// [`MIN_GAIN_DB`] this returns exactly `0.0` so the bottom of the fader is a
/// true mute and the channel can be skipped on the summing path.
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    if !db.is_finite() || db <= MIN_GAIN_DB {
        return 0.0;
    }
    libm_pow10(db / 20.0)
}

/// Converts a linear amplitude to decibels, mapping silence to [`MIN_GAIN_DB`].
#[must_use]
pub fn gain_to_db(gain: f32) -> f32 {
    if !gain.is_finite() || gain <= 0.0 {
        return MIN_GAIN_DB;
    }
    let db = 20.0 * libm_log10(gain);
    db.max(MIN_GAIN_DB)
}

/// Clamps a decibel value into the fader's legal range.
#[must_use]
pub fn clamp_db(db: f32) -> f32 {
    if db.is_nan() {
        return MIN_GAIN_DB;
    }
    db.clamp(MIN_GAIN_DB, MAX_GAIN_DB)
}

/// `10^x` without the standard library's transcendentals.
///
/// The core must compile for `wasm32-unknown-unknown` and stay free of
/// platform maths dependencies, so the handful of exponentials and logarithms
/// the mixer needs are implemented here on top of bit manipulation. Accuracy is
/// ~1e-7 relative, far below the ~1e-4 dB step a fader reports.
fn libm_pow10(x: f32) -> f32 {
    // 10^x = 2^(x * log2(10))
    exp2(x * core::f32::consts::LOG2_10)
}

/// `log10(x)` without the standard library's transcendentals.
fn libm_log10(x: f32) -> f32 {
    log2(x) * core::f32::consts::LN_2 / core::f32::consts::LN_10
}

/// `2^x` via exponent-field construction plus a polynomial mantissa.
fn exp2(x: f32) -> f32 {
    if x <= -126.0 {
        return 0.0;
    }
    if x >= 128.0 {
        return f32::INFINITY;
    }
    let xi = floor_f(x);
    let frac = x - xi;
    // `2^frac` for `frac in [0,1)`.
    //
    // These are minimax coefficients, not the truncated Taylor series: Taylor
    // converges too slowly over a full unit interval, and truncating it at
    // degree 5 leaves ~1.7e-4 of relative error — enough to miss the 6 dB
    // anchor that every fader is calibrated against. The leading coefficient is
    // `ln 2` exactly, so it is written as the named constant; clippy rejects an
    // approximation of a constant that exists in `core::f32::consts`.
    let c1 = core::f32::consts::LN_2;
    let p = 1.0
        + frac
            * (c1
                + frac
                    * (0.240_226_5
                        + frac
                            * (0.055_504_11
                                + frac * (0.009_618_129 + frac * (0.001_333_355 + frac * 0.000_154_035_3)))));
    let e = xi as i32;
    let scale = f32::from_bits(((e + 127) as u32) << 23);
    p * scale
}

/// `log2(x)` via exponent extraction plus an atanh-series mantissa.
fn log2(x: f32) -> f32 {
    if x <= 0.0 || !x.is_finite() {
        return f32::NEG_INFINITY;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xFF) as i32 - 127;
    let mut m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000); // mantissa in [1,2)
    // Fold to [sqrt(0.5), sqrt(2)) so the series converges quickly.
    if m > core::f32::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let series = 1.0
        + z2 * (1.0 / 3.0 + z2 * (1.0 / 5.0 + z2 * (1.0 / 7.0 + z2 * (1.0 / 9.0 + z2 / 11.0))));
    (2.0 * z * series) / core::f32::consts::LN_2 + e as f32
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

/// A single console channel.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    /// Routing index of this channel; also its address in the graph.
    pub id: ChannelId,
    /// Role, which decides the available stages.
    pub role: ChannelRole,
    /// Fader position in decibels, `MIN_GAIN_DB..=MAX_GAIN_DB`.
    pub gain_db: f32,
    /// Pan position, `-1.0` (hard left) to `1.0` (hard right).
    pub pan: f32,
    /// Muted: contributes silence but keeps its position in the sum.
    pub mute: bool,
    /// Soloed: participates in the solo set.
    pub solo: bool,
    /// Polarity invert, applied before gain.
    pub phase_invert: bool,
    /// Whether the channel currently contributes to the mix.
    ///
    /// Cached rather than derived so the audio thread never walks the whole
    /// console to answer "is anything soloed?" mid-block.
    pub audible: bool,
}

impl Channel {
    /// Creates a channel with unity gain and centre pan.
    #[must_use]
    pub fn new(id: ChannelId, role: ChannelRole) -> Self {
        Self {
            id,
            role,
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            phase_invert: false,
            // Recomputed by the graph whenever solo/mute state changes.
            audible: role == ChannelRole::Master,
        }
    }

    /// Linear gain implied by [`Self::gain_db`], with mute folded in.
    #[must_use]
    pub fn linear_gain(&self) -> f32 {
        if self.mute || !self.audible {
            return 0.0;
        }
        db_to_gain(self.gain_db)
    }

    /// Sets the fader from a linear value, storing the equivalent decibels.
    pub fn set_linear_gain(&mut self, gain: f32) {
        self.gain_db = clamp_db(gain_to_db(gain));
    }

    /// Sets the fader in decibels, clamped to the legal range.
    pub fn set_gain_db(&mut self, db: f32) {
        self.gain_db = clamp_db(db);
    }

    /// Sets the pan position, clamped to `-1.0..=1.0`.
    pub fn set_pan(&mut self, pan: f32) {
        self.pan = if pan.is_nan() {
            0.0
        } else {
            pan.clamp(-1.0, 1.0)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_gain_is_zero_db_and_round_trips() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert!(gain_to_db(1.0).abs() < 1e-5);
    }

    #[test]
    fn db_conversion_matches_known_anchors() {
        // The three values every fader in the world is calibrated against.
        assert!((db_to_gain(6.0) - 1.995_262).abs() < 1e-4, "6 dB ≈ ×2");
        assert!((db_to_gain(12.0) - 3.981_072).abs() < 1e-4, "+12 dB");
        assert!((db_to_gain(-6.0) - 0.501_187).abs() < 1e-4, "-6 dB ≈ ×0.5");
        assert!((db_to_gain(20.0) - 10.0).abs() < 1e-3, "20 dB = ×10");
    }

    #[test]
    fn silence_is_exact_zero_at_the_bottom_of_the_fader() {
        assert_eq!(db_to_gain(MIN_GAIN_DB), 0.0);
        assert_eq!(db_to_gain(-200.0), 0.0);
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn round_trip_is_stable_across_the_range() {
        for db in [-60.0f32, -24.0, -12.0, -6.0, -3.0, 0.0, 3.0, 6.0, 12.0] {
            let back = gain_to_db(db_to_gain(db));
            assert!((back - db).abs() < 1e-3, "{db} dB round-tripped to {back}");
        }
    }

    #[test]
    fn gain_clamps_to_the_declared_fader_range() {
        assert_eq!(clamp_db(99.0), MAX_GAIN_DB);
        assert_eq!(clamp_db(-200.0), MIN_GAIN_DB);
        let mut ch = Channel::new(ChannelId(1), ChannelRole::Insert);
        ch.set_gain_db(99.0);
        assert_eq!(ch.gain_db, MAX_GAIN_DB);
    }

    #[test]
    fn muting_and_inaudible_both_yield_digital_silence() {
        let mut ch = Channel::new(ChannelId(1), ChannelRole::Insert);
        ch.audible = true;
        ch.set_gain_db(6.0);
        assert!(ch.linear_gain() > 1.9);
        ch.mute = true;
        assert_eq!(ch.linear_gain(), 0.0, "mute must be exactly zero");
        ch.mute = false;
        ch.audible = false;
        assert_eq!(ch.linear_gain(), 0.0, "solo-excluded must be exactly zero");
    }

    #[test]
    fn pan_is_clamped_and_nan_safe() {
        let mut ch = Channel::new(ChannelId(1), ChannelRole::Insert);
        ch.set_pan(2.0);
        assert_eq!(ch.pan, 1.0);
        ch.set_pan(-2.0);
        assert_eq!(ch.pan, -1.0);
        ch.set_pan(f32::NAN);
        assert_eq!(ch.pan, 0.0, "NaN pan must not poison the pan law");
    }

    #[test]
    fn only_master_needs_no_send_hosting() {
        assert!(!ChannelRole::Master.hosts_sends());
        assert!(ChannelRole::Insert.hosts_sends());
        assert!(ChannelRole::Return.hosts_sends());
        assert!(ChannelRole::Group.hosts_sends());
    }

    #[test]
    fn master_id_is_zero_and_indices_are_distinct() {
        assert_eq!(ChannelId::MASTER.get(), 0);
        assert_ne!(ChannelId(0), ChannelId(1));
    }
}
