//! Regression tests for the Dart 3.14 Future runtime, pinned to
//! `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb
//! (Dart SDK 3.14.0).

use opurom_core::{AsyncError, Completion, FutureRuntime, FutureValue, OnError, OnValue};
use std::cell::RefCell;
use std::rc::Rc;

fn int(value: i64) -> FutureValue {
    FutureValue::Int(value)
}

#[test]
fn value_completes_through_the_microtask_queue() {
    // future_impl.dart:364-366: Future.value completes asynchronously.
    let runtime = FutureRuntime::new();
    let future = runtime.value(int(1));
    assert!(!future.is_completed());
    runtime.pump(16);
    assert!(future.is_completed());
    assert_eq!(future.completion(), Some(Completion::Value(int(1))));
}

#[test]
fn then_maps_the_value() {
    // future.dart:824: then maps the value into the returned future.
    let runtime = FutureRuntime::new();
    let future = runtime.value(int(1));
    let mapped = future.then(
        &runtime,
        Rc::new(|value| match value {
            FutureValue::Int(v) => int(v + 1),
            other => other,
        }) as OnValue,
    );
    runtime.pump(16);
    assert_eq!(mapped.completion(), Some(Completion::Value(int(2))));
}

#[test]
fn then_forwards_an_error_unchanged() {
    // future.dart:812-813: without an error handler the error is forwarded.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("boom"));
    let chained = future.then(&runtime, Rc::new(|value| value) as OnValue);
    runtime.pump(16);
    assert_eq!(
        chained.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
}

#[test]
fn then_on_error_handles_the_error() {
    // future.dart:824: with an error handler the error is handled.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("boom"));
    let chained = future.then_on_error(
        &runtime,
        Rc::new(|value| value) as OnValue,
        Rc::new(|error: &AsyncError| int(error.message.len() as i64)) as OnError,
    );
    runtime.pump(16);
    assert_eq!(chained.completion(), Some(Completion::Value(int(4))));
}

#[test]
fn catch_error_with_a_false_test_keeps_the_error() {
    // future.dart:836-846: a false test leaves the error untouched.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("boom"));
    let caught = future.catch_error_test(
        &runtime,
        Rc::new(|_: &AsyncError| false),
        Rc::new(|_: &AsyncError| int(0)) as OnError,
    );
    runtime.pump(16);
    assert_eq!(
        caught.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
}

#[test]
fn catch_error_with_a_true_test_handles_the_error() {
    // future.dart:843-846: a true test runs the handler.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("boom"));
    let caught = future.catch_error_test(
        &runtime,
        Rc::new(|_: &AsyncError| true),
        Rc::new(|_: &AsyncError| int(7)) as OnError,
    );
    runtime.pump(16);
    assert_eq!(caught.completion(), Some(Completion::Value(int(7))));
}

#[test]
fn when_complete_runs_on_both_paths() {
    // future.dart:877-926: the action runs whether the future completed with
    // a value or an error, and the original result passes through.
    let runtime = FutureRuntime::new();
    let ran = Rc::new(RefCell::new(0));

    let ok = runtime.value(int(1));
    let ok_ran = Rc::clone(&ran);
    let chained_ok = ok.when_complete(
        &runtime,
        Rc::new(move |value: FutureValue| {
            *ok_ran.borrow_mut() += 1;
            value
        }) as OnValue,
    );

    let failed = runtime.error(AsyncError::new("boom"));
    let fail_ran = Rc::clone(&ran);
    let chained_fail = failed.when_complete(
        &runtime,
        Rc::new(move |value: FutureValue| {
            *fail_ran.borrow_mut() += 1;
            value
        }) as OnValue,
    );

    runtime.pump(16);
    assert_eq!(*ran.borrow(), 2);
    assert_eq!(chained_ok.completion(), Some(Completion::Value(int(1))));
    assert_eq!(
        chained_fail.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
}

#[test]
fn an_unhandled_error_is_reported() {
    // future.dart:819-823,1336-1351: an error nobody listens for is
    // reported as unhandled.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("unconsumed"));
    runtime.pump(16);
    assert_eq!(
        runtime.unhandled_errors(),
        vec![AsyncError::new("unconsumed")]
    );
    // The future still carries its error.
    assert_eq!(
        future.completion(),
        Some(Completion::Error(AsyncError::new("unconsumed")))
    );
}

#[test]
fn a_handled_error_is_not_reported() {
    // future.dart:819-823: an error with a listener is not unhandled.
    let runtime = FutureRuntime::new();
    let future = runtime.error(AsyncError::new("handled"));
    let _ = future.catch_error(&runtime, Rc::new(|_: &AsyncError| int(0)) as OnError);
    runtime.pump(16);
    assert!(runtime.unhandled_errors().is_empty());
}

#[test]
fn completer_completes_once() {
    // future.dart:1307,1316: complete and completeError may be called at
    // most once.
    let runtime = FutureRuntime::new();
    let completer = runtime.completer();
    assert!(completer.complete(int(5)));
    assert!(!completer.complete(int(6)));
    assert!(completer.is_completed());
    runtime.pump(16);
    assert_eq!(
        completer.future().completion(),
        Some(Completion::Value(int(5)))
    );
}

#[test]
fn completer_error_without_listener_is_unhandled() {
    // future.dart:1329-1351: an error handler must be installed before
    // completeError, otherwise the error is uncaught.
    let runtime = FutureRuntime::new();
    let completer = runtime.completer();
    assert!(completer.complete_error(AsyncError::new("late")));
    runtime.pump(16);
    assert_eq!(runtime.unhandled_errors(), vec![AsyncError::new("late")]);
}

#[test]
fn chains_run_in_microtask_order() {
    // schedule_microtask.dart:63-76: callbacks run in scheduling order.
    let runtime = FutureRuntime::new();
    let order = Rc::new(RefCell::new(Vec::<i32>::new()));

    let first = runtime.value(int(1));
    let first_order = Rc::clone(&order);
    let second = first.then(
        &runtime,
        Rc::new(move |value: FutureValue| {
            first_order.borrow_mut().push(1);
            value
        }) as OnValue,
    );
    let second_order = Rc::clone(&order);
    let third = second.then(
        &runtime,
        Rc::new(move |value: FutureValue| {
            second_order.borrow_mut().push(2);
            value
        }) as OnValue,
    );

    runtime.pump(16);
    assert_eq!(*order.borrow(), vec![1, 2]);
    assert_eq!(third.completion(), Some(Completion::Value(int(1))));
}

#[test]
fn a_listener_added_after_completion_runs_immediately() {
    // future_impl.dart:395-420: a listener on an already completed future
    // is notified right away.
    let runtime = FutureRuntime::new();
    let future = runtime.value(int(4));
    runtime.pump(16);
    let mapped = future.then(
        &runtime,
        Rc::new(|value| match value {
            FutureValue::Int(v) => int(v * 2),
            other => other,
        }) as OnValue,
    );
    assert_eq!(mapped.completion(), Some(Completion::Value(int(8))));
}

#[test]
fn false_error_test_preserves_stack_trace_byte_for_byte() {
    // future.dart:836-841: a false test leaves the error AND stack trace
    // untouched.
    let runtime = FutureRuntime::new();
    let original = AsyncError {
        message: "boom".to_owned(),
        stack_trace: "frame0\nframe1".to_owned(),
    };
    let future = runtime.error(original.clone());
    let chained = future.catch_error_test(
        &runtime,
        Rc::new(|_: &AsyncError| false),
        Rc::new(|_: &AsyncError| int(0)) as OnError,
    );
    runtime.pump(16);
    assert_eq!(chained.completion(), Some(Completion::Error(original)));
}

#[test]
fn unhandled_errors_keep_insertion_order() {
    // future.dart:819-823: errors are reported as unhandled in completion
    // order.
    let runtime = FutureRuntime::new();
    runtime.error(AsyncError::new("first"));
    runtime.error(AsyncError::new("second"));
    runtime.pump(16);
    assert_eq!(
        runtime.unhandled_errors(),
        vec![AsyncError::new("first"), AsyncError::new("second")]
    );
}

#[test]
fn chained_error_becomes_handled_at_the_end() {
    // future.dart:812-813,840-841: a rejected future is unhandled only until
    // a listener handles it somewhere in the chain.
    let runtime = FutureRuntime::new();
    let source = runtime.error(AsyncError::new("caught"));
    let _ = source
        .then(&runtime, Rc::new(|value| value) as OnValue)
        .catch_error(&runtime, Rc::new(|_: &AsyncError| int(1)) as OnError);
    runtime.pump(16);
    assert!(runtime.unhandled_errors().is_empty());
}
