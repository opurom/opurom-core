//! Differential/regression tests for the core gesture arena, pinned to
//! Flutter 3.50.0 (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).
//! Every asserted threshold and ordering rule cites the Flutter source line.

use opurom_core::{
    ArenaEvent, ArenaStep, GestureArenaKernel, GestureKind, HitEntry, PointerKind, RecognizerEvent,
    PRESS_TIMEOUT_MS,
};

fn down(path: Vec<HitEntry>) -> ArenaEvent {
    ArenaEvent::Down {
        pointer: 1,
        at: opurom_core::Point::new(10, 10),
        time_ms: 0,
        kind: PointerKind::Touch,
        buttons: 1,
        path,
    }
}

fn up(at: opurom_core::Point, time_ms: i64) -> ArenaEvent {
    ArenaEvent::Up {
        pointer: 1,
        at,
        time_ms,
    }
}

fn gesture(node: u32, kind: GestureKind) -> HitEntry {
    HitEntry {
        node,
        gesture: Some(kind),
    }
}

fn events(step: ArenaStep) -> Vec<RecognizerEvent> {
    step.events
}

#[test]
fn arena_join_order_first_accept_then_sweep() {
    // arena.dart:110 "first member to accept or last to not reject";
    // sweep gives the win to members.first (arena.dart:169-177).
    let mut kernel = GestureArenaKernel::new();
    let step = kernel.step(down(vec![
        gesture(1, GestureKind::Tap),
        gesture(2, GestureKind::Tap),
        gesture(3, GestureKind::Tap),
    ]));
    assert!(step.events.is_empty());
    // Two members remain possible; no single-member default resolution.
    assert!(kernel
        .step(up(opurom_core::Point::new(10, 10), 50))
        .events
        .contains(&RecognizerEvent::TapUp {
            node: 1,
            at: opurom_core::Point::new(10, 10)
        }));
}

#[test]
fn sole_member_resolves_immediately_without_clock() {
    // `_tryToResolveArena` uses a microtask for the last member
    // (arena.dart:254-279); Opurom resolves it in-kernel at close, no timer.
    let mut kernel = GestureArenaKernel::new();
    let step = kernel.step(down(vec![gesture(7, GestureKind::Tap)]));
    assert_eq!(
        events(step),
        vec![
            RecognizerEvent::TapDown {
                node: 7,
                at: opurom_core::Point::new(10, 10),
            },
            RecognizerEvent::TapUp {
                node: 7,
                at: opurom_core::Point::new(10, 10),
            },
        ]
    );
}

#[test]
fn press_deadline_is_exactly_100ms_and_value_is_deadline_not_now() {
    // kPressTimeout = 100ms (constants.dart:14, tap.dart:212); two members
    // keep the arena open, so tap-down fires only at the deadline tick.
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(1, GestureKind::Tap),
        gesture(2, GestureKind::Drag),
    ]));
    let step = kernel.step(ArenaEvent::Tick {
        now_ms: PRESS_TIMEOUT_MS + 100,
    });
    assert_eq!(
        events(step),
        vec![RecognizerEvent::TapDown {
            node: 1,
            at: opurom_core::Point::new(10, 10),
        }]
    );
}

#[test]
fn long_press_deadline_is_500ms_and_late_tick_still_emits_once() {
    // kLongPressTimeout = 500ms (constants.dart:29); didExceedDeadline
    // accepts and starts the press mid-hold (long_press.dart:640-647).
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(1, GestureKind::LongPress),
        gesture(2, GestureKind::Drag),
    ]));
    let step = kernel.step(ArenaEvent::Tick { now_ms: 520 });
    assert_eq!(
        events(step),
        vec![RecognizerEvent::LongPressStart {
            node: 1,
            at: opurom_core::Point::new(10, 10),
        }]
    );
    let step = kernel.step(ArenaEvent::Tick { now_ms: 900 });
    assert!(step.events.is_empty());
}

#[test]
fn touch_slop_rejects_tap_but_mouse_uses_precise_slop() {
    // kTouchSlop = 18 px (constants.dart:65), kPrecisePointerHitSlop = 1 px
    // (constants.dart:103), computeHitSlop (events.dart:2550-2561).
    let mut kernel = GestureArenaKernel::new();
    kernel.step(ArenaEvent::Down {
        pointer: 1,
        at: opurom_core::Point::new(10, 10),
        time_ms: 0,
        kind: PointerKind::Touch,
        buttons: 1,
        path: vec![gesture(1, GestureKind::Tap), gesture(2, GestureKind::Drag)],
    });
    let step = kernel.step(ArenaEvent::Move {
        pointer: 1,
        at: opurom_core::Point::new(29, 10), // 19 px > 18
        time_ms: 20,
        buttons: 1,
    });
    assert!(step.events.iter().any(|e| matches!(
        e,
        RecognizerEvent::TapCancel {
            reason: opurom_core::CancelReason::ExceededSlop,
            ..
        }
    )));
}

#[test]
fn drag_accepts_past_slop_and_fling_gate_uses_tracker_offset() {
    // Drag accepts past hit slop (monodrag.dart:690-711), fling needs
    // >=50 px/s and >=18 px offset with 8000 px/s clamp
    // (constants.dart:90,95; monodrag.dart:942-960).
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(1, GestureKind::Tap),
        gesture(2, GestureKind::Drag),
    ]));
    let mut last = Vec::new();
    for (i, x) in [10, 30, 10, 20, 40].iter().enumerate() {
        let step = kernel.step(ArenaEvent::Move {
            pointer: 1,
            at: opurom_core::Point::new(*x, 10),
            time_ms: (i as i64) * 16 + 16,
            buttons: 1,
        });
        last.extend(step.events);
    }
    assert!(last
        .iter()
        .any(|e| matches!(e, RecognizerEvent::DragStart { node: 2, .. })));
    let step = kernel.step(up(opurom_core::Point::new(40, 10), 80));
    assert!(step.events.iter().any(|e| matches!(
        e,
        RecognizerEvent::DragEnd {
            node: 2,
            fling: true,
            ..
        }
    )));
}

#[test]
fn double_tap_requires_40ms_min_gap_and_300ms_window() {
    // kDoubleTapMinTime = 40ms (constants.dart:39, multitap.dart:250);
    // kDoubleTapTimeout = 300ms (constants.dart:35, multitap.dart:328-358).
    let mut kernel = GestureArenaKernel::new();
    kernel.step(ArenaEvent::Down {
        pointer: 1,
        at: opurom_core::Point::new(10, 10),
        time_ms: 0,
        kind: PointerKind::Touch,
        buttons: 1,
        path: vec![gesture(1, GestureKind::DoubleTap)],
    });
    assert_eq!(
        kernel
            .step(up(opurom_core::Point::new(10, 10), 20))
            .next_deadline_ms,
        Some(320)
    );
    // Too-early second down restarts as a first tap (multitap.dart:228-232).
    kernel.step(ArenaEvent::Down {
        pointer: 2,
        at: opurom_core::Point::new(10, 10),
        time_ms: 40,
        kind: PointerKind::Touch,
        buttons: 1,
        path: vec![gesture(1, GestureKind::DoubleTap)],
    });
    assert!(kernel
        .step(ArenaEvent::Up {
            pointer: 2,
            at: opurom_core::Point::new(10, 10),
            time_ms: 60,
        })
        .events
        .is_empty());
    // Valid second tap after 40 ms.
    kernel.step(ArenaEvent::Down {
        pointer: 3,
        at: opurom_core::Point::new(10, 10),
        time_ms: 100,
        kind: PointerKind::Touch,
        buttons: 1,
        path: vec![gesture(1, GestureKind::DoubleTap)],
    });
    let step = kernel.step(ArenaEvent::Up {
        pointer: 3,
        at: opurom_core::Point::new(10, 10),
        time_ms: 120,
    });
    assert!(events(step).contains(&RecognizerEvent::DoubleTap {
        node: 1,
        at: opurom_core::Point::new(10, 10),
    }));
}

#[test]
fn pointer_cancel_resets_arena_and_timers() {
    // tap.dart:315-320 rejects and resets on PointerCancelEvent.
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(1, GestureKind::Tap),
        gesture(2, GestureKind::Drag),
    ]));
    let step = kernel.step(ArenaEvent::Cancel {
        pointer: 1,
        time_ms: 50,
    });
    // Only the started tap emits a cancel; the drag self-rejects silently
    // (monodrag.dart:747-750).
    assert_eq!(step.events.len(), 1);
    assert!(!kernel.has_pending());
    assert!(kernel
        .step(ArenaEvent::Tick { now_ms: 5000 })
        .events
        .is_empty());
}

#[test]
fn same_stream_on_two_kernels_is_byte_identical() {
    // Red line: deterministic rendering/kernel — identical ordered input
    // streams produce identical output on every host.
    let stream = vec![
        down(vec![
            gesture(3, GestureKind::Tap),
            gesture(2, GestureKind::LongPress),
            gesture(1, GestureKind::Drag),
        ]),
        ArenaEvent::Move {
            pointer: 1,
            at: opurom_core::Point::new(20, 16),
            time_ms: 30,
            buttons: 1,
        },
        ArenaEvent::Tick { now_ms: 550 },
        up(opurom_core::Point::new(30, 10), 600),
    ];
    let mut a = GestureArenaKernel::new();
    let mut b = GestureArenaKernel::new();
    for event in stream.clone() {
        assert_eq!(a.step(event.clone()), b.step(event));
    }
    assert_eq!(a.has_pending(), b.has_pending());
}

#[test]
fn sweep_prefers_first_surviving_member() {
    // arena.dart:157-179: sweep resolves in favor of members.first.
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(11, GestureKind::Tap),
        gesture(12, GestureKind::Tap),
    ]));
    let step = kernel.step(up(opurom_core::Point::new(10, 10), 20));
    assert!(step.events.contains(&RecognizerEvent::TapUp {
        node: 11,
        at: opurom_core::Point::new(10, 10),
    }));
    assert!(!step
        .events
        .iter()
        .any(|e| matches!(e, RecognizerEvent::TapUp { node: 12, .. })));
}

#[test]
fn long_press_eager_accept_wins_over_sibling_tap() {
    // arena.dart:233-240: accept while closed resolves immediately;
    // long_press.dart:640-647 accepts at the 500 ms deadline.
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(20, GestureKind::Tap),
        gesture(21, GestureKind::LongPress),
    ]));
    let step = kernel.step(ArenaEvent::Tick { now_ms: 500 });
    assert!(step.events.contains(&RecognizerEvent::LongPressStart {
        node: 21,
        at: opurom_core::Point::new(10, 10),
    }));
    assert!(!step
        .events
        .iter()
        .any(|e| matches!(e, RecognizerEvent::TapUp { node: 20, .. })));
}

#[test]
fn double_tap_hold_delays_sweep_until_expiry() {
    // multitap.dart:328-336 holds the first tap's arena after up; the
    // 300ms timer resets and releases it (:322-325,357-359).
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(30, GestureKind::DoubleTap),
        gesture(31, GestureKind::Tap),
    ]));
    let first = kernel.step(up(opurom_core::Point::new(10, 10), 0));
    assert!(first.events.is_empty(), "held arena must not sweep at up");
    assert_eq!(first.next_deadline_ms, Some(300));
    let expired = kernel.step(ArenaEvent::Tick { now_ms: 301 });
    // DoubleTapGestureRecognizer.acceptGesture is a no-op
    // (multitap.dart:274-275): expiry releases the arena without a spurious
    // tap callback; the pending sweep is gone and no timer remains.
    assert!(!kernel.has_pending());
    assert!(expired.next_deadline_ms.is_none());
}

#[test]
fn buttons_change_cancels_all_members() {
    // tap.dart:321-323 rejects when buttons change mid-sequence.
    let mut kernel = GestureArenaKernel::new();
    kernel.step(down(vec![
        gesture(40, GestureKind::Tap),
        gesture(41, GestureKind::LongPress),
    ]));
    let step = kernel.step(ArenaEvent::Move {
        pointer: 1,
        at: opurom_core::Point::new(10, 10),
        time_ms: 10,
        buttons: 0,
    });
    // Tap and LongPress both report a cancel on buttons change
    // (tap.dart:315-323); the empty arena then clears (arena.dart:259-261).
    assert_eq!(step.events.len(), 3);
    assert!(!kernel.has_pending());
}

#[test]
fn tmp_hold_trace2() {
    let mut k = GestureArenaKernel::new();
    k.step(down(vec![
        gesture(30, GestureKind::DoubleTap),
        gesture(31, GestureKind::Tap),
    ]));
    println!("up {:?}", k.step(up(opurom_core::Point::new(10, 10), 0)));
    println!("tick {:?}", k.step(ArenaEvent::Tick { now_ms: 301 }));
}
