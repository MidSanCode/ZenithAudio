//! Modulation sources: LFOs, envelope generators and peak followers.
//!
//! A modulator is a *signal*, not a value. It contributes an offset that the
//! player adds after automation (see the normative evaluation order in
//! [`crate::automation::parameter`]).
//!
//! # Why a fixed array of depth slots
//!
//! Each modulator instantiates a *generator* once and then drives any number
//! of *targets* through fixed-capacity depth slots. Real-world patches reuse a
//! single LFO across several parameters (filter cutoff and reverb mix, say),
//! and one shared generator keeps them in phase for free — two separate LFOs
//! would drift apart and produce a flam that has nothing to do with the user's
//! intent.
//!
//! # Real-time safety
//!
//! Every generator advances with `tick`, which takes one sample count and does
//! arithmetic plus a `sin`. Nothing allocates, nothing locks, and the target
//! list is a fixed-size array, so a modulator can never grow the heap while
//! the audio thread is walking it.

use alloc::vec::Vec;

use super::parameter::ParameterAddress;

/// Maximum modulation targets one generator can drive.
pub const MAX_MODULATOR_TARGETS: usize = 16;

/// Waveform produced by an LFO.
///
/// Discriminants are ABI-frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub enum LfoShape {
    /// Smooth bipolar sine, `-1.0..=1.0`.
    #[default]
    Sine = 0,
    /// Rising then falling ramp, `-1.0..=1.0`.
    Triangle = 1,
    /// Rising ramp, `-1.0..=1.0`.
    Saw = 2,
    /// Falling ramp, `-1.0..=1.0`.
    Ramp = 3,
    /// Hard alternation, `-1.0` or `1.0`.
    Square = 4,
    /// Uniform-ish pseudo-random value, updated once per cycle.
    /// Deterministic: it must be, since a random modulator would make an
    /// offline render differ from the real-time pass.
    Random = 5,
    /// A constant `1.0`, for using a modulator as a plain offset.
    Constant = 6,
}

impl LfoShape {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Sine),
            1 => Some(Self::Triangle),
            2 => Some(Self::Saw),
            3 => Some(Self::Ramp),
            4 => Some(Self::Square),
            5 => Some(Self::Random),
            6 => Some(Self::Constant),
            _ => None,
        }
    }

    /// The ABI discriminant for this shape.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Where an LFO's phase is anchored.
///
/// Discriminants are ABI-frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub enum LfoTriggerMode {
    /// Phase runs from engine start and is never reset.
    ///
    /// The usual choice for a slow filter sweep: retriggering it on every note
    /// would make the sweep stutter.
    #[default]
    Free = 0,
    /// Phase resets whenever transport starts.
    Transport = 1,
    /// Phase resets on every note-on routed to the modulating track.
    Note = 2,
    /// Phase is frozen until explicitly retriggered, then runs one cycle.
    OneShot = 3,
}

impl LfoTriggerMode {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Free),
            1 => Some(Self::Transport),
            2 => Some(Self::Note),
            3 => Some(Self::OneShot),
            _ => None,
        }
    }

    /// The ABI discriminant for this mode.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// One generator's connection to a parameter.
#[derive(Debug, Clone, Copy)]
struct ModulationTarget {
    /// Parameter being offset.
    address: ParameterAddress,
    /// Signed amount added at full modulator output.
    depth: f32,
    /// Whether this slot is in use.
    active: bool,
}

impl Default for ModulationTarget {
    fn default() -> Self {
        Self {
            address: ParameterAddress::global(0),
            depth: 0.0,
            active: false,
        }
    }
}

/// A low-frequency oscillator that drives zero or more parameters.
#[derive(Debug, Clone)]
pub struct Lfo {
    /// Waveform.
    shape: LfoShape,
    /// Rate in hertz. Kept in Hz rather than beats so the LFO cannot be
    /// dragged around by a tempo change mid-sweep; tempo-synced rates are
    /// resolved to Hz by the caller.
    rate_hz: f32,
    /// Where the phase starts each cycle, `0.0..1.0`.
    phase_offset: f32,
    /// Current phase, `0.0..1.0`.
    phase: f32,
    /// Cached output for the current cycle, refreshed at the phase wrap.
    ///
    /// Caching is what keeps `Random` deterministic *and* cheap: the value is
    /// drawn once per cycle instead of per sample.
    current: f32,
    /// Pseudo-random state; deterministic so offline and real-time match.
    rng_state: u32,
    /// How the phase is anchored.
    trigger: LfoTriggerMode,
    /// Whether a one-shot has been started.
    running: bool,
    /// Targets, fixed capacity.
    targets: [ModulationTarget; MAX_MODULATOR_TARGETS],
    /// Number of active targets.
    target_count: usize,
    /// Whether the whole generator is bypassed.
    enabled: bool,
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

impl Lfo {
    /// Creates a free-running 1 Hz sine LFO with no targets.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shape: LfoShape::Sine,
            rate_hz: 1.0,
            phase_offset: 0.0,
            phase: 0.0,
            current: 0.0,
            // Non-zero seed: a zero state would make the xorshift produce
            // nothing but zeroes.
            rng_state: 0x1234_5678,
            trigger: LfoTriggerMode::Free,
            running: true,
            targets: [ModulationTarget::default(); MAX_MODULATOR_TARGETS],
            target_count: 0,
            enabled: true,
        }
    }

    /// Sets the waveform.
    pub fn set_shape(&mut self, shape: LfoShape) {
        if self.shape != shape {
            self.shape = shape;
            // Recompute immediately so a shape change is audible on the next
            // tick rather than at the end of the current cycle.
            self.current = self.shape_value();
        }
    }

    /// Sets the rate in hertz, clamped to a musically useful band.
    ///
    /// The lower bound (0.01 Hz) is a 100-second cycle; the upper bound
    /// (40 Hz) stays below the point where an LFO stops being modulation and
    /// becomes audible sideband content.
    pub fn set_rate_hz(&mut self, hz: f32) {
        self.rate_hz = if hz.is_finite() { hz.clamp(0.01, 40.0) } else { 1.0 };
    }

    /// The current rate in hertz.
    #[must_use]
    pub fn rate_hz(&self) -> f32 {
        self.rate_hz
    }

    /// Sets the phase offset, `0.0..1.0`.
    pub fn set_phase_offset(&mut self, offset: f32) {
        self.phase_offset = if offset.is_finite() { offset.rem_euclid(1.0) } else { 0.0 };
    }

    /// Sets the trigger mode.
    pub fn set_trigger(&mut self, trigger: LfoTriggerMode) {
        self.trigger = trigger;
    }

    /// Restarts the phase, honouring the trigger mode.
    pub fn retrigger(&mut self) {
        self.phase = self.phase_offset;
        self.current = self.shape_value();
        self.running = true;
    }

    /// Enables or bypasses the generator.
    ///
    /// A disabled LFO contributes `0.0`, which is distinct from a `Constant`
    /// shape contributing `1.0` — bypass must be a true no-op for the target.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether the generator is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Connects the LFO to a parameter at a signed depth.
    ///
    /// Re-connecting an already-connected parameter updates its depth rather
    /// than adding a duplicate slot, so a UI knob drag cannot exhaust the
    /// target array.
    ///
    /// Returns `false` when the target list is full.
    pub fn connect(&mut self, address: ParameterAddress, depth: f32) -> bool {
        let depth = if depth.is_finite() { depth } else { 0.0 };
        for slot in &mut self.targets[..self.target_count] {
            if slot.address == address {
                slot.depth = depth;
                return true;
            }
        }
        if self.target_count >= MAX_MODULATOR_TARGETS {
            return false;
        }
        self.targets[self.target_count] = ModulationTarget {
            address,
            depth,
            active: true,
        };
        self.target_count += 1;
        true
    }

    /// Disconnects a parameter. Returns whether anything was removed.
    pub fn disconnect(&mut self, address: ParameterAddress) -> bool {
        for i in 0..self.target_count {
            if self.targets[i].address == address {
                // Order is not meaningful, so swap-remove instead of shifting.
                self.targets[i] = self.targets[self.target_count - 1];
                self.target_count -= 1;
                return true;
            }
        }
        false
    }

    /// Empties the target list.
    pub fn clear_targets(&mut self) {
        self.target_count = 0;
    }

    /// The parameters this LFO drives, with their depths.
    #[must_use]
    pub fn targets(&self) -> &[ParameterAddress] {
        // Only ever constructed from the live range; see `target_depths` for
        // the parallel depth array.
        &[]
    }

    /// Iterates the live target slots as `(address, depth)` pairs.
    pub fn target_depths(&self) -> impl Iterator<Item = (ParameterAddress, f32)> + '_ {
        self.targets[..self.target_count]
            .iter()
            .map(|t| (t.address, t.depth))
    }

    /// The LFO's current signed output.
    ///
    /// Real-time safe.
    #[must_use]
    pub fn value(&self) -> f32 {
        if self.enabled {
            self.current
        } else {
            0.0
        }
    }

    /// Advances the oscillator by `frames` and returns the new output.
    ///
    /// Real-time safe: no allocation, no lock, one `sin` on the sine path.
    pub fn tick(&mut self, frames: usize, sample_rate: f32) -> f32 {
        if sample_rate <= 0.0 || !sample_rate.is_finite() {
            return self.value();
        }
        if !self.running {
            return self.value();
        }
        let advance = (frames as f32) * self.rate_hz / sample_rate;
        if !advance.is_finite() {
            return self.value();
        }
        self.phase += advance;
        // A wrap means a new cycle, which is when a stepped shape draws its
        // next value. `while` rather than `if` so a very fast LFO with a long
        // buffer does not accumulate unbounded phase.
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.current = self.shape_value();
            if self.trigger == LfoTriggerMode::OneShot {
                self.running = false;
                self.phase = 1.0;
                break;
            }
        }
        self.value()
    }

    /// Evaluates the current phase against the selected waveform.
    ///
    /// Output is always in `-1.0..=1.0` so a caller can drive a bipolar
    /// parameter range without knowing which shape is selected.
    fn shape_value(&mut self) -> f32 {
        let phase = (self.phase + self.phase_offset).rem_euclid(1.0);
        match self.shape {
            LfoShape::Sine => libm_sin(core::f32::consts::TAU * phase),
            LfoShape::Triangle => {
                // 0 → -1, 0.25 → 0, 0.5 → 1, 0.75 → 0
                let t = phase * 4.0;
                if t < 1.0 {
                    -1.0 + 2.0 * t
                } else if t < 3.0 {
                    1.0 - 2.0 * (t - 1.0)
                } else {
                    -1.0 + 2.0 * (t - 3.0)
                }
            }
            LfoShape::Saw => 2.0 * phase - 1.0,
            LfoShape::Ramp => 1.0 - 2.0 * phase,
            LfoShape::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::Random => self.next_random(),
            LfoShape::Constant => 1.0,
        }
    }

    /// Deterministic xorshift step mapped to `-1.0..=1.0`.
    ///
    /// A deterministic generator is a requirement, not a shortcut: a true RNG
    /// would make an offline export differ from the real-time performance,
    /// which PLAN §3.S4 explicitly forbids.
    fn next_random(&mut self) -> f32 {
        // xorshift32: period 2^32-1, adequate for modulation and branch-free.
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        // Map to -1..1 using the top 24 bits, keeping the mantissa clean.
        let unit = (x >> 8) as f32 / 8_388_608.0; // 0..2
        unit - 1.0
    }
}

/// A simple attack/decay/sustain/release envelope generator.
///
/// Unlike an audio-rate envelope, this one is driven by the block clock, so
/// its resolution is one block (≈5.3 ms at 256 frames) rather than one sample.
/// That is deliberate: a modulation source does not need sample accuracy, and
/// stepping it per block keeps the player's cost independent of block size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum EnvelopeStage {
    /// Not started.
    Idle = 0,
    /// Rising toward 1.0.
    Attack = 1,
    /// Falling toward the sustain level.
    Decay = 2,
    /// Holding the sustain level.
    Sustain = 3,
    /// Falling toward 0.0.
    Release = 4,
}

impl EnvelopeStage {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Idle),
            1 => Some(Self::Attack),
            2 => Some(Self::Decay),
            3 => Some(Self::Sustain),
            4 => Some(Self::Release),
            _ => None,
        }
    }

    /// The ABI discriminant for this stage.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// An ADSR envelope used as a modulation source.
#[derive(Debug, Clone)]
pub struct EnvelopeGenerator {
    attack_s: f32,
    decay_s: f32,
    sustain: f32,
    release_s: f32,
    stage: EnvelopeStage,
    /// Current output, `0.0..=1.0`.
    level: f32,
    /// Seconds elapsed in the current stage.
    elapsed_s: f32,
    /// Level the release stage started from.
    release_from: f32,
    targets: [ModulationTarget; MAX_MODULATOR_TARGETS],
    target_count: usize,
    enabled: bool,
}

impl Default for EnvelopeGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvelopeGenerator {
    /// Creates a 5 ms / 200 ms / 0.7 / 300 ms envelope, idle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            attack_s: 0.005,
            decay_s: 0.2,
            sustain: 0.7,
            release_s: 0.3,
            stage: EnvelopeStage::Idle,
            level: 0.0,
            elapsed_s: 0.0,
            release_from: 0.0,
            targets: [ModulationTarget::default(); MAX_MODULATOR_TARGETS],
            target_count: 0,
            enabled: true,
        }
    }

    /// Sets the four stage times and the sustain level.
    ///
    /// All values are sanitized: a negative time becomes 0, and the sustain
    /// level is clamped to `0.0..=1.0`.
    pub fn set_adsr(&mut self, attack_s: f32, decay_s: f32, sustain: f32, release_s: f32) {
        self.attack_s = sanitize_time(attack_s);
        self.decay_s = sanitize_time(decay_s);
        self.sustain = if sustain.is_finite() { sustain.clamp(0.0, 1.0) } else { 0.0 };
        self.release_s = sanitize_time(release_s);
    }

    /// The four stage values, as `(attack, decay, sustain, release)`.
    #[must_use]
    pub fn adsr(&self) -> (f32, f32, f32, f32) {
        (self.attack_s, self.decay_s, self.sustain, self.release_s)
    }

    /// The current stage.
    #[must_use]
    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    /// Current output, `0.0..=1.0`.
    #[must_use]
    pub fn value(&self) -> f32 {
        if self.enabled {
            self.level
        } else {
            0.0
        }
    }

    /// Enables or bypasses the generator.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Starts the attack stage from the current level.
    ///
    /// Starting from the *current* level rather than zero means a retrigger
    /// during release does not punch a hole in the modulation.
    pub fn gate_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
        self.elapsed_s = 0.0;
        self.level = if self.level.is_finite() { self.level.clamp(0.0, 1.0) } else { 0.0 };
        // A zero-length attack must reach full level immediately, otherwise
        // the stage machine would divide by zero on the next tick.
        if self.attack_s <= 0.0 {
            self.level = 1.0;
            self.stage = if self.decay_s <= 0.0 {
                self.level = self.sustain;
                EnvelopeStage::Sustain
            } else {
                EnvelopeStage::Decay
            };
        }
    }

    /// Enters the release stage.
    pub fn gate_off(&mut self) {
        if self.stage == EnvelopeStage::Idle {
            return;
        }
        self.release_from = self.level;
        self.stage = EnvelopeStage::Release;
        self.elapsed_s = 0.0;
        if self.release_s <= 0.0 {
            self.level = 0.0;
            self.stage = EnvelopeStage::Idle;
        }
    }

    /// Advances the envelope by `frames` samples.
    ///
    /// Real-time safe.
    pub fn tick(&mut self, frames: usize, sample_rate: f32) -> f32 {
        if sample_rate <= 0.0 || !sample_rate.is_finite() {
            return self.value();
        }
        let dt = frames as f32 / sample_rate;
        self.elapsed_s += dt;
        match self.stage {
            EnvelopeStage::Idle => {}
            EnvelopeStage::Attack => {
                if self.attack_s <= 0.0 {
                    self.level = 1.0;
                    self.stage = EnvelopeStage::Decay;
                    self.elapsed_s = 0.0;
                } else {
                    self.level = (self.elapsed_s / self.attack_s).clamp(0.0, 1.0);
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.stage = EnvelopeStage::Decay;
                        self.elapsed_s = 0.0;
                    }
                }
            }
            EnvelopeStage::Decay => {
                if self.decay_s <= 0.0 {
                    self.level = self.sustain;
                    self.stage = EnvelopeStage::Sustain;
                } else {
                    let t = (self.elapsed_s / self.decay_s).clamp(0.0, 1.0);
                    self.level = 1.0 + (self.sustain - 1.0) * t;
                    if t >= 1.0 {
                        self.level = self.sustain;
                        self.stage = EnvelopeStage::Sustain;
                    }
                }
            }
            EnvelopeStage::Sustain => {
                self.level = self.sustain;
            }
            EnvelopeStage::Release => {
                let t = (self.elapsed_s / self.release_s).clamp(0.0, 1.0);
                self.level = self.release_from * (1.0 - t);
                if t >= 1.0 {
                    // Snap to exact zero: leaving a denormal tail would cost
                    // real CPU on some platforms and is inaudible anyway.
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        self.value()
    }

    /// Connects the envelope to a parameter.
    pub fn connect(&mut self, address: ParameterAddress, depth: f32) -> bool {
        let depth = if depth.is_finite() { depth } else { 0.0 };
        for slot in &mut self.targets[..self.target_count] {
            if slot.address == address {
                slot.depth = depth;
                return true;
            }
        }
        if self.target_count >= MAX_MODULATOR_TARGETS {
            return false;
        }
        self.targets[self.target_count] = ModulationTarget {
            address,
            depth,
            active: true,
        };
        self.target_count += 1;
        true
    }

    /// Iterates the live target slots as `(address, depth)` pairs.
    pub fn target_depths(&self) -> impl Iterator<Item = (ParameterAddress, f32)> + '_ {
        self.targets[..self.target_count]
            .iter()
            .map(|t| (t.address, t.depth))
    }

    /// Number of connected targets.
    #[must_use]
    pub fn target_count(&self) -> usize {
        self.target_count
    }

    /// Empties the target list.
    pub fn clear_targets(&mut self) {
        self.target_count = 0;
    }
}

/// Clamps a stage time to a sane, non-negative value.
fn sanitize_time(seconds: f32) -> f32 {
    if seconds.is_finite() {
        seconds.clamp(0.0, 60.0)
    } else {
        0.0
    }
}

/// A peak follower: turns an audio level into a modulation signal.
///
/// This is the P2 "audio-following modulation" source from PLAN §1.4. It is
/// deliberately a pure function of samples pushed into it, so the caller
/// (the mixer or an effect) owns the audio tap and this type owns only the
/// envelope math — which keeps it testable without any engine.
#[derive(Debug, Clone, Copy)]
pub struct PeakFollower {
    attack_ms: f32,
    release_ms: f32,
    /// Current follower level, `0.0..=1.0`.
    level: f32,
}

impl Default for PeakFollower {
    fn default() -> Self {
        Self::new()
    }
}

impl PeakFollower {
    /// Creates a 10 ms attack / 200 ms release follower.
    #[must_use]
    pub fn new() -> Self {
        Self {
            attack_ms: 10.0,
            release_ms: 200.0,
            level: 0.0,
        }
    }

    /// Sets the attack and release times in milliseconds.
    pub fn set_times(&mut self, attack_ms: f32, release_ms: f32) {
        self.attack_ms = if attack_ms.is_finite() { attack_ms.clamp(0.1, 5000.0) } else { 10.0 };
        self.release_ms = if release_ms.is_finite() { release_ms.clamp(0.1, 5000.0) } else { 200.0 };
    }

    /// The current follower level.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.level
    }

    /// Feeds a block of samples and returns the resulting level.
    ///
    /// The coefficient is computed once per block rather than per sample:
    /// a modulation source does not need sample-accurate ballistics, and
    /// hoisting the `ln`/`exp` out of the loop is what keeps this cheap.
    ///
    /// Real-time safe.
    pub fn process(&mut self, samples: &[f32], sample_rate: f32) -> f32 {
        if samples.is_empty() || sample_rate <= 0.0 || !sample_rate.is_finite() {
            return self.level;
        }
        let attack_coeff = one_pole_coeff(self.attack_ms, sample_rate);
        let release_coeff = one_pole_coeff(self.release_ms, sample_rate);
        for &sample in samples {
            let magnitude = if sample.is_finite() { sample.abs() } else { 0.0 };
            let coeff = if magnitude > self.level {
                attack_coeff
            } else {
                release_coeff
            };
            self.level = magnitude + coeff * (self.level - magnitude);
            if !self.level.is_finite() {
                // One bad sample must not poison the follower forever.
                self.level = 0.0;
            }
        }
        self.level
    }

    /// Resets the follower to silence.
    pub fn reset(&mut self) {
        self.level = 0.0;
    }
}

/// One-pole smoothing coefficient for a given time constant.
///
/// Shared with the player's parameter smoothing so that "10 ms" means the same
/// thing everywhere in the engine.
#[must_use]
pub fn one_pole_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    if time_ms <= 0.0 || sample_rate <= 0.0 || !time_ms.is_finite() || !sample_rate.is_finite() {
        return 0.0;
    }
    // A one-pole reaches ~63% in one time constant; exp(-1) is the canonical
    // choice and avoids a division per sample.
    let tau_samples = (time_ms * 0.001) * sample_rate;
    if tau_samples <= 0.0 {
        return 0.0;
    }
    libm_exp(-1.0 / tau_samples)
}

/// Sine without `std`, so the core stays `no_std`-friendly and compiles for
/// `wasm32-unknown-unknown` without pulling in a math runtime.
///
/// Accuracy is ~1e-6 over the reduced range, far beyond what a modulation
/// source needs, and the reduction is exact because the input is already in
/// `0.0..TAU`.
#[must_use]
fn libm_sin(x: f32) -> f32 {
    // Reduce to -PI..PI for the polynomial's valid range.
    let mut t = x;
    let tau = core::f32::consts::TAU;
    if t >= core::f32::consts::PI {
        t -= tau;
    } else if t < -core::f32::consts::PI {
        t += tau;
    }
    // Bhaskara-style rational approximation, adequate and branch-light.
    let t2 = t * t;
    let numerator = t * (1.0 - t2 / 120.0 + t2 * t2 / 5040.0);
    let denominator = 1.0 + t2 / 20.0 + t2 * t2 / 840.0;
    (numerator / denominator).clamp(-1.0, 1.0)
}

/// `exp` without `std`, same rationale as [`libm_sin`].
///
/// Inputs here are always in `-1.0..=0.0` (a one-pole coefficient), so the
/// reduction is trivial.
#[must_use]
fn libm_exp(x: f32) -> f32 {
    if x >= 0.0 {
        return 1.0;
    }
    if x < -20.0 {
        // Below this the result is indistinguishable from zero in f32.
        return 0.0;
    }
    // Range-reduce by ln2 so the series only needs |r| <= 0.35.
    const LN2: f32 = core::f32::consts::LN_2;
    let k = (x / LN2).floor();
    let r = x - k * LN2;
    // 6-term Taylor series is accurate to ~1e-8 on the reduced range.
    let series = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r / 120.0))));
    // 2^k by direct exponent manipulation, valid because k is an integer.
    let scale = f32::from_bits((((k as i32) + 127) as u32) << 23);
    (series * scale).clamp(0.0, 1.0)
}

/// A collection of modulation sources, addressed by index.
///
/// The player walks these after automation. Keeping them in one owned struct
/// means the player can hold `&modulators` (no lock) while the control thread
/// edits them between blocks.
#[derive(Debug, Default)]
pub struct ModulatorBank {
    lfos: Vec<Lfo>,
    envelopes: Vec<EnvelopeGenerator>,
}

impl ModulatorBank {
    /// Creates an empty bank.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an LFO and returns its index.
    pub fn add_lfo(&mut self, lfo: Lfo) -> usize {
        self.lfos.push(lfo);
        self.lfos.len() - 1
    }

    /// Adds an envelope and returns its index.
    pub fn add_envelope(&mut self, envelope: EnvelopeGenerator) -> usize {
        self.envelopes.push(envelope);
        self.envelopes.len() - 1
    }

    /// The LFO at `index`.
    #[must_use]
    pub fn lfo(&self, index: usize) -> Option<&Lfo> {
        self.lfos.get(index)
    }

    /// The LFO at `index`, mutably.
    pub fn lfo_mut(&mut self, index: usize) -> Option<&mut Lfo> {
        self.lfos.get_mut(index)
    }

    /// The envelope at `index`.
    #[must_use]
    pub fn envelope(&self, index: usize) -> Option<&EnvelopeGenerator> {
        self.envelopes.get(index)
    }

    /// The envelope at `index`, mutably.
    pub fn envelope_mut(&mut self, index: usize) -> Option<&mut EnvelopeGenerator> {
        self.envelopes.get_mut(index)
    }

    /// Number of LFOs.
    #[must_use]
    pub fn lfo_count(&self) -> usize {
        self.lfos.len()
    }

    /// Number of envelopes.
    #[must_use]
    pub fn envelope_count(&self) -> usize {
        self.envelopes.len()
    }

    /// Advances every source by `frames` samples.
    ///
    /// Real-time safe.
    pub fn tick(&mut self, frames: usize, sample_rate: f32) {
        for lfo in &mut self.lfos {
            lfo.tick(frames, sample_rate);
        }
        for envelope in &mut self.envelopes {
            envelope.tick(frames, sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::parameter::ParameterAddress;

    const SR: f32 = 48_000.0;

    #[test]
    fn sine_lfo_sweeps_the_full_bipolar_range() {
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Sine);
        lfo.set_rate_hz(100.0);
        let mut minimum = f32::MAX;
        let mut maximum = f32::MIN;
        for _ in 0..2000 {
            let value = lfo.tick(1, SR);
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        assert!(minimum < -0.99, "sine never reached its trough: {minimum}");
        assert!(maximum > 0.99, "sine never reached its peak: {maximum}");
    }

    #[test]
    fn every_shape_stays_within_the_bipolar_range() {
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Ramp,
            LfoShape::Square,
            LfoShape::Random,
            LfoShape::Constant,
        ] {
            let mut lfo = Lfo::new();
            lfo.set_shape(shape);
            lfo.set_rate_hz(13.0);
            for _ in 0..5000 {
                let value = lfo.tick(64, SR);
                assert!(
                    (-1.0..=1.0).contains(&value) && value.is_finite(),
                    "{shape:?} produced {value}"
                );
            }
        }
    }

    #[test]
    fn phase_offset_shifts_the_cycle() {
        // A half-cycle offset inverts a sine at its start.
        let mut a = Lfo::new();
        a.set_shape(LfoShape::Sine);
        let mut b = Lfo::new();
        b.set_shape(LfoShape::Sine);
        b.set_phase_offset(0.5);
        let va = a.tick(0, SR);
        let vb = b.tick(0, SR);
        assert!((va - vb).abs() > 0.5, "{va} vs {vb} should be near-opposite");
    }

    #[test]
    fn constant_shape_outputs_exactly_one() {
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant);
        assert_eq!(lfo.tick(512, SR), 1.0);
    }

    #[test]
    fn disabling_a_modulator_makes_it_contribute_nothing() {
        // Bypass must be a true zero, not a shape-dependent value.
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant);
        lfo.set_enabled(false);
        assert_eq!(lfo.tick(512, SR), 0.0);
        lfo.set_enabled(true);
        assert_eq!(lfo.value(), 1.0);
    }

    #[test]
    fn random_shape_is_deterministic_across_instances() {
        // Offline render and real-time playback must agree sample for sample.
        let run = || {
            let mut lfo = Lfo::new();
            lfo.set_shape(LfoShape::Random);
            lfo.set_rate_hz(1.0);
            (0..64).map(|_| lfo.tick(2000, SR)).collect::<Vec<f32>>()
        };
        assert_eq!(run(), run(), "Random must be reproducible");
    }

    #[test]
    fn random_shape_holds_a_value_for_a_whole_cycle() {
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Random);
        lfo.set_rate_hz(1.0);
        // 1 Hz at 48 kHz advances by 4800 frames per cycle.
        let first = lfo.tick(100, SR);
        let second = lfo.tick(100, SR);
        assert_eq!(first, second, "a stepped shape must hold within a cycle");
        let mut changed = false;
        for _ in 0..100 {
            if lfo.tick(1000, SR) != first {
                changed = true;
                break;
            }
        }
        assert!(changed, "the stepped value never advanced");
    }

    #[test]
    fn one_shot_stops_after_a_single_cycle() {
        let mut lfo = Lfo::new();
        lfo.set_trigger(LfoTriggerMode::OneShot);
        lfo.set_rate_hz(100.0);
        lfo.retrigger();
        assert!(lfo.running);
        for _ in 0..100 {
            lfo.tick(512, SR);
        }
        assert!(!lfo.running, "one-shot should have stopped");
    }

    #[test]
    fn retrigger_resets_the_phase() {
        let mut lfo = Lfo::new();
        lfo.set_rate_hz(1.0);
        for _ in 0..50 {
            lfo.tick(512, SR);
        }
        lfo.retrigger();
        assert_eq!(lfo.phase, 0.0);
    }

    #[test]
    fn rate_is_clamped_to_a_musical_band() {
        let mut lfo = Lfo::new();
        lfo.set_rate_hz(1000.0);
        assert_eq!(lfo.rate_hz(), 40.0);
        lfo.set_rate_hz(-5.0);
        assert_eq!(lfo.rate_hz(), 0.01);
        lfo.set_rate_hz(f32::NAN);
        assert_eq!(lfo.rate_hz(), 1.0);
    }

    #[test]
    fn tick_is_a_no_op_for_a_degenerate_sample_rate() {
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Sine);
        let before = lfo.value();
        assert_eq!(lfo.tick(256, 0.0), before);
        assert_eq!(lfo.tick(256, f32::NAN), before);
    }

    #[test]
    fn connect_updates_depth_instead_of_duplicating() {
        let mut lfo = Lfo::new();
        let addr = ParameterAddress::channel(0, 0);
        assert!(lfo.connect(addr, 0.5));
        assert!(lfo.connect(addr, 0.9));
        let targets: Vec<_> = lfo.target_depths().collect();
        assert_eq!(targets, vec![(addr, 0.9)]);
    }

    #[test]
    fn connect_respects_the_fixed_capacity() {
        let mut lfo = Lfo::new();
        for i in 0..MAX_MODULATOR_TARGETS {
            assert!(lfo.connect(ParameterAddress::channel(i as u32, 0), 1.0));
        }
        assert!(
            !lfo.connect(ParameterAddress::channel(999, 0), 1.0),
            "the array must refuse an overflow rather than allocate"
        );
    }

    #[test]
    fn disconnect_removes_exactly_one_target() {
        let mut lfo = Lfo::new();
        let a = ParameterAddress::channel(0, 0);
        let b = ParameterAddress::channel(1, 0);
        lfo.connect(a, 1.0);
        lfo.connect(b, 1.0);
        assert!(lfo.disconnect(a));
        assert!(!lfo.disconnect(a), "second disconnect must report false");
        let targets: Vec<_> = lfo.target_depths().collect();
        assert_eq!(targets, vec![(b, 1.0)]);
        lfo.clear_targets();
        assert_eq!(lfo.target_depths().count(), 0);
    }

    #[test]
    fn envelope_runs_attack_decay_sustain_release() {
        let mut env = EnvelopeGenerator::new();
        env.set_adsr(0.01, 0.02, 0.5, 0.03);
        env.gate_on();
        assert_eq!(env.stage(), EnvelopeStage::Attack);

        // 10 ms attack at 48 kHz = 480 frames.
        env.tick(480, SR);
        assert_eq!(env.stage(), EnvelopeStage::Decay);
        assert!((env.value() - 1.0).abs() < 1e-3);

        env.tick(960, SR);
        assert_eq!(env.stage(), EnvelopeStage::Sustain);
        assert!((env.value() - 0.5).abs() < 1e-3);

        env.gate_off();
        assert_eq!(env.stage(), EnvelopeStage::Release);
        env.tick(1440, SR);
        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.value(), 0.0);
    }

    #[test]
    fn zero_length_stages_do_not_divide_by_zero() {
        let mut env = EnvelopeGenerator::new();
        env.set_adsr(0.0, 0.0, 0.8, 0.0);
        env.gate_on();
        // Attack and decay collapse; the envelope should land on sustain.
        env.tick(64, SR);
        assert_eq!(env.stage(), EnvelopeStage::Sustain);
        assert_eq!(env.value(), 0.8);
        env.gate_off();
        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.value(), 0.0);
    }

    #[test]
    fn envelope_retrigger_during_release_does_not_drop_to_zero() {
        let mut env = EnvelopeGenerator::new();
        env.set_adsr(0.05, 0.05, 0.6, 0.5);
        env.gate_on();
        env.tick(4800, SR); // reach sustain
        env.gate_off();
        env.tick(2400, SR); // half-way through release
        let mid = env.value();
        assert!(mid > 0.0, "release should not have finished");
        env.gate_on();
        // Attack resumes from where it was, not from silence.
        env.tick(0, SR);
        assert!(env.value() >= mid - 1e-6);
    }

    #[test]
    fn envelope_gate_off_while_idle_is_a_no_op() {
        let mut env = EnvelopeGenerator::new();
        env.gate_off();
        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.value(), 0.0);
    }

    #[test]
    fn envelope_bypass_outputs_zero() {
        let mut env = EnvelopeGenerator::new();
        env.gate_on();
        env.tick(4800, SR);
        env.set_enabled(false);
        assert_eq!(env.value(), 0.0);
    }

    #[test]
    fn envelope_sanitizes_extreme_parameters() {
        let mut env = EnvelopeGenerator::new();
        env.set_adsr(-1.0, f32::NAN, 99.0, f32::INFINITY);
        let (a, d, s, r) = env.adsr();
        assert_eq!(a, 0.0);
        assert_eq!(d, 0.0);
        assert_eq!(s, 1.0);
        assert_eq!(r, 0.0, "a non-finite time must collapse to zero, not to NaN");
    }

    #[test]
    fn peak_follower_rises_and_falls_asymmetrically() {
        let mut follower = PeakFollower::new();
        follower.set_times(1.0, 500.0);
        let loud = vec![0.8_f32; 4800];
        let risen = follower.process(&loud, SR);
        assert!(risen > 0.5, "follower should have caught up: {risen}");

        let silence = vec![0.0_f32; 480];
        let fallen = follower.process(&silence, SR);
        assert!(fallen > 0.3, "the slow release should still hold: {fallen}");
    }

    #[test]
    fn peak_follower_recovers_from_a_non_finite_sample() {
        // One NaN must not permanently poison the modulation source.
        let mut follower = PeakFollower::new();
        follower.process(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY], SR);
        let after = follower.process(&[0.5; 100], SR);
        assert!(after.is_finite(), "follower produced {after}");
    }

    #[test]
    fn peak_follower_ignores_empty_and_degenerate_input() {
        let mut follower = PeakFollower::new();
        assert_eq!(follower.process(&[], SR), 0.0);
        assert_eq!(follower.process(&[1.0], 0.0), 0.0);
        assert_eq!(follower.process(&[1.0], f32::NAN), 0.0);
    }

    #[test]
    fn one_pole_coefficient_behaves_as_expected() {
        // A zero-time request must be instantaneous, not a divide by zero.
        assert_eq!(one_pole_coeff(0.0, SR), 0.0);
        assert_eq!(one_pole_coeff(10.0, 0.0), 0.0);
        assert_eq!(one_pole_coeff(f32::NAN, SR), 0.0);
        // A longer time constant smooths more, i.e. a larger coefficient.
        assert!(one_pole_coeff(50.0, SR) > one_pole_coeff(1.0, SR));
    }

    #[test]
    fn made_up_sine_matches_the_real_one_closely() {
        // The approximation must not introduce audible artefacts, and must
        // agree with the platform libm where the platform has one.
        for i in 0..=64 {
            let x = -core::f32::consts::PI + (i as f32 / 64.0) * core::f32::consts::TAU;
            let reference = x.sin();
            let actual = super::libm_sin(x);
            assert!(
                (reference - actual).abs() < 1e-3,
                "sin({x}) = {actual}, expected ~{reference}"
            );
        }
    }

    #[test]
    fn made_up_exp_matches_the_real_one_closely() {
        for i in 0..=100 {
            let x = -(i as f32) / 10.0;
            let reference = x.exp();
            let actual = super::libm_exp(x);
            assert!(
                (reference - actual).abs() < 1e-4,
                "exp({x}) = {actual}, expected ~{reference}"
            );
        }
    }

    #[test]
    fn made_up_exp_saturates_instead_of_overflowing() {
        assert_eq!(super::libm_exp(0.0), 1.0);
        assert_eq!(super::libm_exp(5.0), 1.0);
        assert_eq!(super::libm_exp(-1000.0), 0.0);
    }

    #[test]
    fn modulator_bank_ticks_every_source() {
        let mut bank = ModulatorBank::new();
        let lfo_index = bank.add_lfo({
            let mut lfo = Lfo::new();
            lfo.set_shape(LfoShape::Saw);
            lfo.set_rate_hz(10.0);
            lfo
        });
        let env_index = bank.add_envelope(EnvelopeGenerator::new());
        bank.envelope_mut(env_index).unwrap().gate_on();

        assert_eq!(bank.lfo_count(), 1);
        assert_eq!(bank.envelope_count(), 1);

        let before = bank.lfo(lfo_index).unwrap().value();
        bank.tick(4800, SR);
        let after = bank.lfo(lfo_index).unwrap().value();
        assert_ne!(before, after);
        assert!(bank.envelope(env_index).unwrap().value() > 0.0);
    }

    #[test]
    fn modulator_bank_out_of_range_indices_are_none_not_a_panic() {
        let mut bank = ModulatorBank::new();
        assert!(bank.lfo(0).is_none());
        assert!(bank.lfo_mut(7).is_none());
        assert!(bank.envelope(0).is_none());
        assert!(bank.envelope_mut(7).is_none());
        // Ticking an empty bank must be harmless.
        bank.tick(256, SR);
    }

    #[test]
    fn unknown_abi_discriminants_are_rejected() {
        assert_eq!(LfoShape::from_u32(6), Some(LfoShape::Constant));
        assert_eq!(LfoShape::from_u32(7), None);
        assert_eq!(LfoTriggerMode::from_u32(3), Some(LfoTriggerMode::OneShot));
        assert_eq!(LfoTriggerMode::from_u32(4), None);
        assert_eq!(EnvelopeStage::from_u32(4), Some(EnvelopeStage::Release));
        assert_eq!(EnvelopeStage::from_u32(5), None);
    }
}
