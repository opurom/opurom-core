//! Regression tests for the multi-pointer scale recognizer, pinned to
//! Flutter 3.50.0 (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).

use opurom_core::{
    compute_pan_slop, compute_scale_slop, Point, PointerKind, ScaleRecognizer, ScaleState,
    PAN_SLOP, SCALE_SLOP,
};

fn two_finger_span(half: i64) -> f64 {
    // Two pointers at ±half on x: the focal point is the origin and each
    // pointer deviates by `half`, so the span is `half`.
    half as f64
}

#[test]
fn slop_constants_match_flutter() {
    // constants.dart:76,80,103,106; events.dart:2564-2588.
    assert_eq!(SCALE_SLOP, 18);
    assert_eq!(PAN_SLOP, 36);
    assert_eq!(compute_scale_slop(PointerKind::Touch), 18);
    assert_eq!(compute_scale_slop(PointerKind::Mouse), 1);
    assert_eq!(compute_pan_slop(PointerKind::Touch), 36);
    assert_eq!(compute_pan_slop(PointerKind::Mouse), 2);
}

#[test]
fn scale_factor_is_span_ratio() {
    // scale.dart:455: currentSpan / initialSpan.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(-50, 0));
    recognizer.add_pointer(2, Point::new(50, 0));
    // Initial span is 50 (average deviation from the focal point).
    let details = recognizer.advance(0, true).expect("started");
    assert!((details.scale - 1.0).abs() < 1e-9);
    assert_eq!(details.pointer_count, 2);
    recognizer.move_pointer(1, Point::new(-100, 0));
    recognizer.move_pointer(2, Point::new(100, 0));
    let details = recognizer.advance(16, true).expect("started");
    // Span doubled from 50 to 100.
    assert!((details.scale - 2.0).abs() < 1e-9);
    assert!((details.horizontal_scale - 2.0).abs() < 1e-9);
    assert!((details.vertical_scale - 1.0).abs() < 1e-9);
}

#[test]
fn span_matches_average_deviation() {
    // scale.dart:630-643: the span is the average deviation from the focal
    // point; the vertical span is the average absolute y deviation.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(0, -20));
    recognizer.add_pointer(2, Point::new(0, 20));
    let details = recognizer.advance(0, true).expect("started");
    assert!((details.scale - 1.0).abs() < 1e-9);
    assert!((details.vertical_scale - 1.0).abs() < 1e-9);
    assert_eq!(details.focal_point, Point::new(0, 0));
}

#[test]
fn rotation_is_line_angle_difference() {
    // scale.dart:487-504: the rotation factor is the difference of the
    // atan2 angles of the initial and current pointer lines.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(-50, 0));
    recognizer.add_pointer(2, Point::new(50, 0));
    recognizer.advance(0, true).expect("started");
    // Rotate both pointers by 90 degrees about the origin.
    recognizer.move_pointer(1, Point::new(0, -50));
    recognizer.move_pointer(2, Point::new(0, 50));
    let details = recognizer.advance(16, true).expect("started");
    // Flutter: factor = atan2(current) - atan2(initial)
    // (gestures/scale.dart:500-503).
    let initial_angle = (0.0f64 - 0.0f64).atan2(-50.0f64 - 50.0f64);
    let current_angle = ((-50.0f64) - 50.0f64).atan2(0.0f64 - 0.0f64);
    let expected = current_angle - initial_angle;
    assert!(
        (details.rotation - expected).abs() < 1e-9,
        "rotation {} expected {expected}",
        details.rotation
    );
}

#[test]
fn accept_requires_scale_or_pan_slop_or_ratio() {
    // scale.dart:743-753: accept when the span delta exceeds the scale
    // slop, the focal delta exceeds the pan slop, or the ratio exceeds 1.05.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(-50, 0));
    recognizer.add_pointer(2, Point::new(50, 0));
    assert_eq!(recognizer.advance(0, false), None);
    assert_eq!(recognizer.state(), ScaleState::Possible);
    // 10 px of span growth is below the 18 px scale slop.
    recognizer.move_pointer(1, Point::new(-60, 0));
    recognizer.move_pointer(2, Point::new(60, 0));
    assert_eq!(recognizer.advance(16, false), None);
    // 30 px of span growth passes the slop.
    recognizer.move_pointer(1, Point::new(-80, 0));
    recognizer.move_pointer(2, Point::new(80, 0));
    assert!(recognizer.advance(32, false).is_some());
    assert_eq!(recognizer.state(), ScaleState::Started);
}

#[test]
fn focal_point_delta_tracks_movement() {
    // scale.dart:611-618: the delta is the focal point movement.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(0, 0));
    recognizer.add_pointer(2, Point::new(10, 0));
    recognizer.advance(0, true).expect("started");
    recognizer.move_pointer(1, Point::new(5, 0));
    recognizer.move_pointer(2, Point::new(15, 0));
    // Flutter recomputes the focal point once per pointer event
    // (gestures/scale.dart:597-618), so each move reports its own delta.
    let details = recognizer.advance(16, true).expect("started");
    assert_eq!(details.focal_point, Point::new(10, 0));
    assert_eq!(details.focal_point_delta, Point::new(3, 0));
}

#[test]
fn single_pointer_keeps_scale_at_one() {
    // scale.dart:653-656: with fewer than two pointers the line is dropped
    // and the span is zero, so the factor stays 1.0.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(10, 10));
    let details = recognizer.advance(0, true).expect("started");
    assert!((details.scale - 1.0).abs() < 1e-9);
    assert_eq!(details.pointer_count, 1);
}

#[test]
fn pointer_removal_updates_count() {
    // scale.dart:563-567: up/cancel removes the pointer.
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(-50, 0));
    recognizer.add_pointer(2, Point::new(50, 0));
    recognizer.advance(0, true).expect("started");
    recognizer.remove_pointer(2);
    assert_eq!(recognizer.pointer_count(), 1);
    let details = recognizer.details();
    assert!((details.scale - 1.0).abs() < 1e-9);
}

#[test]
fn scale_velocity_follows_recorded_samples() {
    // scale.dart:709,762: the scale velocity comes from the recorded scale
    // samples.
    // The tracker needs three samples inside the 100 ms horizon with no gap
    // above 40 ms (velocity_tracker.dart:142-145,213-232).
    let mut recognizer = ScaleRecognizer::new(PointerKind::Touch);
    recognizer.add_pointer(1, Point::new(-50, 0));
    recognizer.add_pointer(2, Point::new(50, 0));
    recognizer.advance(0, true).expect("started");
    for (index, half) in [60, 70, 80].iter().enumerate() {
        recognizer.move_pointer(1, Point::new(-*half, 0));
        recognizer.move_pointer(2, Point::new(*half, 0));
        recognizer
            .advance((index as i64 + 1) * 16, true)
            .expect("started");
    }
    let velocity = recognizer.scale_velocity();
    assert!(velocity > 0.0, "scale velocity {velocity} must be positive");
}

#[test]
fn span_helper_matches_two_finger_layout() {
    assert!((two_finger_span(50) - 50.0).abs() < 1e-9);
}
