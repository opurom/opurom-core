use crate::error::SchedulerError;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle as ThreadHandle};

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Queued {
    priority: u8,
    seq: u64,
    job: Job,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

struct Inner {
    heap: BinaryHeap<Queued>,
    shutdown: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    ready: Condvar,
}

pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Mutex<Vec<ThreadHandle<()>>>,
    seq: AtomicU64,
}

pub struct TaskHandle<T> {
    receiver: mpsc::Receiver<T>,
}

impl<T> TaskHandle<T> {
    pub fn join(self) -> Result<T, SchedulerError> {
        self.receiver.recv().map_err(|_| SchedulerError::TaskFailed)
    }
}

impl Scheduler {
    /// Bounded worker pool. Higher `priority` runs first; equal priorities are FIFO.
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                heap: BinaryHeap::new(),
                shutdown: false,
            }),
            ready: Condvar::new(),
        });
        let mut handles = Vec::with_capacity(workers.max(1));
        for _ in 0..workers.max(1) {
            let shared = Arc::clone(&shared);
            handles.push(thread::spawn(move || worker_loop(shared)));
        }
        Scheduler {
            shared,
            workers: Mutex::new(handles),
            seq: AtomicU64::new(0),
        }
    }

    pub fn spawn<T, F>(&self, priority: u8, task: F) -> Result<TaskHandle<T>, SchedulerError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel::<T>(1);
        let job: Job = Box::new(move || {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let _ = sender.send(task());
            }));
        });
        let seq = self.seq.fetch_add(1, AtomicOrdering::Relaxed);
        {
            let mut inner = lock(&self.shared.inner);
            if inner.shutdown {
                return Err(SchedulerError::ShuttingDown);
            }
            inner.heap.push(Queued { priority, seq, job });
        }
        self.shared.ready.notify_one();
        Ok(TaskHandle { receiver })
    }

    /// Stop accepting tasks, drain queued ones, then join every worker. Idempotent.
    pub fn shutdown(&self) {
        {
            let mut inner = lock(&self.shared.inner);
            inner.shutdown = true;
        }
        self.shared.ready.notify_all();
        let handles: Vec<ThreadHandle<()>> = lock(&self.workers).drain(..).collect();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn worker_loop(shared: Arc<Shared>) {
    loop {
        let queued = {
            let mut inner = lock(&shared.inner);
            loop {
                if let Some(queued) = inner.heap.pop() {
                    break Some(queued);
                }
                if inner.shutdown {
                    break None;
                }
                inner = shared
                    .ready
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        match queued {
            Some(queued) => (queued.job)(),
            None => break,
        }
    }
}
