//! Recovery path of the plugin bus circuit breaker: trip, cooldown, one trial,
//! close on success, re-open on failure. All timing uses an injected
//! `CounterClock`, so no test sleeps on the wall clock.

use opurom_core::{Clock, CounterClock, PluginBus, PluginError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn plugin_bus_blocks_during_cooldown_and_a_trial_success_closes_it() {
    let clock = CounterClock::at(0);
    let clock_trait: Arc<dyn Clock> = Arc::new(clock.clone());
    let bus = PluginBus::new(2).with_cooldown(5).with_clock(clock_trait);
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(true));
    let handler_calls = Arc::clone(&calls);
    let handler_fail = Arc::clone(&fail);
    bus.register("flaky", move |input| {
        handler_calls.fetch_add(1, Ordering::SeqCst);
        if handler_fail.load(Ordering::SeqCst) {
            Err("down".to_owned())
        } else {
            Ok(format!("ok:{input}"))
        }
    });

    for _ in 0..2 {
        assert!(matches!(
            bus.call("flaky", "x"),
            Err(PluginError::Failed(_))
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(bus.is_disabled("flaky"));
    assert_eq!(bus.retry_after("flaky"), Some(5));

    match bus.call("flaky", "x") {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 5),
        other => panic!("expected CircuitOpen during cooldown, got {other:?}"),
    }
    assert_eq!(clock.advance(4), 4);
    match bus.call("flaky", "x") {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 1),
        other => panic!("expected CircuitOpen before cooldown expires, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    fail.store(false, Ordering::SeqCst);
    assert_eq!(clock.advance(1), 5);
    assert_eq!(bus.call("flaky", "x").unwrap(), "ok:x");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(!bus.is_disabled("flaky"));
    assert_eq!(bus.retry_after("flaky"), None);
    assert_eq!(bus.call("flaky", "y").unwrap(), "ok:y");
    assert_eq!(calls.load(Ordering::SeqCst), 4);

    fail.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        assert!(matches!(
            bus.call("flaky", "z"),
            Err(PluginError::Failed(_))
        ));
    }
    assert_eq!(bus.retry_after("flaky"), Some(5));
}

#[test]
fn plugin_bus_trial_failure_reopens_the_circuit() {
    let clock = CounterClock::at(0);
    let clock_trait: Arc<dyn Clock> = Arc::new(clock.clone());
    let bus = PluginBus::new(2).with_cooldown(5).with_clock(clock_trait);
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    bus.register("flaky", move |_| {
        handler_calls.fetch_add(1, Ordering::SeqCst);
        Err("down".to_owned())
    });

    for _ in 0..2 {
        assert!(matches!(bus.call("flaky", ""), Err(PluginError::Failed(_))));
    }
    assert_eq!(bus.retry_after("flaky"), Some(5));

    assert_eq!(clock.advance(5), 5);
    assert!(matches!(bus.call("flaky", ""), Err(PluginError::Failed(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(bus.is_disabled("flaky"));
    assert_eq!(bus.retry_after("flaky"), Some(5));

    match bus.call("flaky", "") {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 5),
        other => panic!("expected CircuitOpen after a failed trial, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    assert_eq!(clock.advance(5), 10);
    assert!(matches!(bus.call("flaky", ""), Err(PluginError::Failed(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[test]
fn plugin_bus_isolates_breakers_per_plugin_under_concurrent_calls() {
    let clock = CounterClock::at(0);
    let clock_trait: Arc<dyn Clock> = Arc::new(clock.clone());
    let bus = Arc::new(PluginBus::new(2).with_cooldown(5).with_clock(clock_trait));
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let handler_entered = Arc::clone(&entered);
    let handler_release = Arc::clone(&release);
    bus.register("flaky", move |input| {
        if handler_calls.fetch_add(1, Ordering::SeqCst) < 2 {
            Err("down".to_owned())
        } else {
            handler_entered.wait();
            handler_release.wait();
            Ok(format!("ok:{input}"))
        }
    });
    bus.register("steady", |input| Ok(format!("steady:{input}")));

    for _ in 0..2 {
        assert!(matches!(bus.call("flaky", ""), Err(PluginError::Failed(_))));
    }
    assert_eq!(clock.advance(5), 5);

    let trial_bus = Arc::clone(&bus);
    let trial = thread::spawn(move || trial_bus.call("flaky", "x"));
    entered.wait();

    match bus.call("flaky", "y") {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 5),
        other => panic!("expected CircuitOpen while a trial is in flight, got {other:?}"),
    }
    assert_eq!(bus.call("steady", "z").unwrap(), "steady:z");
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    release.wait();
    assert_eq!(trial.join().unwrap().unwrap(), "ok:x");
    assert!(!bus.is_disabled("flaky"));
    assert_eq!(bus.retry_after("flaky"), None);
}
