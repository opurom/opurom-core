//! Deterministic pointer-event core ported from Flutter 3.50.0's gesture
//! arena and recognizer state machines. This module owns plain data only:
//! no widget tree, no wall clock, no I/O and no dependency on opurom-render.
//!
//! Base checkout: `flutter-golden` HEAD `ab2912035e160d9cad6306df307576da6128ebdb`
//! (`[flutter-3.50-candidate.0]`, 2026-11-09), `bin/cache/flutter.version`
//! `3.50.0`, Dart SDK `3.14.0`. Every ported rule below carries its source
//! file and line number relative to `packages/flutter/lib/src/` at that HEAD.
//!
//! Ported mechanisms:
//! - arena membership/close/eager-winner/sweep/hold (`gestures/arena.dart:59-292`);
//! - press 100 ms, long press 500 ms, double tap 300/40/100 ms
//!   (`gestures/constants.dart:14,29,35,39,49`, `gestures/tap.dart:212`,
//!   `gestures/long_press.dart:288,640-647`, `gestures/multitap.dart:328-359`);
//! - touch slop 18 px, precise pointer 1 px, fling gate 50/8000 px/s
//!   (`gestures/constants.dart:65,90,95,103`, `gestures/events.dart:2550-2561`,
//!   `gestures/recognizer.dart:651-668,720-742`);
//! - velocity tracker 20 samples / 100 ms horizon / 40 ms stopped /
//!   3-sample minimum / quadratic least-squares fit
//!   (`gestures/velocity_tracker.dart:142-145,159-171,179-244`,
//!   `gestures/lsq_solver.dart:55-203`, `foundation/constants.dart:71`).
//!
//! The host supplies every timestamp via events and ticks. A single ordered
//! event stream always produces the same recognizer events, on every host.

use std::collections::{HashMap, VecDeque};

/// Logical pixels. Flutter's gesture constants are `double`; Opurom freezes
/// them to integers at the IR boundary so the kernel stays deterministic.
pub type Px = i64;
/// Milliseconds on a monotonic clock owned by the host.
pub type Ms = i64;

pub const PRESS_TIMEOUT_MS: Ms = 100; // gestures/constants.dart:14; tap.dart:212
pub const LONG_PRESS_TIMEOUT_MS: Ms = 500; // gestures/constants.dart:29; long_press.dart:288
pub const DOUBLE_TAP_TIMEOUT_MS: Ms = 300; // gestures/constants.dart:35; multitap.dart:358
pub const DOUBLE_TAP_MIN_MS: Ms = 40; // gestures/constants.dart:39; multitap.dart:250
pub const DOUBLE_TAP_SLOP: Px = 100; // gestures/constants.dart:49; multitap.dart:225,911
pub const TOUCH_SLOP: Px = 18; // gestures/constants.dart:65; events.dart:2550-2561
pub const PRECISE_HIT_SLOP: Px = 1; // gestures/constants.dart:103; events.dart:2552-2553
pub const PRECISE_POINTER_PAN_SLOP: Px = 2; // gestures/constants.dart:106; events.dart:2564-2575
pub const MIN_FLING_VELOCITY: Px = 50; // gestures/constants.dart:90; monodrag.dart:943
pub const MAX_FLING_VELOCITY: Px = 8000; // gestures/constants.dart:95; monodrag.dart:954

/// `PointerDeviceKind` subset used by `computeHitSlop` (`events.dart:2550-2561`).
/// Only touch-like and precise devices differ in Flutter 3.50.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Touch,
    Stylus,
    InvertedStylus,
    Trackpad,
    Unknown,
    Mouse,
}

impl PointerKind {
    /// Hit slop in logical pixels: mouse uses `kPrecisePointerHitSlop`,
    /// every other kind uses `touchSlop` or `kTouchSlop`.
    /// Source: `gestures/events.dart:2550-2561`, `constants.dart:65,103`.
    pub fn hit_slop(self) -> Px {
        match self {
            Self::Mouse => PRECISE_HIT_SLOP,
            _ => TOUCH_SLOP,
        }
    }
}

/// A 2D point in the host's global logical-pixel coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: Px,
    pub y: Px,
}

impl Point {
    pub fn new(x: Px, y: Px) -> Self {
        Self { x, y }
    }

    fn squared_distance_to(self, other: Self) -> u128 {
        let dx = (self.x - other.x) as i128;
        let dy = (self.y - other.y) as i128;
        (dx * dx + dy * dy) as u128
    }
}

/// One hit-test entry, leaf first, carrying the node's gesture declaration
/// from IR `props.gesture`. Geometry and hit testing stay in the renderer;
/// the kernel receives plain data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitEntry {
    pub node: u32,
    pub gesture: Option<GestureKind>,
}

/// Recognizer kinds supported in Phase 0. One member per hit-path node;
/// multi-recognizer lists would be an additive IR change and are out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKind {
    Tap,
    DoubleTap,
    LongPress,
    Drag,
}

/// Disposition passed to `GestureArenaEntry.resolve` (`arena.dart:54-56`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Accepted,
    Rejected,
}

/// Input to the pure kernel. Timestamps are host-owned; pointer events at the
/// same `time_ms` as a tick are always injected before the tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaEvent {
    /// One arena opens per pointer (`arena.dart:118-129`). `path` is leaf→root,
    /// matching Flutter's hit-test dispatch order (`gestures/binding.dart:506-508`).
    Down {
        pointer: u64,
        at: Point,
        time_ms: Ms,
        kind: PointerKind,
        buttons: u32,
        path: Vec<HitEntry>,
    },
    Move {
        pointer: u64,
        at: Point,
        time_ms: Ms,
        buttons: u32,
    },
    Up {
        pointer: u64,
        at: Point,
        time_ms: Ms,
    },
    Cancel {
        pointer: u64,
        time_ms: Ms,
    },
    /// Host arms a timer for `next_deadline_ms` and injects this when it fires.
    Tick {
        now_ms: Ms,
    },
}

/// Why a recognizer stopped tracking a pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// `PointerCancelEvent` (`tap.dart:315-320`).
    PointerCancel,
    /// Movement exceeded pre/post-accept slop (`recognizer.dart:720-742`).
    ExceededSlop,
    /// Buttons changed during the sequence (`tap.dart:321-323`).
    ButtonsChanged,
    /// Another recognizer won the arena (`tap.dart:357-367`).
    ArenaLost,
}

/// Output of the pure kernel; hosts map these onto `core::Event` unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognizerEvent {
    /// `didExceedDeadline` → `_checkDown` (`tap.dart:342-345,370-376`).
    TapDown {
        node: u32,
        at: Point,
    },
    /// Tap recognizer accepts at up; emitted on arena win (`tap.dart:347-354,378-385`).
    TapUp {
        node: u32,
        at: Point,
    },
    /// Second tap accepted; both arena entries resolve (`multitap.dart:339-345,368-372`).
    DoubleTap {
        node: u32,
        at: Point,
    },
    TapCancel {
        node: u32,
        reason: CancelReason,
    },
    /// Deadline fires mid-hold (`long_press.dart:640-647`).
    LongPressStart {
        node: u32,
        at: Point,
    },
    LongPressEnd {
        node: u32,
        at: Point,
    },
    /// Drag accepts past hit slop (`monodrag.dart:690-711`).
    DragStart {
        node: u32,
        at: Point,
    },
    DragUpdate {
        node: u32,
        dx: Px,
        dy: Px,
        velocity: Velocity,
    },
    /// Fling gate: ≥50 px/s and ≥hit-slop offset; velocity clipped to 8000
    /// (`monodrag.dart:942-960,1006-1020`).
    DragEnd {
        node: u32,
        velocity: Velocity,
        fling: bool,
    },
    Cancel {
        node: u32,
        reason: CancelReason,
    },
}

/// Velocity estimate from the Flutter 3.50 tracker; coordinates are px/s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Velocity {
    pub x: Px,
    pub y: Px,
}

/// Result of one kernel step: ordered recognizer events and the next host timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaStep {
    pub events: Vec<RecognizerEvent>,
    pub next_deadline_ms: Option<Ms>,
}

impl ArenaStep {
    fn empty() -> Self {
        Self {
            events: Vec::new(),
            next_deadline_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberState {
    /// `GestureRecognizerState.possible` (`recognizer.dart:670-674`).
    Possible,
    Won,
    Rejected,
}

#[derive(Debug, Clone)]
struct Member {
    node: u32,
    kind: GestureKind,
    state: MemberState,
    /// Earliest live deadline of this recognizer, in host ms.
    deadline: Option<Ms>,
    /// Whether the recognizer sent tap-down/long-press-start already.
    started: bool,
    down: Point,
    last: Point,
    buttons: u32,
}

impl Member {
    fn new(node: u32, kind: GestureKind, at: Point, time_ms: Ms, buttons: u32) -> Self {
        let deadline = match kind {
            GestureKind::Tap => Some(time_ms + PRESS_TIMEOUT_MS), // tap.dart:212
            GestureKind::LongPress => Some(time_ms + LONG_PRESS_TIMEOUT_MS), // long_press.dart:288
            GestureKind::DoubleTap | GestureKind::Drag => None,
        };
        Self {
            node,
            kind,
            state: MemberState::Possible,
            deadline,
            started: false,
            down: at,
            last: at,
            buttons,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArenaPhase {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy)]
struct FirstTap {
    pointer: u64,
    node: u32,
    at: Point,
    up_ms: Ms,
    expiry: Ms,
}

#[derive(Debug, Clone)]
struct PointerArena {
    phase: ArenaPhase,
    kind: PointerKind,
    /// Members are in join order (leaf→root); removals preserve that order.
    members: Vec<Member>,
    /// First accept while still open (`arena.dart:59-69,233-237`).
    eager_winner: Option<u32>,
    held: bool,
    pending_sweep: bool,
    tracker: VelocityTracker,
}

impl PointerArena {
    fn new(kind: PointerKind) -> Self {
        Self {
            phase: ArenaPhase::Open,
            kind,
            members: Vec::new(),
            eager_winner: None,
            held: false,
            pending_sweep: false,
            tracker: VelocityTracker::new(kind),
        }
    }

    fn member(&self, node: u32) -> Option<&Member> {
        self.members.iter().find(|m| m.node == node)
    }

    fn member_mut(&mut self, node: u32) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.node == node)
    }

    /// `acceptGesture` on a member: while open it only records the eager
    /// winner; while closed it resolves immediately (`arena.dart:233-240`).
    fn accept(&mut self, node: u32, events: &mut Vec<RecognizerEvent>) {
        let Some(member) = self.member_mut(node) else {
            return;
        };
        if member.state == MemberState::Rejected {
            return;
        }
        if self.phase == ArenaPhase::Open {
            if self.eager_winner.is_none() {
                self.eager_winner = Some(node);
            }
            return;
        }
        if self.held {
            // Double-tap keeps the arena open past up until its timer fires
            // (`multitap.dart:328-336`, `arena.dart:181-191`).
            return;
        }
        self.resolve_in_favor_of(node, events);
    }

    fn reject(&mut self, node: u32, events: &mut Vec<RecognizerEvent>, reason: CancelReason) {
        let Some(index) = self.members.iter().position(|m| m.node == node) else {
            return;
        };
        let member = self.members.remove(index);
        match member.kind {
            GestureKind::Tap | GestureKind::LongPress if member.started => {
                events.push(RecognizerEvent::Cancel { node, reason });
            }
            GestureKind::Tap | GestureKind::LongPress => {
                events.push(RecognizerEvent::TapCancel { node, reason })
            }
            // Drag/DoubleTap self-rejections are silent in Flutter
            // (`monodrag.dart:747-750`, `multitap.dart:277-292`).
            GestureKind::Drag | GestureKind::DoubleTap => {}
        }
        if self.eager_winner == Some(node) {
            self.eager_winner = None;
        }
        // Flutter's `_resolve` calls `_tryToResolveArena` after each rejection
        // (arena.dart:248-250); a pending eager winner resolves once only one
        // member remains.
        if self.phase == ArenaPhase::Closed {
            self.try_resolve(events);
        }
    }

    fn resolve_in_favor_of(&mut self, node: u32, events: &mut Vec<RecognizerEvent>) {
        // `_resolveInFavorOf` rejects all other members before accepting the
        // winner (`arena.dart:281-292`).
        let losers: Vec<(u32, GestureKind, bool)> = self
            .members
            .iter()
            .filter(|m| m.node != node)
            .map(|m| (m.node, m.kind, m.started))
            .collect();
        for (other, kind, started) in losers {
            // A recognizer that never sent its down/start callback has no
            // cancellation to emit (`tap.dart:357-367` only cancels when
            // `_sentTapDown` is true).
            if started {
                events.push(match kind {
                    GestureKind::Tap | GestureKind::LongPress => RecognizerEvent::Cancel {
                        node: other,
                        reason: CancelReason::ArenaLost,
                    },
                    GestureKind::Drag | GestureKind::DoubleTap => RecognizerEvent::Cancel {
                        node: other,
                        reason: CancelReason::ArenaLost,
                    },
                });
            }
        }
        self.members.retain(|m| m.node == node);
        self.eager_winner = None;
        let Some(member) = self.member_mut(node) else {
            return;
        };
        member.state = MemberState::Won;
        member.deadline = None;
        emit_accept(member, events);
    }

    /// `close` + `_tryToResolveArena` (`arena.dart:134-142,254-266`). The
    /// single-member microtask default (`arena.dart:257-259,268-279`) is
    /// resolved synchronously: no host timer is needed for a sole member.
    fn close(&mut self, events: &mut Vec<RecognizerEvent>) {
        self.phase = ArenaPhase::Closed;
        self.try_resolve(events);
    }

    fn try_resolve(&mut self, events: &mut Vec<RecognizerEvent>) {
        if self.phase != ArenaPhase::Closed {
            return;
        }
        if self.members.len() == 1 {
            let node = self.members[0].node;
            self.resolve_in_favor_of(node, events);
        } else if self.members.is_empty() {
            self.eager_winner = None;
        } else if let Some(node) = self.eager_winner {
            self.resolve_in_favor_of(node, events);
        }
    }

    /// `sweep` gives victory to the first surviving member (`arena.dart:157-179`).
    fn sweep(&mut self, events: &mut Vec<RecognizerEvent>) {
        if self.held {
            self.pending_sweep = true;
            return; // arena.dart:163-167
        }
        let Some(first) = self.members.first().map(|m| m.node) else {
            return;
        };
        self.resolve_in_favor_of(first, events);
    }

    fn release(&mut self, events: &mut Vec<RecognizerEvent>) {
        self.held = false;
        if self.pending_sweep {
            self.pending_sweep = false;
            self.sweep(events); // arena.dart:211-221
        }
    }

    fn next_deadline(&self) -> Option<Ms> {
        self.members
            .iter()
            .filter(|m| m.state == MemberState::Possible)
            .filter_map(|m| m.deadline)
            .min()
    }
}

/// Emits the callbacks Flutter fires when `acceptGesture` reaches a
/// recognizer. Source order: down, then up (`tap.dart:347-354,378-385`).
fn emit_accept(member: &mut Member, events: &mut Vec<RecognizerEvent>) {
    match member.kind {
        GestureKind::Tap => {
            if !member.started {
                member.started = true;
                events.push(RecognizerEvent::TapDown {
                    node: member.node,
                    at: member.down,
                });
            }
            events.push(RecognizerEvent::TapUp {
                node: member.node,
                at: member.last,
            });
        }
        GestureKind::DoubleTap => {
            // DoubleTapGestureRecognizer's acceptGesture is a no-op; its
            // callback fires when the second up is registered
            // (`multitap.dart:274-275,339-345`). See `on_up`.
        }
        GestureKind::LongPress => {
            if !member.started {
                member.started = true;
                events.push(RecognizerEvent::LongPressStart {
                    node: member.node,
                    at: member.down,
                });
            }
        }
        GestureKind::Drag => {}
    }
}

/// Quadratic least-squares fit ported from `gestures/lsq_solver.dart:88-203`.
/// Inputs are `(t, x)`, `(t, y)` arrays with unit weights, exactly as the
/// tracker builds them (`velocity_tracker.dart:190-235`).
pub(crate) fn least_squares_quadratic(time: &[f64], values: &[f64]) -> Option<[f64; 3]> {
    const TOLERANCE: f64 = 1e-10; // foundation/constants.dart:71
    let m = time.len();
    let n = 3usize;
    if n > m {
        return None;
    }
    // A: row i holds w[h] * x[h]^i, i in 0..n (lsq_solver.dart:118-125).
    let mut a = vec![vec![0.0f64; m]; n];
    for h in 0..m {
        a[0][h] = 1.0;
        for i in 1..n {
            a[i][h] = a[i - 1][h] * time[h];
        }
    }
    let mut q = vec![vec![0.0f64; m]; n];
    let mut r = vec![vec![0.0f64; n]; n];
    for j in 0..n {
        q[j].copy_from_slice(&a[j]);
        for i in 0..j {
            let dot: f64 = (0..m).map(|h| q[j][h] * q[i][h]).sum();
            for h in 0..m {
                q[j][h] -= dot * q[i][h];
            }
        }
        let norm: f64 = q[j].iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm < TOLERANCE {
            return None; // lsq_solver.dart:144-148
        }
        let inverse_norm = 1.0 / norm;
        for h in 0..m {
            q[j][h] *= inverse_norm;
        }
        for i in 0..n {
            r[j][i] = if i < j {
                0.0
            } else {
                (0..m).map(|h| q[j][h] * a[i][h]).sum()
            };
        }
    }
    let mut coefficients = [0.0f64; 3];
    for i in (0..n).rev() {
        let mut value: f64 = (0..m).map(|h| q[i][h] * values[h]).sum();
        for j in (i + 1..n).rev() {
            value -= r[i][j] * coefficients[j];
        }
        coefficients[i] = value / r[i][i];
    }
    Some(coefficients)
}

/// 20-sample circular buffer with quadratic least-squares velocity estimation,
/// ported from `gestures/velocity_tracker.dart:138-264`. The 40 ms "stopped"
/// check uses the last two injected sample timestamps; the host clock is never
/// read inside the kernel.
#[derive(Debug, Clone)]
struct VelocityTracker {
    kind: PointerKind,
    samples: VecDeque<(Ms, Point)>,
}

impl VelocityTracker {
    const ASSUME_STOPPED_MS: Ms = 40; // velocity_tracker.dart:142
    const HISTORY_SIZE: usize = 20; // velocity_tracker.dart:143
    const HORIZON_MS: Ms = 100; // velocity_tracker.dart:144
    const MIN_SAMPLE_SIZE: usize = 3; // velocity_tracker.dart:145

    fn new(kind: PointerKind) -> Self {
        Self {
            kind,
            samples: VecDeque::with_capacity(Self::HISTORY_SIZE),
        }
    }

    fn add(&mut self, time_ms: Ms, point: Point) {
        if self.samples.len() == Self::HISTORY_SIZE {
            self.samples.pop_front();
        }
        self.samples.push_back((time_ms, point)); // :163-171
    }

    /// Returns `(velocity_px_per_s, duration_ms, offset)`.
    fn estimate(&self) -> Option<(Velocity, Ms, Point)> {
        let newest = *self.samples.back()?;
        let last = self.samples.len() - 1;
        if last > 0 && newest.0 - self.samples[last - 1].0 > Self::ASSUME_STOPPED_MS {
            // velocity_tracker.dart:180-188: motion considered stopped.
            return Some((Velocity::default(), 0, Point::new(0, 0)));
        }
        let mut used: Vec<(Ms, Point)> = Vec::new();
        let mut previous = newest;
        for &(time, point) in self.samples.iter().rev() {
            let age = newest.0 - time;
            let delta = (time - previous.0).abs();
            if age > Self::HORIZON_MS || delta > Self::ASSUME_STOPPED_MS {
                break; // velocity_tracker.dart:213-219
            }
            previous = (time, point);
            used.push((time, point));
            if used.len() == Self::HISTORY_SIZE {
                break;
            }
        }
        if used.len() < Self::MIN_SAMPLE_SIZE {
            // Fallback: raw delta between oldest and newest valid samples
            // (`velocity_tracker.dart:248-264`).
            let oldest = used.first().copied().unwrap_or(newest);
            let duration = newest.0 - oldest.0;
            if duration <= 0 {
                return Some((Velocity::default(), duration, Point::new(0, 0)));
            }
            let scale = 1000.0 / duration as f64;
            return Some((
                Velocity {
                    x: ((newest.1.x - oldest.1.x) as f64 * scale) as Px,
                    y: ((newest.1.y - oldest.1.y) as f64 * scale) as Px,
                },
                duration,
                Point::new(newest.1.x - oldest.1.x, newest.1.y - oldest.1.y),
            ));
        }
        let oldest = *used.last().unwrap();
        let time: Vec<f64> = used.iter().map(|&(t, _)| -(newest.0 - t) as f64).collect(); // velocity_tracker.dart:226
        let xs: Vec<f64> = used.iter().map(|&(_, p)| p.x as f64).collect();
        let ys: Vec<f64> = used.iter().map(|&(_, p)| p.y as f64).collect();
        let x_fit = least_squares_quadratic(&time, &xs)?;
        let y_fit = least_squares_quadratic(&time, &ys)?;
        Some((
            Velocity {
                x: (x_fit[1] * 1000.0) as Px, // velocity_tracker.dart:239-240
                y: (y_fit[1] * 1000.0) as Px,
            },
            newest.0 - oldest.0,
            Point::new(newest.1.x - oldest.1.x, newest.1.y - oldest.1.y),
        ))
    }

    /// Fling gate from `monodrag.dart:942-960,1006-1020`: the tracker's
    /// estimate offset must exceed hit slop and velocity must exceed 50 px/s;
    /// velocity is clipped at 8000 px/s.
    fn fling(&self) -> (Velocity, bool) {
        let Some((velocity, _, offset)) = self.estimate() else {
            return (Velocity::default(), false);
        };
        let slop = self.kind.hit_slop();
        let fling = (velocity.x.abs() >= MIN_FLING_VELOCITY && offset.x.abs() >= slop)
            || (velocity.y.abs() >= MIN_FLING_VELOCITY && offset.y.abs() >= slop);
        (
            Velocity {
                x: velocity.x.clamp(-MAX_FLING_VELOCITY, MAX_FLING_VELOCITY),
                y: velocity.y.clamp(-MAX_FLING_VELOCITY, MAX_FLING_VELOCITY),
            },
            fling,
        )
    }
}

/// The pure kernel: one arena per pointer id, no wall clock, no host calls.
#[derive(Debug, Default)]
pub struct GestureArenaKernel {
    arenas: HashMap<u64, PointerArena>,
    /// The recognizer-owned first tap of a potential double-tap pair
    /// (`multitap.dart:202-204,328-359`). Recognizers, not arenas, own this
    /// state in Flutter; one pending pair per recognizer node is enough for
    /// the current one-member-per-node IR model.
    first_tap: Option<FirstTap>,
}

impl GestureArenaKernel {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_deadline(&self) -> Option<Ms> {
        self.arenas
            .values()
            .filter_map(|a| a.next_deadline())
            .chain(self.first_tap.map(|t| t.expiry))
            .min()
    }

    /// `true` while a host timer is needed (recognizer deadline or a delayed
    /// sweep after double-tap hold; `arena.dart:163-167`).
    pub fn has_pending(&self) -> bool {
        self.next_deadline().is_some() || self.arenas.values().any(|a| a.pending_sweep)
    }

    /// Feeds one ordered event and returns recognizer callbacks in deterministic
    /// order. Pointer events at the same timestamp as a tick must be fed first.
    pub fn step(&mut self, event: ArenaEvent) -> ArenaStep {
        let mut events = Vec::new();
        match event {
            ArenaEvent::Down {
                pointer,
                at,
                time_ms,
                kind,
                buttons,
                path,
            } => {
                // Resolve a previous double-tap first tap: a new down that is
                // not a valid second touch starts a fresh sequence
                // (`multitap.dart:214-242`). A valid second touch is decided
                // on the *up* (`multitap.dart:257-265`).
                if let Some(first) = self.first_tap {
                    let valid_second = time_ms - first.up_ms >= DOUBLE_TAP_MIN_MS
                        && time_ms - first.up_ms <= DOUBLE_TAP_TIMEOUT_MS
                        && at.squared_distance_to(first.at) <= (DOUBLE_TAP_SLOP as u128).pow(2)
                        && path.iter().any(|e| {
                            e.gesture == Some(GestureKind::DoubleTap) && e.node == first.node
                        });
                    if !valid_second {
                        self.first_tap = None;
                        if let Some(old) = self.arenas.get_mut(&first.pointer) {
                            old.held = false;
                            old.release(&mut events); // multitap.dart:322-325
                        }
                    }
                }
                let mut arena = PointerArena::new(kind);
                // Flutter opens one arena per pointer and adds each hit-path
                // target in leaf→root order (`arena.dart:118-129`,
                // `binding.dart:506-508,539-541`).
                for entry in path {
                    if let Some(gesture) = entry.gesture {
                        arena
                            .members
                            .push(Member::new(entry.node, gesture, at, time_ms, buttons));
                    }
                }
                arena.tracker.add(time_ms, at);
                arena.close(&mut events);
                self.arenas.insert(pointer, arena);
            }
            ArenaEvent::Move {
                pointer,
                at,
                time_ms,
                buttons,
            } => {
                let Some(arena) = self.arenas.get_mut(&pointer) else {
                    return ArenaStep::empty();
                };
                let changed = arena.members.first().is_some_and(|m| m.buttons != buttons);
                if changed {
                    // Buttons changed mid-sequence → reject all
                    // (`tap.dart:321-323`, `monodrag.dart:668-670`).
                    let nodes: Vec<u32> = arena.members.iter().map(|m| m.node).collect();
                    for node in nodes {
                        arena.reject(node, &mut events, CancelReason::ButtonsChanged);
                    }
                } else {
                    self.on_move(pointer, at, time_ms, &mut events);
                }
            }
            ArenaEvent::Up {
                pointer,
                at,
                time_ms,
            } => {
                if !self.arenas.contains_key(&pointer) {
                    return ArenaStep::empty();
                }
                self.arenas
                    .get_mut(&pointer)
                    .unwrap()
                    .tracker
                    .add(time_ms, at);
                self.on_up(pointer, at, time_ms, &mut events);
                if let Some(arena) = self.arenas.get_mut(&pointer) {
                    arena.sweep(&mut events); // binding.dart:541-542
                }
            }
            ArenaEvent::Cancel {
                pointer,
                time_ms: _,
            } => {
                let Some(mut arena) = self.arenas.remove(&pointer) else {
                    return ArenaStep::empty();
                };
                for member in &arena.members {
                    match member.kind {
                        GestureKind::Tap | GestureKind::LongPress => {
                            events.push(if member.started {
                                RecognizerEvent::Cancel {
                                    node: member.node,
                                    reason: CancelReason::PointerCancel,
                                }
                            } else {
                                RecognizerEvent::TapCancel {
                                    node: member.node,
                                    reason: CancelReason::PointerCancel,
                                }
                            });
                        }
                        GestureKind::Drag | GestureKind::DoubleTap => {}
                    }
                }
                arena.members.clear();
                if self.first_tap.is_some_and(|t| t.pointer == pointer) {
                    self.first_tap = None; // multitap.dart:269-271
                }
            }
            ArenaEvent::Tick { now_ms } => {
                // Evaluate each due deadline at its own timestamp, never at
                // the late host `now_ms` (member order, then deadline order).
                let pointers: Vec<u64> = self.arenas.keys().copied().collect();
                for pointer in pointers {
                    loop {
                        let due = self.arenas.get(&pointer).and_then(|a| {
                            a.members
                                .iter()
                                .filter(|m| {
                                    m.state == MemberState::Possible
                                        && m.deadline.is_some_and(|d| d <= now_ms)
                                })
                                .map(|m| (m.deadline.unwrap(), m.node))
                                .min()
                        });
                        match due {
                            Some((deadline, node)) => {
                                self.on_deadline(pointer, node, deadline, &mut events)
                            }
                            None => break,
                        }
                    }
                }
                if let Some(first) = self.first_tap {
                    if first.expiry <= now_ms {
                        // Dart's Timer callback re-enters the event loop;
                        // take the arena out so pointer-up bookkeeping cannot
                        // re-clear the newly resolved winner.
                        self.first_tap = None; // multitap.dart:357-359
                        let mut held = self.arenas.remove(&first.pointer);
                        if let Some(arena) = held.as_mut() {
                            // The arena was held at pointer-up, so a sweep
                            // is pending (`arena.dart:163-167`). Expiry calls
                            // `release`, which performs that deferred sweep
                            // (`arena.dart:211-221`, `multitap.dart:322-325`).
                            arena.held = false;
                            let first = arena.members.first().map(|m| m.node);
                            if let Some(node) = first {
                                arena.resolve_in_favor_of(node, &mut events);
                            }
                            arena.pending_sweep = false;
                        }
                        if let Some(arena) = held {
                            self.arenas.insert(first.pointer, arena);
                        }
                    }
                }
            }
        }
        self.cleanup();
        ArenaStep {
            events,
            next_deadline_ms: self.next_deadline(),
        }
    }

    fn on_move(&mut self, pointer: u64, at: Point, time_ms: Ms, events: &mut Vec<RecognizerEvent>) {
        let Some(arena) = self.arenas.get_mut(&pointer) else {
            return;
        };
        arena.tracker.add(time_ms, at);
        let slop = arena.kind.hit_slop();
        let nodes: Vec<u32> = arena.members.iter().map(|m| m.node).collect();
        for node in nodes {
            let Some(member) = arena.member(node).cloned() else {
                continue;
            };
            if member.state != MemberState::Possible && member.state != MemberState::Won {
                continue;
            }
            let distance = at.squared_distance_to(member.down);
            match member.kind {
                GestureKind::Tap | GestureKind::LongPress | GestureKind::DoubleTap => {
                    // distance > hit slop rejects (`recognizer.dart:720-742`,
                    // `multitap.dart:265-268`).
                    if distance > (slop as u128).pow(2) {
                        arena.reject(node, events, CancelReason::ExceededSlop);
                        // A sole survivor wins by the arena default path
                        // (`arena.dart:257-259,268-279`).
                        if arena.members.len() == 1 {
                            let survivor = arena.members[0].node;
                            if arena.members[0].kind == GestureKind::Drag {
                                let down = arena.members[0].down;
                                arena.members[0].state = MemberState::Won;
                                arena.members[0].deadline = None;
                                events.push(RecognizerEvent::DragStart {
                                    node: survivor,
                                    at: down,
                                });
                            }
                        }
                    }
                }
                GestureKind::Drag => {
                    if distance > (slop as u128).pow(2) && member.state == MemberState::Possible {
                        // `hasSufficientGlobalDistanceToAccept` → resolve
                        // accepted (`monodrag.dart:690-711,965-970`). Drag
                        // rejects every sibling and promotes itself; a sole
                        // survivor wins via the arena's default.
                        let down = member.down;
                        let others: Vec<u32> = arena
                            .members
                            .iter()
                            .filter(|m| m.node != node)
                            .map(|m| m.node)
                            .collect();
                        for other in others {
                            arena.reject(other, events, CancelReason::ExceededSlop);
                        }
                        if let Some(winner) = arena.member_mut(node) {
                            winner.state = MemberState::Won;
                            winner.deadline = None;
                        }
                        events.push(RecognizerEvent::DragStart { node, at: down });
                    }
                    if arena
                        .member(node)
                        .is_some_and(|m| m.state == MemberState::Won)
                    {
                        let velocity = arena.tracker.estimate().map(|v| v.0).unwrap_or_default();
                        events.push(RecognizerEvent::DragUpdate {
                            node,
                            dx: at.x - member.last.x,
                            dy: at.y - member.last.y,
                            velocity,
                        });
                    }
                }
            }
            if let Some(member) = arena.member_mut(node) {
                member.last = at;
            }
        }
    }

    fn on_up(&mut self, pointer: u64, at: Point, time_ms: Ms, events: &mut Vec<RecognizerEvent>) {
        let nodes: Vec<(u32, GestureKind)> = self
            .arenas
            .get(&pointer)
            .map(|a| a.members.iter().map(|m| (m.node, m.kind)).collect())
            .unwrap_or_default();
        for (node, kind) in nodes {
            let Some(arena) = self.arenas.get_mut(&pointer) else {
                break;
            };
            let Some(member) = arena.member_mut(node) else {
                continue;
            };
            member.last = at;
            member.deadline = None;
            match kind {
                GestureKind::Tap => arena.accept(node, events),
                GestureKind::DoubleTap => {
                    match self.first_tap {
                        Some(first) if first.node == node => {
                            // Second up: both arena entries resolve accepted and
                            // the callback fires (`multitap.dart:339-345`).
                            events.push(RecognizerEvent::DoubleTap { node, at });
                            self.first_tap = None;
                            arena.held = false;
                            let others: Vec<u32> = arena
                                .members
                                .iter()
                                .filter(|m| m.node != node)
                                .map(|m| m.node)
                                .collect();
                            for other in others {
                                arena.reject(other, events, CancelReason::ArenaLost);
                            }
                            if let Some(member) = arena.member_mut(node) {
                                member.state = MemberState::Won;
                            }
                        }
                        Some(_) => {
                            // A different node won this up; old first tap expires.
                            self.first_tap = None;
                            arena.held = false;
                        }
                        None => {
                            // First up starts the timer and holds the arena
                            // (`multitap.dart:328-337,357-359`).
                            self.first_tap = Some(FirstTap {
                                pointer,
                                node,
                                at,
                                up_ms: time_ms,
                                expiry: time_ms + DOUBLE_TAP_TIMEOUT_MS,
                            });
                            arena.held = true;
                        }
                    }
                }
                GestureKind::LongPress => {
                    if arena.member(node).is_some_and(|m| m.started) {
                        events.push(RecognizerEvent::LongPressEnd { node, at });
                    }
                }
                GestureKind::Drag => {
                    if arena
                        .member(node)
                        .is_some_and(|m| m.state == MemberState::Won)
                    {
                        let (velocity, fling) = arena.tracker.fling();
                        events.push(RecognizerEvent::DragEnd {
                            node,
                            velocity,
                            fling,
                        });
                    }
                }
            }
        }
    }

    fn on_deadline(
        &mut self,
        pointer: u64,
        node: u32,
        deadline: Ms,
        events: &mut Vec<RecognizerEvent>,
    ) {
        let Some(arena) = self.arenas.get_mut(&pointer) else {
            return;
        };
        let Some(member) = arena.member(node).cloned() else {
            return;
        };
        if member.deadline != Some(deadline) || member.state != MemberState::Possible {
            return;
        }
        match member.kind {
            GestureKind::Tap => {
                // `didExceedDeadline` → `_checkDown` (`tap.dart:342-345`).
                if let Some(member) = arena.member_mut(node) {
                    member.started = true;
                    member.deadline = None;
                }
                events.push(RecognizerEvent::TapDown {
                    node,
                    at: member.down,
                });
            }
            GestureKind::LongPress => {
                // `didExceedDeadline` → resolve(accepted) → start
                // (`long_press.dart:640-647`).
                if let Some(member) = arena.member_mut(node) {
                    member.deadline = None;
                }
                arena.accept(node, events);
                if let Some(member) = arena.member(node) {
                    if !member.started {
                        events.push(RecognizerEvent::LongPressStart {
                            node,
                            at: member.down,
                        });
                    }
                }
                if let Some(member) = arena.member_mut(node) {
                    member.started = true;
                }
            }
            _ => {}
        }
    }

    fn cleanup(&mut self) {
        self.arenas
            .retain(|_, arena| !arena.members.is_empty() || arena.pending_sweep || arena.held);
    }
}
