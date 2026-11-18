//! Regression tests for the Dart 3.14 microtask queue and the Flutter 3.50
//! frame-phase pump, pinned to `flutter-golden` HEAD
//! ab2912035e160d9cad6306df307576da6128ebdb (Dart 3.14.0).

use opurom_core::{FrameCallbacks, FramePhase, FramePump, FrameStep, MicrotaskQueue};

#[test]
fn normal_microtasks_are_fifo() {
    // schedule_microtask.dart:63-76 appends to one linked list.
    let mut q = MicrotaskQueue::new();
    q.schedule(1);
    q.schedule(2);
    q.schedule(3);
    assert_eq!(q.drain(10), vec![1, 2, 3]);
    assert_eq!(q.pending(), 0);
    assert_eq!(q.executed(), 3);
}

#[test]
fn priority_microtasks_run_first_and_keep_scheduling_order() {
    // schedule_microtask.dart:84-105: error-reporting priority callbacks are
    // inserted at the head, behind earlier priority callbacks.
    let mut q = MicrotaskQueue::new();
    q.schedule(1);
    q.schedule_priority(9);
    q.schedule_priority(10);
    q.schedule(2);
    assert_eq!(q.drain(10), vec![9, 10, 1, 2]);
}

#[test]
fn drain_survives_nested_scheduling() {
    // schedule_microtask.dart:34-42 re-reads the head each iteration, so a
    // callback scheduled during a drain runs in that same drain.
    let mut q = MicrotaskQueue::new();
    q.schedule(1);
    let mut out = q.drain(10);
    q.schedule(2); // callback 1 scheduled 2
    out.extend(q.drain(10));
    assert_eq!(out, vec![1, 2]);
    assert_eq!(q.executed(), 2);
}

#[test]
fn drain_limit_preserves_remaining_queue() {
    // Opurom liveness guard; Dart can starve events by design (:127-137).
    let mut q = MicrotaskQueue::new();
    q.schedule(1);
    q.schedule(2);
    q.schedule(3);
    assert_eq!(q.drain(2), vec![1, 2]);
    assert_eq!(q.pending(), 1);
    assert_eq!(q.drain(2), vec![3]);
}

#[test]
fn pump_follows_transient_microtask_persistent_post_frame_order() {
    // binding.dart:1229-1276 (transient → midFrameMicrotasks) and
    // :1332-1375 (persistent → postFrame → idle).
    let mut pump = FramePump::new();
    pump.microtasks_mut().schedule(50);
    let steps = pump.pump(
        FrameCallbacks {
            transient: vec![10, 11],
            persistent: vec![20],
            post_frame: vec![30],
        },
        16,
    );
    assert_eq!(
        steps,
        vec![
            FrameStep::BeginFrame,
            FrameStep::Transient(10),
            FrameStep::Transient(11),
            FrameStep::Microtasks(50),
            FrameStep::Persistent(20),
            FrameStep::PostFrame(30),
            FrameStep::EndFrame,
        ]
    );
    assert_eq!(pump.phase(), FramePhase::Idle);
    assert_eq!(pump.frame_seq(), 1);
}

#[test]
fn microtasks_scheduled_after_boundary_wait_for_next_drain() {
    // binding.dart:1342 asserts midFrameMicrotasks on draw-frame entry; only
    // the boundary before persistent callbacks drains microtasks.
    let mut pump = FramePump::new();
    pump.microtasks_mut().schedule(1);
    let first = pump.pump(FrameCallbacks::default(), 16);
    assert_eq!(
        first,
        vec![
            FrameStep::BeginFrame,
            FrameStep::Microtasks(1),
            FrameStep::EndFrame
        ]
    );
    pump.microtasks_mut().schedule(2);
    assert_eq!(pump.microtasks().pending(), 1);
}

#[test]
fn frames_are_deterministic_across_pumps() {
    let make_frame = || FrameCallbacks {
        transient: vec![7, 8],
        persistent: vec![9],
        post_frame: vec![10],
    };
    let mut a = FramePump::new();
    let mut b = FramePump::new();
    for _ in 0..3 {
        a.microtasks_mut().schedule(42);
        b.microtasks_mut().schedule(42);
        assert_eq!(a.pump(make_frame(), 8), b.pump(make_frame(), 8));
    }
    assert_eq!(a.frame_seq(), b.frame_seq());
}
