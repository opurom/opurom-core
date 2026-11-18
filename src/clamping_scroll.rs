//! Clamping scroll physics ported from Flutter 3.50.0
//! (`widgets/scroll_simulation.dart`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`). Source paths and
//! line numbers are relative to `packages/flutter/lib/src/`.
//!
//! Ported formulas:
//! - `friction = 0.015` (`scroll_simulation.dart:169`), derived from Android's
//!   `mFlingFriction` (`:191`);
//! - `decelerationRate = log(0.78) / log(0.9)`, `inflexion = 0.35`, and
//!   `physicalCoeff = 9.80665 * 39.37 * 160 * 0.84` (`:201-215`);
//! - duration = `decelerationRate * inflexion * (|v| / referenceVelocity) ^
//!   (1 / (decelerationRate - 1))` where `referenceVelocity =
//!   friction * physicalCoeff / inflexion` (`:218-230`);
//! - total distance = `v * duration / decelerationRate` (`:234-248`);
//! - `x(t) = position + distance * (1 - (1 - t)^decelerationRate)` and
//!   `dx(t) = v * (1 - t)^(decelerationRate - 1)` with `t` clamped to
//!   `[0, 1]` (`:250-260`);
//! - `isDone(time)` uses Flutter's default tolerance of 1e-3 for time and
//!   velocity (`physics/tolerance.dart:9-23`,
//!   `physics/simulation.dart:isDone`).
//!
//! The simulation reads no clock: every `x`/`dx` call takes a caller-supplied
//! time in seconds, so the same call sequence yields identical positions on
//! every host and at every refresh rate.

/// Flutter's default friction for `ClampingScrollSimulation`
/// (`widgets/scroll_simulation.dart:169`).
pub const DEFAULT_FRICTION: f64 = 0.015;
/// `log(0.78) / log(0.9)` is computed at runtime by [`deceleration_rate`]
/// (`scroll_simulation.dart:201`).
/// `0.35` (`scroll_simulation.dart:204`).
pub const INFLEXION: f64 = 0.35;
/// `9.80665 * 39.37 * 160 * 0.84` (`scroll_simulation.dart:208-215`).
pub const PHYSICAL_COEFF: f64 = 9.80665 * 39.37 * 160.0 * 0.84;
/// Flutter's default tolerance (`physics/tolerance.dart:20`).
pub const TOLERANCE: f64 = 1e-3;

/// The exact deceleration rate computed from Flutter's source expression.
pub fn deceleration_rate() -> f64 {
    0.78f64.ln() / 0.9f64.ln()
}

/// A pure clamping-scroll simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct ClampingScrollSimulation {
    position: f64,
    velocity: f64,
    friction: f64,
    duration: f64,
    distance: f64,
}

impl ClampingScrollSimulation {
    /// Creates a simulation from Flutter's required inputs
    /// (`scroll_simulation.dart:166-174`).
    pub fn new(position: f64, velocity: f64, friction: f64) -> Self {
        if velocity == 0.0 {
            // Flutter's duration formula divides by zero for an exact-zero
            // velocity; a real fling is never created with v == 0, so the
            // kernel keeps the degenerate case stable with no motion.
            return Self {
                position,
                velocity,
                friction,
                duration: 0.0,
                distance: 0.0,
            };
        }
        let rate = deceleration_rate();
        let reference_velocity = friction * PHYSICAL_COEFF / INFLEXION;
        let android_duration = (velocity.abs() / reference_velocity).powf(1.0 / (rate - 1.0));
        let duration = rate * INFLEXION * android_duration;
        let distance = velocity * duration / rate;
        Self {
            position,
            velocity,
            friction,
            duration,
            distance,
        }
    }

    /// Creates a simulation with Flutter's default friction
    /// (`scroll_simulation.dart:169`).
    pub fn with_default_friction(position: f64, velocity: f64) -> Self {
        Self::new(position, velocity, DEFAULT_FRICTION)
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn velocity(&self) -> f64 {
        self.velocity
    }

    pub fn friction(&self) -> f64 {
        self.friction
    }

    /// Total simulated duration in seconds (`scroll_simulation.dart:194-195`).
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Total signed travel distance in logical pixels
    /// (`scroll_simulation.dart:197-198`).
    pub fn distance(&self) -> f64 {
        self.distance
    }

    /// Position at `time` seconds (`scroll_simulation.dart:250-254`).
    pub fn x(&self, time: f64) -> f64 {
        if self.duration == 0.0 {
            return self.position;
        }
        let t = (time / self.duration).clamp(0.0, 1.0);
        self.position + self.distance * (1.0 - (1.0 - t).powf(deceleration_rate()))
    }

    /// Velocity at `time` seconds (`scroll_simulation.dart:256-260`).
    pub fn dx(&self, time: f64) -> f64 {
        if self.duration == 0.0 {
            return 0.0;
        }
        let t = (time / self.duration).clamp(0.0, 1.0);
        self.velocity * (1.0 - t).powf(deceleration_rate() - 1.0)
    }

    /// Whether the simulation has finished by `time`. Flutter's
    /// `Simulation.isDone` returns `time >= _endTime || dx(time).abs() <
    /// tolerance.velocity` (`physics/simulation.dart:isDone`).
    pub fn is_done(&self, time: f64) -> bool {
        time >= self.duration || self.dx(time).abs() < TOLERANCE
    }

    /// Final resting position; equivalent to `x(duration)`
    /// (`scroll_simulation.dart:250-254`).
    pub fn final_x(&self) -> f64 {
        self.position + self.distance
    }
}
