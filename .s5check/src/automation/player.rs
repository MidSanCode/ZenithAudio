//! The automation player: the real-time evaluation path.
//!
//! # Normative evaluation order
//!
//! Every parameter the player touches is computed as:
//!
//! ```text
//!   base value  →  automation  →  modulator sum  →  clamp
//! ```
//!
//! See [`crate::automation::parameter`] for *why* this order and not another.
//! This module is the only place that may implement it; effects and the mixer
//! must read the smoothed result through the store rather than re-deriving it.
//!
//! # Real-time safety
//!
//! [`AutomationPlayer::advance_block`] is the audio-thread entry point. It
//! allocates nothing, locks nothing, takes no reference counts and performs no
//! IO:
//!
//! * the candidate lane and modulator lists are borrowed slices, not owned
//!   collections;
//! * the per-parameter scratch is a fixed-capacity array on the stack;
//! * every descriptor lookup is a binary search over a contiguous `Vec`.
//!
//! The `assert_no_alloc` test in [`crate::automation`] enforces this rather
//! than trusting it.

use alloc::vec::Vec;

use super::lane::{Lane, RecordMode};
use super::modulator::{one_pole_coeff_for_elapsed, ModulatorBank};
use super::parameter::ParameterAddress;
use super::store::ParameterStore;

/// Default maximum automated parameters processed in one block.
///
/// The player's per-block scratch lives on the stack (see
/// [`ModulatorAccumulator`]), so there is a hard ceiling on how many
/// parameters can be tracked. 512 is far above any plausible patch
/// (PLAN §3.S2's acceptance case is 1000 points, typically spread over a
/// handful of parameters), and exceeding it is reported through
/// [`PlayerStats::skipped`] rather than silently ignored.
pub const MAX_TRACKED_PARAMETERS: usize = 512;

/// Alias kept for readability at call sites that talk about "per block".
pub const MAX_PARAMETERS_PER_BLOCK: usize = MAX_TRACKED_PARAMETERS;

/// Counters describing the last block, for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerStats {
    /// Parameters evaluated this block.
    pub evaluated: usize,
    /// Parameters that had a lane contributing a value.
    pub automated: usize,
    /// Parameters that had at least one modulator contributing.
    pub modulated: usize,
    /// Parameters that were written because their target changed.
    pub written: usize,
    /// Parameters dropped because the per-block cap was reached.
    pub skipped: usize,
    /// Parameters whose address was not registered in the store.
    pub unresolved: usize,
}

/// A smoothing state for one parameter.
#[derive(Debug, Clone, Copy)]
struct SmoothingState {
    /// Slot index in the store, so the write needs no second lookup.
    store_index: usize,
    /// One-pole coefficient for this parameter's smoothing time.
    ///
    /// Recomputed per block, which makes a smoothing-time edit take effect
    /// immediately without a control-thread round trip.
    coefficient: f32,
    /// Current smoothed value.
    current: f32,
    /// Target the smoothed value is heading toward.
    target: f32,
    /// Whether `current` holds a real previous value yet.
    primed: bool,
}

/// Evaluates automation and modulation into the parameter store.
///
/// One player per engine. It holds no pointer to the store it writes into —
/// the store is passed to [`Self::advance_block`] — so a player cannot
/// outlive the data it is bound to.
#[derive(Debug)]
pub struct AutomationPlayer {
    /// Smoothing state, one entry per automated parameter.
    ///
    /// A `Vec` because the *set* of automated parameters is a control-thread
    /// property that changes only when a lane is added or removed; the audio
    /// thread only mutates the entries in place, never the length.
    smoothing: Vec<SmoothingState>,
    /// Whether the sample rate has been supplied.
    prepared: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// How many parameters the player will track simultaneously.
    ///
    /// Defaults to [`MAX_PARAMETERS_PER_BLOCK`]; the field exists so the cap
    /// can be lowered (or raised, up to the fixed accumulator size) without
    /// editing the constant, and so a test can exercise both sides of the
    /// boundary.
    capacity: usize,
    /// Stats from the most recent block.
    stats: PlayerStats,
}

impl Default for AutomationPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationPlayer {
    /// Creates an unprepared player.
    #[must_use]
    pub fn new() -> Self {
        Self {
            smoothing: Vec::new(),
            prepared: false,
            sample_rate: 48_000.0,
            capacity: MAX_PARAMETERS_PER_BLOCK,
            stats: PlayerStats::default(),
        }
    }

    /// The number of parameters the player will track simultaneously.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Overrides the tracking capacity, clamped to the supported ceiling.
    ///
    /// The accumulator is a stack array sized by [`MAX_TRACKED_PARAMETERS`],
    /// so raising the capacity past that would silently drop modulator
    /// contributions; the clamp makes the limit explicit instead.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.clamp(1, MAX_TRACKED_PARAMETERS);
    }

    /// Whether [`Self::prepare`] has run.
    #[must_use]
    pub fn is_prepared(&self) -> bool {
        self.prepared
    }

    /// Stats from the most recent [`Self::advance_block`].
    #[must_use]
    pub fn stats(&self) -> PlayerStats {
        self.stats
    }

    /// Number of parameters the player is currently smoothing.
    #[must_use]
    pub fn tracked_parameters(&self) -> usize {
        self.smoothing.len()
    }

    /// Reserves storage and records the sample rate.
    ///
    /// Called from the control thread before the audio thread starts. Passing
    /// the automated lane count lets the reserve cover the whole project, so
    /// `advance_block` never triggers a reallocation.
    pub fn prepare(&mut self, sample_rate: f32, lane_count: usize) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate = sample_rate;
        }
        // Everything is primed false so the first block snaps to target rather
        // than gliding from a meaningless zero — an audible "fade in" on the
        // first block after a seek would be a bug, not a feature.
        self.smoothing.clear();
        self.smoothing.reserve(lane_count.min(MAX_PARAMETERS_PER_BLOCK));
        self.prepared = true;
        self.stats = PlayerStats::default();
    }

    /// Whether a parameter is currently being smoothed.
    #[must_use]
    pub fn is_smoothing(&self, store_index: usize) -> bool {
        self.smoothing
            .iter()
            .any(|s| s.store_index == store_index)
    }

    /// The smoothed value the player last wrote for a slot.
    #[must_use]
    pub fn smoothed_value(&self, store_index: usize) -> Option<f32> {
        self.smoothing
            .iter()
            .find(|s| s.store_index == store_index)
            .map(|s| s.current)
    }

    /// Finds or creates smoothing state for a store slot.
    ///
    /// Allocation happens only when the set of automated parameters grows —
    /// i.e. the first time a lane becomes active — and never on a steady-state
    /// block. The `reserve` in [`Self::prepare`] is sized to cover that.
    fn state_for(
        &mut self,
        store_index: usize,
        coefficient: f32,
        target: f32,
    ) -> Option<&mut SmoothingState> {
        let position = self
            .smoothing
            .iter()
            .position(|s| s.store_index == store_index);
        let index = match position {
            Some(i) => i,
            None => {
                if self.smoothing.len() >= self.capacity {
                    return None;
                }
                self.smoothing.push(SmoothingState {
                    store_index,
                    coefficient,
                    // Prime from the base value so the very first block emits
                    // the parameter's real value, not a ramp from zero.
                    current: target,
                    target,
                    primed: false,
                });
                self.smoothing.len() - 1
            }
        };
        let state = &mut self.smoothing[index];
        state.coefficient = coefficient;
        state.target = target;
        Some(state)
    }

    /// Advances every armed lane and modulator by one block.
    ///
    /// * `lanes` — every automation lane in the project.
    /// * `modulators` — every modulation source.
    /// * `store` — the registry to read base values from and write results to.
    /// * `frame` — the transport position at the *start* of the block, in
    ///   frames.
    /// * `frames` — block length, for advancing modulators and smoothing.
    ///
    /// A lane is evaluated at a single position (the block start) rather than
    /// per sample. Per-sample automation would multiply the cost by the block
    /// size for no audible benefit: the smoothing filter that follows already
    /// interpolates between block values, and PLAN §3.S2's zipper-noise
    /// requirement is met by that filter, not by the evaluation rate.
    ///
    /// Real-time safe.
    pub fn advance_block(
        &mut self,
        lanes: &[Lane],
        modulators: &mut ModulatorBank,
        store: &ParameterStore,
        frame: i64,
        frames: usize,
    ) {
        self.stats = PlayerStats::default();

        // Advance modulators once per block; their outputs are read below.
        modulators.tick(frames, self.sample_rate);

        if lanes.is_empty() && modulators.lfo_count() == 0 && modulators.envelope_count() == 0 {
            return;
        }

        // Modulator contributions are folded into a fixed stack array so the
        // hot path needs no heap. An address appearing twice is summed, which
        // is what "modulator累加" means in the normative order.
        let mut accumulator = ModulatorAccumulator::default();
        self.collect_modulator_targets(modulators, &mut accumulator);

        for lane in lanes {
            if !lane.is_enabled() {
                continue;
            }
            let Some(automated_value) = lane.value_at(frame) else {
                continue;
            };
            let address = lane.address();
            let Some(store_index) = store.index_of(address) else {
                self.stats.unresolved += 1;
                continue;
            };
            let Some(descriptor) = store.descriptor_at(store_index) else {
                self.stats.unresolved += 1;
                continue;
            };

            // ── Order: base → automation → modulator → clamp ──
            let base = store
                .read(address)
                .unwrap_or(descriptor.default_value);
            let after_automation = automated_value;
            let modulation_offset = accumulator.offset_for(address);
            let unclamped = after_automation + modulation_offset;
            let clamped = descriptor.clamp(unclamped);

            let smoothing_ms = store
                .smoothing_ms_at(store_index)
                .unwrap_or(descriptor.smoothing_ms);
            // The coefficient must describe *this block's* duration, not one
            // sample. The player advances once per block, so using a
            // per-sample coefficient would make the effective smoothing time
            // scale with the block size: a 10 ms setting would become 80 ms at
            // a 2048-frame buffer. Passing the elapsed time keeps the
            // documented 1..50 ms range meaning the same thing at every buffer
            // size, which is what the ABI promises.
            let block_ms = if self.sample_rate > 0.0 {
                (frames as f32 / self.sample_rate) * 1000.0
            } else {
                0.0
            };
            let coefficient = one_pole_coeff_for_elapsed(smoothing_ms, block_ms);

            let Some(state) = self.state_for(store_index, coefficient, clamped) else {
                // Per-block cap reached; leave the parameter at its previous
                // value rather than writing a half-processed one.
                self.stats.skipped += 1;
                continue;
            };

            if !state.primed {
                // First block for this parameter: adopt the target exactly,
                // so no spurious ramp is introduced at start or after a seek.
                state.current = clamped;
                state.primed = true;
            } else {
                // One-pole: current += (1 - coeff) * (target - current).
                let step = (1.0 - coefficient) * (clamped - state.current);
                state.current += step;
                // Snap once the residual is inaudible. The threshold is scaled
                // to the parameter's own range rather than being a fixed
                // epsilon: on a -60..+12 dB fader, 1e-7 dB is far below f32
                // resolution near the top of the range, so a fixed epsilon
                // would leave the filter grinding away forever on tiny steps
                // (and risk denormal stalls) without ever landing exactly.
                let range = (descriptor.max_value - descriptor.min_value).abs().max(1.0);
                if (clamped - state.current).abs() < range * 1e-6 {
                    state.current = clamped;
                }
            }
            let written = state.current;
            store.write_clamped_unchecked(store_index, written);

            self.stats.evaluated += 1;
            self.stats.automated += 1;
            if modulation_offset != 0.0 {
                self.stats.modulated += 1;
            }
            if base != written {
                self.stats.written += 1;
            }
        }
    }

    /// Folds every modulator's output into the accumulator.
    fn collect_modulator_targets(
        &self,
        modulators: &ModulatorBank,
        accumulator: &mut ModulatorAccumulator,
    ) {
        for index in 0..modulators.lfo_count() {
            if let Some(lfo) = modulators.lfo(index) {
                let output = lfo.value();
                if output == 0.0 {
                    continue;
                }
                for (address, depth) in lfo.target_depths() {
                    accumulator.add(address, output * depth);
                }
            }
        }
        for index in 0..modulators.envelope_count() {
            if let Some(envelope) = modulators.envelope(index) {
                let output = envelope.value();
                if output == 0.0 {
                    continue;
                }
                for (address, depth) in envelope.target_depths() {
                    accumulator.add(address, output * depth);
                }
            }
        }
    }

    /// Drops all smoothing state.
    ///
    /// Called on stop/seek so a later play does not glide from a stale value.
    pub fn reset(&mut self) {
        for state in &mut self.smoothing {
            state.primed = false;
        }
        self.stats = PlayerStats::default();
    }

    /// Forgets every smoothing entry.
    ///
    /// Used when lanes are removed; the next block re-creates only the entries
    /// that are still needed.
    pub fn clear(&mut self) {
        self.smoothing.clear();
        self.stats = PlayerStats::default();
    }
}

/// Fixed-capacity sum of modulator contributions within one block.
///
/// `MAX_PARAMETERS_PER_BLOCK` entries of `(u64, f32)`, so the whole thing is
/// 12 KiB on the stack — deliberately not a `Vec`, because a `Vec` here would
/// put an allocation in the audio path.
#[derive(Debug)]
struct ModulatorAccumulator {
    entries: [(u64, f32); MAX_PARAMETERS_PER_BLOCK],
    len: usize,
}

impl Default for ModulatorAccumulator {
    fn default() -> Self {
        Self {
            entries: [(0, 0.0); MAX_PARAMETERS_PER_BLOCK],
            len: 0,
        }
    }
}

impl ModulatorAccumulator {
    /// Adds `amount` to `address`'s running total, creating the entry if new.
    fn add(&mut self, address: ParameterAddress, amount: f32) {
        if amount == 0.0 || !amount.is_finite() {
            return;
        }
        let key = address.key();
        for entry in &mut self.entries[..self.len] {
            if entry.0 == key {
                entry.1 += amount;
                return;
            }
        }
        if self.len < MAX_PARAMETERS_PER_BLOCK {
            self.entries[self.len] = (key, amount);
            self.len += 1;
        }
        // Past the cap a contribution is dropped rather than growing the
        // array: silently missing one modulation target is far better than
        // allocating on the audio thread.
    }

    /// The summed offset for `address`, or `0.0` when nothing modulates it.
    fn offset_for(&self, address: ParameterAddress) -> f32 {
        let key = address.key();
        for entry in &self.entries[..self.len] {
            if entry.0 == key {
                return entry.1;
            }
        }
        0.0
    }
}

/// What the recorder should do with a control movement.
///
/// The player module owns this because the decision depends on playback state
/// (transport position, whether a control is held) that lives with the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    /// Record the value at the current frame.
    Write,
    /// Skip: the mode or arm state says no.
    Ignore,
    /// End the current take.
    EndTake,
}

/// Decides what a control movement should do, given the record mode.
///
/// Extracted as a pure function so the three modes' behaviour is testable
/// without an engine, a transport or a clock — the subtleties here (Touch ends
/// on release, Latch does not) are exactly the kind that regress unnoticed.
#[must_use]
pub fn touch_action(mode: RecordMode, armed: bool, touching: bool, take_open: bool) -> TouchAction {
    if !armed || !mode.is_recording() {
        return TouchAction::Ignore;
    }
    match mode {
        RecordMode::Off => TouchAction::Ignore,
        RecordMode::Touch => {
            if touching {
                TouchAction::Write
            } else if take_open {
                TouchAction::EndTake
            } else {
                TouchAction::Ignore
            }
        }
        // Latch and Write both keep writing after release, so a released
        // control never ends the take; only the transport stopping does.
        RecordMode::Latch | RecordMode::Write => TouchAction::Write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::clip::AutomationPoint;
    use crate::automation::lane::Lane;
    use crate::automation::modulator::{EnvelopeGenerator, Lfo, LfoShape};
    use crate::automation::parameter::{
        parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
    };
    use alloc::vec;

    const SR: f32 = 48_000.0;
    const VOLUME: ParameterAddress = ParameterAddress::channel(0, 0);
    const PAN: ParameterAddress = ParameterAddress::channel(0, 1);

    static VOLUME_DESC: ParameterDescriptor = ParameterDescriptor {
        address: VOLUME,
        key: "volume",
        label: "Volume",
        unit: ParameterUnit::Decibels,
        flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
        min_value: -60.0,
        max_value: 12.0,
        default_value: 0.0,
        smoothing_ms: 10.0,
    };

    static PAN_DESC: ParameterDescriptor = ParameterDescriptor {
        address: PAN,
        key: "pan",
        label: "Pan",
        unit: ParameterUnit::Linear,
        flags: parameter_flags::AUTOMATABLE | parameter_flags::BIPOLAR,
        min_value: -1.0,
        max_value: 1.0,
        default_value: 0.0,
        smoothing_ms: 10.0,
    };

    fn store_with_defaults() -> ParameterStore {
        let mut store = ParameterStore::with_capacity(8);
        store.register(&VOLUME_DESC).unwrap();
        store.register(&PAN_DESC).unwrap();
        store
    }

    fn lane(address: ParameterAddress, points: &[(i64, f32)]) -> Lane {
        Lane::with_points(
            address,
            points
                .iter()
                .map(|&(f, v)| AutomationPoint::new(f, v))
                .collect(),
        )
    }

    #[test]
    fn an_unprepared_player_still_needs_an_explicit_prepare_for_tracking() {
        let mut player = AutomationPlayer::new();
        assert!(!player.is_prepared());
        assert_eq!(player.tracked_parameters(), 0);
        player.prepare(SR, 4);
        assert!(player.is_prepared());
    }

    #[test]
    fn automation_drives_the_parameter_to_the_curve_value() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, -12.0), (1000, -12.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let value = store.read(VOLUME).unwrap();
        assert!((value + 12.0).abs() < 1e-3, "expected -12 dB, got {value}");
        assert_eq!(player.stats().automated, 1);
    }

    #[test]
    fn a_disabled_lane_leaves_the_base_value_untouched() {
        let store = store_with_defaults();
        store.write(VOLUME, -3.0);
        let mut l = lane(VOLUME, &[(0, -12.0)]);
        l.set_enabled(false);
        let lanes = vec![l];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        assert_eq!(store.read(VOLUME), Some(-3.0));
        assert_eq!(player.stats().automated, 0);
    }

    #[test]
    fn the_first_block_adopts_the_target_without_ramping() {
        // A ramp on the first block after start/seek would be an audible
        // swoop that the user never asked for.
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 6.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        assert!((store.read(VOLUME).unwrap() - 6.0).abs() < 1e-6);
    }

    #[test]
    fn smoothing_glides_toward_a_new_target_rather_than_jumping() {
        let store = store_with_defaults();
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        // Settle on -60 dB, then step the curve to 0 dB.
        let quiet = vec![lane(VOLUME, &[(0, -60.0), (100_000, -60.0)])];
        player.advance_block(&quiet, &mut modulators, &store, 0, 256);
        assert!((store.read(VOLUME).unwrap() + 60.0).abs() < 1e-3);

        let loud = vec![lane(VOLUME, &[(0, 0.0), (100_000, 0.0)])];
        player.advance_block(&loud, &mut modulators, &store, 0, 256);
        let after_one_block = store.read(VOLUME).unwrap();
        assert!(
            after_one_block > -60.0 && after_one_block < 0.0,
            "the value should be mid-glide, not jumped: {after_one_block}"
        );
    }

    #[test]
    fn smoothing_converges_and_then_snaps_exactly() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 0.0), (100_000, 0.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        // Seed at the bottom, then let the one-pole settle.
        let quiet = vec![lane(VOLUME, &[(0, -60.0), (100_000, -60.0)])];
        player.advance_block(&quiet, &mut modulators, &store, 0, 256);

        // A 10 ms one-pole over 256-frame blocks decays by ~0.2% per block, so
        // settling 60 dB of range takes a few thousand blocks. Iterating less
        // would test the filter's time constant rather than the snap.
        for _ in 0..8_000 {
            player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        }
        assert_eq!(
            store.read(VOLUME),
            Some(0.0),
            "a settled parameter must snap exactly, leaving no denormal tail"
        );
    }

    #[test]
    fn evaluation_order_is_automation_then_modulation_then_clamp() {
        // Automation sets the anchor; a modulator offsets from it; the clamp
        // is applied once at the very end. If modulation were applied before
        // automation, the LFO's contribution would be discarded here.
        let store = store_with_defaults();
        let lanes = vec![lane(PAN, &[(0, 0.5), (100_000, 0.5)])];

        let mut modulators = ModulatorBank::new();
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant); // output = 1.0
        lfo.connect(PAN, 0.25); // offset = +0.25
        modulators.add_lfo(lfo);

        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let value = store.read(PAN).unwrap();
        assert!(
            (value - 0.75).abs() < 1e-4,
            "automation 0.5 + modulation 0.25 should be 0.75, got {value}"
        );
    }

    #[test]
    fn the_final_clamp_catches_a_modulator_overshoot() {
        let store = store_with_defaults();
        let lanes = vec![lane(PAN, &[(0, 0.9), (100_000, 0.9)])];

        let mut modulators = ModulatorBank::new();
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant);
        lfo.connect(PAN, 5.0); // wildly out of range
        modulators.add_lfo(lfo);

        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        assert_eq!(
            store.read(PAN),
            Some(1.0),
            "the clamp must be the last stage"
        );
    }

    #[test]
    fn a_negative_modulator_is_not_destroyed_by_an_early_clamp() {
        // Clamping before modulation would flatten this to the minimum.
        let store = store_with_defaults();
        let lanes = vec![lane(PAN, &[(0, 0.0), (100_000, 0.0)])];

        let mut modulators = ModulatorBank::new();
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant);
        lfo.connect(PAN, -0.4);
        modulators.add_lfo(lfo);

        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let value = store.read(PAN).unwrap();
        assert!((value + 0.4).abs() < 1e-4, "expected -0.4, got {value}");
    }

    #[test]
    fn two_modulators_on_one_parameter_are_summed() {
        let store = store_with_defaults();
        let lanes = vec![lane(PAN, &[(0, 0.0), (100_000, 0.0)])];

        let mut modulators = ModulatorBank::new();
        for depth in [0.1_f32, 0.2] {
            let mut lfo = Lfo::new();
            lfo.set_shape(LfoShape::Constant);
            lfo.connect(PAN, depth);
            modulators.add_lfo(lfo);
        }

        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let value = store.read(PAN).unwrap();
        assert!((value - 0.3).abs() < 1e-4, "expected 0.3, got {value}");
    }

    #[test]
    fn an_envelope_modulates_while_gated_and_stops_after_release() {
        let store = store_with_defaults();
        let lanes = vec![lane(PAN, &[(0, 0.0), (100_000, 0.0)])];

        let mut modulators = ModulatorBank::new();
        let mut envelope = EnvelopeGenerator::new();
        envelope.set_adsr(0.001, 0.001, 0.5, 0.001);
        envelope.connect(PAN, 0.5);
        let index = modulators.add_envelope(envelope);

        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        modulators.envelope_mut(index).unwrap().gate_on();
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        let modulated = store.read(PAN).unwrap();
        assert!(modulated > 0.0, "the envelope should be lifting pan: {modulated}");

        modulators.envelope_mut(index).unwrap().gate_off();
        // The smoothing filter, not the envelope, is what governs how long the
        // parameter takes to return: a 10 ms one-pole needs a few hundred
        // blocks of 256 frames to settle, and the envelope's own 1 ms release
        // is over almost immediately. Iterating "a few" blocks would be a test
        // of the filter's time constant rather than of the release behaviour.
        for _ in 0..2_000 {
            player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        }
        let released = store.read(PAN).unwrap();
        assert!(
            released.abs() < 1e-3,
            "after release the parameter should return to its base: {released}"
        );
    }

    #[test]
    fn an_unregistered_lane_is_counted_not_silently_dropped() {
        let store = store_with_defaults();
        let lanes = vec![lane(ParameterAddress::channel(77, 0), &[(0, 1.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        assert_eq!(player.stats().unresolved, 1);
        assert_eq!(player.stats().automated, 0);
        // The engine must keep running; a stale lane cannot poison a parameter.
        assert_eq!(store.read(VOLUME), Some(0.0));
    }

    #[test]
    fn an_empty_block_with_no_lanes_is_a_no_op() {
        let store = store_with_defaults();
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&[], &mut modulators, &store, 0, 256);
        assert_eq!(player.stats().evaluated, 0);
        assert_eq!(store.read(VOLUME), Some(0.0));
    }

    #[test]
    fn many_lanes_in_one_block_are_all_evaluated() {
        // Mirrors the acceptance case: a project with a lot of automation
        // must evaluate every lane in a single block, with no truncation.
        // The lane count is chosen to sit exactly at the supported ceiling, so
        // this test fails loudly if the ceiling is ever lowered by accident.
        let lane_count = MAX_TRACKED_PARAMETERS as u32;
        let mut store = ParameterStore::with_capacity(lane_count as usize);
        let mut lanes = Vec::new();
        for index in 0..lane_count {
            let descriptor: &'static ParameterDescriptor =
                alloc::boxed::Box::leak(alloc::boxed::Box::new(ParameterDescriptor {
                    address: ParameterAddress::channel(index, 0),
                    key: "volume",
                    label: "Volume",
                    unit: ParameterUnit::Decibels,
                    flags: parameter_flags::AUTOMATABLE,
                    min_value: -60.0,
                    max_value: 12.0,
                    default_value: 0.0,
                    smoothing_ms: 10.0,
                }));
            store.register(descriptor).unwrap();
            lanes.push(lane(
                ParameterAddress::channel(index, 0),
                &[(0, -6.0), (100_000, -6.0)],
            ));
        }

        let mut player = AutomationPlayer::new();
        player.prepare(SR, lanes.len());

        let mut modulators = ModulatorBank::new();
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let stats = player.stats();
        assert_eq!(stats.evaluated, lane_count as usize);
        assert_eq!(stats.automated, lane_count as usize);
        assert_eq!(stats.skipped, 0, "no lane should have been skipped");
        assert_eq!(
            store.read(ParameterAddress::channel(lane_count - 1, 0)),
            Some(-6.0),
            "even the last lane must have been written"
        );
    }

    #[test]
    fn the_tracking_cap_is_reported_rather_than_hidden() {
        let mut store = ParameterStore::with_capacity(MAX_PARAMETERS_PER_BLOCK + 10);
        let mut lanes = Vec::new();
        for index in 0..(MAX_PARAMETERS_PER_BLOCK + 5) as u32 {
            let descriptor: &'static ParameterDescriptor =
                alloc::boxed::Box::leak(alloc::boxed::Box::new(ParameterDescriptor {
                    address: ParameterAddress::channel(index, 0),
                    key: "volume",
                    label: "Volume",
                    unit: ParameterUnit::Linear,
                    flags: parameter_flags::AUTOMATABLE,
                    min_value: 0.0,
                    max_value: 1.0,
                    default_value: 0.0,
                    smoothing_ms: 1.0,
                }));
            store.register(descriptor).unwrap();
            lanes.push(lane(
                ParameterAddress::channel(index, 0),
                &[(0, 1.0), (100_000, 1.0)],
            ));
        }

        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, lanes.len());
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        assert_eq!(player.stats().skipped, 5);
        assert_eq!(player.tracked_parameters(), MAX_PARAMETERS_PER_BLOCK);
    }

    #[test]
    fn reset_drops_priming_so_the_next_block_snaps() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 6.0), (100_000, 6.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        store.write(VOLUME, -60.0);

        player.reset();
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        assert!(
            (store.read(VOLUME).unwrap() - 6.0).abs() < 1e-6,
            "after a reset the parameter must snap to the curve, not glide"
        );
    }

    #[test]
    fn clear_forgets_tracked_parameters() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 3.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        assert_eq!(player.tracked_parameters(), 1);
        player.clear();
        assert_eq!(player.tracked_parameters(), 0);
    }

    #[test]
    fn smoothing_time_is_independent_of_block_size() {
        // The documented 1..50 ms range must mean the same wall-clock time at
        // every buffer size. A naive per-sample coefficient applied once per
        // block makes a 10 ms setting behave like 80 ms at a 2048-frame
        // buffer, which is exactly the kind of bug that only shows up on a
        // user's machine with a different audio device.
        let settle = |block_frames: usize| -> f32 {
            let store = store_with_defaults();
            let lanes = vec![lane(VOLUME, &[(0, 0.0), (100_000, 0.0)])];
            let mut modulators = ModulatorBank::new();
            let mut player = AutomationPlayer::new();
            player.prepare(SR, 1);

            // Seed at the bottom of the range, then measure after ~100 ms.
            let quiet = vec![lane(VOLUME, &[(0, -60.0), (100_000, -60.0)])];
            player.advance_block(&quiet, &mut modulators, &store, 0, block_frames);

            let blocks = ((SR * 0.1) as usize) / block_frames;
            for _ in 0..blocks {
                player.advance_block(&lanes, &mut modulators, &store, 0, block_frames);
            }
            store.read(VOLUME).unwrap()
        };

        // A 10 ms one-pole reaches ~63% of the way in 10 ms, so after 100 ms it
        // is within a fraction of a dB of the target. Both buffer sizes must
        // land in the same neighbourhood.
        let at_256 = settle(256);
        let at_2048 = settle(2048);
        assert!(
            at_256 > -3.0,
            "at 256 frames the value should have nearly settled: {at_256}"
        );
        assert!(
            (at_256 - at_2048).abs() < 1.0,
            "smoothing must not depend on block size: 256 → {at_256}, 2048 → {at_2048}"
        );
    }

    #[test]
    fn a_one_millisecond_smoothing_time_is_nearly_transparent() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 9.0), (100_000, 9.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);

        // Seed a low value, then step to 9 dB with the fastest legal smoothing.
        let quiet = vec![lane(VOLUME, &[(0, -40.0), (100_000, -40.0)])];
        player.advance_block(&quiet, &mut modulators, &store, 0, 256);

        store.set_smoothing_ms(VOLUME, 1.0);
        // 100 ms is 100 time constants at a 1 ms setting, so the glide is over
        // long before this loop finishes.
        for _ in 0..20 {
            player.advance_block(&lanes, &mut modulators, &store, 0, 256);
        }
        assert!(
            store.read(VOLUME).unwrap() > 8.0,
            "a 1 ms smoothing time must be nearly transparent"
        );
    }

    #[test]
    fn the_player_reports_which_parameters_it_smoothes() {
        let store = store_with_defaults();
        let lanes = vec![lane(VOLUME, &[(0, 1.0)])];
        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(SR, 1);
        player.advance_block(&lanes, &mut modulators, &store, 0, 256);

        let index = store.index_of(VOLUME).unwrap();
        assert!(player.is_smoothing(index));
        assert!(player.smoothed_value(index).is_some());
        assert!(!player.is_smoothing(store.index_of(PAN).unwrap()));
    }

    #[test]
    fn touch_mode_writes_only_while_touched_and_ends_on_release() {
        // Touch: the safe default. Releasing ends the take and only the
        // touched span is affected.
        assert_eq!(
            touch_action(RecordMode::Touch, true, true, false),
            TouchAction::Write
        );
        assert_eq!(
            touch_action(RecordMode::Touch, true, false, true),
            TouchAction::EndTake
        );
        assert_eq!(
            touch_action(RecordMode::Touch, true, false, false),
            TouchAction::Ignore
        );
    }

    #[test]
    fn latch_and_write_keep_going_after_release() {
        for mode in [RecordMode::Latch, RecordMode::Write] {
            assert_eq!(touch_action(mode, true, true, false), TouchAction::Write);
            assert_eq!(
                touch_action(mode, true, false, true),
                TouchAction::Write,
                "{mode:?} must not end its take on release"
            );
        }
    }

    #[test]
    fn recording_requires_both_a_mode_and_an_armed_lane() {
        assert_eq!(
            touch_action(RecordMode::Off, true, true, false),
            TouchAction::Ignore
        );
        assert_eq!(
            touch_action(RecordMode::Touch, false, true, false),
            TouchAction::Ignore,
            "an unarmed lane must never be overwritten"
        );
        for mode in [RecordMode::Touch, RecordMode::Latch, RecordMode::Write] {
            assert_eq!(
                touch_action(mode, false, true, true),
                TouchAction::Ignore,
                "{mode:?} must respect the arm state"
            );
        }
    }
}
