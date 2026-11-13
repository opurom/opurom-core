use opurom_core::{
    cpu_bound_workload, measure_speedup, Clock, CounterClock, Scheduler, SchedulerError,
    SchedulerMetrics, WorkStealingTaskHandle,
};
use std::fmt::Debug;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::available_parallelism;
use std::time::Duration;

fn typed_panic_message<T: Debug>(handle: WorkStealingTaskHandle<T>) -> String {
    match handle.join() {
        Err(SchedulerError::TaskPanicked(message)) => message,
        other => panic!("expected a typed panic error, got {other:?}"),
    }
}

#[test]
fn single_worker_scheduler_is_fifo() {
    let scheduler = Scheduler::with_workers(1).with_queue_capacity(16);
    assert_eq!(scheduler.worker_count(), 1);
    let order = Arc::new(Mutex::new(Vec::<i32>::new()));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate_order = Arc::clone(&order);
    let gate = scheduler
        .spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            gate_order.lock().unwrap().push(0);
        })
        .unwrap();
    started_rx.recv().unwrap();

    let handles = (1..=5i32)
        .map(|i| {
            let order = Arc::clone(&order);
            scheduler
                .spawn(move || {
                    order.lock().unwrap().push(i);
                    i
                })
                .unwrap()
        })
        .collect::<Vec<_>>();

    release_tx.send(()).unwrap();
    gate.join().unwrap();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![1, 2, 3, 4, 5]);
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    scheduler.shutdown();
}

#[test]
fn multi_worker_scheduler_results_are_complete() {
    let scheduler = Scheduler::with_workers(4).with_queue_capacity(64);
    let handles = (0..64u64)
        .map(|seed| {
            scheduler
                .spawn(move || cpu_bound_workload(seed, 2_000))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let mut results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    results.sort_unstable();
    let mut expected = (0..64u64)
        .map(|seed| cpu_bound_workload(seed, 2_000))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(results, expected);
    scheduler.shutdown();
}

#[test]
fn idle_worker_steals_from_busy_scheduler_queue() {
    let scheduler = Scheduler::with_workers(2).with_queue_capacity(16);
    let (started_tx, started_rx) = mpsc::channel::<(usize, usize)>();
    let (gate0_tx, gate0_rx) = mpsc::channel::<()>();
    let (gate1_tx, gate1_rx) = mpsc::channel::<()>();
    let gates = [gate0_tx, gate1_tx];

    let started0 = started_tx.clone();
    let waiter0 = scheduler
        .spawn_on(0, move |worker| {
            started0.send((0, worker)).unwrap();
            gate0_rx.recv().unwrap();
        })
        .unwrap();
    let waiter1 = scheduler
        .spawn_on(1, move |worker| {
            started_tx.send((1, worker)).unwrap();
            gate1_rx.recv().unwrap();
        })
        .unwrap();

    let mut started = [started_rx.recv().unwrap(), started_rx.recv().unwrap()];
    started.sort_by_key(|(task, _)| *task);
    let busy_worker = started[0].1;
    let released_worker = started[1].1;
    assert_ne!(busy_worker, released_worker);

    gates[1].send(()).unwrap();
    waiter1.join().unwrap();

    let observed = Arc::new(Mutex::new(Vec::<usize>::new()));
    let stolen = (0..4i32)
        .map(|i| {
            let observed = Arc::clone(&observed);
            scheduler
                .spawn_on(busy_worker, move |worker| {
                    observed.lock().unwrap().push(worker);
                    i * 3
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let results = stolen
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![0, 3, 6, 9]);
    let stealers = observed.lock().unwrap().clone();
    assert_eq!(stealers.len(), 4);
    assert!(stealers.iter().all(|&worker| worker == released_worker));
    assert!(scheduler.steals() >= 4);

    gates[0].send(()).unwrap();
    waiter0.join().unwrap();
    scheduler.shutdown();
}

#[test]
fn bounded_scheduler_queue_rejects_excess_tasks() {
    let scheduler = Scheduler::with_workers(1).with_queue_capacity(2);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = scheduler
        .spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            0
        })
        .unwrap();
    started_rx.recv().unwrap();

    let first = scheduler.spawn(|| 1).unwrap();
    let second = scheduler.spawn(|| 2).unwrap();
    match scheduler.spawn(|| 3) {
        Err(SchedulerError::QueueFull { capacity }) => assert_eq!(capacity, 2),
        other => panic!("expected QueueFull, got {other:?}"),
    }

    release_tx.send(()).unwrap();
    assert_eq!(gate.join().unwrap(), 0);
    assert_eq!(first.join().unwrap(), 1);
    assert_eq!(second.join().unwrap(), 2);
    scheduler.shutdown();
}

#[test]
fn panic_is_isolated_and_returns_typed_error() {
    let scheduler = Scheduler::with_workers(2).with_queue_capacity(8);
    let boom = scheduler
        .spawn(|| -> i32 { panic!("kernel task exploded") })
        .unwrap();
    let message = typed_panic_message(boom);
    assert!(message.contains("kernel task exploded"));
    assert_eq!(scheduler.consecutive_failures(), 1);
    assert!(!scheduler.circuit_open());

    let handles = (0..8i32)
        .map(|i| scheduler.spawn(move || i * i).unwrap())
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![0, 1, 4, 9, 16, 25, 36, 49]);
    assert_eq!(scheduler.consecutive_failures(), 0);
    scheduler.shutdown();
}

#[test]
fn circuit_breaker_trips_and_recovers_with_injected_clock() {
    let counter = CounterClock::at(10);
    let clock: Arc<dyn Clock> = Arc::new(counter.clone());
    let scheduler = Scheduler::with_workers(1)
        .with_queue_capacity(8)
        .with_failure_threshold(2)
        .with_cooldown(5)
        .with_clock(clock);

    for _ in 0..2 {
        let boom = scheduler
            .spawn(|| -> i32 { panic!("breaker failure") })
            .unwrap();
        assert!(typed_panic_message(boom).contains("breaker failure"));
    }
    assert!(scheduler.circuit_open());
    assert_eq!(scheduler.consecutive_failures(), 2);
    match scheduler.spawn(|| 1) {
        Err(SchedulerError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 5),
        other => panic!("expected CircuitOpen, got {other:?}"),
    }

    assert_eq!(counter.advance(4), 14);
    match scheduler.spawn(|| 1) {
        Err(SchedulerError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 1),
        other => panic!("expected CircuitOpen before cooldown, got {other:?}"),
    }

    assert_eq!(counter.advance(1), 15);
    let recovered = scheduler.spawn(|| 7).unwrap();
    assert_eq!(recovered.join().unwrap(), 7);
    assert!(!scheduler.circuit_open());
    assert_eq!(scheduler.consecutive_failures(), 0);
    scheduler.shutdown();
}

#[test]
fn circuit_breaker_resets_on_success() {
    let counter = CounterClock::new();
    let clock: Arc<dyn Clock> = Arc::new(counter.clone());
    let scheduler = Scheduler::with_workers(1)
        .with_failure_threshold(3)
        .with_cooldown(10)
        .with_clock(clock);

    let first = scheduler
        .spawn(|| -> i32 { panic!("first failure") })
        .unwrap();
    assert!(typed_panic_message(first).contains("first failure"));
    assert_eq!(scheduler.consecutive_failures(), 1);

    assert_eq!(scheduler.spawn(|| 11).unwrap().join().unwrap(), 11);
    assert_eq!(scheduler.consecutive_failures(), 0);

    let second = scheduler
        .spawn(|| -> i32 { panic!("second failure") })
        .unwrap();
    assert!(typed_panic_message(second).contains("second failure"));
    assert_eq!(scheduler.consecutive_failures(), 1);
    assert!(!scheduler.circuit_open());
    scheduler.shutdown();
}

#[test]
fn scheduler_shutdown_drains_and_joins_workers() {
    let scheduler = Scheduler::with_workers(1).with_queue_capacity(8);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = scheduler
        .spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
        .unwrap();
    started_rx.recv().unwrap();
    let handles = (0..4i32)
        .map(|i| scheduler.spawn(move || i * i).unwrap())
        .collect::<Vec<_>>();

    release_tx.send(()).unwrap();
    scheduler.shutdown();
    gate.join().unwrap();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![0, 1, 4, 9]);
    match scheduler.spawn(|| 0) {
        Err(SchedulerError::ShuttingDown) => {}
        other => panic!("expected ShuttingDown, got {other:?}"),
    }
    scheduler.shutdown();
}

#[test]
fn measured_scheduler_speedup_reports_real_timings() {
    let workers = available_parallelism().map_or(2, |value| value.get().clamp(2, 4));
    let tasks = 8usize;
    let iterations = 100_000u64;
    let measurement = measure_speedup(workers, tasks, iterations).unwrap();
    let expected = (0..tasks as u64)
        .map(|seed| cpu_bound_workload(seed, iterations))
        .collect::<Vec<_>>();

    assert_eq!(measurement.workers, workers.max(2));
    assert_eq!(measurement.tasks, tasks);
    assert_eq!(measurement.iterations, iterations);
    assert_eq!(measurement.one_results, expected);
    assert_eq!(measurement.many_results, expected);
    assert!(measurement.one_worker > Duration::ZERO);
    assert!(measurement.many_workers > Duration::ZERO);
    let computed_ratio =
        measurement.one_worker.as_secs_f64() / measurement.many_workers.as_secs_f64();
    assert_eq!(measurement.ratio, computed_ratio);
    assert!(measurement.ratio > 0.0);
    eprintln!(
        "measured scheduler timings: 1 worker {:?}, {} workers {:?}, measured ratio {:.3}x",
        measurement.one_worker, measurement.workers, measurement.many_workers, measurement.ratio
    );
}

#[test]
fn scheduler_builder_clamps_invalid_sizes() {
    let scheduler = Scheduler::with_workers(0)
        .with_queue_capacity(0)
        .with_failure_threshold(0)
        .with_cooldown(0);
    assert_eq!(scheduler.worker_count(), 1);
    assert_eq!(scheduler.queue_capacity(), 1);
    assert_eq!(scheduler.pending(), 0);
    assert_eq!(scheduler.consecutive_failures(), 0);
    assert!(!scheduler.circuit_open());
    assert_eq!(scheduler.steals(), 0);
    scheduler.shutdown();
}

#[test]
fn metrics_snapshot_counts_accepted_and_completed_tasks() {
    let scheduler = Scheduler::with_workers(1).with_queue_capacity(8);
    assert_eq!(
        scheduler.metrics(),
        SchedulerMetrics::default(),
        "a fresh scheduler starts at zero"
    );

    let handles = (0..4i32)
        .map(|i| scheduler.spawn(move || i * 2).unwrap())
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![0, 2, 4, 6]);
    assert_eq!(
        scheduler.metrics(),
        SchedulerMetrics {
            spawned: 4,
            completed: 4,
            panicked: 0,
            pending: 0,
            steals: 0,
        },
        "one worker never steals and every accepted task completes"
    );
    scheduler.shutdown();
}

#[test]
fn metrics_count_a_panicking_task_exactly_once() {
    let scheduler = Scheduler::with_workers(2).with_queue_capacity(8);
    let ok = scheduler.spawn(|| 7).unwrap();
    let boom = scheduler
        .spawn(|| -> i32 { panic!("metrics probe") })
        .unwrap();
    assert_eq!(ok.join().unwrap(), 7);
    assert!(typed_panic_message(boom).contains("metrics probe"));

    let metrics = scheduler.metrics();
    assert_eq!(
        (
            metrics.spawned,
            metrics.completed,
            metrics.panicked,
            metrics.pending
        ),
        (2, 2, 1, 0),
        "a panic completes the task and increments the panic counter once"
    );
    scheduler.shutdown();
    let after = scheduler.metrics();
    assert_eq!(after.completed, after.spawned, "every worker has joined");
}

#[test]
fn metrics_ignore_spawns_the_scheduler_rejects() {
    let scheduler = Scheduler::with_workers(1);
    scheduler.shutdown();
    match scheduler.spawn(|| 1) {
        Err(SchedulerError::ShuttingDown) => {}
        other => panic!("expected ShuttingDown, got {other:?}"),
    }
    assert_eq!(
        scheduler.metrics(),
        SchedulerMetrics::default(),
        "a rejected spawn never counts"
    );
}
