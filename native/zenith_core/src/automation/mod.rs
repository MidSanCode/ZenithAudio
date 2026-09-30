//! Parameter system and automation (PLAN §3.S2).
//!
//! # What this module owns
//!
//! * a **registry** of every automatable parameter, addressable by a compact
//!   integer triple rather than a string ([`store`]);
//! * **clips and lanes** — the curves, their interpolation and their editing
//!   surface ([`clip`], [`lane`]);
//! * the **player** — the real-time evaluation path that turns base values,
//!   automation and modulation into the value the DSP graph reads ([`player`]);
//! * **modulation sources** ([`modulator`]);
//! * **recording** of control movements back into clips ([`recorder`]).
//!
//! # The normative evaluation order
//!
//! ```text
//!   base value  →  automation  →  modulator sum  →  clamp
//! ```
//!
//! Fixed, documented, and implemented in exactly one place
//! ([`player::AutomationPlayer::advance_block`]). See [`parameter`] for the
//! reasoning behind the order and why it must not be re-derived locally.
//!
//! # Relationship to S1
//!
//! This module deliberately holds **no reference to the DSP graph**. It reads
//! and writes a [`store::ParameterStore`] and nothing else, so it compiles,
//! runs and is fully testable before S1 lands. Wiring it into the audio
//! callback is a single call to [`player::AutomationPlayer::advance_block`] at
//! a block boundary, which is Agent-A's change to make once
//! `native/zenith_core/src/engine/` exists.
//!
//! # Real-time safety
//!
//! Everything reachable from [`player::AutomationPlayer::advance_block`] is
//! allocation-free and lock-free. The test
//! `advance_block_does_not_allocate` enforces that at runtime rather than
//! asserting it in prose: it installs a global allocator that panics on any
//! allocation and then runs a full evaluation pass.

pub mod clip;
pub mod lane;
pub mod modulator;
pub mod parameter;
pub mod player;
pub mod recorder;
pub mod store;

pub use clip::{AutomationClip, AutomationPoint, CurveKind};
pub use lane::{Lane, LaneProblem, LaneSet, RecordMode};
pub use modulator::{
    one_pole_coeff, one_pole_coeff_for_elapsed, EnvelopeGenerator, EnvelopeStage, Lfo, LfoShape,
    LfoTriggerMode, ModulatorBank, PeakFollower,
};
pub use parameter::{
    ParameterAddress, ParameterDescriptor, ParameterKind, ParameterUnit,
};
pub use player::{AutomationPlayer, PlayerStats, TouchAction, MAX_TRACKED_PARAMETERS};
pub use recorder::{RecordOutcome, Recorder, Take};
pub use store::{ParameterStore, DEFAULT_PARAMETER_CAPACITY, MAX_SMOOTHING_MS, MIN_SMOOTHING_MS};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    // ── A panicking allocator, to prove the hot path allocates nothing ──
    //
    // Installed only for this test binary. Any allocation while the flag is
    // armed aborts the test with a message naming the operation, which is far
    // more useful than a corrupted audio buffer in production.

    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicBool, Ordering};

    /// Set while the real-time path is under test.
    static WATCH_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
    /// Set once an allocation was observed, to avoid re-entering the hook.
    static ALLOCATION_SEEN: AtomicBool = AtomicBool::new(false);

    /// Wraps the system allocator, tripping when armed.
    struct WatchingAllocator;

    // SAFETY: every method forwards directly to `System` with the same
    // layout and pointer, so the allocator contract is exactly `System`'s.
    unsafe impl GlobalAlloc for WatchingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if WATCH_ALLOCATIONS.load(Ordering::Relaxed) {
                ALLOCATION_SEEN.store(true, Ordering::Relaxed);
            }
            unsafe { std::alloc::System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            if WATCH_ALLOCATIONS.load(Ordering::Relaxed) {
                ALLOCATION_SEEN.store(true, Ordering::Relaxed);
            }
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: WatchingAllocator = WatchingAllocator;

    fn percentile_addresses(count: u32) -> Vec<ParameterAddress> {
        (0..count).map(|i| ParameterAddress::channel(i, 0)).collect()
    }

    fn leaked_descriptor(address: ParameterAddress) -> &'static ParameterDescriptor {
        alloc::boxed::Box::leak(alloc::boxed::Box::new(ParameterDescriptor {
            address,
            key: "volume",
            label: "Volume",
            unit: ParameterUnit::Decibels,
            flags: parameter::parameter_flags::AUTOMATABLE,
            min_value: -60.0,
            max_value: 12.0,
            default_value: 0.0,
            smoothing_ms: 10.0,
        }))
    }

    #[test]
    fn the_evaluation_order_is_documented_and_single_sourced() {
        // This test exists to fail loudly if someone re-derives the order
        // somewhere else. It asserts the observable consequence: modulation
        // offsets automation, and the clamp happens after both.
        let address = ParameterAddress::channel(0, 0);
        let mut store = ParameterStore::with_capacity(4);
        store.register(leaked_descriptor(address)).unwrap();

        let mut lanes = LaneSet::new();
        lanes.add(Lane::with_points(
            address,
            alloc::vec![
                AutomationPoint::new(0, 0.5),
                AutomationPoint::new(100_000, 0.5)
            ],
        ));

        let mut modulators = ModulatorBank::new();
        let mut lfo = Lfo::new();
        lfo.set_shape(LfoShape::Constant);
        lfo.connect(address, 20.0); // deliberately out of range

        modulators.add_lfo(lfo);

        let mut player = AutomationPlayer::new();
        player.prepare(48_000.0, 1);
        player.advance_block(lanes.lanes(), &mut modulators, &store, 0, 256);

        // Automation said 0.5, modulation pushed it to 20.5, clamp capped it.
        assert_eq!(store.read(address), Some(12.0));
    }

    #[test]
    fn advance_block_does_not_allocate() {
        // The PLAN §3.S2 acceptance criterion "求值路径零分配", enforced rather
        // than asserted. Fixture construction is done with the watcher off;
        // only the steady-state evaluation runs with it armed.
        let track_count = 128u32;
        let mut store = ParameterStore::with_capacity(track_count as usize);
        let mut lanes = LaneSet::new();

        for address in percentile_addresses(track_count) {
            store.register(leaked_descriptor(address)).unwrap();
            // A curve with a handful of points, as a real project would have.
            lanes.add(Lane::with_points(
                address,
                alloc::vec![
                    AutomationPoint::new(0, -12.0),
                    AutomationPoint::new(9_600, 0.0),
                    AutomationPoint::new(19_200, -6.0),
                    AutomationPoint::new(28_800, 3.0),
                ],
            ));
        }

        let mut modulators = ModulatorBank::new();
        for index in 0..8 {
            let mut lfo = Lfo::new();
            lfo.set_shape(LfoShape::Sine);
            lfo.set_rate_hz(0.5 + index as f32);
            // Spread the targets across the tracks.
            lfo.connect(ParameterAddress::channel(index, 0), 1.0);
            modulators.add_lfo(lfo);
        }

        let mut player = AutomationPlayer::new();
        player.prepare(48_000.0, lanes.len());

        // Warm-up: the first block legitimately creates smoothing state for
        // every parameter, which is a one-time control-thread-sized cost.
        player.advance_block(lanes.lanes(), &mut modulators, &store, 0, 256);
        assert_eq!(
            player.tracked_parameters(),
            track_count as usize,
            "every parameter should be tracked after the warm-up block"
        );

        // Now the steady state must be allocation-free.
        ALLOCATION_SEEN.store(false, Ordering::Relaxed);
        WATCH_ALLOCATIONS.store(true, Ordering::Relaxed);

        let mut frame = 256_i64;
        for _ in 0..600 {
            player.advance_block(lanes.lanes(), &mut modulators, &store, frame, 256);
            frame += 256;
        }

        WATCH_ALLOCATIONS.store(false, Ordering::Relaxed);
        assert!(
            !ALLOCATION_SEEN.load(Ordering::Relaxed),
            "advance_block allocated during steady-state evaluation — this is a \
             real-time safety violation (P5)"
        );
    }

    #[test]
    fn mutating_lanes_while_playing_is_not_required_to_be_allocation_free() {
        // Sanity check that the watcher actually works: a deliberately
        // allocating operation must trip it. Without this, a broken watcher
        // would make the test above pass vacuously.
        ALLOCATION_SEEN.store(false, Ordering::Relaxed);
        WATCH_ALLOCATIONS.store(true, Ordering::Relaxed);
        let mut clip = AutomationClip::new();
        clip.insert(AutomationPoint::new(0, 1.0));
        WATCH_ALLOCATIONS.store(false, Ordering::Relaxed);
        assert!(
            ALLOCATION_SEEN.load(Ordering::Relaxed),
            "the allocation watcher is not detecting allocations"
        );
    }

    #[test]
    fn a_thousand_points_evaluate_cheaply() {
        // The PLAN §3.S2 acceptance target is "1000 points under 2% CPU". A
        // wall-clock budget here is a smoke check, not a benchmark: it catches
        // an accidental O(n^2) or per-sample scan, which is what actually
        // regresses. The real measurement is S9's.
        let mut store = ParameterStore::with_capacity(16);
        let mut lanes = LaneSet::new();

        // 1000 points spread over 10 parameters at 100 points each.
        for channel in 0..10u32 {
            let address = ParameterAddress::channel(channel, 0);
            store.register(leaked_descriptor(address)).unwrap();
            let points: Vec<AutomationPoint> = (0..100)
                .map(|i| AutomationPoint::new(i * 480, (i % 20) as f32 - 10.0))
                .collect();
            lanes.add(Lane::with_points(address, points));
        }
        assert_eq!(lanes.total_points(), 1000);

        let mut modulators = ModulatorBank::new();
        let mut player = AutomationPlayer::new();
        player.prepare(48_000.0, lanes.len());

        let start = std::time::Instant::now();
        let blocks = 10_000;
        let mut frame = 0_i64;
        for _ in 0..blocks {
            player.advance_block(lanes.lanes(), &mut modulators, &store, frame, 256);
            frame += 256;
        }
        let elapsed = start.elapsed();

        // 10 000 blocks at 256 frames = 2 560 000 frames ≈ 53 seconds of audio.
        // Evaluating must take a small fraction of that.
        assert!(
            elapsed.as_millis() < 2_000,
            "evaluating 1000 points took {elapsed:?} for ~53 s of audio, which is \
             far above the 2% CPU target"
        );
        assert_eq!(player.stats().skipped, 0);
    }

    #[test]
    fn the_public_reexports_are_complete() {
        // A compile-level check that the module's surface is what the FFI layer
        // expects to import; if a re-export is dropped, this fails to build.
        let address = ParameterAddress::global(0);
        let lane = Lane::new(address);
        let clip = AutomationClip::new();
        let store = ParameterStore::new();
        let player = AutomationPlayer::new();
        let recorder = Recorder::new();
        let modulators = ModulatorBank::new();
        let set = LaneSet::new();

        assert!(lane.clip().is_empty());
        assert!(clip.is_empty());
        assert!(store.is_empty());
        assert!(!player.is_prepared());
        assert!(!recorder.is_enabled());
        assert_eq!(modulators.lfo_count(), 0);
        assert!(set.is_empty());
        assert_eq!(RecordMode::default(), RecordMode::Off);
        assert_eq!(CurveKind::default(), CurveKind::Linear);
    }
}
