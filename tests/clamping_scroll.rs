//! Regression tests for the Flutter 3.50 clamping scroll simulation, pinned
//! to `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb.

use opurom_core::{
    clamping_scroll::deceleration_rate, ClampingScrollSimulation, DEFAULT_FRICTION, INFLEXION,
    PHYSICAL_COEFF, TOLERANCE,
};

#[test]
fn constants_match_flutter_source() {
    // scroll_simulation.dart:169,204,208-215; tolerance.dart:20.
    assert_eq!(DEFAULT_FRICTION, 0.015);
    assert_eq!(INFLEXION, 0.35);
    assert!((PHYSICAL_COEFF - 51890.2017312).abs() < 1e-5);
    assert_eq!(TOLERANCE, 1e-3);
    assert!((deceleration_rate() - 2.3582018).abs() < 1e-6);
}

#[test]
fn zero_velocity_produces_no_motion() {
    let simulation = ClampingScrollSimulation::with_default_friction(100.0, 0.0);
    assert_eq!(simulation.duration(), 0.0);
    assert_eq!(simulation.x(0.0), 100.0);
    assert_eq!(simulation.x(10.0), 100.0);
    assert_eq!(simulation.dx(0.0), 0.0);
}

#[test]
fn fling_moves_in_velocity_direction_and_stops_at_final_x() {
    // scroll_simulation.dart:250-260: x rises from position toward the
    // signed total distance and clamps after the duration.
    let simulation = ClampingScrollSimulation::with_default_friction(0.0, 1000.0);
    assert!(simulation.duration() > 0.0);
    assert!(simulation.distance() > 0.0);
    assert_eq!(simulation.x(0.0), 0.0);
    let middle = simulation.x(simulation.duration() / 2.0);
    assert!(middle > 0.0 && middle < simulation.final_x());
    assert!((simulation.x(simulation.duration()) - simulation.final_x()).abs() < 1e-9);
    assert!((simulation.x(simulation.duration() * 10.0) - simulation.final_x()).abs() < 1e-9);
}

#[test]
fn negative_velocity_moves_in_negative_direction() {
    let simulation = ClampingScrollSimulation::with_default_friction(500.0, -1000.0);
    assert!(simulation.distance() < 0.0);
    assert!(simulation.final_x() < 500.0);
    assert!(simulation.dx(0.0) < 0.0);
}

#[test]
fn velocity_decays_monotonically_to_zero() {
    // scroll_simulation.dart:256-260: dx(t) = v * (1-t)^(rate-1), with the
    // exponent negative, so the magnitude decreases with time.
    let simulation = ClampingScrollSimulation::with_default_friction(0.0, 1000.0);
    let samples = [0.0, 0.1, 0.3, 0.6, 0.9, 1.0];
    let mut previous = f64::INFINITY;
    for time in samples {
        let speed = simulation.dx(time).abs();
        assert!(speed <= previous + 1e-9);
        previous = speed;
    }
    assert_eq!(simulation.dx(1.0), 0.0);
}

#[test]
fn duration_scales_with_velocity_exponent() {
    // scroll_simulation.dart:218-230: duration is proportional to
    // velocity^(1/(rate-1)), so quadrupling velocity multiplies the duration.
    let slow = ClampingScrollSimulation::with_default_friction(0.0, 500.0);
    let fast = ClampingScrollSimulation::with_default_friction(0.0, 2000.0);
    let expected = 4.0f64.powf(1.0 / (deceleration_rate() - 1.0));
    assert!((fast.duration() / slow.duration() - expected).abs() < 1e-6);
}

#[test]
fn is_done_uses_flutter_tolerance() {
    let simulation = ClampingScrollSimulation::with_default_friction(0.0, 1000.0);
    assert!(!simulation.is_done(0.0));
    assert!(simulation.is_done(simulation.duration()));
    assert!(simulation.is_done(simulation.duration() + 1.0));
}

#[test]
fn same_time_sequence_is_deterministic() {
    let a = ClampingScrollSimulation::with_default_friction(12.5, 800.0);
    let b = ClampingScrollSimulation::with_default_friction(12.5, 800.0);
    for time in [0.0, 0.016, 0.033, 0.05, 0.1, 0.5] {
        assert_eq!(a.x(time).to_bits(), b.x(time).to_bits());
        assert_eq!(a.dx(time).to_bits(), b.dx(time).to_bits());
    }
}

#[test]
fn friction_changes_distance() {
    // scroll_simulation.dart:184-192: more friction means a shorter fling.
    let low = ClampingScrollSimulation::new(0.0, 1000.0, 0.01);
    let high = ClampingScrollSimulation::new(0.0, 1000.0, 0.03);
    assert!(low.distance() > high.distance());
}
