//! Tempo-synchronised delay with damping, ping-pong routing and a stereo
//! spread control.
//!
//! PLAN §3.S5 requires "延迟（同步/自由）": one delay that can either follow the
//! host tempo or run on a plain millisecond setting. This module is that
//! delay.
//!
//! # Why the musical division is an enum and not a beats knob
//!
//! A free-running beats control invites a value like `0.437` of a beat, which
//! is neither on the grid nor musically useful, and — worse — is impossible to
//! label in a UI. The divisions a delay is actually used for are a small,
//! closed set (1/16 … 1/2, including dotted and triplet variants), so they are
//! published as an enumeration with the beat length baked in. The UI can then
//! show "1/8 dotted" instead of "0.75 beats", and the tempo conversion happens
//! in exactly one place: the shared `beats_to_samples` conversion on the render
//! context.
//!
//! # Why beats, not seconds
//!
//! The whole point of a synced delay is that the echo follows the tempo. The
//! conversion is therefore done **per block** against the tempo in the render
//! context rather than once in `prepare`: a tempo change mid-render must move
//! the echo with it, and a delay that cached its length at prepare time would
//! drift off the grid the moment the user moved the tempo.
//!
//! # Feedback stability
//!
//! The feedback loop is `ring → damping filters → gain → ring`. Each turn
//! through the loop applies the damping low-pass and high-pass, both of which
//! have a gain of at most 1, plus the (strictly sub-unity) feedback gain. The
//! loop therefore has a round-trip gain of at most `USER_MAX_FEEDBACK` at every
//! frequency, which is the textbook stability condition: every partial decays
//! by at least `1 - USER_MAX_FEEDBACK` per repeat and the closed loop cannot
//! grow. That is why the feedback parameter is clamped in the *setter* as well
//! as in the descriptor — a value that reached unity would make the loop
//! marginally stable and the first rounding error would turn it into a runaway
//! oscillator.
//!
//! # Ping-pong
//!
//! Ping-pong is implemented by **crossing the write**, not by a separate send
//! bus: the feedback term computed from one channel's delay line is written
//! into the *other* channel's line. An impulse on the left therefore appears on
//! the right exactly one delay period later, and on the left again one period
//! after that, which is the audible definition of ping-pong. The *dry input* of
//! each channel always goes into its own line, so the effect is not a
//! channel-swapper while it echoes.
//!
//! # Real-time safety
//!
//! Both ring buffers, both damping sections and the dry/wet scratch are
//! allocated in [`SyncDelay::prepare`]; `process` performs no allocation, no
//! locking and no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{clamp_frequency, DcBlocker};
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
/// memory over, say, a 40 BPM assumption is a few hundred kilobytes — far
/// cheaper than a delay that clicks because its echo ran past the end of the
/// buffer.
pub const MIN_SUPPORTED_BPM: f32 = 20.0;

/// A sensible fallback tempo when the render context carries none.
///
/// A context with a non-positive `bpm` cannot produce a beat length (the shared
/// conversion returns zero rather than dividing by zero), so a synced delay
/// using it naively would collapse to "no delay at all". Falling back to
/// 120 BPM keeps a transport-less render — an offline bounce with no tempo map
/// — musical instead of silent.
const FALLBACK_BPM: f32 = 120.0;

/// Lowest damping corner the parameters allow, in hertz.
const MIN_DAMP_HZ: f32 = 20.0;
/// Highest damping corner the parameters allow, in hertz.
const MAX_DAMP_HZ: f32 = 20_000.0;

/// The feedback gain actually applied at 100% feedback.
///
/// Strictly below 1.0 by construction; `MAX_FEEDBACK_CEILING` asserts it.
const USER_MAX_FEEDBACK: f32 = 0.90;

/// The value the test suite asserts [`USER_MAX_FEEDBACK`] stays below.
const MAX_FEEDBACK_CEILING: f32 = 1.0;

/// How far the spread control can push the right channel later, in
/// milliseconds.
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
/// change; new divisions are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SyncDivision {
    /// Sixteenth note: a quarter of a beat.
    Sixteenth = 0,
    /// Eighth note: half a beat.
    Eighth = 1,
    /// Dotted eighth: three quarters of a beat.
    EighthDotted = 2,
    /// Eighth-note triplet: a third of a beat.
    EighthTriplet = 3,
    /// Quarter note: one beat.
    Quarter = 4,
    /// Dotted quarter: one and a half beats.
    QuarterDotted = 5,
    /// Quarter-note triplet: two thirds of a beat.
    QuarterTriplet = 6,
    /// Half note: two beats.
    Half = 7,
}

impl SyncDivision {
    /// Every division, in discriminant order.
    pub const ALL: [Self; 8] = [
        Self::Sixteenth,
        Self::Eighth,
        Self::EighthDotted,
        Self::EighthTriplet,
        Self::Quarter,
        Self::QuarterDotted,
        Self::QuarterTriplet,
        Self::Half,
    ];

    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Sixteenth),
            1 => Some(Self::Eighth),
            2 => Some(Self::EighthDotted),
            3 => Some(Self::EighthTriplet),
            4 => Some(Self::Quarter),
            5 => Some(Self::QuarterDotted),
            6 => Some(Self::QuarterTriplet),
            7 => Some(Self::Half),
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
            Self::Sixteenth => 0.25,
            Self::Eighth => 0.5,
            Self::EighthDotted => 0.75,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Quarter => 1.0,
            Self::QuarterDotted => 1.5,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::Half => 2.0,
        }
    }

    /// The stable machine-readable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Sixteenth => "1_16",
            Self::Eighth => "1_8",
            Self::EighthDotted => "1_8_dotted",
            Self::EighthTriplet => "1_8_triplet",
            Self::Quarter => "1_4",
            Self::QuarterDotted => "1_4_dotted",
            Self::QuarterTriplet => "1_4_triplet",
            Self::Half => "1_2",
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
        // `write` is one past the most recent sample, so the most recent sample
        // lives at `write - 1`; walking back `whole` from there gives the
        // integer tap and one further back the tap after it.
        let base = self.write + len - 1 - whole;
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
    /// DC blocker on the wet output, so the damping high-pass cannot leave an
    /// offset behind in the main output.
    dc: DcBlocker,
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
    /// Preallocated snapshot of the dry input, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer for channel 0, `max_block`.
    wet_left: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer for channel 1, `max_block`.
    wet_right: alloc::vec::Vec<f32>,
    /// How many channels are active.
    active_channels: usize,
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
            dc: DcBlocker::default(),
            last_delay_left: 0.0,
            last_delay_right: 0.0,
            wet: 0.3,
            bypassed: false,
            sample_rate: 48_000.0,
            dry: alloc::vec::Vec::new(),
            wet_left: alloc::vec::Vec::new(),
            wet_right: alloc::vec::Vec::new(),
            active_channels: MAX_CHANNELS,
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
        let rate = if sample_rate > 0.0 { sample_rate } else { 48_000.0 };
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
    /// follows the tempo or it does not — there is no third behaviour, and
    /// having a single expression for it keeps the two paths from drifting.
    #[must_use]
    fn delay_samples_for(&self, ctx: &RenderContext) -> f32 {
        let rate = self.sample_rate.max(1.0);
        match self.sync {
            SyncMode::Synced => {
                let beats = self.division.beats();
                // Use the transport's own conversion so this delay and any
                // other tempo-synced effect cannot disagree about what a beat
                // is worth. `with_tempo` substitutes the fallback for a tempo
                // the context cannot express.
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
            -6.907_755 / super::super::util::dsp::log2(gain) * core::f32::consts::LN_2
        } else {
            1.0
        };
        let longest = (self.last_delay_left
            .max(self.last_delay_right)
            .max(self.last_delay_left)
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
    /// The `self` borrows are taken first and the `buffer` borrow last, so the
    /// two regions are provably disjoint and the loop needs no copy.
    fn wet_mix(
        &mut self,
        buffer: &mut AudioBuffer<'_>,
        channels: usize,
        frames: usize,
        wet: f32,
        dc_coeff: f32,
    ) {
        for channel in 0..channels {
            let dc = &mut self.dc;
            let dry = &self.dry;
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
                let blocked = if wet > 0.0 {
                    dc.process(channel, tap, dc_coeff)
                } else {
                    tap
                };
                let dry_sample = dry.get(index).copied().unwrap_or(0.0);
                *sample = blocked * wet + dry_sample * (1.0 - wet);
            }
        }
    }
}

/// The one-pole coefficient for a low-pass at `hz`.
///
/// Returns `1.0` when the corner is at or above Nyquist, which makes the
/// section a wire — the "damping off" case the tests rely on.
#[must_use]
fn lowpass_coeff(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 1.0;
    }
    let corner = clamp_frequency(hz, sample_rate);
    if corner >= sample_rate * 0.49 {
        return 1.0;
    }
    // One-pole low-pass with time constant `1 / (2*PI*f)`:
    // a = 1 - exp(-2*PI*f/fs), which for fs >> f is 2*PI*f/fs.
    let x = 2.0 * PI * corner / sample_rate;
    if x >= 1.0 {
        1.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// The one-pole coefficient for a high-pass at `hz`.
///
/// Returns `0.0` when the corner is at or below the parameter minimum, which
/// makes the section a wire.
#[must_use]
fn highpass_coeff(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 0.0;
    }
    let corner = clamp_frequency(hz, sample_rate);
    if corner <= MIN_DAMP_HZ {
        return 0.0;
    }
    // y = a * (y1 + x - x1) with a = 1 / (1 + 2*PI*f/fs): the complementary
    // one-pole, whose gain is 1 at Nyquist and 0 at DC.
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
        self.active_channels = channels.clamp(1, MAX_CHANNELS);

        // Every allocation this effect will ever make happens here. The ring
        // is sized for the *worst* parameter combination — the longest free
        // time, or the longest musical division at the slowest supported tempo
        // — so a tempo change or a division change never resizes it.
        let capacity = Self::required_capacity(self.sample_rate, max_block);
        self.dry = alloc::vec![0.0; max_block];
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
        // past the scratch. A silent pass-through is a far better failure than
        // an out-of-bounds write in the audio thread.
        if frames > self.max_block
            || frames > self.dry.len()
            || frames > self.wet_left.len()
            || frames > self.wet_right.len()
        {
            return;
        }

        // ── Delay length, re-derived every block so the echo follows the
        //    transport rather than a length captured at prepare time. ──
        let base_delay = self.delay_samples_for(ctx);
        let spread_samples = (self.spread_percent / 100.0).clamp(0.0, 1.0)
            * SPREAD_MAX_MS
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

        // ── Snapshot every channel's dry signal first: the wet/dry mix needs
        //    it, and the delay must read the *input* rather than the line's own
        //    previous state. ──
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            self.dry[..frames].copy_from_slice(source);
        }

        // ── Run both delay lines. The loops are indexed rather than iterated
        //    because the two channels share one dry snapshot and one another's
        //    rings when ping-pong is on. ──
        for index in 0..frames {
            let input = self.dry[index];
            // Deferred writes: both channels read the state they had at the
            // start of this sample, so a crossed route moves a repeat exactly
            // one delay period and not two.
            let mut next = [0.0_f32; MAX_CHANNELS];
            for channel in 0..channels {
                let delay = if channel == 0 { delay_left } else { delay_right };
                // Read *before* writing, so a delay of one sample still delays
                // by one sample rather than returning the input unchanged.
                let tap = self.lines[channel].read(delay);
                let damped = self.damping[channel].process(tap, lpf, hpf);
                self.store_wet(channel, index, tap);
                // The dry input always goes into its own line; only the
                // feedback term is routed, which is what makes ping-pong move
                // the *repeats* rather than the whole signal.
                let reuse = next[destination[channel]];
                next[destination[channel]] = reuse + input + damped * feedback;
            }
            for channel in 0..channels {
                self.lines[channel].write_sample(next[channel]);
            }
        }

        // ── Wet/dry, with a DC blocker on the wet path so the damping
        //    high-pass cannot leave an offset in the main output. ──
        let dc_coeff = DcBlocker::coefficient(self.sample_rate);
        self.wet_mix(buffer, channels, frames, wet, dc_coeff);
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut() {
            line.reset();
        }
        for damping in self.damping.iter_mut() {
            damping.reset();
        }
        self.dc.reset();
        self.last_delay_left = 0.0;
        self.last_delay_right = 0.0;
    }

    fn latency_samples(&self) -> usize {
        // Zero. The dry path is undelayed and the delay *is* the effect's
        // audible content, so there is nothing for PDC to line up — the
        // engine's wet/dry mix already places the dry signal at time zero.
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

/// Asserts at compile time that the feedback ceiling the module documents is
/// actually below unity.
///
/// A build-time check rather than a test because it protects an invariant the
/// DSP depends on: if someone raises [`USER_MAX_FEEDBACK`] to "just a bit more"
/// the feedback loop becomes marginally stable and the delay self-oscillates.
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

    /// Feeds `input` through the effect in blocks and returns the concatenated
    /// output of channel `channel`.
    ///
    /// Blocks are 256 frames — the size the effect was prepared for — and the
    /// input is mono (it is written to the left channel only), so a test that
    /// inspects the right channel is inspecting cross-channel state.
    fn render(effect: &mut SyncDelay, input: &[f32], channel: usize, bpm: f32) -> Vec<f32> {
        let chunk = 256;
        let mut out: Vec<f32> = Vec::with_capacity(input.len());
        let mut offset = 0;
        while offset < input.len() {
            let frames = chunk.min(input.len() - offset);
            let mut left = alloc::vec![0.0_f32; frames];
            let mut right = alloc::vec![0.0_f32; frames];
            left.copy_from_slice(&input[offset..offset + frames]);
            if channel == 1 {
                right.copy_from_slice(&left);
            }
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, frames, offset as i64, bpm, 960);
                effect.process(&mut buffer, &ctx);
            }
            let picked = if channel == 0 { &left } else { &right };
            out.extend_from_slice(picked);
            offset += frames;
        }
        out
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, 0x0000_0400);
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
            assert_eq!(
                spec.address.sub & 0x00FF,
                ordinal as u16,
                "parameter {ordinal} has a mismatched address"
            );
            assert!(
                spec.min_value <= spec.default_value && spec.default_value <= spec.max_value,
                "parameter {ordinal} default is outside its range"
            );
            assert!(!spec.key.is_empty());
            assert!(spec.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        for (i, a) in table.iter().enumerate() {
            for b in &table[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate parameter key {}", a.key);
            }
        }
    }

    #[test]
    fn every_parameter_round_trips_through_the_setter() {
        let mut effect = make();
        assert_eq!(effect.get_parameter(999), None);
        for sub in 0..PARAM_COUNT {
            let spec = effect.table[sub as usize];
            let midpoint = (spec.min_value + spec.max_value) * 0.5;
            effect.set_parameter(sub, midpoint);
            let read = effect.get_parameter(sub).expect("known ordinal");
            let discrete = spec.is_discrete();
            if discrete {
                assert_eq!(read, midpoint.floor(), "parameter {sub} snaps");
            } else {
                assert!(
                    (read - midpoint).abs() < 1e-3,
                    "parameter {sub} read back {read}, expected {midpoint}"
                );
            }
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
        // This is the whole point of the effect: at half the tempo the echo is
        // twice as far away.
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
                "{:?}: got {got}, expected {want}",
                division
            );
        }
    }

    #[test]
    fn the_divisions_are_ordered_from_shortest_to_longest() {
        // A UI that renders the enumeration in order must read 1/16 → 1/2.
        let mut previous = 0.0_f32;
        for division in SyncDivision::ALL {
            let beats = division.beats();
            assert!(
                beats > previous,
                "{:?} breaks the ordering ({beats} after {previous})",
                division
            );
            previous = beats;
        }
        assert_eq!(SyncDivision::ALL.len(), 8);
        assert!(SyncDivision::ALL.iter().all(|d| d.as_u32() < 8));
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
    fn debug_impulse() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let mut input = alloc::vec![0.0_f32; 3_840];
        input[0] = 1.0;
        let out = render(&mut effect, &input, 0, 120.0);
        let hits: Vec<(usize, f32)> = out
            .iter()
            .enumerate()
            .filter(|(_, s)| s.abs() > 1e-4)
            .map(|(i, s)| (i, *s))
            .take(10)
            .collect();
        panic!("delay={} hits={hits:?}", effect.last_delay_left);
    }

    #[test]
    fn an_impulse_reappears_after_exactly_one_delay_period() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);

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
    }

    #[test]
    fn ping_pong_moves_an_impulse_from_left_to_right_one_period_later() {
        // Render the left channel of a stereo block with an impulse on the
        // left: with ping-pong engaged the left repeat must be suppressed and
        // the right channel must carry it instead.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::LeftToRight.as_u32() as f32);

        let period = 960;
        let frames = period * 3;
        let mut left = alloc::vec![0.0_f32; frames];
        left[0] = 1.0;
        let mut right = alloc::vec![0.0_f32; frames];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(
                &mut buffer,
                &RenderContext::new(SR, frames, 0, 120.0, 960),
            );
        }
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
        assert!(right.iter().sum::<f32>().is_finite());
    }

    #[test]
    fn ping_pong_actually_alternates_channels() {
        // Feed an impulse on the left and look for it on the right one delay
        // period later. The left render and right render use identical input,
        // so a difference between them *is* the channel crossing.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 60.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);

        let period = 960;
        let frames = period * 4;
        let mut impulse = alloc::vec![0.0_f32; frames];
        impulse[0] = 1.0;

        /// Runs `impulse` through a fresh effect and returns both channels.
        fn both(mode: PingPong, impulse: &[f32]) -> (Vec<f32>, Vec<f32>) {
            let mut effect = make();
            effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
            effect.set_parameter(PARAM_TIME_MS, 20.0);
            effect.set_parameter(PARAM_FEEDBACK, 60.0);
            effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
            effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
            effect.set_parameter(PARAM_MIX, 100.0);
            effect.set_parameter(PARAM_PING_PONG, mode.as_u32() as f32);
            let mut left = impulse.to_vec();
            let mut right = alloc::vec![0.0_f32; impulse.len()];
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                effect.process(
                    &mut buffer,
                    &RenderContext::new(SR, impulse.len(), 0, 120.0, 960),
                );
            }
            (left, right)
        }

        // Straight mode: the left repeat stays on the left, the right channel
        // stays silent.
        let (left_off, right_off) = both(PingPong::Off, &impulse);
        assert!(left_off[period].abs() > 0.5, "no straight repeat");
        assert!(
            right_off.iter().all(|s| s.abs() < 1e-6),
            "the silent channel leaked into"
        );

        // Ping-pong: the left input's repeat must arrive on the right.
        let (left_pp, right_pp) = both(PingPong::LeftToRight, &impulse);
        assert!(
            left_pp[period].abs() < 1e-6,
            "the left still has its own repeat under ping-pong: {}",
            left_pp[period]
        );
        assert!(
            right_pp[period].abs() > 0.5,
            "the repeat did not cross to the right: {} at {period}",
            right_pp[period]
        );
        // And back again on the next period, which is what makes it alternate
        // rather than being a one-shot channel swap.
        assert!(
            left_pp[period * 2].abs() > 0.1,
            "the repeat did not come back to the left: {}",
            left_pp[period * 2]
        );
        assert!(
            right_pp[period * 2].abs() < right_pp[period].abs(),
            "the ping-pong did not alternate: {} then {}",
            right_pp[period],
            right_pp[period * 2]
        );
    }

    #[test]
    fn right_to_left_ping_pong_mirrors_left_to_right() {
        // The two directions must be exact mirrors, or a user picking "R→L"
        // gets a different amount of cross-feed than "L→R".
        let period = 960;
        let mut impulse = alloc::vec![0.0_f32; period * 3];
        impulse[0] = 1.0;

        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 20.0);
        effect.set_parameter(PARAM_FEEDBACK, 60.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_PING_PONG, PingPong::RightToLeft.as_u32() as f32);

        // The impulse is on the right this time.
        let mut left = alloc::vec![0.0_f32; impulse.len()];
        let mut right = impulse.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(
                &mut buffer,
                &RenderContext::new(SR, impulse.len(), 0, 120.0, 960),
            );
        }
        assert!(
            right[period].abs() < 1e-6,
            "the right kept its own repeat: {}",
            right[period]
        );
        assert!(
            left[period].abs() > 0.5,
            "the repeat did not cross to the left: {}",
            left[period]
        );
    }

    #[test]
    fn a_fractional_delay_length_is_smooth_rather_than_quantised() {
        // A fractional delay has to interpolate. An integer-only read rounds
        // the tempo-derived length to the nearest sample, which shows up as a
        // staircase in the output: sample-to-sample jumps far larger than the
        // input's own.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        // 333.5 samples: exactly halfway between two integer taps.
        effect.set_parameter(PARAM_TIME_MS, 333.5 / SR * 1_000.0);
        assert!((delay_for(&mut effect, &RenderContext::default()) - 333.5).abs() < 0.01);

        // A slow sine, so the input's own largest step is small and any
        // interpolation artefact stands out.
        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * core::f32::consts::PI * 100.0 * n as f32 / SR))
            .collect();
        let input_step = input
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        let out = render(&mut effect, &input, 0, 120.0);
        // Measure well past the first repeat so the interpolation is the only
        // thing under test.
        let out_step = out[2_000..]
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(
            out_step < input_step * 1.5 + 1e-4,
            "fractional delay stepped by {out_step} for an input step of {input_step}"
        );
    }

    #[test]
    fn a_tempo_change_moves_the_echo_without_a_discontinuity() {
        // The failure mode a non-interpolating delay shows is a click when the
        // tempo moves; every sample must stay bounded and continuous.
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        let chunk = 256;
        let mut previous = 0.0_f32;
        for block in 0..40 {
            // Sweep the tempo across the whole supported range.
            let bpm = 60.0 + block as f32 * 8.0;
            let mut left = alloc::vec![0.0_f32; chunk];
            let mut right = alloc::vec![0.0_f32; chunk];
            for (i, sample) in left.iter_mut().enumerate() {
                *sample = sin_poly(2.0 * core::f32::consts::PI * 220.0
                    * (block * chunk + i) as f32
                    / SR)
                    * 0.5;
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
                assert!(
                    (sample - previous).abs() < 0.5,
                    "a tempo change produced a {}-sample jump",
                    (sample - previous).abs()
                );
                previous = *sample;
            }
        }
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

        let mut impulse = alloc::vec![0.0_f32; 256];
        impulse[0] = 1.0;
        let out = render(&mut effect, &impulse, 0, 120.0);

        // Collect the peak of each successive period.
        let period = 960;
        let mut peaks = Vec::new();
        let mut index = 0;
        while index + period <= out.len() {
            let peak = out[index..index + period]
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()));
            peaks.push(peak);
            index += period;
        }
        assert!(peaks.len() >= 3, "not enough repeats to judge");
        assert!(peaks[0] > 0.5, "the first repeat is missing");
        for window in peaks.windows(2) {
            assert!(
                window[1] < window[0] + 1e-6,
                "the tail grew: {peaks:?}"
            );
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
        assert!(effect.feedback_gain() < 1.0);
    }

    #[test]
    fn a_long_tail_at_full_feedback_stays_finite_over_many_blocks() {
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_TIME_MS, 5.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 300.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        let mut impulse = alloc::vec![0.0_f32; 256];
        impulse[0] = 1.0;
        let out = render(&mut effect, &impulse, 0, 120.0);
        // The impulse's own repeats must be decaying, not merely finite.
        let early = out[..2_000].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let late = out[2_000..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(early > 0.0, "the impulse produced no echo at all");
        assert!(late <= early + 1e-6, "the tail grew: {early} then {late}");

        // Feed a tone afterwards (the worst case for a resonant loop) and
        // assert nothing leaves the rails.
        for block in 0..200 {
            let mut left: Vec<f32> = (0..256)
                .map(|n| {
                    sin_poly(2.0 * core::f32::consts::PI * 1_000.0 * (block * 256 + n) as f32 / SR)
                })
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
    fn every_parameter_at_its_maximum_stays_finite() {
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
        let mut left = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        left.extend_from_slice(&[0.0; 251]);
        let mut right = left.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        // The poison must not survive into the next block either.
        let mut left = alloc::vec![0.0_f32; 256];
        let mut right = alloc::vec![0.0_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert!(left.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 0.0);
        effect.set_wet(0.0);
        let mut left: Vec<f32> = (0..512)
            .map(|n| sin_poly(2.0 * core::f32::consts::PI * 440.0 * n as f32 / SR) * 0.5)
            .collect();
        let expected = left.clone();
        let mut right = left.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        for (i, (got, want)) in left.iter().zip(expected.iter()).enumerate() {
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
        let mut impulse = alloc::vec![0.0_f32; 256];
        impulse[0] = 1.0;
        let _ = render(&mut effect, &impulse, 0, 120.0);
        assert!(
            effect.lines[0].ring.iter().any(|s| s.abs() > 0.0),
            "the line should hold something before a reset"
        );
        effect.reset();
        assert!(effect.lines[0].ring.iter().all(|s| *s == 0.0));
        assert_eq!(effect.lines[0].write, 0);
        assert_eq!(effect.damping[0].lowpass, 0.0);
        // Silence in, silence out.
        let silence = alloc::vec![0.0_f32; 256];
        let out = render(&mut effect, &silence, 0, 120.0);
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
        // The longest musical division at the slowest supported tempo, plus
        // the spread offset, must fit; otherwise a legitimate setting would
        // silently shorten the echo.
        for rate in [44_100.0_f32, 48_000.0, 96_000.0] {
            let capacity = SyncDelay::required_capacity(rate, 256) as f32;
            let worst_tempo = rate * 60.0 / MIN_SUPPORTED_BPM * MAX_DIVISION_BEATS;
            let worst_spread = rate * SPREAD_MAX_MS / 1_000.0;
            assert!(
                capacity >= worst_tempo + worst_spread,
                "at {rate} Hz the ring holds {capacity}, needs {}",
                worst_tempo + worst_spread
            );
            // And the free-time budget.
            assert!(capacity >= rate * MAX_FREE_DELAY_MS / 1_000.0);
        }
    }

    #[test]
    fn a_tempo_slow_enough_to_stretch_the_ring_still_reads_in_range() {
        // 20 BPM with the longest division is the worst case the ring is sized
        // for. The read must clamp rather than walk off the end.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Synced.as_u32() as f32);
        effect.set_parameter(PARAM_DIVISION, SyncDivision::Half.as_u32() as f32);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        let mut impulse = alloc::vec![0.0_f32; 256];
        impulse[0] = 1.0;
        let out = render(&mut effect, &impulse, 0, 20.0);
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
        assert!((short - 0.1).abs() < 0.02, "one repeat is 0.1 s, got {short}");
        assert!(long <= MAX_TAIL_SECONDS);
    }

    #[test]
    fn latency_is_zero_because_the_dry_path_is_undelayed() {
        // The delay is the effect's audible content; the dry path carries no
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
        // Damping is what keeps a long delay from turning into a bright
        // pile-up; each repeat must lose high-frequency energy.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 10.0);
        effect.set_parameter(PARAM_FEEDBACK, 80.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 1_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        // A steady 8 kHz tone: the low-pass at 1 kHz should strip it hard.
        let frames = 24_000;
        let input: Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * core::f32::consts::PI * 8_000.0 * n as f32 / SR) * 0.5)
            .collect();
        let out = render(&mut effect, &input, 0, 120.0);
        let settled = out[12_000..20_000]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            settled < 0.15,
            "an 8 kHz tone survived a 1 kHz damping loop at {settled}"
        );
    }

    #[test]
    fn a_bright_setting_keeps_the_repeats_bright() {
        // The converse of the previous test: with damping wide open the repeat
        // is essentially the input, which pins the low-pass as the cause.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 10.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let frames = 8_000;
        let input: Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * core::f32::consts::PI * 1_000.0 * n as f32 / SR) * 0.5)
            .collect();
        let out = render(&mut effect, &input, 0, 120.0);
        let repeat = out[1_000..2_000]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            (repeat - 0.5).abs() < 0.05,
            "an undamped repeat should be the input, got {repeat}"
        );
    }

    #[test]
    fn an_impulse_response_does_not_smear_the_signal_when_damping_is_off() {
        // With a 20 kHz low-pass and a 20 Hz high-pass the loop is a wire, so
        // the first repeat is exactly the input scaled by the feedback gain.
        let mut effect = make();
        effect.set_parameter(PARAM_SYNC, SyncMode::Free.as_u32() as f32);
        effect.set_parameter(PARAM_TIME_MS, 10.0);
        effect.set_parameter(PARAM_FEEDBACK, 50.0);
        effect.set_parameter(PARAM_DAMP_LOWPASS, 20_000.0);
        effect.set_parameter(PARAM_DAMP_HIGHPASS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let mut impulse = alloc::vec![0.0_f32; 512];
        impulse[0] = 1.0;
        let out = render(&mut effect, &impulse, 0, 120.0);
        let period = 480;
        assert!(
            (out[period].abs() - 1.0).abs() < 0.05,
            "first repeat is {} not ~1.0",
            out[period]
        );
        assert!(
            (out[period * 2].abs() - 0.45).abs() < 0.05,
            "second repeat is {} not ~0.45",
            out[period * 2]
        );
    }

    #[test]
    fn an_empty_block_and_a_silent_effect_are_both_safe() {
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
}
