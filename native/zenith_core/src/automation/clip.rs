//! Automation clips: a pre-sorted, pre-indexed point set plus interpolation.
//!
//! # Why the point set is sorted and indexed at load time
//!
//! The audio thread evaluates a clip once per block *per automated parameter*.
//! If it had to search an unsorted `Vec` — or sort one — every block, the cost
//! would be both unbounded and allocating, violating P5. So:
//!
//! * [`AutomationClip::set_points`] sorts by frame **once**, on the control
//!   thread, and keeps a small uniform bucket index alongside the points;
//! * [`AutomationClip::value_at`] does a bucket lookup plus a short linear
//!   scan, touches no allocator, and takes no lock.
//!
//! The index is rebuilt only on mutation, never on evaluation.

use alloc::vec::Vec;

use super::parameter::ParameterDescriptor;

/// Interpolation used between a point and its successor.
///
/// Discriminants are ABI-frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub enum CurveKind {
    /// Straight line between the two points.
    #[default]
    Linear = 0,
    /// Hold the left point's value until the right point is reached (step).
    Hold = 1,
    /// A power curve governed by the left point's `tension`.
    Curve = 2,
    /// Fast approach then flatten, `tension` controls the bend.
    Exponential = 3,
    /// Slow approach then steepen, `tension` controls the bend.
    Logarithmic = 4,
}

impl CurveKind {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Linear),
            1 => Some(Self::Hold),
            2 => Some(Self::Curve),
            3 => Some(Self::Exponential),
            4 => Some(Self::Logarithmic),
            _ => None,
        }
    }

    /// The ABI discriminant for this curve.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Number of buckets in a clip's uniform search index.
///
/// 64 keeps a 10 000-point clip's scan to ~150 points worst case while costing
/// only 64 `u32`s of memory. Larger values trade memory for a shorter scan;
/// the index is a cache-locality aid, not a correctness requirement.
const INDEX_BUCKETS: usize = 64;

/// A single point on an automation curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutomationPoint {
    /// Position in frames from the project origin.
    ///
    /// Frames rather than seconds, matching the tick-first model: automation
    /// must not drift when the tempo changes (PLAN §3.S0 requirement 2).
    pub frame: i64,
    /// Value at `frame`, in the parameter's own units.
    pub value: f32,
    /// Curvature toward the *next* point, `-1.0..=1.0`.
    ///
    /// `0.0` is the neutral midpoint; negative bends one way, positive the
    /// other. Stored per point (not per segment) so that inserting a point
    /// never has to rewrite its neighbour's data.
    pub tension: f32,
    /// How to interpolate from this point to the next.
    pub curve: CurveKind,
}

impl AutomationPoint {
    /// Builds a linear point with neutral tension.
    #[must_use]
    pub const fn new(frame: i64, value: f32) -> Self {
        Self {
            frame,
            value,
            tension: 0.0,
            curve: CurveKind::Linear,
        }
    }

    /// Builds a point with an explicit curve shape.
    #[must_use]
    pub const fn with_curve(frame: i64, value: f32, curve: CurveKind, tension: f32) -> Self {
        Self {
            frame,
            value,
            tension,
            curve,
        }
    }

    /// The point's tension clamped into its legal range.
    #[must_use]
    fn safe_tension(&self) -> f32 {
        if self.tension.is_finite() {
            self.tension.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A parameter's automation curve over time.
///
/// Points are kept sorted by `frame`, which is an invariant every mutating
/// method restores before returning.
#[derive(Debug, Clone, Default)]
pub struct AutomationClip {
    points: Vec<AutomationPoint>,
    /// Frame of the first point, for the bucket index origin.
    index_origin: i64,
    /// Frames covered by one bucket; `0` means the index is degenerate.
    bucket_span: i64,
    /// Uniform bucket boundaries into `points`; see [`INDEX_BUCKETS`].
    buckets: [u32; INDEX_BUCKETS + 1],
    /// Whether the point set changed since the index was built.
    dirty: bool,
}

impl AutomationClip {
    /// Creates an empty clip.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a clip from points, sorting and indexing them immediately.
    ///
    /// This is the load path: it may allocate, and it must be called from the
    /// control thread (typically when opening a project or recording a take).
    #[must_use]
    pub fn from_points(mut points: Vec<AutomationPoint>) -> Self {
        points.sort_by_key(|p| p.frame);
        let mut clip = Self {
            points,
            ..Self::default()
        };
        clip.rebuild_index();
        clip
    }

    /// Number of points in the clip.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the clip has no points.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The points, in frame order.
    #[must_use]
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    /// Mutable access to the points.
    ///
    /// Marks the index dirty: the caller may reorder or retime points, and the
    /// next evaluation rebuilds the index lazily rather than trusting stale
    /// bucket boundaries.
    pub fn points_mut(&mut self) -> &mut Vec<AutomationPoint> {
        self.dirty = true;
        &mut self.points
    }

    /// Replaces the whole point set.
    ///
    /// Allocates and sorts — control thread only.
    pub fn set_points(&mut self, mut points: Vec<AutomationPoint>) {
        points.sort_by_key(|p| p.frame);
        self.points = points;
        self.rebuild_index();
    }

    /// The frame range the clip spans, or `None` when empty.
    #[must_use]
    pub fn frame_range(&self) -> Option<(i64, i64)> {
        match (self.points.first(), self.points.last()) {
            (Some(first), Some(last)) => Some((first.frame, last.frame)),
            _ => None,
        }
    }

    /// Inserts a point, keeping the set sorted.
    ///
    /// Returns the index it landed at. Allocates — control thread only.
    pub fn insert(&mut self, point: AutomationPoint) -> usize {
        let at = self.points.partition_point(|p| p.frame <= point.frame);
        self.points.insert(at, point);
        self.rebuild_index();
        at
    }

    /// Removes the point at `index`, returning it.
    pub fn remove(&mut self, index: usize) -> Option<AutomationPoint> {
        if index >= self.points.len() {
            return None;
        }
        let removed = self.points.remove(index);
        self.rebuild_index();
        Some(removed)
    }

    /// Removes every point whose frame lies in `start..=end`.
    ///
    /// Returns how many were removed. Used by Touch/Latch recording to clear
    /// the region a pass is about to overwrite.
    pub fn remove_range(&mut self, start: i64, end: i64) -> usize {
        if start > end {
            return 0;
        }
        let before = self.points.len();
        self.points.retain(|p| p.frame < start || p.frame > end);
        let removed = before - self.points.len();
        if removed > 0 {
            self.rebuild_index();
        }
        removed
    }

    /// Removes all points.
    pub fn clear(&mut self) {
        self.points.clear();
        self.rebuild_index();
    }

    /// Moves a point's frame and/or value.
    ///
    /// Re-sorts because a drag can carry a point past its neighbours; returns
    /// `false` when `index` is out of bounds.
    pub fn move_point(&mut self, index: usize, frame: i64, value: f32) -> bool {
        if index >= self.points.len() {
            return false;
        }
        self.points[index].frame = frame;
        self.points[index].value = value;
        self.points.sort_by_key(|p| p.frame);
        self.rebuild_index();
        true
    }

    /// Sets a point's curve shape and tension, clamped to `-1.0..=1.0`.
    pub fn set_curve(&mut self, index: usize, curve: CurveKind, tension: f32) -> bool {
        if index >= self.points.len() {
            return false;
        }
        self.points[index].curve = curve;
        self.points[index].tension = if tension.is_finite() {
            tension.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        true
    }

    /// Rebuilds the uniform bucket index over the current point set.
    ///
    /// Called only after a mutation, never from the audio path.
    fn rebuild_index(&mut self) {
        self.dirty = false;
        self.buckets = [0; INDEX_BUCKETS + 1];
        if self.points.is_empty() {
            self.index_origin = 0;
            self.bucket_span = 0;
            return;
        }
        let first = self.points[0].frame;
        let last = self.points[self.points.len() - 1].frame;
        self.index_origin = first;
        let span = last - first;
        if span <= 0 {
            self.bucket_span = 0;
            // Every point sits at the same frame; one bucket holds them all.
            self.buckets[INDEX_BUCKETS] = self.points.len() as u32;
            return;
        }
        // Ceil-divided so the final bucket still covers `last`.
        let bucket_span = span.div_ceil(INDEX_BUCKETS as i64).max(1);
        self.bucket_span = bucket_span;
        for (i, point) in self.points.iter().enumerate() {
            let bucket = ((point.frame - first) / bucket_span).min(INDEX_BUCKETS as i64 - 1);
            // bucket+1 is the exclusive upper bound of bucket `bucket`.
            for b in (bucket as usize + 1)..=INDEX_BUCKETS {
                if self.buckets[b] == 0 {
                    self.buckets[b] = i as u32;
                } else {
                    break;
                }
            }
        }
        self.buckets[INDEX_BUCKETS] = self.points.len() as u32;
    }

    /// Narrows the search window to the points that can bracket `frame`.
    ///
    /// Returns `(lo, hi)` — a half-open range of candidate indices. Falls back
    /// to the whole set when the index is degenerate, so a stale index can
    /// cost time but never correctness.
    fn search_window(&self, frame: i64) -> (usize, usize) {
        if self.bucket_span <= 0 || self.dirty {
            return (0, self.points.len());
        }
        if frame < self.index_origin {
            return (0, 1);
        }
        let bucket = ((frame - self.index_origin) / self.bucket_span) as usize;
        if bucket >= INDEX_BUCKETS {
            return (self.points.len().saturating_sub(1), self.points.len());
        }
        let lo = self.buckets[bucket] as usize;
        let hi = self.buckets[bucket + 1] as usize;
        // Widen by one on each side: the bracketing pair may start in the
        // previous bucket when a segment spans a boundary.
        (lo.saturating_sub(1), (hi + 2).min(self.points.len()))
    }

    /// Evaluates the curve at `frame`.
    ///
    /// * before the first point → that point's value (no extrapolation, so
    ///   scrubbing before the lane starts cannot produce wild values);
    /// * after the last point → the last value;
    /// * exactly on a point → that point's value, unchanged.
    ///
    /// Real-time safe: performs no allocation and takes no lock.
    #[must_use]
    pub fn value_at(&self, frame: i64) -> Option<f32> {
        if self.points.is_empty() {
            return None;
        }
        let first = self.points[0];
        if frame <= first.frame {
            return Some(first.value);
        }
        let last = self.points[self.points.len() - 1];
        if frame >= last.frame {
            return Some(last.value);
        }

        let (lo, hi) = self.search_window(frame);
        // Find the last point at or before `frame` within the window.
        let window = &self.points[lo..hi];
        let offset = window.partition_point(|p| p.frame <= frame);
        if offset == 0 {
            // Window was too narrow to contain the bracket; fall back to the
            // full set rather than returning a wrong (but plausible) value.
            return self.value_at_linear_scan(frame);
        }
        let left_index = lo + offset - 1;
        let right = self.points.get(left_index + 1)?;
        let left = self.points[left_index];
        Some(interpolate(left, *right, frame))
    }

    /// Unindexed fallback used when a bucket window proves too narrow.
    fn value_at_linear_scan(&self, frame: i64) -> Option<f32> {
        let offset = self.points.partition_point(|p| p.frame <= frame);
        if offset == 0 || offset >= self.points.len() {
            return None;
        }
        let left = self.points[offset - 1];
        let right = self.points[offset];
        Some(interpolate(left, right, frame))
    }
}

/// Interpolates between `left` and `right` at `frame`.
///
/// `frame` is assumed to lie in `left.frame..=right.frame`.
#[must_use]
fn interpolate(left: AutomationPoint, right: AutomationPoint, frame: i64) -> f32 {
    let span = right.frame - left.frame;
    if span <= 0 {
        // Coincident points: the later one wins, which is what a stepped edit
        // at the same frame would produce.
        return right.value;
    }
    let t = ((frame - left.frame) as f32 / span as f32).clamp(0.0, 1.0);
    let shaped = shape_tension(t, left.safe_tension(), left.curve);
    left.value + (right.value - left.value) * shaped
}

/// Applies a curve shape to a normalized parameter `t`.
///
/// Tension `0.0` is always the identity, so switching a segment to `Curve`
/// without touching the tension handle changes nothing until the user drags
/// it — an important property for an editor where the mode button and the
/// handle are separate controls.
#[must_use]
pub fn shape_tension(t: f32, tension: f32, curve: CurveKind) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let tension = if tension.is_finite() {
        tension.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    match curve {
        CurveKind::Linear => t,
        CurveKind::Hold => 0.0,
        CurveKind::Exponential => {
            // Bend by raising to a power: >1 delays, <1 accelerates.
            let exponent = exp_from_tension(tension);
            t.powf(exponent)
        }
        CurveKind::Logarithmic => {
            let exponent = exp_from_tension(-tension);
            t.powf(exponent)
        }
        CurveKind::Curve => {
            if tension == 0.0 {
                return t;
            }
            // A rational bend rather than a power: it is defined at t == 0
            // and t == 1 for every tension, so a segment can never lose its
            // endpoints no matter how hard the handle is dragged.
            let k = tension * 4.0;
            let denom = 1.0 + k * (1.0 - 2.0 * t);
            if denom.abs() < 1e-6 {
                // Asymptote (only reachable at the extreme tension); fall
                // back to the linear ramp instead of emitting an infinity.
                return t;
            }
            (t / denom).clamp(0.0, 1.0)
        }
    }
}

/// Maps `-1.0..=1.0` tension onto a positive exponent.
#[must_use]
fn exp_from_tension(tension: f32) -> f32 {
    // 2^-3..2^3 → 0.125..8.0, symmetric around the neutral 1.0.
    (2.0_f32).powf(tension * 3.0)
}

/// Convenience wrapper that clamps an evaluated value through a descriptor.
#[must_use]
pub fn clamp_to_descriptor(descriptor: &ParameterDescriptor, value: f32) -> f32 {
    descriptor.clamp(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(values: &[(i64, f32)]) -> Vec<AutomationPoint> {
        values.iter().map(|&(f, v)| AutomationPoint::new(f, v)).collect()
    }

    #[test]
    fn empty_clip_has_no_value() {
        let clip = AutomationClip::new();
        assert!(clip.is_empty());
        assert_eq!(clip.value_at(0), None);
        assert_eq!(clip.frame_range(), None);
    }

    #[test]
    fn points_are_sorted_on_construction() {
        let clip = AutomationClip::from_points(pts(&[(100, 1.0), (0, 0.0), (50, 0.5)]));
        let frames: Vec<i64> = clip.points().iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![0, 50, 100]);
    }

    #[test]
    fn single_point_clip_is_constant() {
        let clip = AutomationClip::from_points(pts(&[(480, 2.5)]));
        assert_eq!(clip.value_at(-1000), Some(2.5));
        assert_eq!(clip.value_at(480), Some(2.5));
        assert_eq!(clip.value_at(99999), Some(2.5));
    }

    #[test]
    fn clamp_holds_before_first_and_after_last() {
        let clip = AutomationClip::from_points(pts(&[(0, 0.0), (100, 10.0)]));
        // No extrapolation: an earlier/later frame reads the nearest endpoint.
        assert_eq!(clip.value_at(-50), Some(0.0));
        assert_eq!(clip.value_at(500), Some(10.0));
    }

    #[test]
    fn linear_interpolation_is_exact_at_endpoints_and_midpoint() {
        let clip = AutomationClip::from_points(pts(&[(0, 0.0), (100, 10.0)]));
        assert_eq!(clip.value_at(0), Some(0.0));
        assert_eq!(clip.value_at(100), Some(10.0));
        assert!((clip.value_at(50).unwrap() - 5.0).abs() < 1e-5);
        assert!((clip.value_at(25).unwrap() - 2.5).abs() < 1e-5);
    }

    #[test]
    fn hold_curve_keeps_the_left_value_until_the_next_point() {
        let clip = AutomationClip::from_points(vec![
            AutomationPoint::with_curve(0, 0.0, CurveKind::Hold, 0.0),
            AutomationPoint::new(100, 1.0),
        ]);
        assert_eq!(clip.value_at(1), Some(0.0));
        assert_eq!(clip.value_at(99), Some(0.0));
        assert_eq!(clip.value_at(100), Some(1.0));
    }

    #[test]
    fn zero_tension_curve_is_identical_to_linear() {
        // Switching a segment's mode must not jump the curve until the user
        // actually drags the tension handle.
        let linear = AutomationClip::from_points(vec![
            AutomationPoint::with_curve(0, 0.0, CurveKind::Linear, 0.0),
            AutomationPoint::new(100, 1.0),
        ]);
        let curved = AutomationClip::from_points(vec![
            AutomationPoint::with_curve(0, 0.0, CurveKind::Curve, 0.0),
            AutomationPoint::new(100, 1.0),
        ]);
        for frame in [0, 10, 25, 50, 75, 99, 100] {
            assert_eq!(linear.value_at(frame), curved.value_at(frame), "frame {frame}");
        }
    }

    #[test]
    fn tension_bends_the_curve_without_moving_endpoints() {
        for tension in [-1.0_f32, -0.5, 0.5, 1.0] {
            let clip = AutomationClip::from_points(vec![
                AutomationPoint::with_curve(0, 0.0, CurveKind::Curve, tension),
                AutomationPoint::new(100, 1.0),
            ]);
            assert_eq!(clip.value_at(0), Some(0.0), "tension {tension}");
            assert_eq!(clip.value_at(100), Some(1.0), "tension {tension}");
            let mid = clip.value_at(50).unwrap();
            assert!(
                mid > 0.0 && mid < 1.0,
                "tension {tension} produced an out-of-range midpoint {mid}"
            );
        }
    }

    #[test]
    fn positive_and_negative_tension_bend_opposite_ways() {
        let bent_up = AutomationClip::from_points(vec![
            AutomationPoint::with_curve(0, 0.0, CurveKind::Curve, 0.8),
            AutomationPoint::new(100, 1.0),
        ]);
        let bent_down = AutomationClip::from_points(vec![
            AutomationPoint::with_curve(0, 0.0, CurveKind::Curve, -0.8),
            AutomationPoint::new(100, 1.0),
        ]);
        // One midpoint is above the linear 0.5, the other below.
        assert!(bent_up.value_at(50).unwrap() > 0.5);
        assert!(bent_down.value_at(50).unwrap() < 0.5);
    }

    #[test]
    fn curve_evaluation_never_escapes_the_segment_bounds() {
        // A monotonic ramp must stay monotonic for every tension, otherwise a
        // lane edit could produce an overshoot that clips audio.
        for tension in [-1.0_f32, -0.7, -0.3, 0.0, 0.3, 0.7, 1.0] {
            let clip = AutomationClip::from_points(vec![
                AutomationPoint::with_curve(0, 0.25, CurveKind::Curve, tension),
                AutomationPoint::new(200, 0.75),
            ]);
            let mut previous = f32::MIN;
            for frame in 0..=200 {
                let value = clip.value_at(frame).unwrap();
                assert!((0.25..=0.75).contains(&value), "t={tension} f={frame} v={value}");
                assert!(value >= previous - 1e-4, "t={tension} f={frame} went backwards");
                previous = value;
            }
        }
    }

    #[test]
    fn non_finite_tension_is_neutralized() {
        assert_eq!(shape_tension(0.5, f32::NAN, CurveKind::Curve), 0.5);
        assert_eq!(shape_tension(0.5, f32::INFINITY, CurveKind::Curve), 0.5);
    }

    #[test]
    fn coincident_points_do_not_divide_by_zero() {
        let clip = AutomationClip::from_points(pts(&[(0, 1.0), (0, 2.0), (10, 3.0)]));
        // The later of the two coincident points governs from frame 0 on.
        assert_eq!(clip.value_at(0), Some(2.0));
        assert!(clip.value_at(5).unwrap().is_finite());
    }

    #[test]
    fn insert_keeps_order_and_reports_the_slot() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (200, 2.0)]));
        let at = clip.insert(AutomationPoint::new(100, 1.0));
        assert_eq!(at, 1);
        let frames: Vec<i64> = clip.points().iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![0, 100, 200]);
        assert!((clip.value_at(100).unwrap() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn remove_and_remove_range_edit_the_curve() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (10, 1.0), (20, 2.0), (30, 3.0)]));
        let removed = clip.remove(1);
        assert_eq!(removed.map(|p| p.frame), Some(10));
        assert_eq!(clip.remove(99), None);

        let cleared = clip.remove_range(15, 25);
        assert_eq!(cleared, 1);
        let frames: Vec<i64> = clip.points().iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![0, 30]);

        assert_eq!(clip.remove_range(50, 10), 0, "inverted range is a no-op");
    }

    #[test]
    fn clear_empties_the_clip_and_its_index() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (100, 1.0)]));
        clip.clear();
        assert!(clip.is_empty());
        assert_eq!(clip.value_at(50), None);
    }

    #[test]
    fn large_clip_evaluates_correctly_through_the_bucket_index() {
        // 4000 points at a 7-frame stride: the index must return the same
        // answers a full scan would, including across bucket boundaries.
        let points: Vec<AutomationPoint> = (0..4000)
            .map(|i| AutomationPoint::new(i * 7, i as f32))
            .collect();
        let clip = AutomationClip::from_points(points);
        for probe in [0_i64, 1, 6, 7, 8, 99, 500, 12345, 27993, 27994, 50000] {
            let expected = {
                // Independent reference: linear scan with explicit lerp.
                let offset = clip.points().partition_point(|p| p.frame <= probe);
                if offset == 0 {
                    clip.points()[0].value
                } else if offset >= clip.points().len() {
                    clip.points()[clip.points().len() - 1].value
                } else {
                    let l = clip.points()[offset - 1];
                    let r = clip.points()[offset];
                    let t = (probe - l.frame) as f32 / (r.frame - l.frame) as f32;
                    l.value + (r.value - l.value) * t
                }
            };
            let actual = clip.value_at(probe).unwrap();
            assert!(
                (actual - expected).abs() < 1e-2,
                "frame {probe}: indexed {actual} vs reference {expected}"
            );
        }
    }

    #[test]
    fn mutation_after_construction_invalidates_the_index() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (1000, 10.0)]));
        // Retiming through the mutable accessor must not leave stale buckets.
        clip.points_mut()[1] = AutomationPoint::new(10, 10.0);
        assert!((clip.value_at(5).unwrap() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn move_point_resorts_when_it_crosses_a_neighbour() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (100, 1.0), (200, 2.0)]));
        assert!(clip.move_point(0, 250, 9.0));
        let frames: Vec<i64> = clip.points().iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![100, 200, 250]);
        assert!(!clip.move_point(99, 0, 0.0));
    }

    #[test]
    fn set_curve_clamps_tension() {
        let mut clip = AutomationClip::from_points(pts(&[(0, 0.0), (100, 1.0)]));
        assert!(clip.set_curve(0, CurveKind::Curve, 5.0));
        assert_eq!(clip.points()[0].tension, 1.0);
        assert!(clip.set_curve(0, CurveKind::Curve, f32::NAN));
        assert_eq!(clip.points()[0].tension, 0.0);
        assert!(!clip.set_curve(9, CurveKind::Linear, 0.0));
    }

    #[test]
    fn unknown_abi_curve_discriminants_are_rejected() {
        assert_eq!(CurveKind::from_u32(0), Some(CurveKind::Linear));
        assert_eq!(CurveKind::from_u32(4), Some(CurveKind::Logarithmic));
        assert_eq!(CurveKind::from_u32(5), None);
    }

    #[test]
    fn exponential_and_logarithmic_curves_stay_in_range() {
        for curve in [CurveKind::Exponential, CurveKind::Logarithmic] {
            for tension in [-1.0_f32, 0.0, 1.0] {
                for step in 0..=10 {
                    let t = step as f32 / 10.0;
                    let shaped = shape_tension(t, tension, curve);
                    assert!(
                        (0.0..=1.0).contains(&shaped),
                        "{curve:?} t={tension} shaped={shaped}"
                    );
                }
            }
        }
    }
}
