//! Tempo-synchronised delay with damping, ping-pong routing and a stereo
//! spread control.
//!
//! PLAN section 3.S5 requires one delay that can either follow the host tempo
//! or run on a plain millisecond setting. This module is that delay.
//!
//! # Why the musical division is an enum and not a beats knob
//!
//! A free-running beats control invites a value like `0.437` of a beat, which
//! is neither on the grid nor musically useful, and -- worse -- is impossible
//! to label in a UI. The divisions a delay is actually used for are a small,
//! closed set (1/16 up to 1/2, including dotted and triplet variants), so they
//! are published as an enumeration with the beat length baked in. The UI can
//! then show "1/8 dotted" instead of "0.75 beats", and the tempo conversion
//! happens in exactly one place: the shared `beats_to_samples` conversion on
//! the render context.
//!
//! # Why the delay length is re-derived every block
//!
//! The whole point of a synced delay is that the echo follows the tempo. The
//! conversion is therefore done per block, against the tempo in the render
//! context, rather than once in `prepare`: a tempo change mid-render must move
//! the echo with it, and a delay that cached its length at prepare time would
//! drift off the grid the moment the user moved the tempo.
//!
//! # Feedback stability
//!
//! The feedback loop is `ring -> damping filters -> gain -> ring`. Each turn
//! through the loop applies the damping low-pass and high-pass, both of which
//! have a gain of at most 1, plus the (strictly sub-unity) feedback gain. The
//! loop therefore has a round-trip gain of at most `USER_MAX_FEEDBACK` at every
//! frequency, which is the textbook stability condition: every partial decays
//! by at least `1 - USER_MAX_FEEDBACK` per repeat and the closed loop cannot
//! grow. That is why the feedback parameter is clamped in the *setter* as well
//! as in the descriptor -- a value that reached unity would make the loop
//! marginally stable and the first rounding error would turn it into a runaway
//! oscillator.
//!
//! # Ping-pong
//!
//! Ping-pong is implemented by crossing the *feedback term*, not by a separate
//! send bus: the feedback computed from one channel's delay line is written
//! into the other channel's line. An impulse on the left therefore appears on
//! the right exactly one delay period later, and on the left again one period
//! after that, which is the audible definition of ping-pong. The dry input of
//! each channel always goes into its own line, so the effect is not a
//! channel-swapper while it echoes.
//!
//! Writes are deferred by one sample (the `next` array in `process`) so that a
//! crossed route costs exactly one delay period rather than two: both lines
//! read the state they had at the start of the sample.
//!
//! # Real-time safety
//!
//! Both ring buffers, both damping sections and the dry/wet scratch are
//! allocated in [`SyncDelay::prepare`]; `process` performs no allocation, no
//! locking and no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{clamp_frequency, log2};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};
use core::f32::consts::PI;

/// Free or tempo-synchronised time.
pub const PARAM_SYNC: u16 = 0;
/// The musical division used when synced.
pub const PARAM_DIVISION: u16 = 1;
/// The delay time used when free, in milliseconds.
pub const PARAM_TIME_MS: u16 = 2;
/// Feedback amount, percent.
pub const PARAM_FEEDBACK: u16 = 3;
/// Low-pass corner inside the feedback loop, hertz.
pub const PARAM_DAMP_LOWPASS: u16 = 4;
/// High-pass corner inside the feedback loop, hertz.
pub const PARAM_DAMP_HIGHPASS: u16 = 5;
/// Ping-pong routing.
pub const PARAM_PING_PONG: u16 = 6;
/// Stereo spread, percent.
pub const PARAM_SPREAD: u16 = 7;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 8;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 9;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The longest un-synced delay time the parameter table allows, in
/// milliseconds. The ring holds at least this much at every sample rate.
pub const MAX_FREE_DELAY_MS: f32 = 2_000.0;

/// The longest musical division's length, in beats.
pub const MAX_DIVISION_BEATS: f32 = 2.0;

/// The slowest tempo the ring is sized for, in beats per minute.
///
/// 20 BPM is well below any tempo a user would record at, and the extra ring
/// memory over, say, a 40 BPM assumption is a few hundred kilobytes -- far
/// cheaper than a delay that clicks because its echo ran past the end of the
/// buffer.
pub const MIN_SUPPORTED_BPM: f32 = 20.0;

/// A sensible fallback tempo when the render context carries none.
///
/// A context with a non-positive `bpm` cannot produce a beat length (the shared
/// conversion returns zero rather than dividing by zero), so a synced delay
/// using it naively would collapse to "no delay at all". Falling back to
/// 120 BPM keeps a transport-less render -- an offline bounce with no tempo map
/// -- musical instead of silent.
const FALLBACK_BPM: f32 = 120.0;

/// Lowest damping corner the parameters allow, in hertz.
const MIN_DAMP_HZ: f32 = 20.0;
/// Highest damping corner the parameters allow, in hertz.
const MAX_DAMP_HZ: f32 = 20_000.0;

/// The feedback gain actually applied at 100% feedback.
///
/// Strictly below 1.0 by construction; the `const _` assertion near the bottom
/// of this module and a test both pin that.
const USER_MAX_FEEDBACK: f32 = 0.90;

/// The value the compile-time assertion requires [`USER_MAX_FEEDBACK`] to stay
/// below.
const MAX_FEEDBACK_CEILING: f32 = 1.0;

/// How far the spread control can push one channel later, in milliseconds.
///
/// Enough to widen a mono echo into stereo without turning it into a second,
/// audible repeat.
const SPREAD_MAX_MS: f32 = 20.0;

/// A ceiling on [`SyncDelay::tail_seconds`].
///
/// An offline bounce must not be asked to render minutes of silence because a
/// user left the feedback at 90%; twelve seconds is longer than any musical
/// tail and keeps the export bounded.
const MAX_TAIL_SECONDS: f32 = 12.0;

/// A musical division, as a number of quarter-note beats.
///
/// The discriminants are the published enumeration values and must never
/// change; new divisions are appended. The listing order is longest first,
/// which is how a delay's division menu is conventionally read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SyncDivision {
    /// Half note: two beats.
    Half = 0,
    /// Dotted quarter: one and a half beats.
    QuarterDotted = 1,
    /// Quarter note: one beat.
    Quarter = 2,
    /// Dotted eighth: three quarters of a beat.
    EighthDotted = 3,
    /// Quarter-note triplet: two thirds of a beat.
    QuarterTriplet = 4,
    /// Eighth note: half a beat.
    Eighth = 5,
    /// Eighth-note triplet: a third of a beat.
    EighthTriplet = 6,
    /// Sixteenth note: a quarter of a beat.
    Sixteenth = 7,
}

impl SyncDivision {
    /// Every division, in discriminant order.
    pub const ALL: [Self; 8] = [
        Self::Half,
        Self::QuarterDotted,
        Self::Quarter,
        Self::EighthDotted,
        Self::QuarterTriplet,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
    ];

    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Half),
            1 => Some(Self::QuarterDotted),
            2 => Some(Self::Quarter),
            3 => Some(Self::EighthDotted),
            4 => Some(Self::QuarterTriplet),
            5 => Some(Self::Eighth),
            6 => Some(Self::EighthTriplet),
            7 => Some(Self::Sixteenth),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// The division's length in quarter-note beats.
    #[must_use]
    pub const fn beats(self) -> f32 {
        match self {
            Self::Half => 2.0,
            Self::QuarterDotted => 1.5,
            Self::Quarter => 1.0,
            Self::EighthDotted => 0.75,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::Eighth => 0.5,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Sixteenth => 0.25,
        }
    }

    /// The stable machine-readable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Half => "1_2",
            Self::QuarterDotted => "1_4_dotted",
            Self::Quarter => "1_4",
            Self::EighthDotted => "1_8_dotted",
            Self::QuarterTriplet => "1_4_triplet",
            Self::Eighth => "1_8",
            Self::EighthTriplet => "1_8_triplet",
            Self::Sixteenth => "1_16",
        }
    }
}

/// Free-running or tempo-synchronised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SyncMode {
    /// The delay time comes from `time_ms` and ignores the tempo.
    Free = 0,
    /// The delay time comes from the musical division and follows the tempo.
    Synced = 1,
}

impl SyncMode {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Free),
            1 => Some(Self::Synced),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Where the feedback is routed.
///
/// `LeftToRight` writes the left line's repeats into the right line, so an
/// impulse on the left is heard on the right one delay period later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PingPong {
    /// Repeats stay on the channel that produced them.
    Off = 0,
    /// The left channel's repeats move to the right.
    LeftToRight = 1,
    /// The right channel's repeats move to the left.
    RightToLeft = 2,
}

impl PingPong {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Off),
            1 => Some(Self::LeftToRight),
            2 => Some(Self::RightToLeft),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Whether the feedback path crosses channels.
    #[must_use]
    pub const fn is_crossed(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The channel that receives channel `source`'s repeats.
    ///
    /// A crossed route swaps the two sides; an uncrossed one leaves them alone.
    /// `source` is folded to `0..=1` with a comparison rather than `Ord::min`,
    /// which is not usable in a `const fn` on this toolchain.
    #[must_use]
    pub(crate) const fn target(self, source: usize) -> usize {
        let side = if source < 1 { source } else { 1 };
        if self.is_crossed() {
            1 - side
        } else {
            side
        }
    }
}

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_DELAY_SYNC,
    key: "sync_delay",
    label: "Sync Delay",
    category: EffectCategory::Delay,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: false,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let slot = address.effect_slot();
    let channel = address.index;
    let at = |sub: u16| ParameterAddress::effect(channel, slot, sub);
    [
        ParameterDescriptor {
            address: at(PARAM_SYNC),
            key: "sync",
            label: "Sync",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 1.0,
            default_value: SyncMode::Synced.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DIVISION),
            key: "division",
            label: "Division",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: (SyncDivision::ALL.len() - 1) as f32,
            default_value: SyncDivision::EighthDotted.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_TIME_MS),
            key: "time_ms",
            label: "Time",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 1.0,
            max_value: MAX_FREE_DELAY_MS,
            default_value: 375.0,
            smoothing_ms: 50.0,
        },
        ParameterDescriptor {
            address: at(PARAM_FEEDBACK),
            key: "feedback",
            label: "Feedback",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 40.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DAMP_LOWPASS),
            key: "damp_lowpass_hz",
            label: "Damp LPF",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: MIN_DAMP_HZ,
            max_value: MAX_DAMP_HZ,
            default_value: 8_000.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DAMP_HIGHPASS),
            key: "damp_highpass_hz",
            label: "Damp HPF",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: MIN_DAMP_HZ,
            max_value: MAX_DAMP_HZ,
            default_value: 40.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_PING_PONG),
            key: "ping_pong",
            label: "Ping-Pong",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 2.0,
            default_value: PingPong::Off.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SPREAD),
            key: "spread",
            label: "Spread",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_MIX),
            key: "mix",
            label: "Mix",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 30.0,
            smoothing_ms: 20.0,
        },
    ]
}

/// One channel's damping filters.
///
/// Both are one-pole sections whose gain never exceeds 1, which is what makes
/// the feedback loop provably non-growing (see the module docs). Each is
/// arranged so that "off" is exactly a wire: the low-pass corner above Nyquist
/// and the high-pass corner at DC.
#[derive(Debug, Clone, Copy)]
struct Damping {
    /// Low-pass state.
    lowpass: f32,
    /// High-pass input history.
    highpass_x1: f32,
    /// High-pass output history.
    highpass_y1: f32,
}

impl Damping {
    /// A bypassed damping stage, passing audio through unchanged.
    #[must_use]
    const fn transparent() -> Self {
        Self {
            lowpass: 0.0,
            highpass_x1: 0.0,
            highpass_y1: 0.0,
        }
    }

    /// Clears the filter history.
    fn reset(&mut self) {
        *self = Self::transparent();
    }

    /// Runs one sample through the low-pass then the high-pass.
    ///
    /// `lpf_coeff` of `1.0` makes the low-pass a wire; `hpf_coeff` of `0.0`
    /// makes the high-pass a wire. Both are guaranteed in `0..=1` by the
    /// coefficient helpers.
    fn process(&mut self, input: f32, lpf_coeff: f32, hpf_coeff: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        // One-pole low-pass: y += (x - y) * a.
        self.lowpass += (x - self.lowpass) * lpf_coeff;
        if !self.lowpass.is_finite() {
            self.lowpass = 0.0;
        }
        let low = self.lowpass;
        // One-pole high-pass, complementary form: y = a * (y1 + x - x1).
        let high = hpf_coeff * (self.highpass_y1 + low - self.highpass_x1);
        self.highpass_x1 = low;
        self.highpass_y1 = if high.is_finite() { high } else { 0.0 };
        self.highpass_y1
    }
}

/// One channel's delay line: a preallocated ring buffer.
#[derive(Debug)]
struct DelayLine {
    /// The ring, `capacity + 2` samples, allocated in `prepare`.
    ring: alloc::vec::Vec<f32>,
    /// Write cursor, one past the most recently written sample.
    write: usize,
}

impl DelayLine {
    /// An empty line; `prepare` sizes the ring.
    fn new() -> Self {
        Self {
            ring: alloc::vec::Vec::new(),
            write: 0,
        }
    }

    /// Allocates a ring holding `capacity` usable samples.
    fn prepare(&mut self, capacity: usize) {
        // Two samples of slack so a fractional read of the longest legal delay
        // can never alias onto the sample being written this instant.
        self.ring = alloc::vec![0.0; capacity.max(1) + 2];
        self.write = 0;
    }

    /// The usable delay range, in samples.
    #[must_use]
    fn capacity(&self) -> usize {
        self.ring.len().saturating_sub(2)
    }

    /// Reads `delay_samples` back from the cursor, with linear interpolation.
    ///
    /// `delay_samples` is clamped into `1..=capacity`, so a caller cannot read
    /// past the end of the ring. The fractional part interpolates between the
    /// two neighbouring taps, which is what turns a tempo change into a
    /// continuous pitch shift instead of a staircase of clicks.
    ///
    /// # Index arithmetic
    ///
    /// `process` calls this *before* writing the current frame, so the cursor
    /// sits at the time of the frame being produced and the sample `d` frames
    /// ago lives at `(cursor - d) mod len` exactly. Reading from `cursor - 1`
    /// instead -- the most recently written sample -- would give a delay one
    /// sample longer than the caller asked for, which is audible as an echo
    /// that never quite lands on the grid.
    #[must_use]
    fn read(&self, delay_samples: f32) -> f32 {
        let capacity = self.capacity();
        let len = self.ring.len();
        if capacity == 0 || len == 0 {
            return 0.0;
        }
        let delay = if delay_samples.is_finite() {
            delay_samples.clamp(1.0, capacity as f32)
        } else {
            1.0
        };
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        // `whole` back from the cursor is the integer tap; one sample further
        // back is the tap after it, which the fraction interpolates towards.
        let base = self.write + len - whole;
        let first = self.ring[base % len];
        let second = self.ring[(base + len - 1) % len];
        first + (second - first) * fraction
    }

    /// Writes one sample and advances the cursor.
    fn write_sample(&mut self, sample: f32) {
        let len = self.ring.len();
        if len == 0 {
            return;
        }
        self.ring[self.write % len] = if sample.is_finite() { sample } else { 0.0 };
        self.write = (self.write + 1) % len;
    }

    /// Zeroes the ring and rewinds the cursor.
    fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }
}

/// The tempo-synchronised delay.
#[derive(Debug)]
pub struct SyncDelay {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Free or synced.
    sync: SyncMode,
    /// The musical division used when synced.
    division: SyncDivision,
    /// Free delay time in milliseconds.
    time_ms: f32,
    /// Feedback in percent, as set by the user.
    feedback_percent: f32,
    /// Low-pass corner in hertz.
    damping_lowpass_hz: f32,
    /// High-pass corner in hertz.
    damping_highpass_hz: f32,
    /// Ping-pong routing.
    ping_pong: PingPong,
    /// Stereo spread in percent.
    spread_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel delay lines.
    lines: [DelayLine; MAX_CHANNELS],
    /// Per-channel damping filters.
    damping: [Damping; MAX_CHANNELS],
    /// The delay length used by the most recent block on channel 0, in
    /// samples.
    ///
    /// Retained so `tail_seconds` (and the tests) can see the effect of a
    /// tempo change without re-deriving it from a context that is no longer
    /// available.
    last_delay_left: f32,
    /// The delay length used by the most recent block on channel 1, in
    /// samples, after spread.
    last_delay_right: f32,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Preallocated snapshot of channel 0's dry input, `max_block`.
    dry_left: alloc::vec::Vec<f32>,
    /// Preallocated snapshot of channel 1's dry input, `max_block`.
    dry_right: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer for channel 0, `max_block`.
    wet_left: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer for channel 1, `max_block`.
    wet_right: alloc::vec::Vec<f32>,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for SyncDelay {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_SYNC))
    }
}

impl SyncDelay {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            table,
            sync: SyncMode::Synced,
            division: SyncDivision::EighthDotted,
            time_ms: 375.0,
            feedback_percent: 40.0,
            damping_lowpass_hz: 8_000.0,
            damping_highpass_hz: 40.0,
            ping_pong: PingPong::Off,
            spread_percent: 0.0,
            mix_percent: 30.0,
            lines: [DelayLine::new(), DelayLine::new()],
            damping: [Damping::transparent(); MAX_CHANNELS],
            last_delay_left: 0.0,
            last_delay_right: 0.0,
            wet: 0.3,
            bypassed: false,
            sample_rate: 48_000.0,
            dry_left: alloc::vec::Vec::new(),
            dry_right: alloc::vec::Vec::new(),
            wet_left: alloc::vec::Vec::new(),
            wet_right: alloc::vec::Vec::new(),
            max_block: 0,
        }
    }

    /// The feedback gain actually applied, `0..=1`.
    #[must_use]
    fn feedback_gain(&self) -> f32 {
        (self.feedback_percent / 100.0).clamp(0.0, 1.0) * USER_MAX_FEEDBACK
    }

    /// The ring capacity the given sample rate requires, in samples.
    ///
    /// The larger of the free-time budget and the slowest-tempo musical
    /// division, plus the spread offset and a block of headroom.
    #[must_use]
    fn required_capacity(sample_rate: f32, max_block: usize) -> usize {
        let rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let by_time = rate * MAX_FREE_DELAY_MS / 1_000.0;
        let by_tempo = rate * 60.0 / MIN_SUPPORTED_BPM * MAX_DIVISION_BEATS;
        // The spread pushes one channel later than the other; the ring has to
        // cover the whole window, not the earlier channel's share of it.
        let with_spread = by_tempo + rate * SPREAD_MAX_MS / 1_000.0 + 2.0;
        let largest = by_time.max(with_spread).max(2.0);
        // A block of headroom costs nothing and removes any possibility of a
        // fractional read reaching the sample being written.
        (largest.ceil() as usize).saturating_add(max_block)
    }

    /// The delay length in samples for this block, before spread.
    ///
    /// This is the one place the tempo conversion happens. Either the delay
    /// follows the tempo or it does not -- there is no third behaviour, and
    /// having a single expression for it keeps the two paths from drifting.
    #[must_use]
    fn delay_samples_for(&self, ctx: &RenderContext) -> f32 {
        let rate = self.sample_rate.max(1.0);
        match self.sync {
            SyncMode::Synced => {
                let beats = self.division.beats();
                // Use the transport's own conversion so this delay and any
                // other tempo-synced effect cannot disagree about what a beat
                // is worth. The tempo is substituted for a broken one, because
                // a zero bpm would otherwise ask for a zero-sample delay.
                let tempo = ctx.with_tempo(self.effective_bpm(ctx), ctx.ppq);
                let samples = tempo.beats_to_samples(beats);
                if samples.is_finite() && samples > 0.0 {
                    samples
                } else {
                    beats * 60.0 / FALLBACK_BPM * rate
                }
            }
            SyncMode::Free => self.time_ms.max(0.0) / 1_000.0 * rate,
        }
    }

    /// The tempo to synchronise to, substituting a usable value for a broken
    /// one.
    #[must_use]
    fn effective_bpm(&self, ctx: &RenderContext) -> f32 {
        if ctx.bpm.is_finite() && ctx.bpm > 0.0 {
            ctx.bpm
        } else {
            FALLBACK_BPM
        }
    }

    /// How long the echo takes to fall below audibility, in seconds.
    #[must_use]
    fn decay_seconds(&self) -> f32 {
        let gain = self.feedback_gain();
        // Repeats for a 60 dB decay: `ln(0.001) / ln(gain)`.
        let repeats = if gain >= MAX_FEEDBACK_CEILING {
            200.0
        } else if gain > 1e-3 {
            -6.907_755 / log2(gain) * core::f32::consts::LN_2
        } else {
            1.0
        };
        let longest = (self.last_delay_left.max(self.last_delay_right).max(0.0)
            / self.sample_rate.max(1.0))
        .max(0.0);
        // A floor of one repeat keeps a short, quiet delay audible in an
        // offline bounce: a tail of zero would truncate the only echo there is.
        (repeats * longest).min(MAX_TAIL_SECONDS)
    }

    /// Stores one wet sample in the scratch buffer for `channel`.
    #[inline]
    fn store_wet(&mut self, channel: usize, index: usize, value: f32) {
        let slot = if channel == 0 {
            self.wet_left.get_mut(index)
        } else {
            self.wet_right.get_mut(index)
        };
        if let Some(slot) = slot {
            *slot = value;
        }
    }

    /// Mixes each channel's wet scratch against the dry snapshot in place.
    ///
    /// # Why there is no DC blocker here
    ///
    /// The feedback loop already runs through a one-pole high-pass at the
    /// user's damping corner, so any offset inside the line is removed where it
    /// matters. A second high-pass on the output would be driven by the echo,
    /// and its own step response is a decaying tail a few hundred samples long
    /// -- which would smear an otherwise perfect impulse response and leave a
    /// spurious residue between the repeats.
    fn wet_mix(&mut self, buffer: &mut AudioBuffer<'_>, channels: usize, frames: usize, wet: f32) {
        for channel in 0..channels {
            let dry = if channel == 0 {
                &self.dry_left
            } else {
                &self.dry_right
            };
            let wet_scratch = if channel == 0 {
                &self.wet_left
            } else {
                &self.wet_right
            };
            let Some(out) = buffer.channel_mut(channel) else {
                continue;
            };
            for (index, sample) in out.iter_mut().enumerate().take(frames) {
                let tap = wet_scratch.get(index).copied().unwrap_or(0.0);
                let dry_sample = dry.get(index).copied().unwrap_or(0.0);
                *sample = tap * wet + dry_sample * (1.0 - wet);
            }
        }
    }
}

/// The one-pole coefficient for a low-pass at `hz`.
///
/// Returns `1.0` when the corner is at or above Nyquist, which makes the
/// section a wire -- the "damping off" case the tests rely on.
#[must_use]
fn lowpass_coeff(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 1.0;
    }
    let corner = clamp_frequency(hz, sample_rate);
    if corner >= sample_rate * 0.49 {
        return 1.0;
    }
    // One-pole low-pass, `a = 1 - exp(-2*PI*f/fs)`, which for fs >> f is
    // approximately `2*PI*f/fs`.
    let x = 2.0 * PI * corner / sample_rate;
    x.clamp(0.0, 1.0)
}

/// The one-pole coefficient for a high-pass at `hz`.
///
/// The section is the complementary one-pole `y = a * (y1 + x - x1)`, whose
/// gain is `1` at Nyquist and `0` at DC. A coefficient of `1.0` therefore makes
/// it a wire, which is the "damping off" case: returning `0.0` here would not
/// bypass the filter, it would *mute* it, which is exactly the bug this
/// documentation exists to prevent.
#[must_use]
fn highpass_coeff(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 1.0;
    }
    let corner = clamp_frequency(hz, sample_rate);
    if corner <= MIN_DAMP_HZ {
        return 1.0;
    }
    // `a = 1 / (1 + 2*PI*f/fs)`, so a higher corner gives a smaller `a` and
    // therefore more attenuation of the lows.
    let a = 1.0 / (1.0 + 2.0 * PI * corner / sample_rate);
    a.clamp(0.0, 1.0)
}

impl EffectProcessor for SyncDelay {
    fn descriptor(&self) -> &'static EffectDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.max_block = max_block;
        let _ = channels;

        // Every allocation this effect will ever make happens here. The ring
        // is sized for the worst parameter combination -- the longest free
        // time, or the longest musical division at the slowest supported tempo
        // -- so a tempo change or a division change never resizes it.
        let capacity = Self::required_capacity(self.sample_rate, max_block);
        self.dry_left = alloc::vec![0.0; max_block];
        self.dry_right = alloc::vec![0.0; max_block];
        self.wet_left = alloc::vec![0.0; max_block];
        self.wet_right = alloc::vec![0.0; max_block];
        for line in self.lines.iter_mut() {
            line.prepare(capacity);
        }
        self.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) {
        if self.bypassed {
            return;
        }
        let channels = buffer.channel_count().min(MAX_CHANNELS);
        let frames = buffer.frames();
        if channels == 0 || frames == 0 {
            return;
        }
        // Refuse a block larger than `prepare` sized for rather than indexing
        // past the scratch. A silent no-op is a far better failure than an
        // out-of-bounds write in the audio thread.
        if frames > self.max_block
            || frames > self.dry_left.len()
            || frames > self.dry_right.len()
            || frames > self.wet_left.len()
            || frames > self.wet_right.len()
        {
            return;
        }

        // -- Delay length, re-derived every block so the echo follows the
        //    transport rather than a length captured at prepare time. The
        //    *previous* block's lengths are retained so the tap can glide from
        //    one to the other rather than jumping at the boundary. --
        let previous_left = self.last_delay_left;
        let previous_right = self.last_delay_right;
        let base_delay = self.delay_samples_for(ctx);
        let spread_samples = (self.spread_percent / 100.0).clamp(0.0, 1.0) * SPREAD_MAX_MS
            / 1_000.0
            * self.sample_rate;
        let delay_left = base_delay.max(1.0);
        let delay_right = (base_delay + spread_samples).max(1.0);
        self.last_delay_left = delay_left;
        self.last_delay_right = delay_right;

        let feedback = self.feedback_gain();
        let lpf = lowpass_coeff(self.damping_lowpass_hz, self.sample_rate);
        let hpf = highpass_coeff(self.damping_highpass_hz, self.sample_rate);
        let destination = [self.ping_pong.target(0), self.ping_pong.target(1)];
        let wet = self.wet;

        // -- Snapshot every channel's dry signal first: the wet/dry mix needs
        //    it, and the delay must read the *input* rather than the line's own
        //    previous state. Each channel gets its own snapshot, because a
        //    single shared buffer would let the second channel overwrite the
        //    first. --
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            let snapshot = if channel == 0 {
                &mut self.dry_left
            } else {
                &mut self.dry_right
            };
            snapshot[..frames].copy_from_slice(source);
            // Sanitize once. A NaN that reached the ring would be re-read every
            // period, so one bad sample would silence the effect forever;
            // replacing it here confines the damage to the frame that had it.
            for sample in snapshot[..frames].iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
        }

        // -- Run both delay lines. The loop is indexed rather than iterated
        //    because the two channels share the sample clock and read one
        //    another's rings when ping-pong is on.
        //
        //    The delay length glides from its previous value to this block's
        //    value over the block. `beats_to_samples` is evaluated once per
        //    block, so a tempo that moved between blocks would otherwise step
        //    the read position discontinuously at the block boundary -- a click
        //    in the middle of what should be a smooth pitch change. Gliding the
        //    tap is the same fix a host's automation smoothing applies, done
        //    here because the tempo itself is not a smoothed parameter. --
        let previous_left = if previous_left > 0.0 {
            previous_left
        } else {
            delay_left
        };
        let previous_right = if previous_right > 0.0 {
            previous_right
        } else {
            delay_right
        };
        let frames_f = frames as f32;
        for index in 0..frames {
            let progress = index as f32 / frames_f;
            let glide_left = previous_left + (delay_left - previous_left) * progress;
            let glide_right = previous_right + (delay_right - previous_right) * progress;
            // Deferred writes: both channels read the state they had at the
            // start of this sample, so a crossed route moves a repeat exactly
            // one delay period and not two.
            let mut next = [0.0_f32; MAX_CHANNELS];
            for channel in 0..channels {
                let delay = if channel == 0 {
                    glide_left
                } else {
                    glide_right
                };
                // Read *before* writing, so a delay of one sample still delays
                // by one sample rather than returning the input unchanged.
                let tap = self.lines[channel].read(delay);
                let damped = self.damping[channel].process(tap, lpf, hpf);
                self.store_wet(channel, index, tap);
                // The dry input always goes into its own line; only the
                // feedback term is routed, which is what makes ping-pong move
                // the repeats rather than the whole signal.
                let input = if channel == 0 {
                    self.dry_left[index]
                } else {
                    self.dry_right[index]
                };
                let reuse = next[destination[channel]];
                next[destination[channel]] = reuse + input + damped * feedback;
            }
            for (channel, sample) in next.iter().enumerate().take(channels) {
                self.lines[channel].write_sample(*sample);
            }
        }

        self.wet_mix(buffer, channels, frames, wet);
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut() {
            line.reset();
        }
        for damping in self.damping.iter_mut() {
            damping.reset();
        }
        self.last_delay_left = 0.0;
        self.last_delay_right = 0.0;
    }

    fn latency_samples(&self) -> usize {
        // Zero. The dry path is undelayed and the echo is the effect's audible
        // content, so there is nothing for PDC to line up -- the engine's
        // wet/dry mix already places the dry signal at time zero.
        0
    }

    fn parameters(&self) -> &[ParameterDescriptor] {
        &self.table
    }

    fn set_parameter(&mut self, sub: u16, value: f32) {
        let Some(spec) = self.table.get(sub as usize).copied() else {
            return;
        };
        let value = clamp_parameter(&spec, value);
        match sub {
            PARAM_SYNC => {
                if let Some(mode) = SyncMode::from_u32(value as u32) {
                    self.sync = mode;
                }
            }
            PARAM_DIVISION => {
                if let Some(division) = SyncDivision::from_u32(value as u32) {
                    self.division = division;
                }
            }
            PARAM_TIME_MS => self.time_ms = value,
            PARAM_FEEDBACK => self.feedback_percent = value,
            PARAM_DAMP_LOWPASS => self.damping_lowpass_hz = value,
            PARAM_DAMP_HIGHPASS => self.damping_highpass_hz = value,
            PARAM_PING_PONG => {
                if let Some(mode) = PingPong::from_u32(value as u32) {
                    self.ping_pong = mode;
                }
            }
            PARAM_SPREAD => self.spread_percent = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_SYNC => Some(self.sync.as_u32() as f32),
            PARAM_DIVISION => Some(self.division.as_u32() as f32),
            PARAM_TIME_MS => Some(self.time_ms),
            PARAM_FEEDBACK => Some(self.feedback_percent),
            PARAM_DAMP_LOWPASS => Some(self.damping_lowpass_hz),
            PARAM_DAMP_HIGHPASS => Some(self.damping_highpass_hz),
            PARAM_PING_PONG => Some(self.ping_pong.as_u32() as f32),
            PARAM_SPREAD => Some(self.spread_percent),
            PARAM_MIX => Some(self.mix_percent),
            _ => None,
        }
    }

    fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }

    fn wet(&self) -> f32 {
        self.wet
    }

    fn set_wet(&mut self, wet: f32) {
        self.wet = sanitize_wet(wet);
        self.mix_percent = self.wet * 100.0;
    }

    fn tail_seconds(&self) -> f32 {
        // The decay time, so an offline bounce does not cut the echo off. With
        // no feedback the tail is exactly one repeat; with heavy feedback it is
        // however many repeats it takes to fall 60 dB.
        self.decay_seconds()
    }
}

/// Asserts at compile time that the feedback ceiling this module documents is
/// actually below unity.
///
/// A build-time check rather than a test because it protects an invariant the
/// DSP depends on: raising [`USER_MAX_FEEDBACK`] to "just a bit more" would make
/// the feedback loop marginally stable and the delay would self-oscillate.
const _: () = assert!(USER_MAX_FEEDBACK < MAX_FEEDBACK_CEILING);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use alloc::vec::Vec;

    const SR: f32 = 48_000.0;

    fn make() -> SyncDelay {
        let mut effect = SyncDelay::new(ParameterAddress::effect(0, 0, PARAM_SYNC));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// The delay length the effect derived from `ctx`, in samples.
    fn delay_for(effect: &mut SyncDelay, ctx: &RenderContext) -> f32 {
        effect.delay_samples_for(ctx)
    }

    /// The largest magnitude in `slice`.
    fn peak(slice: &[f32]) -> f32 {
        slice.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    /// Renders `input` into both channels, returning a chosen channel.
    fn render(effect: &mut SyncDelay, input: &[f32], channel: usize, bpm: f32) -> Vec<f32> {
        let (left, right) = render_stereo(effect, input, bpm);
        if channel == 0 {
            left
        } else {
            right
        }
    }

    /// Feeds the same signal to both channels, returning both outputs.
    fn render_stereo(effect: &mut SyncDelay, input: &[f32], bpm: f32) -> (Vec<f32>, Vec<f32>) {
        render_channels(effect, input, input, bpm)
    }

    /// Renders an impulse that reaches the left channel only.
    fn render_left_impulse(
        effect: &mut SyncDelay,
        input: &[f32],
        bpm: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let silence = alloc::vec![0.0_f32; input.len()];
        render_channels(effect, input, &silence, bpm)
    }

    /// Drives the effect block by block with independent channel content.
    ///
    /// Kept separate from `render_stereo` so a ping-pong test can put the
    /// impulse on one side only -- feeding both channels the same signal would
    /// make such a test vacuous, since both sides would already carry it.
    fn render_channels(
        effect: &mut SyncDelay,
        left_in: &[f32],
        right_in: &[f32],
        bpm: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let chunk = 256;
        let frames_total = left_in.len().min(right_in.len());
        let mut left_out: Vec<f32> = Vec::with_capacity(frames_total);
        let mut right_out: Vec<f32> = Vec::with_capacity(frames_total);
        let mut offset = 0;
        while offset < frames_total {
            let frames = chunk.min(frames_total - offset);
            let mut left = alloc::vec![0.0_f32; frames];
            let mut right = alloc::vec![0.0_f32; frames];
            left.copy_from_slice(&left_in[offset..offset + frames]);
            right.copy_from_slice(&right_in[offset..offset + frames]);
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, frames, offset as i64, bpm, 960);
                effect.process(&mut buffer, &ctx);
            }
            left_out.extend_from_slice(&left);
            right_out.extend_from_slice(&right);
            offset += frames;
        }
        (left_out, right_out)
    }

    /// Configures a plain, undamped, fully wet delay of `ms` milliseconds.
    fn plain(ms: f32) -> SyncDelay {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, ms);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, MAX_DAMP_HZ);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, MIN_DAMP_HZ);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, super::super::super::registry::KIND_DELAY_SYNC);
        assert_eq!(d.key, "sync_delay");
        assert_eq!(d.label, "Sync Delay");
        assert_eq!(d.category, EffectCategory::Delay);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(!d.has_latency);
        assert!(!d.is_analysis_only);
    }

    #[test]
    fn the_parameter_table_is_ordinal_and_complete() {
        let effect = make();
        let table = effect.parameters();
        assert_eq!(table.len(), PARAM_COUNT as usize);
        for (ordinal, spec) in table.iter().enumerate() {
            assert_eq!(spec.address.sub & 0x00FF, ordinal as u16);
            assert_eq!(spec.address.index, 0, "address channel");
            assert!(
                spec.min_value <= spec.default_value && spec.default_value <= spec.max_value,
                "parameter {ordinal} default is outside its range"
            );
            assert!(!spec.key.is_empty());
            assert!(spec.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(!spec.label.is_empty());
        }
        for (i, a) in table.iter().enumerate() {
            for b in &table[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate parameter key {}", a.key);
            }
        }
    }

    #[test]
    fn each_instances_table_carries_its_own_slot_address() {
        let a = SyncDelay::new(ParameterAddress::effect(3, 2, PARAM_SYNC));
        let b = SyncDelay::new(ParameterAddress::effect(5, 7, PARAM_SYNC));
        assert_ne!(a.parameters()[0].address, b.parameters()[0].address);
        assert_eq!(a.parameters()[0].address.index, 3);
        assert_eq!(b.parameters()[0].address.index, 5);
    }

    #[test]
    fn every_parameter_round_trips_through_the_setter() {
        let mut effect = make();
        assert_eq!(effect.get_parameter(999), None);
        for sub in 0..PARAM_COUNT {
            let spec = effect.table[sub as usize];
            // A discrete parameter only accepts its enumeration values, so the
            // midpoint of an enum range is deliberately not a valid probe: use
            // each legal value instead, which also proves the setter accepts
            // every member of the published enumeration.
            if spec.is_discrete() {
                let mut value = spec.min_value;
                while value <= spec.max_value {
                    effect.set_parameter(sub, value);
                    assert_eq!(
                        effect.get_parameter(sub),
                        Some(value),
                        "parameter {sub} rejected its own enumeration value {value}"
                    );
                    value += 1.0;
                }
                continue;
            }
            let midpoint = (spec.min_value + spec.max_value) * 0.5;
            effect.set_parameter(sub, midpoint);
            let read = effect.get_parameter(sub).expect("known ordinal");
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 1e9);
        assert_eq!(effect.get_parameter(PARAM_FEEDBACK), Some(100.0));
        effect.set_parameter(PARAM_FEEDBACK, -1e9);
        assert_eq!(effect.get_parameter(PARAM_FEEDBACK), Some(0.0));
        effect.set_parameter(PARAM_TIME_MS, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_TIME_MS), Some(375.0));
        // Unknown ordinals are a no-op, never a panic.
        effect.set_parameter(999, 42.0);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn a_synced_delay_follows_the_tempo_proportionally() {
        // The whole point of the effect: at half the tempo the echo is twice as
        // far away.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        effect.set_parameter(PARAM_DIVISION, SyncDivision::Quarter.as_u32() as f32);

        let fast = RenderContext::new(SR, 256, 0, 120.0, 960);
        let slow = RenderContext::new(SR, 256, 0, 60.0, 960);
        let a = delay_for(&mut effect, &fast);
        let b = delay_for(&mut effect, &slow);
        assert!((a - 24_000.0).abs() < 1.0, "120 BPM quarter = {a}");
        assert!((b - 48_000.0).abs() < 1.0, "60 BPM quarter = {b}");
        assert!(
            (b / a - 2.0).abs() < 0.01,
            "halving the tempo should double the delay, got {b}/{a}"
        );
    }

    #[test]
    fn a_synced_delay_actually_moves_its_echo_with_the_tempo() {
        // The conversion test above is analytic; this one renders audio so the
        // two cannot agree vacuously.
        let period_at_120 = 24_000;
        let impulse = {
            let mut v = alloc::vec![0.0_f32; 60_000];
            v[0] = 1.0;
            v
        };

        let mut fast = plain(20.0);
        fast.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        fast.set_parameter(PARAM_DIVISION, SyncDivision::Quarter.as_u32() as f32);
        let out = render(&mut fast, &impulse, 0, 120.0);
        assert!(
            out[period_at_120].abs() > 0.9,
            "120 BPM quarter-note echo missing at {period_at_120}"
        );

        let mut slow = plain(20.0);
        slow.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        slow.set_parameter(PARAM_DIVISION, SyncDivision::Quarter.as_u32() as f32);
        let out = render(&mut slow, &impulse, 0, 60.0);
        assert!(
            out[period_at_120].abs() < 1e-6,
            "at 60 BPM the echo must not still be at the 120 BPM position"
        );
        assert!(
            out[period_at_120 * 2].abs() > 0.9,
            "60 BPM quarter-note echo missing at {}",
            period_at_120 * 2
        );
    }

    #[test]
    fn a_free_delay_ignores_the_tempo() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 250.0);
        let fast = delay_for(&mut effect, &RenderContext::new(SR, 256, 0, 180.0, 960));
        let slow = delay_for(&mut effect, &RenderContext::new(SR, 256, 0, 60.0, 960));
        assert!((fast - 12_000.0).abs() < 1.0, "250 ms at 48 kHz is {fast}");
        assert_eq!(fast, slow, "a free delay must not move with the tempo");
    }

    #[test]
    fn every_division_converts_to_its_documented_beat_length() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        let ctx = RenderContext::new(SR, 256, 0, 120.0, 960);
        // At 120 BPM a beat is 24 000 samples.
        for division in SyncDivision::ALL {
            effect.set_parameter(PARAM_DIVISION, division.as_u32() as f32);
            let got = delay_for(&mut effect, &ctx);
            let want = division.beats() * 24_000.0;
            assert!(
                (got - want).abs() < 1.0,
                "{division:?}: got {got}, expected {want}"
            );
        }
    }

    #[test]
    fn the_divisions_are_ordered_from_longest_to_shortest() {
        // The enumeration lists 1/2 down to 1/16, which is how a division menu
        // reads. A dotted or triplet variant that broke the ordering would make
        // the menu jump around.
        let mut previous = f32::MAX;
        for division in SyncDivision::ALL {
            let beats = division.beats();
            assert!(
                beats < previous,
                "{division:?} breaks the ordering ({beats} after {previous})"
            );
            previous = beats;
        }
        assert_eq!(SyncDivision::ALL.len(), 8);
        for (index, division) in SyncDivision::ALL.iter().enumerate() {
            assert_eq!(division.as_u32() as usize, index);
            assert_eq!(SyncDivision::from_u32(index as u32), Some(*division));
            assert!(SyncDivision::from_u32(8).is_none());
            assert!(!division.key().is_empty());
        }
        assert_eq!(SyncMode::from_u32(0), Some(SyncMode::Free));
        assert_eq!(SyncMode::from_u32(1), Some(SyncMode::Synced));
        assert_eq!(SyncMode::from_u32(2), None);
        assert_eq!(PingPong::from_u32(2), Some(PingPong::RightToLeft));
        assert_eq!(PingPong::from_u32(3), None);
    }

    #[test]
    fn beats_convert_through_the_render_context_not_a_local_formula() {
        // The shared conversion is the contract; a local `60/bpm*rate` that
        // disagreed with it would put this delay off the grid against every
        // other tempo-synced effect.
        let ctx = RenderContext::new(SR, 256, 0, 137.0, 960);
        let expected = ctx.beats_to_samples(SyncDivision::Eighth.beats());
        let mut probe = make();
        probe.set_parameter(PARAM_DIVISION, SyncDivision::Eighth.as_u32() as f32);
        assert!((delay_for(&mut probe, &ctx) - expected).abs() < 1e-2);
        // A context with no usable tempo must not produce zero (no delay at
        // all) or an infinity.
        let broken = RenderContext::new(SR, 256, 0, 0.0, 960);
        let fallback = delay_for(&mut probe, &broken);
        assert!(fallback.is_finite() && fallback > 0.0, "got {fallback}");
    }

    #[test]
    fn an_impulse_reappears_after_exactly_one_delay_period() {
        let mut effect = plain(20.0);
        let mut input = alloc::vec![0.0_f32; 3_840];
        input[0] = 1.0;
        let out = render(&mut effect, &input, 0, 120.0);

        assert!(out[0].abs() < 1e-6, "dry must be gone at 100% wet");
        let expected = 960;
        assert!(
            out[expected].abs() > 0.9,
            "echo missing at {expected}: {}",
            out[expected]
        );
        // Nothing at all in between: a delay must not smear its input.
        for (index, sample) in out.iter().enumerate().take(expected).skip(1) {
            assert!(sample.abs() < 1e-6, "leak at {index}: {sample}");
        }
        // And nothing after, with no feedback.
        for (index, sample) in out.iter().enumerate().skip(expected + 1) {
            assert!(sample.abs() < 1e-6, "spurious tail at {index}: {sample}");
        }
    }

    #[test]
    fn the_delay_is_exact_at_an_awkward_sample_rate() {
        // 44.1 kHz is where an integer-only or rounding delay shows up as a
        // missed grid position. The effect is driven block by block, because
        // `prepare` sized the scratch for 256 frames and a single 8 820-frame
        // call would (correctly) be refused.
        let mut effect = SyncDelay::new(ParameterAddress::effect(0, 0, PARAM_SYNC));
        effect.prepare(44_100.0, 256, 2);
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, MAX_DAMP_HZ);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, MIN_DAMP_HZ);
        effect.set_parameter(PARAM_MIX, 100.0);
        let ctx = RenderContext::new(44_100.0, 256, 0, 120.0, 960);
        assert!((effect.delay_samples_for(&ctx) - 4_410.0).abs() < 1.0);

        let total = 8_820;
        let mut impulse = alloc::vec![0.0_f32; total];
        impulse[0] = 1.0;
        let mut out: Vec<f32> = Vec::with_capacity(total);
        let mut offset = 0;
        while offset < total {
            let frames = 256.min(total - offset);
            let mut left = alloc::vec![0.0_f32; frames];
            let mut right = alloc::vec![0.0_f32; frames];
            left.copy_from_slice(&impulse[offset..offset + frames]);
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(44_100.0, frames, offset as i64, 120.0, 960);
                effect.process(&mut buffer, &ctx);
            }
            out.extend_from_slice(&left);
            offset += frames;
        }
        assert!(out[4_410].abs() > 0.9, "echo at {}", out[4_410]);
        // And it is exactly at 4 410: not one sample either side.
        assert!(out[4_409].abs() < 1e-6, "leak at 4409: {}", out[4_409]);
        assert!(out[4_411].abs() < 1e-6, "leak at 4411: {}", out[4_411]);
    }

    #[test]
    fn an_impulse_on_the_left_only_reaches_the_left_when_ping_pong_is_off() {
        // The baseline the ping-pong tests compare against: with the routing
        // off, a silent right channel stays silent.
        let mut effect = plain(20.0);
        effect.set_parameter(PARAM_FEEDBACK, 60.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::Off.as_u32() as f32);
        let input = {
            let mut v = alloc::vec![0.0_f32; 3_840];
            v[0] = 1.0;
            v
        };
        let (left, right) = render_left_impulse(&mut effect, &input, 120.0);
        assert!(left[960].abs() > 0.5, "no straight repeat: {}", left[960]);
        assert!(
            right.iter().all(|s| s.abs() < 1e-6),
            "the silent channel leaked into"
        );
    }

    #[test]
    fn ping_pong_moves_an_impulse_from_left_to_right_one_period_later() {
        let mut effect = plain(20.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::LeftToRight.as_u32() as f32);

        let period = 960;
        let input = {
            let mut v = alloc::vec![0.0_f32; period * 3];
            v[0] = 1.0;
            v
        };
        let (left, right) = render_left_impulse(&mut effect, &input, 120.0);

        assert!(left[0].abs() < 1e-6, "dry leaked through: {}", left[0]);
        assert!(
            left[period].abs() < 1e-6,
            "with ping-pong on, the left repeat must not stay on the left: {}",
            left[period]
        );
        assert!(
            right[period].abs() > 0.5,
            "the repeat must arrive on the right one period later: {}",
            right[period]
        );
        assert!(right.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn ping_pong_actually_alternates_channels() {
        // The echo must bounce: left, right, left, right. Each hop costs one
        // delay period, so the *same* channel repeats every two periods, and
        // between them the other channel carries it. A one-shot channel swap
        // would put the echo on the right and leave it there.
        let mut effect = plain(20.0);
        effect.set_parameter(PARAM_FEEDBACK, 60.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::LeftToRight.as_u32() as f32);

        let period = 960;
        let input = {
            let mut v = alloc::vec![0.0_f32; period * 5];
            v[0] = 1.0;
            v
        };
        let (left, right) = render_left_impulse(&mut effect, &input, 120.0);

        // Hop 1: left -> right.
        assert!(
            left[period].abs() < 1e-6,
            "the left still has its own first repeat under ping-pong: {}",
            left[period]
        );
        assert!(
            right[period].abs() > 0.4,
            "the repeat did not cross to the right: {}",
            right[period]
        );
        // Hop 2: right -> left, so the echo is back where it started.
        assert!(
            right[period * 2].abs() < 1e-6,
            "the right kept a repeat instead of passing it back: {}",
            right[period * 2]
        );
        assert!(
            left[period * 2].abs() > 0.1,
            "the repeat did not come back to the left: {}",
            left[period * 2]
        );
        // Hop 3: left -> right again, quieter because the feedback loop has
        // turned twice.
        assert!(right[period * 3].abs() > 0.0, "the third hop is missing");
        assert!(
            right[period * 3].abs() < right[period].abs(),
            "the bouncing repeat is not decaying: {} then {}",
            right[period],
            right[period * 3]
        );
        // The two channels must never both carry a repeat at the same hop.
        for hop in 1..=3 {
            let l = left[period * hop].abs();
            let r = right[period * hop].abs();
            assert!(
                l < 1e-6 || r < 1e-6,
                "hop {hop} landed on both channels: {l} and {r}"
            );
        }
    }

    #[test]
    fn right_to_left_ping_pong_mirrors_left_to_right() {
        // The two directions must be exact mirrors, or a user picking "R to L"
        // gets a different amount of cross-feed than "L to R". The impulse is
        // on the right this time, so the mirror of the left-to-right test is a
        // repeat on the left one period later.
        let period = 960;
        let impulse = {
            let mut v = alloc::vec![0.0_f32; period * 3];
            v[0] = 1.0;
            v
        };
        // The impulse goes to the right channel only.
        let silence = alloc::vec![0.0_f32; impulse.len()];

        for direction in [PingPong::RightToLeft, PingPong::LeftToRight] {
            let mut effect = plain(20.0);
            effect.set_parameter(PARAM_PING_PONG, direction.as_u32() as f32);
            effect.set_parameter(PARAM_FEEDBACK, 60.0);
            // R-to-L is fed on the right; L-to-R is fed on the left. The two
            // renders must then be channel-swapped copies of one another.
            let (left, right) = if direction == PingPong::RightToLeft {
                render_channels(&mut effect, &silence, &impulse, 120.0)
            } else {
                render_channels(&mut effect, &impulse, &silence, 120.0)
            };
            let (source, destination) = if direction == PingPong::RightToLeft {
                (right[period], left[period])
            } else {
                (left[period], right[period])
            };
            assert!(
                source.abs() < 1e-6,
                "{direction:?}: the source channel kept its own repeat: {source}"
            );
            assert!(
                destination.abs() > 0.5,
                "{direction:?}: the repeat did not cross: {destination}"
            );
        }
    }

    #[test]
    fn a_fractional_read_interpolates_between_its_two_neighbours() {
        // Linear interpolation, stated directly: reading 1.5 samples back must
        // give the midpoint of the taps at 1 and at 2. The ramp makes the two
        // taps distinguishable, so a read that landed on the wrong neighbour
        // would be caught rather than looking plausible.
        let mut ring = DelayLine::new();
        ring.prepare(64);
        // Written at t = 0, 1, 2, 3 with values 0, 1, 2, 3.
        for sample in [0.0_f32, 1.0, 2.0, 3.0] {
            ring.write_sample(sample);
        }
        // The cursor is now at 4, so a delay of `d` returns the sample written
        // at `4 - d`: 3.0 at d=1, 2.0 at d=2, and the midpoint 2.5 at d=1.5.
        assert!((ring.read(1.0) - 3.0).abs() < 1e-6, "{}", ring.read(1.0));
        assert!((ring.read(2.0) - 2.0).abs() < 1e-6, "{}", ring.read(2.0));
        assert!((ring.read(1.5) - 2.5).abs() < 1e-6, "{}", ring.read(1.5));
        assert!((ring.read(1.25) - 2.75).abs() < 1e-6, "{}", ring.read(1.25));
    }

    #[test]
    fn reading_exactly_at_a_whole_sample_hits_that_sample() {
        // The interpolation is only correct if the endpoints are: a read that
        // was off by one sample would land on the neighbouring tap and the
        // fractional case above could still look plausible.
        let mut ring = DelayLine::new();
        ring.prepare(8);
        for n in 0..6 {
            ring.write_sample(n as f32);
        }
        // The cursor is at 6, so the taps at 1..=6 back are 5, 4, 3, 2, 1, 0.
        for back in 1..=6 {
            let expected = (6 - back) as f32;
            let got = ring.read(back as f32);
            assert!((got - expected).abs() < 1e-6, "read({back}) = {got}");
        }
        // Past the ring, the read clamps rather than wrapping to garbage.
        let clamped = ring.read(1_000.0);
        assert!(clamped.is_finite());
    }

    #[test]
    fn a_fractional_delay_smooths_a_slow_input_rather_than_stepping_it() {
        // The observable consequence of interpolation: an integer-only read
        // rounds the tempo-derived length to the nearest sample, which shows up
        // as a staircase in the output.
        let mut effect = plain(333.5 / SR * 1_000.0);
        assert!((delay_for(&mut effect, &RenderContext::default()) - 333.5).abs() < 0.01);

        // A slow sine, so the input's own largest step is small and any
        // interpolation artefact stands out.
        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * 100.0 * n as f32 / SR) * 0.5)
            .collect();
        let input_step = input
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        let out = render(&mut effect, &input, 0, 120.0);
        // Measure well past the first repeat so the interpolation is the only
        // thing under test.
        let out_step = out[1_000..]
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(
            out_step < input_step * 1.5 + 1e-3,
            "fractional delay stepped by {out_step} for an input step of {input_step}"
        );
    }

    #[test]
    fn a_tempo_change_moves_the_echo_without_a_discontinuity() {
        // The failure mode a non-interpolating delay shows is a click when the
        // tempo moves: the length snaps to a whole sample and the read jumps.
        // The tempo here is ramped slowly -- a realistic automation curve, one
        // BPM per block -- so the output must stay continuous and bounded.
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        let chunk = 256;
        let mut previous = 0.0_f32;
        let mut largest_jump = 0.0_f32;
        let mut amplitude = 0.0_f32;
        for block in 0..200 {
            // 60 BPM up to 260 BPM over the render, one BPM per block.
            let bpm = 60.0 + block as f32;
            let mut left = alloc::vec![0.0_f32; chunk];
            let mut right = alloc::vec![0.0_f32; chunk];
            for (i, sample) in left.iter_mut().enumerate() {
                *sample = sin_poly(2.0 * PI * 220.0 * (block * chunk + i) as f32 / SR) * 0.5;
            }
            right.copy_from_slice(&left);
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, (block * chunk) as i64, bpm, 960);
                effect.process(&mut buffer, &ctx);
            }
            for sample in &left {
                assert!(sample.is_finite(), "non-finite sample during a tempo sweep");
                amplitude = amplitude.max(sample.abs());
                largest_jump = largest_jump.max((sample - previous).abs());
                previous = *sample;
            }
        }
        assert!(amplitude > 0.1, "the sweep produced no signal at all");
        // A 220 Hz sine at 48 kHz steps by at most ~0.015 between samples; a
        // delayed copy of it cannot legitimately step by a large fraction of
        // the signal's own amplitude.
        assert!(
            largest_jump < amplitude * 0.25,
            "a tempo change produced a {largest_jump}-sample jump on an amplitude of {amplitude}"
        );
    }

    #[test]
    fn maximum_feedback_decays_and_never_grows() {
        // The stability requirement: an impulse into a delay at maximum
        // feedback with damping engaged must decay. If the loop gain reached
        // unity the tail would be flat; above it, the tail would grow.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 4_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 100.0);
        effect.set_parameter(PARAM_MIX, 100.0);

        let input = {
            // Long enough for several repeats to be judged: ten periods of the
            // 20 ms delay below.
            let mut v = alloc::vec![0.0_f32; 9_600];
            v[0] = 1.0;
            v
        };
        let out = render(&mut effect, &input, 0, 120.0);
        // Collect the peak of each successive period.
        let period = 960;
        let mut peaks = Vec::new();
        let mut index = 0;
        while index + period <= out.len() {
            peaks.push(peak(&out[index..index + period]));
            index += period;
        }
        assert!(peaks.len() >= 3, "not enough repeats to judge: {peaks:?}");
        // The block at index 0 carries the impulse itself, which a fully wet
        // delay does not pass; the first *repeat* is peaks[1].
        assert!(peaks[1] > 0.5, "the first repeat is missing: {peaks:?}");
        for window in peaks[1..].windows(2) {
            assert!(window[1] < window[0] + 1e-6, "the tail grew: {peaks:?}");
        }
        for (i, sample) in out.iter().enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
            assert!(sample.abs() <= 2.0, "sample {i} exploded to {sample}");
        }
    }

    #[test]
    fn feedback_at_maximum_is_strictly_below_unity() {
        // A loop gain of exactly 1 is marginally stable; anything above it
        // diverges. The ceiling is what makes the "never grows" test above a
        // property of the design rather than of one set of settings.
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        assert!(effect.feedback_gain() < 1.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        assert_eq!(effect.feedback_gain(), 0.0);
        // Even an out-of-range value cannot push it past the ceiling.
        effect.set_parameter(PARAM_FEEDBACK, 1_000.0);
        let ceiling = effect.feedback_gain();
        assert!(ceiling < 1.0);
        assert!((ceiling - USER_MAX_FEEDBACK).abs() < 1e-6, "got {ceiling}");
        // `USER_MAX_FEEDBACK < MAX_FEEDBACK_CEILING` is asserted at compile time
        // by the `const _` item above rather than here: both are constants, so
        // a runtime check of them could only ever be a constant `true`.
    }

    #[test]
    fn a_long_tail_at_full_feedback_stays_finite_over_many_blocks() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_TIME_MS, 5.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 300.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, MIN_DAMP_HZ);
        let input = {
            // The impulse, then enough silence for the tail to be measured
            // rather than truncated by the end of the render.
            let mut v = alloc::vec![0.0_f32; 24_000];
            v[0] = 1.0;
            v
        };
        let out = render(&mut effect, &input, 0, 120.0);
        // The impulse's own repeats must be decaying, not merely finite.
        // The first window excludes the impulse itself: at full wet the dry
        // signal is not passed, so the impulse proper is the first repeat.
        let early = peak(&out[240..2_000]);
        let late = peak(&out[2_000..]);
        assert!(early > 0.0, "the impulse produced no echo at all");
        assert!(late <= early + 1e-6, "the tail grew: {early} then {late}");
        assert!(out.iter().all(|s| s.is_finite()));

        // Feed a tone afterwards (the worst case for a resonant loop) and
        // assert nothing leaves the rails.
        for block in 0..200 {
            let mut left: Vec<f32> = (0..256)
                .map(|n| sin_poly(2.0 * PI * 1_000.0 * (block * 256 + n) as f32 / SR))
                .collect();
            let mut right = left.clone();
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
            }
            for sample in left.iter().chain(right.iter()) {
                assert!(sample.is_finite());
                assert!(sample.abs() < 100.0, "resonant tail reached {sample}");
            }
        }
    }

    #[test]
    fn every_parameter_at_its_extreme_stays_finite() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, f32::MAX);
        }
        assert!(effect.feedback_gain() < 1.0);
        for block in 0..40 {
            let mut left = alloc::vec![0.9_f32; 256];
            let mut right = alloc::vec![-0.9_f32; 256];
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, 256, (block * 256) as i64, 120.0, 960);
                effect.process(&mut buffer, &ctx);
            }
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
                assert!(sample.abs() < 1e3, "block {block} sample {i} = {sample}");
            }
        }
        // And with every parameter at its minimum.
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, f32::MIN);
        }
        let mut left = alloc::vec![0.5_f32; 256];
        let mut right = alloc::vec![0.5_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::default());
        }
        assert!(left.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn non_finite_input_never_leaves_non_finite_state() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 90.0);
        let mut input = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        input.extend_from_slice(&[0.0; 251]);
        let out = render(&mut effect, &input, 0, 120.0);
        for (i, sample) in out.iter().enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        // The poison must not survive into the next block either.
        let out = render(&mut effect, &alloc::vec![0.0_f32; 512], 0, 120.0);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 0.0);
        effect.set_wet(0.0);
        let input: Vec<f32> = (0..512)
            .map(|n| sin_poly(2.0 * PI * 440.0 * n as f32 / SR) * 0.5)
            .collect();
        let out = render(&mut effect, &input, 0, 120.0);
        for (i, (got, want)) in out.iter().zip(input.iter()).enumerate() {
            assert!((got - want).abs() < 1e-6, "sample {i}: {got} vs {want}");
        }
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut left = alloc::vec![0.75_f32; 256];
        left[7] = -0.125;
        let mut right = alloc::vec![-0.25_f32; 256];
        let expected_left = left.clone();
        let expected_right = right.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[test]
    fn reset_clears_the_delay_lines() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 90.0);
        let input = {
            let mut v = alloc::vec![0.0_f32; 256];
            v[0] = 1.0;
            v
        };
        let _ = render(&mut effect, &input, 0, 120.0);
        assert!(
            effect.lines[0].ring.iter().any(|s| s.abs() > 0.0),
            "the line should hold something before a reset"
        );
        effect.reset();
        assert!(effect.lines[0].ring.iter().all(|s| *s == 0.0));
        assert_eq!(effect.lines[0].write, 0);
        assert_eq!(effect.damping[0].lowpass, 0.0);
        // Silence in, silence out.
        let out = render(&mut effect, &alloc::vec![0.0_f32; 256], 0, 120.0);
        assert!(out.iter().all(|s| s.abs() == 0.0));
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        // Prepare sized for 256; a 512-frame block must pass through untouched
        // instead of indexing past the scratch.
        let mut left = alloc::vec![0.5_f32; 512];
        let mut right = alloc::vec![0.25_f32; 512];
        let expected_left = left.clone();
        let expected_right = right.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        assert_eq!(left, expected_left, "oversized block must be a safe no-op");
        assert_eq!(right, expected_right);
    }

    #[test]
    fn the_ring_covers_the_longest_combination_the_parameters_allow() {
        // The longest musical division at the slowest supported tempo, plus the
        // spread offset, must fit; otherwise a legitimate setting would silently
        // shorten the echo.
        for rate in [44_100.0_f32, 48_000.0, 96_000.0] {
            let capacity = SyncDelay::required_capacity(rate, 256) as f32;
            let worst_tempo = rate * 60.0 / MIN_SUPPORTED_BPM * MAX_DIVISION_BEATS;
            let worst_spread = rate * SPREAD_MAX_MS / 1_000.0;
            assert!(
                capacity >= worst_tempo + worst_spread,
                "at {rate} Hz the ring holds {capacity}, needs {}",
                worst_tempo + worst_spread
            );
            assert!(capacity >= rate * MAX_FREE_DELAY_MS / 1_000.0);
            // The longest free time must also fit.
            let mut effect = SyncDelay::new(ParameterAddress::effect(0, 0, PARAM_SYNC));
            effect.prepare(rate, 256, 2);
            effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
            effect.set_parameter(PARAM_TIME_MS, MAX_FREE_DELAY_MS);
            assert!(effect.delay_samples_for(&RenderContext::default()) <= capacity);
        }
    }

    #[test]
    fn a_tempo_slow_enough_to_stretch_the_ring_still_reads_in_range() {
        // 20 BPM with the longest division is the worst case the ring is sized
        // for. The read must clamp rather than walk off the end.
        let mut effect = plain(20.0);
        effect.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        effect.set_parameter(PARAM_DIVISION, SyncDivision::Half.as_u32() as f32);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        let input = {
            let mut v = alloc::vec![0.0_f32; 256];
            v[0] = 1.0;
            v
        };
        let out = render(&mut effect, &input, 0, 20.0);
        assert!(out.iter().all(|s| s.is_finite()));
        let capacity = effect.lines[0].capacity() as f32;
        assert!(
            delay_for(&mut effect, &RenderContext::new(SR, 256, 0, 20.0, 960)) <= capacity,
            "the requested delay is longer than the ring"
        );
    }

    #[test]
    fn stereo_channels_are_processed_independently() {
        // A loud left must not leak into a silent right when ping-pong is off.
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 80.0);
        effect.set_parameter(PARAM_TIME_MS, 10.0);
        let mut left = alloc::vec![0.0_f32; 256];
        left[0] = 1.0;
        let mut right = alloc::vec![0.0_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert!(
            right.iter().all(|s| *s == 0.0),
            "the silent channel picked up the loud one"
        );
        let mut left = alloc::vec![0.0_f32; 256];
        let mut right = alloc::vec![0.0_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 256, 120.0, 960));
        }
        assert!(right.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn the_spread_control_lengthens_one_channel_only() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 100.0);
        effect.set_parameter(PARAM_SPREAD, 100.0);
        let ctx = RenderContext::new(SR, 256, 0, 120.0, 960);
        let mut left = alloc::vec![0.0_f32; 256];
        let mut right = alloc::vec![0.0_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &ctx);
        }
        assert!((effect.last_delay_left - 4_800.0).abs() < 1.0);
        assert!(
            effect.last_delay_right > effect.last_delay_left,
            "spread must delay the right channel further: {} vs {}",
            effect.last_delay_right,
            effect.last_delay_left
        );
        assert!(
            effect.last_delay_right <= effect.lines[1].capacity() as f32,
            "the spread pushed the read past the ring"
        );
    }

    #[test]
    fn tail_seconds_reflects_the_feedback_decay() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 100.0);
        // Settle the delay length so the tail has a period to work with.
        let _ = render(&mut effect, &alloc::vec![0.0_f32; 256], 0, 120.0);

        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        let short = effect.tail_seconds();
        effect.set_parameter(PARAM_FEEDBACK, 90.0);
        let long = effect.tail_seconds();
        assert!(
            long > short * 4.0,
            "more feedback must mean a longer tail: {long} vs {short}"
        );
        assert!(
            (short - 0.1).abs() < 0.02,
            "one repeat is 0.1 s, got {short}"
        );
        assert!(long <= MAX_TAIL_SECONDS);
    }

    #[test]
    fn latency_is_zero_because_the_dry_path_is_undelayed() {
        // The echo is the effect's audible content; the dry path carries no
        // look-ahead, so PDC has nothing to compensate.
        let effect = make();
        assert_eq!(effect.latency_samples(), 0);
    }

    #[test]
    fn wet_is_clamped_and_nan_safe() {
        let mut effect = make();
        effect.set_wet(5.0);
        assert_eq!(effect.wet(), 1.0);
        effect.set_wet(f32::NAN);
        assert_eq!(effect.wet(), 1.0);
        effect.set_wet(-1.0);
        assert_eq!(effect.wet(), 0.0);
        assert_eq!(effect.get_parameter(PARAM_MIX), Some(0.0));
    }

    #[test]
    fn the_damping_low_pass_darkens_successive_repeats() {
        // Damping shapes the *feedback*, so it is the later repeats that get
        // darker -- the first repeat is the raw tap, and with a sustained input
        // every repeat also contains the (undamped) current input. The clean
        // measurement is therefore a *burst*: play a short tone, stop, and
        // compare the level of the burst's later repeats for a high tone
        // against a low one through the same 1 kHz low-pass.
        let repeat_of = |tone: f32| {
            let mut effect = plain(10.0);
            effect.set_parameter(PARAM_FEEDBACK, 80.0);
            effect.set_parameter(PARAM_DAMP_LOWPASS, 1_000.0);
            // A 10 ms burst of the tone, then silence so only the repeats
            // remain in the measurement window.
            let mut input = alloc::vec![0.0_f32; 24_000];
            for (n, sample) in input.iter_mut().enumerate().take(480) {
                *sample = sin_poly(2.0 * PI * tone * n as f32 / SR) * 0.5;
            }
            let out = render(&mut effect, &input, 0, 120.0);
            // The delay is 10 ms (480 samples), so the second repeat of the
            // burst occupies 960..1440 and the third 1440..1920.
            peak(&out[960..1_920])
        };
        let high_echo = repeat_of(8_000.0);
        let low_echo = repeat_of(200.0);
        assert!(
            high_echo < low_echo * 0.6,
            "the 1 kHz damping should strip later 8 kHz repeats: {high_echo} vs {low_echo}"
        );
        assert!(low_echo > 0.01, "the low tone produced no repeats at all");
    }

    #[test]
    fn a_bright_setting_keeps_the_repeats_bright() {
        // The converse of the previous test: with damping wide open the repeat
        // is essentially the input, which pins the low-pass as the cause.
        let mut effect = plain(20.0);
        let input: Vec<f32> = (0..4_000)
            .map(|n| sin_poly(2.0 * PI * 1_000.0 * n as f32 / SR) * 0.5)
            .collect();
        let out = render(&mut effect, &input, 0, 120.0);
        let repeat = peak(&out[960..1_920]);
        assert!(
            (repeat - 0.5).abs() < 0.05,
            "an undamped repeat should be the input, got {repeat}"
        );
    }

    #[test]
    fn an_impulse_response_does_not_smear_the_signal_when_damping_is_off() {
        // With a 20 kHz low-pass and a 20 Hz high-pass the loop is a wire, so
        // each repeat is exactly the input scaled by the feedback gain.
        let mut effect = plain(10.0);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        let input = {
            let mut v = alloc::vec![0.0_f32; 2_048];
            v[0] = 1.0;
            v
        };
        let out = render(&mut effect, &input, 0, 120.0);
        let period = 480;
        assert!(
            (out[period].abs() - 1.0).abs() < 0.02,
            "first repeat is {} not ~1.0",
            out[period]
        );
        // 50% feedback of the 0.90 user ceiling is a 0.45 loop gain.
        assert!(
            (out[period * 2].abs() - 0.45).abs() < 0.02,
            "second repeat is {} not ~0.45",
            out[period * 2]
        );
    }

    #[test]
    fn an_empty_block_and_a_one_sample_block_are_both_safe() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        {
            let mut buffer = AudioBuffer::new(&mut empty);
            effect.process(&mut buffer, &RenderContext::default());
        }
        // A single-sample block is the other edge case.
        let mut left = alloc::vec![1.0_f32; 1];
        let mut right = alloc::vec![0.0_f32; 1];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 1, 0, 120.0, 960));
        }
        assert!(left[0].is_finite());
    }

    #[test]
    fn a_one_channel_block_is_safe() {
        // The engine may hand an effect a mono block; the ping-pong routing
        // must not index a channel that is not there.
        let mut effect = plain(20.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::LeftToRight.as_u32() as f32);
        let mut only_in = alloc::vec![0.0_f32; 2_048];
        only_in[0] = 1.0;
        let mut only = only_in.clone();
        {
            let mut views = [&mut only[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(
                &mut buffer,
                &RenderContext::new(SR, only_in.len(), 0, 120.0, 960),
            );
        }
        assert!(only.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_ping_pong_target_map_is_a_swap() {
        // The routing table is the whole ping-pong mechanism; assert it
        // directly so a regression there is not hidden behind an audio test.
        assert_eq!(PingPong::Off.target(0), 0);
        assert_eq!(PingPong::Off.target(1), 1);
        assert_eq!(PingPong::LeftToRight.target(0), 1);
        assert_eq!(PingPong::LeftToRight.target(1), 0);
        assert_eq!(PingPong::RightToLeft.target(0), 1);
        assert_eq!(PingPong::RightToLeft.target(1), 0);
        // An out-of-range channel must fold rather than index past the array.
        assert_eq!(PingPong::LeftToRight.target(9), 0);
        assert!(!PingPong::Off.is_crossed());
        assert!(PingPong::LeftToRight.is_crossed());
        assert!(PingPong::RightToLeft.is_crossed());
    }
}
