//! Regression tests for axis-specific drag recognizers, pinned to Flutter 3.50.0
//! (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).

use opurom_core::{
    consider_fling, has_sufficient_distance, is_fling_gesture, primary_delta, primary_value,
    DragAxis, PointerKind, Velocity,
};

#[test]
fn horizontal_delta_zeros_the_vertical_axis() {
    // monodrag.dart:1037: Offset(delta.dx, 0.0).
    assert_eq!(primary_delta(DragAxis::Horizontal, 7, 9), (7, 0));
    assert_eq!(primary_delta(DragAxis::Vertical, 7, 9), (0, 9));
}

#[test]
fn primary_value_matches_the_axis() {
    // monodrag.dart:1040,976.
    assert_eq!(primary_value(DragAxis::Horizontal, -5, 30), -5);
    assert_eq!(primary_value(DragAxis::Vertical, -5, 30), 30);
}

#[test]
fn accept_distance_uses_signed_primary_axis_and_hit_slop() {
    // monodrag.dart:697-709,1029-1034: abs(primary distance) > hit slop.
    assert!(has_sufficient_distance(
        DragAxis::Horizontal,
        PointerKind::Touch,
        19,
        0
    ));
    assert!(!has_sufficient_distance(
        DragAxis::Horizontal,
        PointerKind::Touch,
        -18,
        0
    ));
    // Mouse uses the precise 1 px slop (events.dart:2550-2561).
    assert!(has_sufficient_distance(
        DragAxis::Horizontal,
        PointerKind::Mouse,
        2,
        0
    ));
    assert!(!has_sufficient_distance(
        DragAxis::Vertical,
        PointerKind::Mouse,
        2,
        0
    ));
}

#[test]
fn fling_needs_velocity_and_distance_on_the_primary_axis() {
    // monodrag.dart:1006-1011: dx velocity > 50 and dx offset > hit slop.
    assert!(is_fling_gesture(
        DragAxis::Horizontal,
        PointerKind::Touch,
        Velocity { x: 100, y: 0 },
        20,
        0
    ));
    // The off-axis component alone never creates a fling.
    assert!(!is_fling_gesture(
        DragAxis::Horizontal,
        PointerKind::Touch,
        Velocity { x: 0, y: 100 },
        0,
        20
    ));
    // Velocity without distance is not a fling.
    assert!(!is_fling_gesture(
        DragAxis::Horizontal,
        PointerKind::Touch,
        Velocity { x: 100, y: 0 },
        5,
        0
    ));
}

#[test]
fn fling_velocity_is_clipped_and_axis_masked() {
    // monodrag.dart:1014-1025: clamp to [-8000, 8000], zero the other axis.
    let velocity = consider_fling(
        DragAxis::Horizontal,
        Velocity {
            x: 20_000,
            y: 9_000,
        },
    );
    assert_eq!(velocity, Velocity { x: 8_000, y: 0 });
    let velocity = consider_fling(
        DragAxis::Vertical,
        Velocity {
            x: 9_000,
            y: -20_000,
        },
    );
    assert_eq!(velocity, Velocity { x: 0, y: -8_000 });
}

#[test]
fn fling_gate_uses_hit_slop_not_pan_slop() {
    // monodrag.dart:944,1008: computeHitSlop, i.e. 18 px for touch.
    assert!(is_fling_gesture(
        DragAxis::Vertical,
        PointerKind::Touch,
        Velocity { x: 0, y: 60 },
        0,
        19
    ));
    assert!(!is_fling_gesture(
        DragAxis::Vertical,
        PointerKind::Touch,
        Velocity { x: 0, y: 60 },
        0,
        18
    ));
}
