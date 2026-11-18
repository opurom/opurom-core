//! Axis-specific drag recognizers ported from Flutter 3.50.0
//! (`gestures/monodrag.dart`, `gestures/drag_details.dart`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`). Source paths and
//! line numbers are relative to `packages/flutter/lib/src/`.
//!
//! Ported rules:
//! - a horizontal drag reports only `delta.dx` and takes `dx` as the primary
//!   value; a vertical drag reports only `delta.dy` and takes `dy`
//!   (`gestures/monodrag.dart:1036-1043,972-979`,
//!   `gestures/drag_details.dart:119-132`);
//! - a drag is accepted when the signed global distance exceeds the hit slop
//!   (`gestures/monodrag.dart:697-709,965-970,1029-1034`);
//! - a fling requires the primary-axis velocity to exceed 50 px/s AND the
//!   primary-axis offset to exceed the hit slop; the velocity is clipped to
//!   ±8000 px/s (`gestures/monodrag.dart:941-960,1005-1025`,
//!   `gestures/constants.dart:90,95`).
//!
//! This module provides the geometry and gate helpers; the arena in
//! `crate::gestures` owns the pointer state machine. Hosts choose the axis
//! mode when declaring the gesture in the IR bridge, so no IR field changes.

use crate::gestures::{PointerKind, Velocity, MIN_FLING_VELOCITY};

/// Drag axis (`gestures/monodrag.dart:931-1045`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAxis {
    /// `HorizontalDragGestureRecognizer`.
    Horizontal,
    /// `VerticalDragGestureRecognizer`.
    Vertical,
}

/// The masked delta reported to the callback
/// (`gestures/drag_details.dart:149-157`, `monodrag.dart:1037,973`).
pub fn primary_delta(axis: DragAxis, dx: i64, dy: i64) -> (i64, i64) {
    match axis {
        DragAxis::Horizontal => (dx, 0),
        DragAxis::Vertical => (0, dy),
    }
}

/// The signed primary-axis component of an offset.
pub fn primary_value(axis: DragAxis, dx: i64, dy: i64) -> i64 {
    match axis {
        DragAxis::Horizontal => dx,
        DragAxis::Vertical => dy,
    }
}

/// Whether the pointer moved far enough to accept the drag
/// (`monodrag.dart:697-709,965-970,1029-1034`).
pub fn has_sufficient_distance(axis: DragAxis, kind: PointerKind, dx: i64, dy: i64) -> bool {
    let slop = match kind {
        // `computeHitSlop`: mouse uses `kPrecisePointerHitSlop`; touch and
        // stylus use `kTouchSlop` (`gestures/events.dart:2550-2561`).
        PointerKind::Mouse => 1,
        _ => 18,
    };
    let distance = (primary_value(axis, dx, dy) as f64).abs();
    distance > slop as f64
}

/// Whether a completed drag should produce a fling
/// (`monodrag.dart:942-947,1006-1011`).
pub fn is_fling_gesture(
    axis: DragAxis,
    kind: PointerKind,
    velocity: Velocity,
    offset_dx: i64,
    offset_dy: i64,
) -> bool {
    let primary_velocity = primary_value(axis, velocity.x, velocity.y);
    let primary_offset = primary_value(axis, offset_dx, offset_dy);
    // `computeHitSlop` rather than `computePanSlop` is used by the fling
    // gate (`monodrag.dart:944,1008`).
    let min_distance = match kind {
        PointerKind::Mouse => 1,
        _ => 18,
    };
    primary_velocity.abs() > MIN_FLING_VELOCITY && primary_offset.abs() > min_distance
}

/// Returns the fling velocity with the off-axis component zeroed and the
/// primary component clipped to ±8000 px/s
/// (`monodrag.dart:949-960,1013-1025`).
pub fn consider_fling(axis: DragAxis, velocity: Velocity) -> Velocity {
    let primary = primary_value(axis, velocity.x, velocity.y).clamp(-8_000, 8_000);
    primary_delta(axis, primary, primary).into()
}

impl From<(i64, i64)> for Velocity {
    fn from((x, y): (i64, i64)) -> Self {
        Self { x, y }
    }
}
