//! Automation lanes: a parameter's clip plus its playback state.
//!
//! A [`Lane`] is the editing unit the UI shows (one row in an automation
//! view) and the unit the player walks. It pairs a clip with the addressing
//! information the player needs so the player itself stays a stateless
//! evaluator.
//!
//! # Armed vs enabled
//!
//! A lane has two independent switches, and conflating them is a classic
//! source of surprise:
//!
//! * [`Lane::set_enabled`] — is the lane *playing*? Disabling a lane leaves
//!   the points on disk and makes the parameter fall back to its base value.
//! * [`Lane::set_armed`] — is the lane *recordable*? The recorder only writes
//!   takes into armed lanes, so a user with 40 lanes can punch in on one.

use super::clip::{AutomationClip, AutomationPoint};
use super::parameter::{ParameterAddress, ParameterDescriptor};

/// How a lane's clip is written when the transport passes over it.
///
/// The three modes follow the industry convention that users already have in
/// their fingers; the differences are entirely about *when a take ends* and
/// *what happens on touch-up*.
///
/// Discriminants are ABI-frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub enum RecordMode {
    /// Off: moving a control changes the sound but writes nothing.
    #[default]
    Off = 0,
    /// Write only while a control is actually being touched.
    ///
    /// The take ends the moment the touch ends, and *only* the touched span is
    /// overwritten. This is the safe default: a stray fader bump cannot erase
    /// a whole pass.
    Touch = 1,
    /// Keep writing after the touch ends, until the transport stops.
    ///
    /// The take starts at the first touch and runs to the end of the pass, so
    /// the user can dial a value early and have it hold. Touching again
    /// re-arms the initial write.
    Latch = 2,
    /// Write the whole pass regardless of touch.
    ///
    /// Every armed lane is overwritten as the playhead crosses it, whether or
    /// not anything was moved. Reserved for deliberate full-pass rewrites.
    Write = 3,
}

impl RecordMode {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Off),
            1 => Some(Self::Touch),
            2 => Some(Self::Latch),
            3 => Some(Self::Write),
            _ => None,
        }
    }

    /// The ABI discriminant for this mode.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Whether this mode records at all.
    #[must_use]
    pub const fn is_recording(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether the mode keeps writing once the control is released.
    ///
    /// The recorder uses this to decide whether a touch-up ends the take.
    #[must_use]
    pub const fn latches_after_release(self) -> bool {
        matches!(self, Self::Latch | Self::Write)
    }
}

/// One parameter's automation lane.
#[derive(Debug, Clone)]
pub struct Lane {
    /// Parameter this lane drives.
    address: ParameterAddress,
    /// The curve.
    clip: AutomationClip,
    /// Whether the lane plays back.
    enabled: bool,
    /// Whether the lane accepts recorded takes.
    armed: bool,
    /// Whether the lane is hidden in the editor.
    ///
    /// Purely a view concern, but it lives here so that hiding a lane is
    /// saved with the project rather than being a per-session preference.
    collapsed: bool,
    /// Editor height in logical pixels; 0 means "use the default".
    height: f32,
    /// Lane colour as `0xRRGGBB`, for the editor.
    color: u32,
}

impl Lane {
    /// Creates an empty, enabled, unarmed lane for `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        Self {
            address,
            clip: AutomationClip::new(),
            enabled: true,
            armed: false,
            collapsed: false,
            height: 0.0,
            color: 0,
        }
    }

    /// Creates a lane from existing points.
    #[must_use]
    pub fn with_points(address: ParameterAddress, points: alloc::vec::Vec<AutomationPoint>) -> Self {
        Self {
            clip: AutomationClip::from_points(points),
            ..Self::new(address)
        }
    }

    /// The parameter this lane drives.
    #[must_use]
    pub fn address(&self) -> ParameterAddress {
        self.address
    }

    /// The lane's curve.
    #[must_use]
    pub fn clip(&self) -> &AutomationClip {
        &self.clip
    }

    /// The lane's curve, mutably.
    ///
    /// Callers that mutate points directly must follow up with
    /// [`AutomationClip::set_points`] or accept that the search index is
    /// rebuilt lazily on the next evaluation.
    pub fn clip_mut(&mut self) -> &mut AutomationClip {
        &mut self.clip
    }

    /// Replaces the lane's points.
    pub fn set_points(&mut self, points: alloc::vec::Vec<AutomationPoint>) {
        self.clip.set_points(points);
    }

    /// Whether the lane plays back.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enables or disables playback.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether the lane accepts recorded takes.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Arms or disarms the lane for recording.
    pub fn set_armed(&mut self, armed: bool) {
        self.armed = armed;
    }

    /// Whether the lane is collapsed in the editor.
    #[must_use]
    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Collapses or expands the lane in the editor.
    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    /// Editor height in logical pixels; `0.0` means "default".
    #[must_use]
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Sets the editor height, clamped to a usable band.
    pub fn set_height(&mut self, height: f32) {
        self.height = if height.is_finite() { height.clamp(0.0, 400.0) } else { 0.0 };
    }

    /// Lane colour as `0xRRGGBB`; `0` means "use the theme default".
    #[must_use]
    pub fn color(&self) -> u32 {
        self.color
    }

    /// Sets the lane colour.
    pub fn set_color(&mut self, color: u32) {
        self.color = color;
    }

    /// Evaluates the lane at `frame`, or `None` when it has nothing to say.
    ///
    /// Returns `None` for a disabled or empty lane, which is what lets the
    /// player distinguish "no opinion, use the base value" from "the curve
    /// genuinely says 0.0".
    ///
    /// Real-time safe.
    #[must_use]
    pub fn value_at(&self, frame: i64) -> Option<f32> {
        if !self.enabled {
            return None;
        }
        self.clip.value_at(frame)
    }

    /// Whether the lane contributes anything at `frame`.
    #[must_use]
    pub fn is_active_at(&self, frame: i64) -> bool {
        self.enabled && !self.clip.is_empty() && self.clip.value_at(frame).is_some()
    }

    /// The frames this lane spans.
    #[must_use]
    pub fn frame_range(&self) -> Option<(i64, i64)> {
        self.clip.frame_range()
    }

    /// Validates the lane against a descriptor, reporting why it is unusable.
    ///
    /// Called when a project loads: an automation lane pointing at a parameter
    /// that no longer exists (an effect was removed, a channel was deleted) is
    /// a normal occurrence, and it must degrade to a warning rather than a
    /// failed load.
    #[must_use]
    pub fn validate(&self, descriptor: Option<&ParameterDescriptor>) -> LaneProblem {
        match descriptor {
            None => LaneProblem::UnknownParameter,
            Some(d) if !d.is_automatable() => LaneProblem::NotAutomatable,
            Some(_) if self.clip.is_empty() => LaneProblem::Empty,
            Some(_) => LaneProblem::Ok,
        }
    }
}

/// Why a lane might not be usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneProblem {
    /// The lane is fine.
    Ok,
    /// Nothing is registered at the lane's address.
    UnknownParameter,
    /// The parameter exists but declares itself non-automatable.
    NotAutomatable,
    /// The lane has no points.
    Empty,
}

impl LaneProblem {
    /// Whether the lane can be used as-is.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// A set of lanes, addressed by parameter.
///
/// Linear search rather than a map: a project has tens of lanes, not
/// thousands, and a contiguous `Vec` keeps iteration (which happens every
/// block) cache-friendly.
#[derive(Debug, Default, Clone)]
pub struct LaneSet {
    lanes: alloc::vec::Vec<Lane>,
}

impl LaneSet {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of lanes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lanes.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }

    /// All lanes, in insertion order.
    #[must_use]
    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }

    /// All lanes, mutably.
    pub fn lanes_mut(&mut self) -> &mut alloc::vec::Vec<Lane> {
        &mut self.lanes
    }

    /// The lane for `address`, if it exists.
    #[must_use]
    pub fn get(&self, address: ParameterAddress) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.address == address)
    }

    /// The lane for `address`, mutably.
    pub fn get_mut(&mut self, address: ParameterAddress) -> Option<&mut Lane> {
        self.lanes.iter_mut().find(|l| l.address == address)
    }

    /// Adds a lane, or returns the existing one's index.
    pub fn add(&mut self, lane: Lane) -> usize {
        if let Some(index) = self.lanes.iter().position(|l| l.address == lane.address) {
            self.lanes[index] = lane;
            return index;
        }
        self.lanes.push(lane);
        self.lanes.len() - 1
    }

    /// Creates a lane for `address` if none exists, and returns it.
    pub fn entry(&mut self, address: ParameterAddress) -> &mut Lane {
        if let Some(index) = self.lanes.iter().position(|l| l.address == address) {
            return &mut self.lanes[index];
        }
        self.lanes.push(Lane::new(address));
        let last = self.lanes.len() - 1;
        &mut self.lanes[last]
    }

    /// Removes the lane for `address`. Returns whether one was removed.
    pub fn remove(&mut self, address: ParameterAddress) -> bool {
        if let Some(index) = self.lanes.iter().position(|l| l.address == address) {
            self.lanes.remove(index);
            return true;
        }
        false
    }

    /// Every lane that would contribute at `frame`.
    ///
    /// Real-time safe: allocation-free iteration over a borrowed slice.
    pub fn active_at(&self, frame: i64) -> impl Iterator<Item = &Lane> + '_ {
        self.lanes
            .iter()
            .filter(move |lane| lane.is_active_at(frame))
    }

    /// Lanes that are currently recordable.
    pub fn armed(&self) -> impl Iterator<Item = &Lane> + '_ {
        self.lanes.iter().filter(|lane| lane.is_armed())
    }

    /// Total number of points across every lane.
    ///
    /// Used by the "1000 points must cost under 2% CPU" acceptance check in
    /// PLAN §3.S2, and by the editor to show a project's automation weight.
    #[must_use]
    pub fn total_points(&self) -> usize {
        self.lanes.iter().map(|l| l.clip.len()).sum()
    }

    /// Removes every lane.
    pub fn clear(&mut self) {
        self.lanes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn pts(values: &[(i64, f32)]) -> Vec<AutomationPoint> {
        values
            .iter()
            .map(|&(f, v)| AutomationPoint::new(f, v))
            .collect()
    }

    const VOLUME: ParameterAddress = ParameterAddress::channel(0, 0);
    const PAN: ParameterAddress = ParameterAddress::channel(0, 1);

    #[test]
    fn a_new_lane_is_enabled_and_empty() {
        let lane = Lane::new(VOLUME);
        assert!(lane.is_enabled());
        assert!(!lane.is_armed());
        assert!(lane.clip().is_empty());
        assert_eq!(lane.value_at(0), None);
        assert_eq!(lane.frame_range(), None);
    }

    #[test]
    fn a_disabled_lane_has_no_opinion_even_with_points() {
        // The distinction matters: `None` means "use the base value", while
        // `Some(0.0)` would mean "the curve says zero".
        let mut lane = Lane::with_points(VOLUME, pts(&[(0, 0.5), (100, 0.9)]));
        assert_eq!(lane.value_at(50), Some(0.7));
        lane.set_enabled(false);
        assert_eq!(lane.value_at(50), None);
        assert!(!lane.is_active_at(50));
        // Re-enabling restores the same answer: disabling is not destructive.
        lane.set_enabled(true);
        assert!(lane.value_at(50).unwrap() > 0.69);
    }

    #[test]
    fn lane_editor_metadata_round_trips() {
        let mut lane = Lane::new(VOLUME);
        lane.set_collapsed(true);
        lane.set_height(96.0);
        lane.set_color(0x00FF00);
        assert!(lane.is_collapsed());
        assert_eq!(lane.height(), 96.0);
        assert_eq!(lane.color(), 0x00FF00);
    }

    #[test]
    fn lane_height_is_clamped_and_sanitized() {
        let mut lane = Lane::new(VOLUME);
        lane.set_height(10_000.0);
        assert_eq!(lane.height(), 400.0);
        lane.set_height(-5.0);
        assert_eq!(lane.height(), 0.0);
        lane.set_height(f32::NAN);
        assert_eq!(lane.height(), 0.0);
    }

    #[test]
    fn record_mode_semantics_are_distinct() {
        assert!(!RecordMode::Off.is_recording());
        assert!(RecordMode::Touch.is_recording());
        assert!(RecordMode::Latch.is_recording());
        assert!(RecordMode::Write.is_recording());

        // Only Latch and Write survive a control release.
        assert!(!RecordMode::Touch.latches_after_release());
        assert!(RecordMode::Latch.latches_after_release());
        assert!(RecordMode::Write.latches_after_release());
    }

    #[test]
    fn record_mode_rejects_unknown_discriminants() {
        assert_eq!(RecordMode::from_u32(0), Some(RecordMode::Off));
        assert_eq!(RecordMode::from_u32(3), Some(RecordMode::Write));
        assert_eq!(RecordMode::from_u32(4), None);
        assert_eq!(RecordMode::default(), RecordMode::Off);
    }

    #[test]
    fn lane_validation_reports_each_problem() {
        use crate::automation::parameter::{parameter_flags, ParameterKind, ParameterUnit};

        static AUTOMATABLE: ParameterDescriptor = ParameterDescriptor {
            address: VOLUME,
            key: "volume",
            label: "Volume",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE,
            min_value: -60.0,
            max_value: 12.0,
            default_value: 0.0,
            smoothing_ms: 10.0,
        };
        static FIXED: ParameterDescriptor = ParameterDescriptor {
            flags: 0,
            ..AUTOMATABLE
        };

        let populated = Lane::with_points(VOLUME, pts(&[(0, 0.0)]));
        assert_eq!(populated.validate(Some(&AUTOMATABLE)), LaneProblem::Ok);
        assert!(LaneProblem::Ok.is_usable());

        assert_eq!(populated.validate(None), LaneProblem::UnknownParameter);
        assert_eq!(populated.validate(Some(&FIXED)), LaneProblem::NotAutomatable);

        let empty = Lane::new(VOLUME);
        assert_eq!(empty.validate(Some(&AUTOMATABLE)), LaneProblem::Empty);
        let _ = ParameterKind::Channel;
    }

    #[test]
    fn lane_set_adds_and_finds_by_address() {
        let mut set = LaneSet::new();
        assert!(set.is_empty());
        set.add(Lane::with_points(VOLUME, pts(&[(0, 0.5)])));
        set.add(Lane::with_points(PAN, pts(&[(0, -1.0)])));

        assert_eq!(set.len(), 2);
        assert_eq!(set.get(VOLUME).map(|l| l.clip().len()), Some(1));
        assert!(set.get(ParameterAddress::channel(9, 9)).is_none());
        assert_eq!(set.total_points(), 2);
    }

    #[test]
    fn adding_a_lane_twice_replaces_rather_than_duplicates() {
        let mut set = LaneSet::new();
        set.add(Lane::with_points(VOLUME, pts(&[(0, 0.1)])));
        set.add(Lane::with_points(VOLUME, pts(&[(0, 0.2), (10, 0.3)])));
        assert_eq!(set.len(), 1);
        assert_eq!(set.get(VOLUME).map(|l| l.clip().len()), Some(2));
    }

    #[test]
    fn entry_creates_a_lane_on_first_use() {
        let mut set = LaneSet::new();
        set.entry(VOLUME).set_points(pts(&[(0, 0.4)]));
        assert_eq!(set.len(), 1);
        // Asking twice must not add a second lane.
        set.entry(VOLUME).set_armed(true);
        assert_eq!(set.len(), 1);
        assert!(set.get(VOLUME).unwrap().is_armed());
    }

    #[test]
    fn remove_reports_whether_anything_was_removed() {
        let mut set = LaneSet::new();
        set.add(Lane::with_points(VOLUME, pts(&[(0, 0.5)])));
        assert!(set.remove(VOLUME));
        assert!(!set.remove(VOLUME));
        assert!(set.is_empty());
    }

    #[test]
    fn active_at_filters_disabled_and_empty_lanes() {
        let mut set = LaneSet::new();
        set.add(Lane::with_points(VOLUME, pts(&[(0, 0.5)])));
        set.add(Lane::with_points(PAN, pts(&[(0, -1.0)])));
        set.add(Lane::new(ParameterAddress::channel(0, 2))); // empty
        set.get_mut(ParameterAddress::channel(0, 2)).unwrap();

        let active: Vec<_> = set.active_at(0).map(|l| l.address()).collect();
        assert_eq!(active.len(), 2, "the empty lane must not contribute");

        set.get_mut(PAN).unwrap().set_enabled(false);
        let active: Vec<_> = set.active_at(0).map(|l| l.address()).collect();
        assert_eq!(active, vec![VOLUME]);
    }

    #[test]
    fn armed_iterates_only_recordable_lanes() {
        let mut set = LaneSet::new();
        set.add(Lane::new(VOLUME));
        set.add(Lane::new(PAN));
        set.get_mut(PAN).unwrap().set_armed(true);

        let armed: Vec<_> = set.armed().map(|l| l.address()).collect();
        assert_eq!(armed, vec![PAN]);
    }

    #[test]
    fn clear_empties_the_set() {
        let mut set = LaneSet::new();
        set.add(Lane::new(VOLUME));
        set.add(Lane::new(PAN));
        set.clear();
        assert!(set.is_empty());
        assert_eq!(set.total_points(), 0);
    }
}
