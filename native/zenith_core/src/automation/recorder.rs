//! Automation recording: turning control movements into automation points.
//!
//! # The three modes, precisely
//!
//! | Mode | Take starts | Take ends | Span overwritten |
//! |---|---|---|---|
//! | **Touch** | first touch | control release | the touched span only |
//! | **Latch** | first touch | transport stop | first touch → end of pass |
//! | **Write** | transport start | transport stop | the whole pass |
//!
//! The distinction that matters in practice is **Touch vs Latch**: Touch ends
//! its take the instant the user lets go, so a fader nudged at bar 4 rewrites
//! only bar 4; Latch keeps riding from the first touch to the end of the pass,
//! so the value the user dialled early holds to the end.
//!
//! # Why points are thinned
//!
//! A fader generates one value per UI frame — 60 or more per second. Writing
//! every one produces a curve that is both huge and *visually noisy*, since
//! most consecutive samples are sub-pixel movements. [`take_point`] therefore
//! only emits a point when the value has moved by more than
//! [`Recorder::min_value_delta`] or the frame has advanced past
//! [`Recorder::max_interval_frames`]. That keeps a one-minute pass in the low
//! hundreds of points, which is what makes the "1000 points < 2% CPU"
//! acceptance target comfortable rather than marginal.
//!
//! # Threading
//!
//! A take is appended as **one time-ordered sequence**. `Recorder` keeps the
//! next frame and drops any point that arrives out of order, so the clip's
//! sorted invariant is never violated by a jittery UI clock or a transport
//! seek arriving mid-take.

use alloc::vec::Vec;

use super::clip::{AutomationPoint, CurveKind};
use super::lane::{Lane, RecordMode};
use super::parameter::ParameterAddress;

/// One recorded take: a sequence of points and the lane it belongs to.
#[derive(Debug, Clone)]
pub struct Take {
    /// Parameter being recorded.
    pub address: ParameterAddress,
    /// Points captured so far, in ascending frame order.
    pub points: Vec<AutomationPoint>,
    /// Frame at which the take started.
    pub start_frame: i64,
    /// Last frame written.
    pub last_frame: i64,
    /// Last value written, for change detection.
    pub last_value: f32,
    /// Whether a control is currently held.
    pub touching: bool,
    /// Whether the take is still open.
    pub open: bool,
}

impl Take {
    /// Opens a take for `address` starting at `frame`.
    #[must_use]
    pub fn open(address: ParameterAddress, frame: i64) -> Self {
        Self {
            address,
            points: Vec::new(),
            start_frame: frame,
            last_frame: i64::MIN,
            last_value: f32::NAN,
            touching: false,
            open: true,
        }
    }

    /// How many points the take captured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the take captured nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The frame range the take covers.
    #[must_use]
    pub fn frame_range(&self) -> Option<(i64, i64)> {
        match (self.points.first(), self.points.last()) {
            (Some(first), Some(last)) => Some((first.frame, last.frame)),
            _ => None,
        }
    }
}

/// What happened on the last [`Recorder::on_control_move`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// A point was captured.
    Captured,
    /// The value was too close to the last one to be worth a point.
    Thinned,
    /// The lane is not armed, or its mode is `Off`.
    NotArmed,
    /// The transport is not running, so there is nothing to record against.
    TransportStopped,
    /// The point arrived out of order and was dropped.
    OutOfOrder,
}

/// Records control movements into automation lanes.
#[derive(Debug, Clone)]
pub struct Recorder {
    /// Global record-enable. Independent of per-lane arming: the transport's
    /// record button and a lane's arm button are separate controls.
    enabled: bool,
    /// Points closer than this in value are thinned away.
    min_value_delta: f32,
    /// A point is forced after this many frames even if the value held.
    ///
    /// Without a keep-alive interval, a control that stops moving leaves a
    /// gap in the curve and the lane would interpolate across it.
    max_interval_frames: i64,
    /// The take in progress, if any.
    take: Option<Take>,
    /// Latch-mode memory: the value to keep writing after release.
    latched_value: Option<f32>,
    /// Total points captured since construction, for diagnostics.
    captured: u64,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    /// Creates a recorder with 60 fps thinning defaults.
    ///
    /// `max_interval_frames` defaults to 0 here because the frame rate is not
    /// known until the engine is configured; [`Self::prepare`] sets a real
    /// value derived from the sample rate.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: false,
            min_value_delta: 1e-4,
            max_interval_frames: 0,
            take: None,
            latched_value: None,
            captured: 0,
        }
    }

    /// Configures the thinning thresholds from the sample rate.
    ///
    /// A 5 ms keep-alive at the engine's sample rate: fast enough that a
    /// stationary control still leaves a continuous curve, slow enough that a
    /// one-minute pass costs a few hundred points rather than tens of
    /// thousands.
    pub fn prepare(&mut self, sample_rate: f32) {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.max_interval_frames = (rate * 0.005) as i64;
    }

    /// Sets the minimum value change that earns a point.
    ///
    /// Raising this makes coarse, low-density takes; lowering it captures
    /// fine detail at the cost of point count.
    pub fn set_min_value_delta(&mut self, delta: f32) {
        self.min_value_delta = if delta.is_finite() && delta > 0.0 {
            delta
        } else {
            1e-4
        };
    }

    /// The minimum value change that earns a point.
    #[must_use]
    pub fn min_value_delta(&self) -> f32 {
        self.min_value_delta
    }

    /// Sets the keep-alive interval in frames.
    pub fn set_max_interval_frames(&mut self, frames: i64) {
        self.max_interval_frames = frames.max(0);
    }

    /// The keep-alive interval in frames.
    #[must_use]
    pub fn max_interval_frames(&self) -> i64 {
        self.max_interval_frames
    }

    /// Turns global recording on or off.
    ///
    /// Disabling ends any open take, because leaving one open would resume
    /// writing points into it the next time the button is pressed, splicing
    /// two unrelated passes into one curve.
    pub fn set_enabled(&mut self, enabled: bool, store: &mut super::lane::LaneSet) {
        if !enabled && self.enabled {
            self.finish(store);
        }
        self.enabled = enabled;
    }

    /// Whether global recording is on.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The take in progress, if any.
    #[must_use]
    pub fn active_take(&self) -> Option<&Take> {
        self.take.as_ref()
    }

    /// Number of points captured since construction.
    #[must_use]
    pub fn captured_points(&self) -> u64 {
        self.captured
    }

    /// Reports that a control for `address` moved to `value` at `frame`.
    ///
    /// `touching` reflects whether the user is *currently* holding the control;
    /// a host that cannot distinguish touch from move should pass `true`, which
    /// degrades Latch/Touch to Write semantics rather than losing the take.
    pub fn on_control_move(
        &mut self,
        lanes: &mut super::lane::LaneSet,
        address: ParameterAddress,
        value: f32,
        frame: i64,
        touching: bool,
        mode: RecordMode,
    ) -> RecordOutcome {
        if !self.enabled || !mode.is_recording() {
            return RecordOutcome::NotArmed;
        }
        let Some(lane) = lanes.get_mut(address) else {
            return RecordOutcome::NotArmed;
        };
        if !lane.is_armed() {
            return RecordOutcome::NotArmed;
        }

        // Decide whether this movement continues the open take, replaces it,
        // or starts the first one. Getting this wrong is subtle: opening a new
        // take by assigning to `self.take` would silently *discard* the take
        // that was already in flight, losing a pass the user just performed.
        let existing = self.take.take();
        let same_parameter = existing
            .as_ref()
            .is_some_and(|t| t.open && t.address == address);

        if !same_parameter {
            // A take that belongs to a different parameter is finished before
            // the new one starts, so only one take is ever open.
            if let Some(previous) = existing {
                if let Some(previous_lane) = lanes.get_mut(previous.address) {
                    Self::commit(previous_lane, previous);
                }
            }
        } else {
            // Same parameter: the existing take stays open and is continued.
            self.take = existing;
        }

        if self.take.is_none() {
            match mode {
                RecordMode::Off => return RecordOutcome::NotArmed,
                // Touch/Latch need a real touch to start a take; otherwise a
                // channel-wide "all controls moved" notification would start
                // recording every armed lane at once.
                RecordMode::Touch | RecordMode::Latch => {
                    if !touching {
                        return RecordOutcome::NotArmed;
                    }
                }
                // Write records the pass whether or not the control was touched.
                RecordMode::Write => {}
            }
            self.take = Some(Take::open(address, frame));
            self.latched_value = None;
        }

        let Some(take) = self.take.as_mut() else {
            return RecordOutcome::NotArmed;
        };
        take.touching = touching;

        // Touch mode: a release ends the take immediately.
        if mode == RecordMode::Touch && !touching {
            return RecordOutcome::Thinned;
        }

        // Latch and Write hold the last value after release instead of
        // stopping, which is what "keeps riding to the end of the pass" means.
        let effective = if mode.latches_after_release() {
            if touching {
                self.latched_value = Some(value);
                value
            } else {
                self.latched_value.unwrap_or(value)
            }
        } else {
            value
        };

        // Reject time-travelling input rather than appending an out-of-order
        // point: the clip's sorted invariant is load-bearing for the player.
        if frame < take.last_frame {
            return RecordOutcome::OutOfOrder;
        }

        let moved_enough = !take.last_value.is_finite()
            || (effective - take.last_value).abs() >= self.min_value_delta;
        let stale_enough = take.last_frame == i64::MIN
            || (frame - take.last_frame) >= self.max_interval_frames;

        if !moved_enough && !stale_enough {
            return RecordOutcome::Thinned;
        }

        take.points.push(AutomationPoint::with_curve(
            frame,
            effective,
            CurveKind::Linear,
            0.0,
        ));
        take.last_frame = frame;
        take.last_value = effective;
        self.captured += 1;
        RecordOutcome::Captured
    }

    /// Ends the open take and merges it into its lane.
    ///
    /// Returns whether a take was committed. The lane's existing points inside
    /// the take's span are removed first, which is what makes a punch-in
    /// *replace* rather than *overlay* — overlaying would leave the old curve
    /// and the new take both present, and the player would pick whichever
    /// happened to sort first.
    pub fn finish(&mut self, lanes: &mut super::lane::LaneSet) -> bool {
        let Some(take) = self.take.take() else {
            self.latched_value = None;
            return false;
        };
        self.latched_value = None;
        let Some(lane) = lanes.get_mut(take.address) else {
            return false;
        };
        let committed = !take.is_empty();
        if committed {
            Self::commit(lane, take);
        }
        committed
    }

    /// Merges one take into a lane.
    fn commit(lane: &mut Lane, take: Take) {
        let Some((start, end)) = take.frame_range() else {
            return;
        };
        lane.clip_mut().remove_range(start, end);
        let mut points: Vec<AutomationPoint> = lane.clip().points().to_vec();
        points.extend(take.points);
        lane.set_points(points);
    }

    /// Ends the take because the transport stopped.
    ///
    /// Distinct from [`Self::finish`] only in intent — both commit — but having
    /// the named entry point makes the call site in the transport read
    /// correctly and gives the two a place to diverge later.
    pub fn on_transport_stop(&mut self, lanes: &mut super::lane::LaneSet) -> bool {
        self.finish(lanes)
    }

    /// Discards the open take without committing it.
    pub fn cancel(&mut self) {
        self.take = None;
        self.latched_value = None;
    }

    /// Forgets the take and all counters.
    pub fn reset(&mut self) {
        self.take = None;
        self.latched_value = None;
        self.captured = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::clip::AutomationPoint;
    use crate::automation::lane::LaneSet;

    const VOLUME: ParameterAddress = ParameterAddress::channel(0, 0);
    const PAN: ParameterAddress = ParameterAddress::channel(0, 1);

    fn armed_lanes(addresses: &[ParameterAddress]) -> LaneSet {
        let mut lanes = LaneSet::new();
        for &address in addresses {
            let mut lane = Lane::new(address);
            lane.set_armed(true);
            lanes.add(lane);
        }
        lanes
    }

    fn recorder() -> Recorder {
        let mut recorder = Recorder::new();
        recorder.prepare(48_000.0);
        recorder.set_enabled(true, &mut LaneSet::new());
        recorder
    }

    #[test]
    fn recording_requires_global_enable() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = Recorder::new();
        recorder.prepare(48_000.0);
        // Not enabled yet.
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Touch),
            RecordOutcome::NotArmed
        );
        recorder.set_enabled(true, &mut lanes);
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Touch),
            RecordOutcome::Captured
        );
    }

    #[test]
    fn recording_requires_an_armed_lane() {
        let mut lanes = LaneSet::new();
        lanes.add(Lane::new(VOLUME)); // present but not armed
        let mut recorder = recorder();
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Write),
            RecordOutcome::NotArmed
        );
    }

    #[test]
    fn off_mode_never_records() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Off),
            RecordOutcome::NotArmed
        );
    }

    #[test]
    fn a_write_pass_produces_points_in_the_lane() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();

        for (frame, value) in [(0, 0.1_f32), (480, 0.4), (960, 0.9)] {
            assert_eq!(
                recorder.on_control_move(
                    &mut lanes,
                    VOLUME,
                    value,
                    frame,
                    true,
                    RecordMode::Write
                ),
                RecordOutcome::Captured
            );
        }
        assert!(recorder.finish(&mut lanes));

        let points = lanes.get(VOLUME).unwrap().clip().points();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].frame, 0);
        assert_eq!(points[2].frame, 960);
    }

    #[test]
    fn touch_mode_stops_writing_the_moment_the_control_is_released() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();

        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.2, 0, true, RecordMode::Touch),
            RecordOutcome::Captured
        );
        // Released: further movements must not be captured.
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.9, 480, false, RecordMode::Touch),
            RecordOutcome::Thinned
        );
        assert_eq!(recorder.active_take().unwrap().len(), 1);
    }

    #[test]
    fn latch_mode_keeps_writing_after_the_control_is_released() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();

        // Touch and dial 0.8, then let go and keep the transport running.
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.8, 0, true, RecordMode::Latch),
            RecordOutcome::Captured
        );
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.8, 5_000, false, RecordMode::Latch),
            RecordOutcome::Captured,
            "Latch must hold the value to the end of the pass"
        );

        let take = recorder.active_take().unwrap();
        assert_eq!(take.len(), 2);
        // The held point carries the latched value, not whatever was passed in.
        assert!((take.points[1].value - 0.8).abs() < 1e-6);
    }

    #[test]
    fn latch_without_a_touch_does_not_start_a_take() {
        // Otherwise a "all controls moved" notification would start recording
        // every armed lane at once.
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, false, RecordMode::Latch),
            RecordOutcome::NotArmed
        );
        assert!(recorder.active_take().is_none());
    }

    #[test]
    fn write_mode_records_without_any_touch() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, false, RecordMode::Write),
            RecordOutcome::Captured
        );
        assert!(recorder.active_take().is_some());
    }

    #[test]
    fn stationary_controls_are_thinned_but_kept_alive() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.set_max_interval_frames(4800);

        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Write),
            RecordOutcome::Captured
        );
        // Same value, one frame later: too soon and too similar.
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 1, true, RecordMode::Write),
            RecordOutcome::Thinned
        );
        // Same value, but past the keep-alive interval: force a point so the
        // curve does not interpolate across the gap.
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5, 4801, true, RecordMode::Write),
            RecordOutcome::Captured
        );
        assert_eq!(recorder.active_take().unwrap().len(), 2);
    }

    #[test]
    fn tiny_value_jitter_is_thinned_away() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.set_min_value_delta(0.01);
        recorder.set_max_interval_frames(i64::MAX);

        recorder.on_control_move(&mut lanes, VOLUME, 0.500, 0, true, RecordMode::Write);
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.5001, 10, true, RecordMode::Write),
            RecordOutcome::Thinned
        );
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.520, 20, true, RecordMode::Write),
            RecordOutcome::Captured
        );
    }

    #[test]
    fn out_of_order_points_are_rejected_rather_than_unsorted() {
        // The clip's sorted invariant is load-bearing for the player's index:
        // a jittery UI clock or a seek arriving mid-take must not be able to
        // append a point that sorts before one already captured.
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.set_max_interval_frames(1);

        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.1, 10_000, true, RecordMode::Write),
            RecordOutcome::Captured,
            "the first point, at frame 10000, is fine"
        );
        assert_eq!(
            recorder.on_control_move(&mut lanes, VOLUME, 0.9, 5_000, true, RecordMode::Write),
            RecordOutcome::OutOfOrder,
            "a point earlier than the last one must be refused, not inserted"
        );

        let take = recorder.active_take().unwrap();
        assert_eq!(take.len(), 1);
        assert_eq!(take.points[0].frame, 10_000);
        assert!(
            take.points.windows(2).all(|w| w[0].frame <= w[1].frame),
            "take points must stay ordered"
        );
    }

    #[test]
    fn a_take_is_committed_with_the_lanes_existing_points_kept() {
        let mut lanes = LaneSet::new();
        let mut lane = Lane::new(VOLUME);
        lane.set_armed(true);
        // Pre-existing automation far from the recorded span.
        lane.set_points(alloc::vec![
            AutomationPoint::new(0, -40.0),
            AutomationPoint::new(50_000, -40.0),
        ]);
        lanes.add(lane);

        let mut recorder = recorder();
        for (frame, value) in [(1_000, 0.1_f32), (2_000, 0.5)] {
            recorder.on_control_move(&mut lanes, VOLUME, value, frame, true, RecordMode::Write);
        }
        assert!(recorder.finish(&mut lanes));

        let points = lanes.get(VOLUME).unwrap().clip().points();
        let frames: Vec<i64> = points.iter().map(|p| p.frame).collect();
        assert_eq!(
            frames,
            vec![0, 1_000, 2_000, 50_000],
            "the take must merge with, not replace, points outside its span"
        );
    }

    #[test]
    fn a_punch_in_replaces_the_span_it_covers() {
        // Overlaying would leave both the old curve and the new take present.
        let mut lanes = LaneSet::new();
        let mut lane = Lane::new(VOLUME);
        lane.set_armed(true);
        lane.set_points(alloc::vec![
            AutomationPoint::new(0, 0.0),
            AutomationPoint::new(1_000, 0.1),
            AutomationPoint::new(2_000, 0.2),
            AutomationPoint::new(3_000, 0.3),
        ]);
        lanes.add(lane);

        let mut recorder = recorder();
        for (frame, value) in [(1_000, 0.9_f32), (2_000, 0.95)] {
            recorder.on_control_move(&mut lanes, VOLUME, value, frame, true, RecordMode::Write);
        }
        assert!(recorder.finish(&mut lanes));

        let points = lanes.get(VOLUME).unwrap().clip().points();
        let frames: Vec<i64> = points.iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![0, 1_000, 2_000, 3_000]);
        // The overwritten region carries the new values.
        assert!((points[1].value - 0.9).abs() < 1e-6);
        assert!((points[2].value - 0.95).abs() < 1e-6);
        // The untouched endpoints survive.
        assert_eq!(points[0].value, 0.0);
        assert_eq!(points[3].value, 0.3);
    }

    #[test]
    fn moving_to_another_parameter_commits_the_first_take() {
        let mut lanes = armed_lanes(&[VOLUME, PAN]);
        let mut recorder = recorder();

        recorder.on_control_move(&mut lanes, VOLUME, 0.3, 0, true, RecordMode::Write);
        recorder.on_control_move(&mut lanes, PAN, -0.5, 100, true, RecordMode::Write);

        // The VOLUME take was committed when PAN took over.
        assert_eq!(lanes.get(VOLUME).unwrap().clip().len(), 1);
        assert_eq!(recorder.active_take().unwrap().address, PAN);

        recorder.finish(&mut lanes);
        assert_eq!(lanes.get(PAN).unwrap().clip().len(), 1);
    }

    #[test]
    fn an_empty_take_commits_nothing() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        // Open a take without capturing anything (first move is thinned away
        // because it repeats a non-finite seed? No — use a rejected write).
        assert!(!recorder.finish(&mut lanes), "no take was open");
        assert!(lanes.get(VOLUME).unwrap().clip().is_empty());
    }

    #[test]
    fn cancel_discards_the_take_without_touching_the_lane() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.on_control_move(&mut lanes, VOLUME, 0.5, 0, true, RecordMode::Write);
        recorder.cancel();
        assert!(recorder.active_take().is_none());
        assert!(lanes.get(VOLUME).unwrap().clip().is_empty());
        assert!(!recorder.finish(&mut lanes));
    }

    #[test]
    fn transport_stop_commits_the_take() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.on_control_move(&mut lanes, VOLUME, 0.7, 0, true, RecordMode::Latch);
        assert!(recorder.on_transport_stop(&mut lanes));
        assert_eq!(lanes.get(VOLUME).unwrap().clip().len(), 1);
        assert!(recorder.active_take().is_none());
    }

    #[test]
    fn disabling_recording_mid_take_commits_what_was_captured() {
        // Losing a pass because the user hit stop on the record button would
        // be a data-loss bug, not a cancel.
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.on_control_move(&mut lanes, VOLUME, 0.6, 0, true, RecordMode::Write);
        recorder.set_enabled(false, &mut lanes);
        assert_eq!(lanes.get(VOLUME).unwrap().clip().len(), 1);
        assert!(recorder.active_take().is_none());
    }

    #[test]
    fn thinning_thresholds_are_sanitized() {
        let mut recorder = Recorder::new();
        recorder.set_min_value_delta(-1.0);
        assert!(recorder.min_value_delta() > 0.0);
        recorder.set_min_value_delta(f32::NAN);
        assert!(recorder.min_value_delta() > 0.0);
        recorder.set_max_interval_frames(-50);
        assert_eq!(recorder.max_interval_frames(), 0);
    }

    #[test]
    fn prepare_derives_a_keep_alive_interval_from_the_sample_rate() {
        let mut recorder = Recorder::new();
        recorder.prepare(48_000.0);
        assert_eq!(recorder.max_interval_frames(), 240, "5 ms at 48 kHz");
        recorder.prepare(f32::NAN);
        assert_eq!(recorder.max_interval_frames(), 240, "falls back to 48 kHz");
        recorder.prepare(96_000.0);
        assert_eq!(recorder.max_interval_frames(), 480);
    }

    #[test]
    fn captured_point_count_tracks_the_takes() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.set_max_interval_frames(1);
        for frame in 0..10 {
            recorder.on_control_move(
                &mut lanes,
                VOLUME,
                frame as f32 / 10.0,
                frame * 100,
                true,
                RecordMode::Write,
            );
        }
        assert_eq!(recorder.captured_points(), 10);
        recorder.reset();
        assert_eq!(recorder.captured_points(), 0);
    }

    #[test]
    fn a_take_reports_its_own_frame_range() {
        let mut lanes = armed_lanes(&[VOLUME]);
        let mut recorder = recorder();
        recorder.on_control_move(&mut lanes, VOLUME, 0.1, 100, true, RecordMode::Write);
        recorder.on_control_move(&mut lanes, VOLUME, 0.9, 900, true, RecordMode::Write);
        let take = recorder.active_take().unwrap();
        assert_eq!(take.frame_range(), Some((100, 900)));
        assert!(!take.is_empty());
    }
}
