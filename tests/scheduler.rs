use opurom_core::{AsyncScheduler, SchedulingHook, SchedulingTask, TaskId};
use std::sync::{Arc, Mutex};

#[test]
fn now_starts_at_zero_and_only_tick_advances_it() {
    let mut sched = AsyncScheduler::new();
    assert_eq!(sched.now(), 0);
    let a = sched.spawn(1, || {});
    let b = sched.spawn(1, || {});
    assert!(sched.sleep_until(b, 3));
    assert!(sched.cancel(a));
    assert!(sched.drain().is_empty());
    assert_eq!(sched.now(), 0);
    assert_eq!(sched.tick(3), vec![b]);
    assert_eq!(sched.now(), 3);
    assert!(sched.tick(0).is_empty());
    assert_eq!(sched.now(), 3);
}

#[test]
fn higher_priority_tasks_run_first() {
    let mut sched = AsyncScheduler::new();
    let low = sched.spawn(0, || {});
    let high = sched.spawn(10, || {});
    let mid = sched.spawn(5, || {});
    assert_eq!(sched.tick(0), vec![high, mid, low]);
    assert_eq!(sched.pending(), 0);
}

#[test]
fn equal_priority_tasks_run_in_spawn_order() {
    let mut sched = AsyncScheduler::new();
    let first = sched.spawn(3, || {});
    let second = sched.spawn(3, || {});
    let third = sched.spawn(3, || {});
    assert_eq!(sched.tick(1), vec![first, second, third]);
}

#[test]
fn sleeping_task_is_not_ready_before_its_target_tick() {
    let mut sched = AsyncScheduler::new();
    let id = sched.spawn(1, || {});
    assert!(sched.sleep_until(id, 5));
    assert!(sched.tick(4).is_empty());
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.now(), 4);
}

#[test]
fn sleeping_task_becomes_ready_exactly_at_its_target_tick() {
    let mut sched = AsyncScheduler::new();
    let id = sched.spawn(1, || {});
    assert!(sched.sleep_until(id, 5));
    assert_eq!(sched.tick(5), vec![id]);
    assert_eq!(sched.pending(), 0);
    assert_eq!(sched.now(), 5);
}

#[test]
fn cancel_removes_a_pending_task() {
    let mut sched = AsyncScheduler::new();
    let cancelled = sched.spawn(1, || {});
    let kept = sched.spawn(1, || {});
    assert!(sched.cancel(cancelled));
    assert_eq!(sched.tick(0), vec![kept]);
    assert!(!sched.cancel(cancelled));
    assert_eq!(sched.pending(), 0);
}

#[test]
fn cancel_removes_a_sleeping_task() {
    let mut sched = AsyncScheduler::new();
    let id = sched.spawn(1, || {});
    assert!(sched.sleep_until(id, 9));
    assert!(sched.cancel(id));
    assert!(sched.tick(100).is_empty());
    assert_eq!(sched.pending(), 0);
    assert!(!sched.cancel(999));
}

#[test]
fn drain_returns_remaining_ready_tasks_without_waking_sleepers() {
    let mut sched = AsyncScheduler::new();
    let low = sched.spawn(2, || {});
    let sleepy = sched.spawn(1, || {});
    let high = sched.spawn(5, || {});
    assert!(sched.sleep_until(sleepy, 10));
    assert_eq!(sched.drain(), vec![high, low]);
    assert_eq!(sched.now(), 0);
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.tick(10), vec![sleepy]);
}

#[test]
fn two_schedulers_with_the_same_sequence_produce_the_same_ready_order() {
    fn scripted() -> (Vec<Vec<TaskId>>, Vec<&'static str>) {
        let mut sched = AsyncScheduler::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let _ = sched.spawn_with_context(5, {
            let log = Arc::clone(&log);
            move |mut ctx| {
                log.lock().unwrap().push("a");
                ctx.spawn(7, {
                    let log = Arc::clone(&log);
                    move || log.lock().unwrap().push("e")
                });
            }
        });
        let b = sched.spawn(5, {
            let log = Arc::clone(&log);
            move || log.lock().unwrap().push("b")
        });
        let _ = sched.spawn(9, {
            let log = Arc::clone(&log);
            move || log.lock().unwrap().push("c")
        });
        assert!(sched.sleep_until(b, 2));
        let first = sched.tick(2);
        let second = sched.tick(1);
        let f = sched.spawn(1, || {});
        assert!(sched.cancel(f));
        let third = sched.tick(3);
        let events = log.lock().unwrap().clone();
        (vec![first, second, third], events)
    }

    let (ticks_a, log_a) = scripted();
    let (ticks_b, log_b) = scripted();
    assert_eq!(ticks_a, ticks_b);
    assert_eq!(log_a, log_b);
    assert_eq!(ticks_a, vec![vec![3, 1, 2], vec![4], vec![]]);
    assert_eq!(log_a, vec!["c", "a", "b", "e"]);
}

#[test]
fn pending_counts_ready_and_sleeping_tasks() {
    let mut sched = AsyncScheduler::new();
    assert_eq!(sched.pending(), 0);
    let a = sched.spawn(1, || {});
    let b = sched.spawn(1, || {});
    let c = sched.spawn(1, || {});
    assert_eq!(sched.pending(), 3);
    assert!(sched.sleep_until(c, 4));
    assert_eq!(sched.pending(), 3);
    assert!(sched.cancel(b));
    assert_eq!(sched.pending(), 2);
    assert_eq!(sched.tick(0), vec![a]);
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.tick(4), vec![c]);
    assert_eq!(sched.pending(), 0);
}

#[test]
fn tasks_spawn_children_and_continuations_through_the_context() {
    let mut sched = AsyncScheduler::new();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let parent = sched.spawn_with_context(1, {
        let hits = Arc::clone(&hits);
        move |mut ctx| {
            hits.lock().unwrap().push(format!("parent {}", ctx.id()));
            ctx.spawn_at(1, 3, {
                let hits = Arc::clone(&hits);
                move |ctx| {
                    hits.lock().unwrap().push(format!(
                        "continuation {} at {}",
                        ctx.id(),
                        ctx.now()
                    ));
                }
            });
            ctx.spawn(2, {
                let hits = Arc::clone(&hits);
                move || hits.lock().unwrap().push("child".to_owned())
            });
        }
    });
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.tick(0), vec![parent]);
    assert_eq!(sched.pending(), 2);
    assert_eq!(sched.tick(1), vec![3]);
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.tick(2), vec![2]);
    assert_eq!(sched.pending(), 0);
    assert_eq!(
        hits.lock().unwrap().clone(),
        vec![
            "parent 1".to_owned(),
            "child".to_owned(),
            "continuation 2 at 3".to_owned(),
        ]
    );
}

#[test]
fn woken_tasks_compete_with_already_ready_tasks_by_priority() {
    let mut sched = AsyncScheduler::new();
    let sleepy_low = sched.spawn(1, || {});
    let ready_high = sched.spawn(8, || {});
    let sleepy_mid = sched.spawn(4, || {});
    assert!(sched.sleep_until(sleepy_low, 5));
    assert!(sched.sleep_until(sleepy_mid, 5));
    assert_eq!(sched.tick(5), vec![ready_high, sleepy_mid, sleepy_low]);
}

#[test]
fn sleep_until_retargets_sleeping_tasks_and_rejects_unknown_ids() {
    let mut sched = AsyncScheduler::new();
    let id = sched.spawn(1, || {});
    assert!(!sched.sleep_until(999, 5));
    assert!(sched.sleep_until(id, 10));
    assert!(sched.sleep_until(id, 3));
    assert_eq!(sched.tick(3), vec![id]);
    let ran = sched.spawn(1, || {});
    assert_eq!(sched.tick(0), vec![ran]);
    assert!(!sched.sleep_until(ran, 7));
}

#[test]
fn task_ids_are_unique_and_increase_in_spawn_order() {
    let mut sched = AsyncScheduler::new();
    let ids: Vec<TaskId> = (0..5).map(|_| sched.spawn(0, || {})).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    assert_eq!(sched.pending(), 5);
}

#[test]
fn wake_at_is_an_alias_for_sleep_until() {
    let mut sched = AsyncScheduler::new();
    let id = sched.spawn(1, || {});
    assert!(sched.wake_at(id, 2));
    assert_eq!(sched.tick(2), vec![id]);
}

#[test]
fn sleep_until_defers_a_ready_task() {
    let mut sched = AsyncScheduler::new();
    let deferred = sched.spawn(1, || {});
    let ready = sched.spawn(1, || {});
    assert!(sched.sleep_until(deferred, 2));
    assert_eq!(sched.tick(1), vec![ready]);
    assert_eq!(sched.tick(1), vec![deferred]);
}

#[test]
fn drain_does_not_run_children_spawned_during_the_drain() {
    let mut sched = AsyncScheduler::new();
    let parent = sched.spawn_with_context(1, |mut ctx| {
        ctx.spawn(1, || {});
    });
    assert_eq!(sched.drain(), vec![parent]);
    assert_eq!(sched.pending(), 1);
    assert_eq!(sched.drain(), vec![parent + 1]);
    assert_eq!(sched.pending(), 0);
}

// Scheduling-hook fixtures: every verdict is a pure function of the task
// data, so the hook results stay deterministic.

struct SeniorityHook;

impl SchedulingHook for SeniorityHook {
    fn prioritize(&self, task: &SchedulingTask) -> i64 {
        i64::from(task.priority) + task.id as i64
    }
}

struct NoBackgroundHook;

impl SchedulingHook for NoBackgroundHook {
    fn admit(&self, task: &SchedulingTask) -> bool {
        task.priority > 0
    }
}

struct PassthroughHook;

impl SchedulingHook for PassthroughHook {}

struct ObservationHook {
    seen: Arc<Mutex<Vec<(TaskId, u8)>>>,
}

impl SchedulingHook for ObservationHook {
    fn prioritize(&self, task: &SchedulingTask) -> i64 {
        self.seen.lock().unwrap().push((task.id, task.priority));
        i64::from(task.priority)
    }
}

struct GateHook;

impl SchedulingHook for GateHook {
    fn prioritize(&self, task: &SchedulingTask) -> i64 {
        i64::from(task.priority) - task.id as i64
    }

    fn admit(&self, task: &SchedulingTask) -> bool {
        task.id != 4
    }
}

#[test]
fn hook_reorders_ready_tasks_deterministically() {
    let mut sched = AsyncScheduler::new().with_hook(Arc::new(SeniorityHook));
    let a = sched.spawn(5, || {});
    let b = sched.spawn(5, || {});
    let c = sched.spawn(5, || {});
    // Without the hook the tie would keep spawn order; the hook flips it.
    assert_eq!(sched.tick(0), vec![c, b, a]);
    let low = sched.spawn(0, || {});
    let high = sched.spawn(9, || {});
    assert_eq!(sched.tick(0), vec![high, low]);
    assert_eq!(sched.executed(), 5);
}

#[test]
fn hook_can_reject_a_task_at_spawn() {
    let mut sched = AsyncScheduler::new().with_hook(Arc::new(NoBackgroundHook));
    let rejected = sched.spawn(0, || {});
    let kept = sched.spawn(1, || {});
    assert_eq!(sched.tick(0), vec![kept]);
    assert_eq!(sched.pending(), 0);
    assert!(!sched.cancel(rejected));
    assert_eq!(sched.admitted(), 1);
    assert_eq!(sched.rejected(), 1);
    assert_eq!(sched.executed(), 1);
}

#[test]
fn scheduler_without_hook_keeps_the_builtin_order() {
    let mut sched = AsyncScheduler::new();
    let background_a = sched.spawn(0, || {});
    let high = sched.spawn(7, || {});
    let background_b = sched.spawn(0, || {});
    assert_eq!(sched.tick(0), vec![high, background_a, background_b]);
    assert_eq!(sched.admitted(), 3);
    assert_eq!(sched.rejected(), 0);
    assert_eq!(sched.executed(), 3);
}

#[test]
fn hook_with_default_methods_matches_the_builtin_order() {
    let mut sched = AsyncScheduler::new().with_hook(Arc::new(PassthroughHook));
    let background_a = sched.spawn(0, || {});
    let high = sched.spawn(7, || {});
    let background_b = sched.spawn(0, || {});
    assert_eq!(sched.tick(0), vec![high, background_a, background_b]);
    assert_eq!(sched.admitted(), 3);
    assert_eq!(sched.rejected(), 0);
    assert_eq!(sched.executed(), 3);
}

#[test]
fn hook_observes_each_ready_task_once_per_batch() {
    let seen: Arc<Mutex<Vec<(TaskId, u8)>>> = Arc::new(Mutex::new(Vec::new()));
    let hook: Arc<dyn SchedulingHook> = Arc::new(ObservationHook {
        seen: Arc::clone(&seen),
    });
    let mut sched = AsyncScheduler::new().with_hook(hook);
    let a = sched.spawn(2, || {});
    let b = sched.spawn(9, || {});
    assert_eq!(sched.tick(0), vec![b, a]);
    assert_eq!(seen.lock().unwrap().clone(), vec![(a, 2), (b, 9)]);
    assert!(sched.tick(0).is_empty());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn same_spawn_sequence_and_hook_produce_the_same_order_twice() {
    fn scripted() -> (Vec<Vec<TaskId>>, (u64, u64, u64)) {
        let mut sched = AsyncScheduler::new().with_hook(Arc::new(GateHook));
        let _one = sched.spawn(3, || {});
        let _two = sched.spawn(3, || {});
        let _three = sched.spawn(2, || {});
        let four = sched.spawn(1, || {});
        let _five = sched.spawn(4, || {});
        assert!(!sched.cancel(four)); // id 4 was rejected, so it is unknown
        let first = sched.tick(0);
        let _six = sched.spawn(3, || {});
        let _seven = sched.spawn(3, || {});
        let second = sched.drain();
        let sleepy = sched.spawn(5, || {});
        assert!(sched.sleep_until(sleepy, 10));
        let third = sched.tick(10);
        (
            vec![first, second, third],
            (sched.admitted(), sched.rejected(), sched.executed()),
        )
    }

    let (ticks_a, counters_a) = scripted();
    let (ticks_b, counters_b) = scripted();
    assert_eq!(ticks_a, ticks_b);
    assert_eq!(counters_a, counters_b);
    assert_eq!(ticks_a, vec![vec![1, 2, 3, 5], vec![6, 7], vec![8]]);
    assert_eq!(counters_a, (7, 1, 7));
}
