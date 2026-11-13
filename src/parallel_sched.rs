//! Multi-threaded work-stealing scheduler.
//!
//! Every worker owns a deque. Submissions are distributed round robin; a worker
//! takes work from its own deque and, when empty, steals from the front of
//! another worker's deque. A single worker always pops from the front, so its
//! execution order is deterministic FIFO. The total number of queued tasks is
//! bounded. One mutex protects all deques: this is not a lock-free scheduler.
//!
//! This is not OS async I/O, GPU scheduling, priority preemption or
//! cross-process scheduling.

use crate::error::{panic_message, SchedulerError};
use std::collections::VecDeque;
use std::fmt;
use std::hint::black_box;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_QUEUE_CAPACITY: usize = 256;
const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
const DEFAULT_COOLDOWN: u64 = 1_000;

/// Logical time source for the circuit breaker's cooldown.
pub trait Clock: Send + Sync {
    fn now(&self) -> u64;
}

/// Default clock: milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Injectable deterministic clock; clones share the same counter.
#[derive(Debug, Clone, Default)]
pub struct CounterClock {
    ticks: Arc<AtomicU64>,
}

impl CounterClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at(tick: u64) -> Self {
        Self {
            ticks: Arc::new(AtomicU64::new(tick)),
        }
    }

    pub fn advance(&self, ticks: u64) -> u64 {
        self.ticks
            .fetch_add(ticks, AtomicOrdering::SeqCst)
            .wrapping_add(ticks)
    }

    pub fn now(&self) -> u64 {
        self.ticks.load(AtomicOrdering::SeqCst)
    }
}

impl Clock for CounterClock {
    fn now(&self) -> u64 {
        self.ticks.load(AtomicOrdering::SeqCst)
    }
}

type Job = Box<dyn FnOnce(usize) + Send + 'static>;

struct State {
    queues: Vec<VecDeque<Job>>,
    pending: usize,
    next_worker: usize,
    shutdown: bool,
    capacity: usize,
    failure_threshold: u32,
    cooldown: u64,
    failures: u32,
    opened_at: Option<u64>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    clock: Mutex<Arc<dyn Clock>>,
    workers: usize,
    steals: AtomicUsize,
}

/// A bounded, multi-threaded work-stealing scheduler.
pub struct WorkStealingScheduler {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

/// Result handle returned by [`WorkStealingScheduler::spawn`].
pub struct WorkStealingTaskHandle<T> {
    receiver: mpsc::Receiver<Result<T, SchedulerError>>,
}

impl<T> fmt::Debug for WorkStealingTaskHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkStealingTaskHandle")
    }
}

impl<T> WorkStealingTaskHandle<T> {
    pub fn join(self) -> Result<T, SchedulerError> {
        self.receiver
            .recv()
            .unwrap_or(Err(SchedulerError::TaskFailed))
    }
}

/// Real timings from a 1-worker and an N-worker run of the same CPU-bound work.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedupMeasurement {
    pub workers: usize,
    pub tasks: usize,
    pub iterations: u64,
    pub one_worker: Duration,
    pub many_workers: Duration,
    pub ratio: f64,
    pub one_results: Vec<u64>,
    pub many_results: Vec<u64>,
}

impl WorkStealingScheduler {
    /// Starts `workers` OS threads; zero is clamped to one deterministic worker.
    pub fn with_workers(workers: usize) -> Self {
        let workers = workers.max(1);
        let queues = (0..workers).map(|_| VecDeque::new()).collect();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queues,
                pending: 0,
                next_worker: 0,
                shutdown: false,
                capacity: DEFAULT_QUEUE_CAPACITY,
                failure_threshold: DEFAULT_FAILURE_THRESHOLD,
                cooldown: DEFAULT_COOLDOWN,
                failures: 0,
                opened_at: None,
            }),
            ready: Condvar::new(),
            clock: Mutex::new(clock),
            workers,
            steals: AtomicUsize::new(0),
        });
        let mut handles = Vec::with_capacity(workers);
        for id in 0..workers {
            let shared = Arc::clone(&shared);
            handles.push(thread::spawn(move || worker_loop(shared, id)));
        }
        Self {
            shared,
            workers: Mutex::new(handles),
        }
    }

    /// Maximum number of queued (not running) tasks across all worker deques.
    pub fn with_queue_capacity(self, capacity: usize) -> Self {
        lock(&self.shared.state).capacity = capacity.max(1);
        self
    }

    /// Consecutive task panics that open the circuit breaker.
    pub fn with_failure_threshold(self, threshold: u32) -> Self {
        lock(&self.shared.state).failure_threshold = threshold.max(1);
        self
    }

    /// Cooldown in clock ticks; with the default clock, one tick is one ms.
    pub fn with_cooldown(self, cooldown: u64) -> Self {
        lock(&self.shared.state).cooldown = cooldown;
        self
    }

    /// Replaces the clock used for cooldown, enabling deterministic tests.
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Self {
        *lock(&self.shared.clock) = clock;
        self
    }

    pub fn worker_count(&self) -> usize {
        self.shared.workers
    }

    pub fn queue_capacity(&self) -> usize {
        lock(&self.shared.state).capacity
    }

    pub fn pending(&self) -> usize {
        lock(&self.shared.state).pending
    }

    pub fn consecutive_failures(&self) -> u32 {
        lock(&self.shared.state).failures
    }

    pub fn circuit_open(&self) -> bool {
        lock(&self.shared.state).opened_at.is_some()
    }

    pub fn steals(&self) -> usize {
        self.shared.steals.load(AtomicOrdering::SeqCst)
    }

    /// Queues a task round robin and returns its result handle.
    pub fn spawn<T, F>(&self, task: F) -> Result<WorkStealingTaskHandle<T>, SchedulerError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.enqueue(None, move |_| task())
    }

    /// Queues a task on one worker's deque; the closure sees the executing worker id.
    pub fn spawn_on<T, F>(
        &self,
        worker: usize,
        task: F,
    ) -> Result<WorkStealingTaskHandle<T>, SchedulerError>
    where
        T: Send + 'static,
        F: FnOnce(usize) -> T + Send + 'static,
    {
        self.enqueue(Some(worker), task)
    }

    /// Stop accepting tasks, drain queued work, then join every worker. Idempotent.
    pub fn shutdown(&self) {
        {
            let mut state = lock(&self.shared.state);
            state.shutdown = true;
        }
        self.shared.ready.notify_all();
        let handles: Vec<JoinHandle<()>> = lock(&self.workers).drain(..).collect();
        for handle in handles {
            let _ = handle.join();
        }
    }

    fn enqueue<T, F>(
        &self,
        owner: Option<usize>,
        task: F,
    ) -> Result<WorkStealingTaskHandle<T>, SchedulerError>
    where
        T: Send + 'static,
        F: FnOnce(usize) -> T + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel::<Result<T, SchedulerError>>(1);
        let shared = Arc::clone(&self.shared);
        let job: Job = Box::new(move |worker| {
            let outcome = match catch_unwind(AssertUnwindSafe(move || task(worker))) {
                Ok(value) => Ok(value),
                Err(panic) => Err(SchedulerError::TaskPanicked(panic_message(panic))),
            };
            shared.record(outcome.is_ok());
            let _ = sender.send(outcome);
        });

        let mut state = lock(&self.shared.state);
        if state.shutdown {
            return Err(SchedulerError::ShuttingDown);
        }
        if let Some(opened_at) = state.opened_at {
            let now = now(&self.shared);
            let retry_at = opened_at.saturating_add(state.cooldown);
            if now >= retry_at {
                state.opened_at = None;
                state.failures = 0;
            } else {
                return Err(SchedulerError::CircuitOpen {
                    retry_after: retry_at.saturating_sub(now),
                });
            }
        }
        if state.pending >= state.capacity {
            return Err(SchedulerError::QueueFull {
                capacity: state.capacity,
            });
        }
        let worker = match owner {
            Some(worker) => worker % self.shared.workers,
            None => {
                let worker = state.next_worker;
                state.next_worker = (worker + 1) % self.shared.workers;
                worker
            }
        };
        state.queues[worker].push_back(job);
        state.pending += 1;
        drop(state);
        self.shared.ready.notify_all();
        Ok(WorkStealingTaskHandle { receiver })
    }
}

impl Drop for WorkStealingScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Shared {
    fn record(&self, succeeded: bool) {
        let mut state = lock(&self.state);
        if succeeded {
            state.failures = 0;
            return;
        }
        if state.opened_at.is_some() {
            return;
        }
        state.failures = state.failures.saturating_add(1);
        if state.failures >= state.failure_threshold {
            state.opened_at = Some(now(self));
        }
    }
}

fn worker_loop(shared: Arc<Shared>, worker: usize) {
    loop {
        let job = {
            let mut state = lock(&shared.state);
            loop {
                if let Some(job) = take_job(&mut state, worker, &shared) {
                    break Some(job);
                }
                if state.shutdown {
                    break None;
                }
                state = shared
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        match job {
            Some(job) => job(worker),
            None => break,
        }
    }
}

fn take_job(state: &mut State, worker: usize, shared: &Shared) -> Option<Job> {
    let own = if shared.workers == 1 {
        state.queues[worker].pop_front()
    } else {
        state.queues[worker].pop_back()
    };
    if let Some(job) = own {
        state.pending -= 1;
        return Some(job);
    }
    for offset in 1..shared.workers {
        let victim = (worker + offset) % shared.workers;
        if let Some(job) = state.queues[victim].pop_front() {
            state.pending -= 1;
            shared.steals.fetch_add(1, AtomicOrdering::SeqCst);
            return Some(job);
        }
    }
    None
}

fn now(shared: &Shared) -> u64 {
    lock(&shared.clock).now()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Deterministic integer workload; `black_box` prevents the loop from being removed.
#[inline(never)]
pub fn cpu_bound_workload(seed: u64, iterations: u64) -> u64 {
    let mut value = seed ^ 0x9e37_79b9_7f4a_7c15;
    for i in 0..iterations {
        value ^= value.wrapping_shl(13) ^ i.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 30;
    }
    black_box(value)
}

/// Runs the same CPU-bound workload with one worker and with `workers` workers.
/// The ratio is computed only from the two measured `Instant` durations.
pub fn measure_speedup(
    workers: usize,
    tasks: usize,
    iterations: u64,
) -> Result<SpeedupMeasurement, SchedulerError> {
    let workers = workers.max(2);
    let tasks = tasks.max(1);
    let (one_results, one_worker) = run_once(1, tasks, iterations)?;
    let (many_results, many_workers) = run_once(workers, tasks, iterations)?;
    let many_seconds = many_workers.as_secs_f64();
    let ratio = if many_seconds > 0.0 {
        one_worker.as_secs_f64() / many_seconds
    } else {
        f64::INFINITY
    };
    Ok(SpeedupMeasurement {
        workers,
        tasks,
        iterations,
        one_worker,
        many_workers,
        ratio,
        one_results,
        many_results,
    })
}

fn run_once(
    workers: usize,
    tasks: usize,
    iterations: u64,
) -> Result<(Vec<u64>, Duration), SchedulerError> {
    let scheduler = WorkStealingScheduler::with_workers(workers).with_queue_capacity(tasks);
    let started = Instant::now();
    let handles = (0..tasks)
        .map(|task| {
            let seed = task as u64;
            scheduler.spawn(move || cpu_bound_workload(seed, iterations))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(handle.join()?);
    }
    scheduler.shutdown();
    Ok((results, started.elapsed()))
}
