//! Deterministic cooperative scheduler: a tick-driven coroutine queue.
//!
//! `AsyncScheduler` is single-threaded and driven by a logical tick counter
//! that starts at 0 and advances only through [`AsyncScheduler::tick`]. There
//! is no wall clock, no thread, no randomness and no I/O, so the same
//! spawn/tick sequence on two schedulers produces the same ready order every
//! time, and `tick` is pure with respect to external state.
//!
//! This is a schedulable **coroutine queue**, NOT OS async I/O: no reactor,
//! no epoll/kqueue, no sockets, no OS timers, and no claim about network
//! concurrency. Real async I/O is not implemented here.
//!
//! A registered [`SchedulingHook`] is the scheduler's extension point: it may
//! reorder ready tasks and veto admission. Without a hook, or with a hook
//! that keeps the default methods, the queue keeps its built-in order.

use std::cmp::Reverse;
use std::sync::Arc;

/// Spawn-order identity of a task; the first spawned task gets id 1.
pub type TaskId = u64;

/// The task data a [`SchedulingHook`] sees: plain values with no borrow ties
/// to the scheduler.
#[derive(Clone, Copy, Debug)]
pub struct SchedulingTask {
    /// Spawn-order id assigned by the scheduler.
    pub id: TaskId,
    /// Priority passed to [`AsyncScheduler::spawn`].
    pub priority: u8,
}

/// Scheduling extension point for [`AsyncScheduler`].
///
/// Both methods are pure functions of the task data, so the same spawn
/// sequence produces the same order no matter how often the scheduler runs.
/// The defaults leave the built-in behavior untouched, so a hook implements
/// only the influence it needs.
pub trait SchedulingHook: Send + Sync {
    /// Effective priority of one ready task: higher values run first and ties
    /// keep spawn order. The default returns the spawned priority unchanged.
    fn prioritize(&self, task: &SchedulingTask) -> i64 {
        i64::from(task.priority)
    }

    /// Whether a task may be queued at all; `false` drops the task at spawn
    /// time and counts one rejection. The default admits every task.
    fn admit(&self, _task: &SchedulingTask) -> bool {
        true
    }
}

type Task = Box<dyn for<'a> FnOnce(TaskContext<'a>) + 'static>;

struct Entry {
    id: TaskId,
    priority: u8,
    /// `None` = ready now; `Some(tick)` = sleeping until that tick.
    wake: Option<u64>,
    task: Option<Task>,
}

/// Deterministic cooperative scheduler over a logical tick counter.
///
/// [`AsyncScheduler::spawn`] takes a plain one-shot `FnOnce()`. Use
/// [`AsyncScheduler::spawn_with_context`] when a task needs to spawn children
/// or a continuation; a "rescheduled" task is a freshly spawned continuation,
/// because a one-shot closure cannot be run twice.
pub struct AsyncScheduler {
    entries: Vec<Entry>,
    next_id: TaskId,
    now: u64,
    hook: Option<Arc<dyn SchedulingHook>>,
    admitted: u64,
    rejected: u64,
    executed: u64,
}

/// Scheduler handle passed to tasks started with `spawn_with_context`.
///
/// Exposes the task's own [`TaskContext::id`], the scheduler clock, and child
/// spawning via [`TaskContext::spawn`] / [`TaskContext::spawn_at`]
/// (`spawn_at` is the ergonomic way to "reschedule": spawn a continuation at
/// a future tick). It deliberately does not expose `drain`, `cancel` or
/// `tick`: a running task must not re-enter the run loop or move the clock
/// in the middle of a batch.
pub struct TaskContext<'sched> {
    scheduler: &'sched mut AsyncScheduler,
    task_id: TaskId,
}

impl AsyncScheduler {
    /// Empty scheduler with the logical clock at 0.
    pub fn new() -> Self {
        AsyncScheduler {
            entries: Vec::new(),
            next_id: 1,
            now: 0,
            hook: None,
            admitted: 0,
            rejected: 0,
            executed: 0,
        }
    }

    /// Registers the scheduling extension. `admit` is consulted once per
    /// spawn and `prioritize` once per ready batch; register before the first
    /// spawn so admission covers every task.
    pub fn with_hook(mut self, hook: Arc<dyn SchedulingHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// Queues a plain one-shot task to run at the next `tick`/`drain`. Higher
    /// `priority` runs first; equal priorities keep spawn order. Returns the
    /// task id.
    pub fn spawn(&mut self, priority: u8, task: impl FnOnce() + 'static) -> TaskId {
        self.spawn_entry(priority, move |_| task())
    }

    /// Queues a task that receives a [`TaskContext`], so it can spawn children
    /// and continuations through that context.
    pub fn spawn_with_context<F>(&mut self, priority: u8, task: F) -> TaskId
    where
        F: for<'a> FnOnce(TaskContext<'a>) + 'static,
    {
        self.spawn_entry(priority, task)
    }

    /// Defers a pending task to `tick`. A target at or before `now()` makes
    /// the task ready immediately; retargeting a sleeping task updates its
    /// tick. Returns `false` for unknown ids (already run or cancelled).
    pub fn sleep_until(&mut self, id: TaskId, tick: u64) -> bool {
        self.sleep_task(id, tick)
    }

    /// Alias for [`AsyncScheduler::sleep_until`].
    pub fn wake_at(&mut self, id: TaskId, tick: u64) -> bool {
        self.sleep_task(id, tick)
    }

    /// Advances the logical clock by `delta_ms`, wakes every task whose
    /// target has arrived, runs the whole ready batch (freshly woken tasks
    /// and anything already queued) ordered by higher priority first and
    /// spawn order second, and returns the ids that ran. Tasks spawned
    /// during the batch join the queue for a later tick.
    pub fn tick(&mut self, delta_ms: u64) -> Vec<TaskId> {
        self.now = self.now.saturating_add(delta_ms);
        for entry in &mut self.entries {
            if let Some(wake) = entry.wake {
                if wake <= self.now {
                    entry.wake = None;
                }
            }
        }
        self.run_ready()
    }

    /// Runs the ready batch without advancing the clock or waking sleepers;
    /// returns the ids that ran in execution order.
    pub fn drain(&mut self) -> Vec<TaskId> {
        self.run_ready()
    }

    /// Removes a pending or sleeping task. Returns `false` if the id is
    /// unknown (already run or already cancelled).
    pub fn cancel(&mut self, id: TaskId) -> bool {
        let Some(pos) = self.entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        self.entries.remove(pos);
        true
    }

    /// Number of tasks that have neither run nor been cancelled (ready and
    /// sleeping alike).
    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    /// Logical time; starts at 0 and advances only in `tick`.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Spawn attempts the scheduler accepted: total spawns minus rejections,
    /// and exactly total spawns when no hook is registered.
    pub fn admitted(&self) -> u64 {
        self.admitted
    }

    /// Spawn attempts the registered hook rejected; always 0 without a hook.
    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Tasks started by `tick` and `drain` so far.
    pub fn executed(&self) -> u64 {
        self.executed
    }

    fn spawn_entry<F>(&mut self, priority: u8, task: F) -> TaskId
    where
        F: for<'a> FnOnce(TaskContext<'a>) + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        let candidate = SchedulingTask { id, priority };
        if let Some(hook) = &self.hook {
            if !hook.admit(&candidate) {
                self.rejected += 1;
                // The id is still consumed so ids stay unique in spawn order.
                return id;
            }
        }
        self.admitted += 1;
        self.entries.push(Entry {
            id,
            priority,
            wake: None,
            task: Some(Box::new(task)),
        });
        id
    }

    fn sleep_task(&mut self, id: TaskId, tick: u64) -> bool {
        for entry in &mut self.entries {
            if entry.id == id {
                entry.wake = if tick <= self.now { None } else { Some(tick) };
                return true;
            }
        }
        false
    }

    fn run_ready(&mut self) -> Vec<TaskId> {
        // The hook verdict is fetched once per task before sorting, so the
        // comparator never calls back into user code.
        let mut batch: Vec<(usize, i64)> = (0..self.entries.len())
            .filter(|&i| self.entries[i].wake.is_none())
            .map(|i| (i, self.effective_priority(i)))
            .collect();
        // (effective priority desc, spawn order asc) is a total order, so the
        // result never depends on the Vec's internal layout.
        batch.sort_by_key(|&(i, key)| (Reverse(key), self.entries[i].id));
        let mut ids = Vec::with_capacity(batch.len());
        for (i, _) in batch {
            let id = self.entries[i].id;
            let task = self.entries[i].task.take();
            ids.push(id);
            self.executed += 1;
            if let Some(task) = task {
                task(TaskContext {
                    scheduler: self,
                    task_id: id,
                });
            }
        }
        self.entries.retain(|entry| entry.task.is_some());
        ids
    }

    /// Ordering key for one entry: the hook's verdict when one is registered,
    /// otherwise the spawned priority. Both rank equal priorities the same
    /// way, so a hook that keeps its default leaves the order unchanged.
    fn effective_priority(&self, index: usize) -> i64 {
        match &self.hook {
            Some(hook) => hook.prioritize(&SchedulingTask {
                id: self.entries[index].id,
                priority: self.entries[index].priority,
            }),
            None => i64::from(self.entries[index].priority),
        }
    }
}

impl<'sched> TaskContext<'sched> {
    /// Id of the running task.
    pub fn id(&self) -> TaskId {
        self.task_id
    }

    /// Current value of the scheduler's logical clock.
    pub fn now(&self) -> u64 {
        self.scheduler.now
    }

    /// Spawns a plain child task that runs at a later tick or drain.
    pub fn spawn(&mut self, priority: u8, task: impl FnOnce() + 'static) -> TaskId {
        self.scheduler.spawn(priority, task)
    }

    /// Spawns a child task that receives its own [`TaskContext`].
    pub fn spawn_with_context<F>(&mut self, priority: u8, task: F) -> TaskId
    where
        F: for<'a> FnOnce(TaskContext<'a>) + 'static,
    {
        self.scheduler.spawn_with_context(priority, task)
    }

    /// Spawns a continuation scheduled for `tick`: how a one-shot task
    /// "reschedules itself". A target at or before `now()` leaves the child
    /// simply queued for the next tick or drain.
    pub fn spawn_at<F>(&mut self, priority: u8, tick: u64, task: F) -> TaskId
    where
        F: for<'a> FnOnce(TaskContext<'a>) + 'static,
    {
        let id = self.scheduler.spawn_with_context(priority, task);
        self.scheduler.sleep_task(id, tick);
        id
    }
}
