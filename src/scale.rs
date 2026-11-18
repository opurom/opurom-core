//! Multi-pointer scale recognizer ported from Flutter 3.50.0
//! (`gestures/scale.dart`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`). Source paths and
//! line numbers below are relative to `packages/flutter/lib/src/`.
//!
//! Ported rules:
//! - the recognizer tracks every pointer in contact and computes the focal
//!   point as the average of the tracked positions
//!   (`gestures/scale.dart:597-628`);
//! - the span is the average deviation of the pointers from the focal point;
//!   the horizontal and vertical spans are the average absolute deviations
//!   per axis (`gestures/scale.dart:630-643`);
//! - every pointer-configuration change (a pointer going down or up)
//!   re-baselines the initial focal point, spans and pointer line
//!   (`gestures/scale.dart:591,677-692`);
//! - the scale factor is `currentSpan / initialSpan` (1.0 when the initial
//!   span is zero), per axis for the horizontal and vertical factors
//!   (`gestures/scale.dart:455-461`);
//! - the rotation factor is the difference of the `atan2` angles of the
//!   initial and current pointer lines (`gestures/scale.dart:487-504`);
//! - the gesture accepts when the span delta exceeds the scale slop or the
//!   focal point delta exceeds the pan slop (`gestures/scale.dart:743-753`);
//!   the slops come from `gestures/events.dart:2564-2588` and
//!   `gestures/constants.dart:76,80`;
//! - acceptance moves the state to `started` and emits the start callback
//!   (`gestures/scale.dart:755-759,806-827`); updates carry scale,
//!   horizontal/vertical scale, focal point, rotation, pointer count and the
//!   focal point delta (`gestures/scale.dart:761-779`);
//! - the scale velocity comes from a velocity tracker fed with the scale
//!   factor as its position (`gestures/scale.dart:709,727-729,762`).
//!
//! The kernel owns only the geometry and the state machine; the host owns the
//! clock and the callback bodies. All geometry is integer px, so the same
//! pointer stream always yields the same factors.

use std::collections::BTreeMap;

use crate::gestures::{least_squares_quadratic, Ms, Point, PointerKind};

/// `kScaleSlop` = `kTouchSlop` = 18 px (`gestures/constants.dart:65,80`).
pub const SCALE_SLOP: i64 = 18;
/// `kPanSlop` = `kTouchSlop * 2` = 36 px (`gestures/constants.dart:76`).
pub const PAN_SLOP: i64 = 36;
/// `kPrecisePointerScaleSlop` = `kPrecisePointerHitSlop` = 1 px
/// (`gestures/constants.dart:108-109`).
pub const PRECISE_SCALE_SLOP: i64 = 1;
/// `kPrecisePointerPanSlop` = 2 px (`gestures/constants.dart:105-106`).
pub const PRECISE_PAN_SLOP: i64 = 2;

/// The scale-accept ratio Flutter compares for pan-zoom pointers
/// (`gestures/scale.dart:748`). A pointer-only scale always has a ratio of
/// 1.0 there, because `_scaleFactor` equals `_pointerScaleFactor` when no
/// pan-zoom pointer is tracked (`gestures/scale.dart:463-469`).
pub const SCALE_ACCEPT_RATIO: f64 = 1.05;

/// Per-pointer-kind scale slop, ported from `gestures/events.dart:2578-2588`.
pub fn compute_scale_slop(kind: PointerKind) -> i64 {
    match kind {
        PointerKind::Mouse => PRECISE_SCALE_SLOP,
        _ => SCALE_SLOP,
    }
}

/// Per-pointer-kind pan slop, ported from `gestures/events.dart:2564-2575`.
pub fn compute_pan_slop(kind: PointerKind) -> i64 {
    match kind {
        PointerKind::Mouse => PRECISE_PAN_SLOP,
        _ => PAN_SLOP,
    }
}

/// Scale recognizer states (`gestures/scale.dart:433-447`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleState {
    Ready,
    Possible,
    Accepted,
    Started,
}

/// Where the scale baseline is taken from
/// (`gestures/scale.dart:349-356,806-826`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragStartBehavior {
    /// The baseline is the pointer-down position; Flutter's default for
    /// `ScaleGestureRecognizer` (`gestures/scale.dart:353`).
    Down,
    /// The baseline is the moment the gesture started
    /// (`gestures/scale.dart:810-825`).
    Start,
}

/// The two-pointer line used for the rotation factor
/// (`gestures/scale.dart:313-335`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerLine {
    pub start_id: u64,
    pub start: Point,
    pub end_id: u64,
    pub end: Point,
}

/// Scale gesture state as Flutter computes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleDetails {
    pub scale: f64,
    pub horizontal_scale: f64,
    pub vertical_scale: f64,
    pub focal_point: Point,
    pub rotation: f64,
    pub pointer_count: u32,
    pub focal_point_delta: Point,
}

/// Multi-pointer scale recognizer.
#[derive(Debug, Clone)]
pub struct ScaleRecognizer {
    state: ScaleState,
    /// Tracked pointer positions, ordered by pointer id for determinism
    /// (`gestures/scale.dart:602-640` iterates the tracked map).
    locations: BTreeMap<u64, Point>,
    /// Pointers in entrance order; the first two define the rotation line
    /// (`gestures/scale.dart:446,652-674`).
    queue: Vec<u64>,
    kind: PointerKind,
    drag_start_behavior: DragStartBehavior,
    current_focal: Option<Point>,
    initial_focal: Option<Point>,
    current_span: f64,
    initial_span: f64,
    current_h_span: f64,
    initial_h_span: f64,
    current_v_span: f64,
    initial_v_span: f64,
    initial_line: Option<PointerLine>,
    current_line: Option<PointerLine>,
    delta: Point,
    /// Scale-factor samples for the scale velocity
    /// (`gestures/scale.dart:762`).
    scale_samples: Vec<(Ms, f64)>,
}

impl Default for ScaleRecognizer {
    fn default() -> Self {
        Self::new(PointerKind::Touch)
    }
}

impl ScaleRecognizer {
    pub fn new(kind: PointerKind) -> Self {
        Self {
            state: ScaleState::Ready,
            locations: BTreeMap::new(),
            queue: Vec::new(),
            kind,
            drag_start_behavior: DragStartBehavior::Down,
            current_focal: None,
            initial_focal: None,
            current_span: 0.0,
            initial_span: 0.0,
            current_h_span: 0.0,
            initial_h_span: 0.0,
            current_v_span: 0.0,
            initial_v_span: 0.0,
            initial_line: None,
            current_line: None,
            delta: Point::new(0, 0),
            scale_samples: Vec::new(),
        }
    }

    /// Sets the baseline behavior; Flutter's default is `Down`
    /// (`gestures/scale.dart:353`).
    pub fn with_drag_start_behavior(mut self, behavior: DragStartBehavior) -> Self {
        self.drag_start_behavior = behavior;
        self
    }

    pub fn state(&self) -> ScaleState {
        self.state
    }

    pub fn pointer_count(&self) -> u32 {
        self.locations.len() as u32
    }

    /// Registers a pointer (`gestures/scale.dart:557-562`).
    pub fn add_pointer(&mut self, pointer: u64, at: Point) {
        self.locations.insert(pointer, at);
        if !self.queue.contains(&pointer) {
            self.queue.push(pointer);
        }
        if self.state == ScaleState::Ready {
            self.state = ScaleState::Possible;
            // `addAllowedPointer` resets the spans for the first pointer
            // (`gestures/scale.dart:517-525`).
            self.initial_span = 0.0;
            self.current_span = 0.0;
            self.initial_h_span = 0.0;
            self.current_h_span = 0.0;
            self.initial_v_span = 0.0;
            self.current_v_span = 0.0;
        }
        self.reconfigure();
    }

    /// Configuration change handling (`gestures/scale.dart:591,677-692`).
    ///
    /// A pointer joining or leaving ends a running scale and restarts it on
    /// the next event (`gestures/scale.dart:692-731`).
    fn reconfigure(&mut self) {
        self.update_lines();
        self.update();
        if self.state == ScaleState::Started {
            self.state = ScaleState::Accepted;
        }
        self.baseline();
    }

    /// Moves a tracked pointer (`gestures/scale.dart:549-556`).
    ///
    /// A move is not a configuration change, so the baseline is kept
    /// (`gestures/scale.dart:591`).
    pub fn move_pointer(&mut self, pointer: u64, at: Point) {
        if let Some(slot) = self.locations.get_mut(&pointer) {
            *slot = at;
        } else {
            self.locations.insert(pointer, at);
            if !self.queue.contains(&pointer) {
                self.queue.push(pointer);
            }
        }
        self.update_lines();
        self.update();
    }

    /// Removes a pointer (`gestures/scale.dart:563-567`).
    pub fn remove_pointer(&mut self, pointer: u64) {
        self.locations.remove(&pointer);
        self.queue.retain(|id| *id != pointer);
        self.reconfigure();
    }

    /// Advances the state machine and returns the emitted details.
    ///
    /// `arena_accepted` reports that the gesture arena resolved in this
    /// recognizer's favor; Flutter reaches `accepted` through
    /// `resolve(GestureDisposition.accepted)` (`gestures/scale.dart:749`).
    pub fn advance(&mut self, time_ms: Ms, arena_accepted: bool) -> Option<ScaleDetails> {
        if self.state == ScaleState::Ready {
            self.state = ScaleState::Possible;
        }
        if self.state == ScaleState::Possible {
            let span_delta = (self.current_span - self.initial_span).abs();
            let focal_delta = match (self.current_focal, self.initial_focal) {
                (Some(current), Some(initial)) => distance(current, initial),
                _ => 0.0,
            };
            // `gestures/scale.dart:743-753`. The pan-zoom ratio term is 1.0
            // for a pointer-only scale (`gestures/scale.dart:463-469`), so
            // the pointer path accepts on the span and focal deltas only.
            if span_delta > compute_scale_slop(self.kind) as f64
                || focal_delta > compute_pan_slop(self.kind) as f64
                || arena_accepted
            {
                self.state = ScaleState::Accepted;
                if self.drag_start_behavior == DragStartBehavior::Start {
                    // `gestures/scale.dart:810-825`: re-baseline at start.
                    self.baseline();
                }
            }
        } else if arena_accepted {
            self.state = ScaleState::Accepted;
        }
        if self.state == ScaleState::Accepted {
            self.state = ScaleState::Started;
        }
        if self.state != ScaleState::Started {
            return None;
        }
        let details = self.details();
        self.scale_samples.push((time_ms, details.scale));
        if self.scale_samples.len() > 20 {
            self.scale_samples.remove(0);
        }
        Some(details)
    }

    /// The scale factor of the tracked pointers (`gestures/scale.dart:455`).
    pub fn pointer_scale_factor(&self) -> f64 {
        if self.initial_span > 0.0 {
            self.current_span / self.initial_span
        } else {
            1.0
        }
    }

    pub fn details(&self) -> ScaleDetails {
        ScaleDetails {
            scale: self.pointer_scale_factor(),
            horizontal_scale: self.pointer_horizontal_scale_factor(),
            vertical_scale: self.pointer_vertical_scale_factor(),
            focal_point: self.current_focal.unwrap_or(Point::new(0, 0)),
            rotation: self.rotation_factor(),
            pointer_count: self.pointer_count(),
            focal_point_delta: self.delta,
        }
    }

    /// Scale velocity in scale-units per second, estimated with the same
    /// tracker rules as the pointer velocity
    /// (`gestures/velocity_tracker.dart:142-145,179-244`).
    pub fn scale_velocity(&self) -> f64 {
        const ASSUME_STOPPED_MS: Ms = 40; // velocity_tracker.dart:142
        const HORIZON_MS: Ms = 100; // velocity_tracker.dart:144
        const MIN_SAMPLE_SIZE: usize = 3; // velocity_tracker.dart:145
        let Some(newest) = self.scale_samples.last().copied() else {
            return 0.0;
        };
        let mut previous = newest;
        let mut used: Vec<(Ms, f64)> = Vec::new();
        for &(time, scale) in self.scale_samples.iter().rev() {
            let age = newest.0 - time;
            let delta = (time - previous.0).abs();
            if age > HORIZON_MS || delta > ASSUME_STOPPED_MS {
                break; // velocity_tracker.dart:213-219
            }
            previous = (time, scale);
            used.push((time, scale));
        }
        if used.len() < MIN_SAMPLE_SIZE {
            return 0.0; // velocity_tracker.dart:232,248-255
        }
        let oldest = *used.last().unwrap();
        let time: Vec<f64> = used.iter().map(|&(t, _)| -(newest.0 - t) as f64).collect();
        let values: Vec<f64> = used.iter().map(|&(_, v)| v).collect();
        let Some(fit) = least_squares_quadratic(&time, &values) else {
            return 0.0;
        };
        let _ = oldest;
        fit[1] * 1000.0 // velocity_tracker.dart:239-240
    }

    fn pointer_horizontal_scale_factor(&self) -> f64 {
        if self.initial_h_span > 0.0 {
            self.current_h_span / self.initial_h_span
        } else {
            1.0
        }
    }

    fn pointer_vertical_scale_factor(&self) -> f64 {
        if self.initial_v_span > 0.0 {
            self.current_v_span / self.initial_v_span
        } else {
            1.0
        }
    }

    /// Rotation factor (`gestures/scale.dart:487-504`).
    fn rotation_factor(&self) -> f64 {
        let (Some(initial), Some(current)) = (self.initial_line, self.current_line) else {
            return 0.0;
        };
        // `gestures/scale.dart:500-503`: the factor is the current angle
        // minus the initial angle.
        angle_between(current.start, current.end) - angle_between(initial.start, initial.end)
    }

    /// Re-baselines the initial values (`gestures/scale.dart:679-682`).
    fn baseline(&mut self) {
        self.initial_focal = self.current_focal;
        self.initial_span = self.current_span;
        self.initial_h_span = self.current_h_span;
        self.initial_v_span = self.current_v_span;
        self.initial_line = self.current_line;
    }

    /// Recomputes the tracked pointer line (`gestures/scale.dart:646-675`).
    fn update_lines(&mut self) {
        let count = self.locations.len();
        if count < 2 {
            self.current_line = None;
            if count == 1 {
                self.initial_line = None;
            }
            return;
        }
        let start_id = self.queue[0];
        let end_id = self.queue[1];
        let start = self.locations[&start_id];
        let end = self.locations[&end_id];
        let same_pair = self
            .initial_line
            .is_some_and(|line| line.start_id == start_id && line.end_id == end_id);
        let line = PointerLine {
            start_id,
            start,
            end_id,
            end,
        };
        if same_pair {
            // Rotation continues with the same pair (`gestures/scale.dart:657-664`).
            self.current_line = Some(line);
        } else {
            // A new pair starts a new rotation baseline (`gestures/scale.dart:665-674`).
            self.initial_line = Some(line);
            self.current_line = Some(line);
        }
    }

    /// Recomputes focal point and spans (`gestures/scale.dart:597-643`).
    fn update(&mut self) {
        let previous_focal = self.current_focal;
        let count = self.locations.len();
        if count == 0 {
            self.current_focal = None;
            self.current_span = 0.0;
            self.current_h_span = 0.0;
            self.current_v_span = 0.0;
            self.delta = Point::new(0, 0);
            return;
        }
        let mut sum_x = 0i64;
        let mut sum_y = 0i64;
        for point in self.locations.values() {
            sum_x += point.x;
            sum_y += point.y;
        }
        let focal = Point::new(sum_x / count as i64, sum_y / count as i64);
        self.current_focal = Some(focal);
        // `gestures/scale.dart:611-618`: the delta is the focal point movement
        // since the previous update.
        self.delta = match previous_focal {
            Some(previous) => Point::new(focal.x - previous.x, focal.y - previous.y),
            None => Point::new(0, 0),
        };
        let mut total = 0.0f64;
        let mut total_h = 0.0f64;
        let mut total_v = 0.0f64;
        for point in self.locations.values() {
            total += distance(focal, *point);
            total_h += (focal.x - point.x).abs() as f64;
            total_v += (focal.y - point.y).abs() as f64;
        }
        self.current_span = total / count as f64;
        self.current_h_span = total_h / count as f64;
        self.current_v_span = total_v / count as f64;
    }
}

/// Euclidean distance between two points.
fn distance(a: Point, b: Point) -> f64 {
    let dx = (a.x - b.x) as f64;
    let dy = (a.y - b.y) as f64;
    (dx * dx + dy * dy).sqrt()
}

/// Angle of the line between two points (`gestures/scale.dart:500-501`).
fn angle_between(start: Point, end: Point) -> f64 {
    let fx = start.x as f64;
    let fy = start.y as f64;
    let sx = end.x as f64;
    let sy = end.y as f64;
    (fy - sy).atan2(fx - sx)
}
