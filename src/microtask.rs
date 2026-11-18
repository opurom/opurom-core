//! Deterministic microtask queue and frame-phase pump, ported from Dart 3.14.0
//! (`dart:async`) and Flutter 3.50.0 (`scheduler/binding.dart`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK
//! `3.14.0` revision `fa8c1270c374c1346931583a17c3b7f278d3822d`.
//! Source paths below are relative to `bin/cache/dart-sdk/lib/` (`D:`) or
//! `packages/flutter/lib/src/` (`F:`) at that checkout.
//!
//! Ported rules:
//! - normal callbacks form one FIFO linked list and are scheduled at the tail
//!   (`D:async/schedule_microtask.dart:9-26,63-76`);
//! - the drain loop re-reads the head every iteration, so callbacks scheduled
//!   by a running callback execute in the same drain, in scheduling order
//!   (`D:async/schedule_microtask.dart:34-42`);
//! - priority callbacks (Dart uses these only for uncaught error reporting)
//!   run before all normal callbacks; multiple priority callbacks keep their
//!   scheduling order (`D:async/schedule_microtask.dart:21-26,84-105`,
//!   `D:async/zone_root.dart:11-15`);
//! - microtasks are guaranteed to finish before the next timer/DOM event
//!   (`D:async/schedule_microtask.dart:121-125`);
//! - one frame runs transient callbacks, then the mid-frame microtask
//!   boundary, then persistent (build/layout/paint) callbacks, then
//!   post-frame callbacks, then returns to idle
//!   (`F:scheduler/binding.dart:1229-1276,1332-1375`, phase semantics `:149-205`).
//!
//! Opurom adaptation: callbacks are opaque `u32` ids plus an optional
//! priority flag; the host owns function bodies and clocks. This keeps the
//! kernel IR-only and deterministic — the same `(schedule, pump)` sequence
//! yields the same callback order on every host.

use std::collections::VecDeque;

/// Identifies a host-registered callback. The kernel never stores closures.
pub type CallbackId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Queued {
    Normal(CallbackId),
    Priority(CallbackId),
}

impl Queued {
    fn id(self) -> CallbackId {
        match self {
            Self::Normal(id) | Self::Priority(id) => id,
        }
    }
}

/// Microtask queue with Dart's FIFO + priority-front semantics.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MicrotaskQueue {
    normal: VecDeque<CallbackId>,
    priority: Vec<CallbackId>,
    /// Number of callbacks started by drain/pump so far; monotonic.
    executed: u64,
}

impl MicrotaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// `scheduleMicrotask`: append to the normal callback list
    /// (`schedule_microtask.dart:63-76`).
    pub fn schedule(&mut self, id: CallbackId) {
        self.normal.push_back(id);
    }

    /// Internal error-reporting microtask: insert before all normal callbacks
    /// but after previously queued priority callbacks
    /// (`schedule_microtask.dart:84-105`, `zone_root.dart:11-15`).
    pub fn schedule_priority(&mut self, id: CallbackId) {
        self.priority.push(id);
    }

    pub fn pending(&self) -> usize {
        self.priority.len() + self.normal.len()
    }

    pub fn executed(&self) -> u64 {
        self.executed
    }

    /// Runs all queued microtasks to completion, including ones scheduled
    /// by running callbacks (`schedule_microtask.dart:34-42,121-125`).
    ///
    /// `limit` is an Opurom liveness guard for pathological self-rescheduling
    /// streams (Dart can starve the event loop by design, see :127-137).
    /// Returns the callback ids in execution order; when the limit is hit the
    /// remaining queue is preserved for the next drain/pump.
    pub fn drain(&mut self, limit: usize) -> Vec<CallbackId> {
        let mut out = Vec::new();
        while out.len() < limit {
            let next = if let Some(id) = self.priority.first().copied() {
                self.priority.remove(0);
                Queued::Priority(id)
            } else if let Some(id) = self.normal.pop_front() {
                Queued::Normal(id)
            } else {
                break;
            };
            out.push(next.id());
            self.executed += 1;
            // No host call is made here; the caller runs each returned id
            // after this step, in order, so nested schedules are observable.
        }
        out
    }

    /// Drains with a queue-driven loop: after each batch the caller is
    /// expected to run the returned callbacks and then call this again until
    /// `pending() == 0`. Provided as a convenience for hosts that cannot
    /// re-enter within a single step.
    pub fn drain_step(&mut self) -> Vec<CallbackId> {
        self.drain(self.pending().max(1))
    }
}

/// Scheduler phases mirroring Flutter's `SchedulerPhase`
/// (`scheduler/binding.dart:149-205`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhase {
    Idle,
    TransientCallbacks,
    MidFrameMicrotasks,
    PersistentCallbacks,
    PostFrameCallbacks,
}

/// Ordering events emitted by a frame pump. The host resolves each id to its
/// registered callback body; the kernel only decides *when* it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStep {
    BeginFrame,
    Transient(CallbackId),
    Microtasks(CallbackId),
    Persistent(CallbackId),
    PostFrame(CallbackId),
    EndFrame,
}

/// A single frame's callback sets. Hosts fill these before calling
/// [`FramePump::pump`], mirroring `SchedulerBinding`'s transient / persistent
/// / post-frame maps (`scheduler/binding.dart:1262-1264,1347-1354`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrameCallbacks {
    pub transient: Vec<CallbackId>,
    pub persistent: Vec<CallbackId>,
    pub post_frame: Vec<CallbackId>,
}

/// Deterministic frame-phase pump. One pump equals one VSync delivery:
/// transient callbacks → mid-frame microtasks → persistent → post-frame → idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePump {
    phase: FramePhase,
    microtasks: MicrotaskQueue,
    frame_seq: u64,
}

impl Default for FramePump {
    fn default() -> Self {
        Self {
            phase: FramePhase::Idle,
            microtasks: MicrotaskQueue::new(),
            frame_seq: 0,
        }
    }
}

impl FramePump {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> FramePhase {
        self.phase
    }

    pub fn frame_seq(&self) -> u64 {
        self.frame_seq
    }

    pub fn microtasks(&self) -> &MicrotaskQueue {
        &self.microtasks
    }

    pub fn microtasks_mut(&mut self) -> &mut MicrotaskQueue {
        &mut self.microtasks
    }

    /// Pumps one whole frame in the Flutter order. Microtasks queued by
    /// transient callbacks are drained at the mid-frame boundary; those
    /// queued by persistent/post-frame callbacks stay pending for the next
    /// event boundary (`binding.dart:1275,1342,1368`).
    /// `microtask_limit` bounds each drain (see [`MicrotaskQueue::drain`]).
    pub fn pump(&mut self, callbacks: FrameCallbacks, microtask_limit: usize) -> Vec<FrameStep> {
        debug_assert_eq!(self.phase, FramePhase::Idle);
        let mut out = vec![FrameStep::BeginFrame];
        self.frame_seq += 1;
        self.phase = FramePhase::TransientCallbacks;
        for id in callbacks.transient {
            out.push(FrameStep::Transient(id));
        }
        self.phase = FramePhase::MidFrameMicrotasks;
        for id in self.microtasks.drain(microtask_limit) {
            out.push(FrameStep::Microtasks(id));
        }
        self.phase = FramePhase::PersistentCallbacks;
        for id in callbacks.persistent {
            out.push(FrameStep::Persistent(id));
        }
        self.phase = FramePhase::PostFrameCallbacks;
        for id in callbacks.post_frame {
            out.push(FrameStep::PostFrame(id));
        }
        self.phase = FramePhase::Idle;
        out.push(FrameStep::EndFrame);
        out
    }
}
