//! Regression tests for the Dart 3.14 Future runtime, pinned to
//! `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb
//! (Dart SDK 3.14.0).

use opurom_core::{
    AsyncError, CombineOutcome, Completion, FutureRuntime, FutureValue, OnError, OnValue,
};
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

// Future combinator boundary tests, pinned to Dart 3.14
// (flutter-golden HEAD ab2912035e160d9cad6306df307576da6128ebdb).
use opurom_core::{combine_completions, should_complete_wait_early, FutureCombineErrorPolicy};

fn ok(value: i64) -> Completion {
    Completion::Value(int(value))
}

fn err(message: &str) -> Completion {
    Completion::Error(AsyncError::new(message))
}

#[test]
fn wait_empty_returns_empty_list() {
    // future.dart:598-600: no futures completes with an empty list.
    assert_eq!(
        combine_completions(&[], FutureCombineErrorPolicy::Wait),
        CombineOutcome::Wait(Vec::new())
    );
}

#[test]
fn wait_collects_values_in_source_order() {
    // future.dart:562-577: values are stored at their original positions.
    assert_eq!(
        combine_completions(&[ok(1), ok(2), ok(3)], FutureCombineErrorPolicy::Wait,),
        CombineOutcome::Wait(vec![int(1), int(2), int(3)])
    );
}

#[test]
fn wait_keeps_first_error_and_its_stack_trace() {
    // future.dart:530-537,553-558: later errors never replace the first.
    let first = AsyncError {
        message: "first".to_owned(),
        stack_trace: "frame-a\nframe-b".to_owned(),
    };
    let second = AsyncError::new("second");
    let outcome = combine_completions(
        &[
            ok(1),
            Completion::Error(first.clone()),
            Completion::Error(second),
        ],
        FutureCombineErrorPolicy::Wait,
    );
    assert_eq!(outcome, CombineOutcome::Error(first));
}

#[test]
fn wait_eager_policy_signals_immediate_completion() {
    // future.dart:550-552: eagerError completes on the first failure.
    let inputs = [ok(1), err("boom"), ok(3)];
    assert!(should_complete_wait_early(
        &inputs,
        FutureCombineErrorPolicy::WaitEager
    ));
    assert!(!should_complete_wait_early(
        &inputs,
        FutureCombineErrorPolicy::Wait
    ));
}

#[test]
fn any_returns_first_input_value() {
    // future.dart:656-670: listeners are attached in source order; the
    // first completed value wins.
    assert_eq!(
        combine_completions(&[ok(10), ok(20)], FutureCombineErrorPolicy::Any),
        CombineOutcome::Any(int(10))
    );
}

#[test]
fn any_returns_first_error_when_it_precedes_values() {
    // future.dart:662-664: an error also completes `any`.
    assert_eq!(
        combine_completions(&[err("first"), ok(20)], FutureCombineErrorPolicy::Any,),
        CombineOutcome::Error(AsyncError::new("first"))
    );
}

#[test]
fn any_empty_never_completes_and_is_represented_as_empty_wait() {
    // future.dart:666-669: with no inputs no listener can complete the
    // completer, so the kernel returns a distinguishable empty outcome.
    assert_eq!(
        combine_completions(&[], FutureCombineErrorPolicy::Any),
        CombineOutcome::Wait(Vec::new())
    );
}

#[test]
fn combinator_results_are_byte_deterministic() {
    let inputs = [ok(1), err("x"), ok(3)];
    for policy in [
        FutureCombineErrorPolicy::Wait,
        FutureCombineErrorPolicy::WaitEager,
        FutureCombineErrorPolicy::Any,
    ] {
        let a = format!("{:?}", combine_completions(&inputs, policy));
        let b = format!("{:?}", combine_completions(&inputs, policy));
        assert_eq!(a, b);
    }
}

// Future.forEach sequential boundary tests, pinned to Dart 3.14
// (async/future.dart:672-699).
use opurom_core::for_each_outcomes;

#[test]
fn for_each_processes_elements_in_order() {
    let elements = vec![int(1), int(2), int(3)];
    let outcomes = vec![ok(10), ok(20), ok(30)];
    let result = for_each_outcomes(&elements, &outcomes);
    assert_eq!(result.processed, elements);
    assert_eq!(result.error, None);
}

#[test]
fn for_each_stops_at_first_error() {
    // future.dart:686-687: an error stops the iteration.
    let elements = vec![int(1), int(2), int(3)];
    let outcomes = vec![ok(10), err("stop"), ok(30)];
    let result = for_each_outcomes(&elements, &outcomes);
    assert_eq!(result.processed, vec![int(1)]);
    assert_eq!(result.error, Some(AsyncError::new("stop")));
}

#[test]
fn for_each_empty_list_completes_without_actions() {
    let result = for_each_outcomes(&[], &[]);
    assert!(result.processed.is_empty());
    assert_eq!(result.error, None);
}

#[test]
fn for_each_values_are_discarded() {
    // future.dart:680-684: future values do not enter the result.
    let elements = vec![int(7)];
    let outcomes = vec![Completion::Value(FutureValue::Str("ignored".to_owned()))];
    let result = for_each_outcomes(&elements, &outcomes);
    assert_eq!(result.processed, vec![int(7)]);
}

// Future.wait after-error ordering, pinned to Dart 3.14
// (async/future.dart:517-600).
use opurom_core::{wait_error_at, wait_uncaught_errors};

#[test]
fn wait_error_uses_source_order_not_completion_order() {
    // future.dart:528-537,550-559: listeners are attached in source order;
    // the first failed input wins even when a later input completed earlier.
    let inputs = [None, Some(err("late")), None, Some(err("early"))];
    let outcome = wait_error_at(&inputs, FutureCombineErrorPolicy::WaitEager);
    assert_eq!(outcome, Some((1, AsyncError::new("late"))));
    assert_eq!(
        combine_completions(
            &[
                Completion::Value(int(1)),
                Completion::Error(AsyncError::new("late")),
                Completion::Error(AsyncError::new("early")),
            ],
            FutureCombineErrorPolicy::WaitEager,
        ),
        CombineOutcome::Error(AsyncError::new("late"))
    );
}

#[test]
fn wait_error_waits_for_all_inputs_without_eager_error() {
    // future.dart:587-590: without eagerError the combined future completes
    // only after every input, even if one has already failed.
    let inputs = [None, Some(err("boom")), None];
    assert_eq!(wait_error_at(&inputs, FutureCombineErrorPolicy::Wait), None);
    assert_eq!(
        should_complete_wait_early(
            &[Completion::Error(AsyncError::new("boom"))],
            FutureCombineErrorPolicy::Wait
        ),
        false
    );
}

#[test]
fn wait_error_keeps_stack_trace_byte_for_byte() {
    let original = AsyncError {
        message: "first".to_owned(),
        stack_trace: "frame-a\nframe-b".to_owned(),
    };
    let inputs = [
        Some(Completion::Error(original.clone())),
        Some(err("second")),
    ];
    let outcome = wait_error_at(&inputs, FutureCombineErrorPolicy::WaitEager);
    assert_eq!(outcome, Some((0, original)));
}

// Future.wait partial completion and non-eager error delay, pinned to Dart
// 3.14 (async/future.dart:517-600).
use opurom_core::WaitStep;

#[test]
fn wait_partial_values_stay_unresolved_until_all_inputs_complete() {
    // future.dart:567-577: a value only completes `wait` when remaining
    // reaches zero.
    let step = WaitStep::advance(
        &[None, Some(ok(2)), None],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    assert_eq!(step.outcome, None);
    assert_eq!(step.values, vec![None, Some(int(2)), None]);
}

#[test]
fn wait_non_eager_error_is_delayed_until_the_last_input() {
    // future.dart:530-537,555-557: the first error is stored, but non-eager
    // wait keeps waiting for every other input.
    let first = AsyncError::new("first");
    let partial = WaitStep::advance(
        &[Some(ok(1)), Some(Completion::Error(first.clone())), None],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    assert_eq!(partial.outcome, None);
    let second = err("second");
    let complete = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(first.clone())),
            Some(second),
        ],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    assert_eq!(complete.outcome, Some(CombineOutcome::Error(first)));
}

#[test]
fn wait_eager_error_completes_before_later_inputs() {
    // future.dart:550-552: eagerError reports the first error as soon as its
    // listener fires, without waiting for the remaining inputs.
    let step = WaitStep::advance(
        &[None, Some(err("boom")), None],
        FutureCombineErrorPolicy::WaitEager,
        None,
    );
    assert_eq!(
        step.outcome,
        Some(CombineOutcome::Error(AsyncError::new("boom")))
    );
}

#[test]
fn wait_non_eager_error_after_all_values_keeps_first_error() {
    // Values completing after the error must not replace the stored first
    // error (`future.dart:579-590`). The first error wins even if a later
    // input fails first in arrival order.
    let first = AsyncError::new("first");
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(first.clone())),
            Some(err("later")),
        ],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    assert_eq!(step.outcome, Some(CombineOutcome::Error(first)));
}

// Future.wait cleanUp ordering, pinned to Dart 3.14
// (async/future.dart:517-600).
#[test]
fn wait_cleanup_uses_source_order_before_and_after_error() {
    // future.dart:538-548,579-586: values already produced are cleaned up in
    // source order when the first error fires; later values follow.
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
            None,
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|_| Ok(())),
    );
    assert_eq!(step.cleaned_up, vec![int(1), int(2)]);
}

#[test]
fn wait_cleanup_ignores_later_errors_and_respects_flag() {
    let step = WaitStep::advance(
        &[
            Some(ok(7)),
            Some(Completion::Error(AsyncError::new("first"))),
            Some(Completion::Error(AsyncError::new("second"))),
            Some(ok(8)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|_| Ok(())),
    );
    assert_eq!(step.cleaned_up, vec![int(7), int(8)]);
    let without = WaitStep::advance(
        &[
            Some(ok(7)),
            Some(Completion::Error(AsyncError::new("first"))),
            Some(ok(8)),
        ],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    assert!(without.cleaned_up.is_empty());
}

#[test]
fn wait_cleanup_errors_are_collected_in_source_order() {
    // future.dart:542-546,581-585: cleanup runs through Future.sync, so each
    // cleanup error is reported independently in value order.
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(1) => Err(AsyncError::new("cleanup-1")),
            FutureValue::Int(2) => Err(AsyncError::new("cleanup-2")),
            _ => Ok(()),
        }),
    );
    assert_eq!(step.cleaned_up, vec![int(1), int(2)]);
    assert_eq!(
        step.cleanup_errors,
        vec![AsyncError::new("cleanup-1"), AsyncError::new("cleanup-2")]
    );
    assert_eq!(
        step.outcome,
        Some(CombineOutcome::Error(AsyncError::new("boom")))
    );
}

#[test]
fn wait_empty_input_completes_with_empty_list() {
    // future.dart:598-600: an empty iterable completes immediately with an
    // empty list, including with a cleanUp callback installed.
    for policy in [
        FutureCombineErrorPolicy::Wait,
        FutureCombineErrorPolicy::WaitEager,
    ] {
        let step = WaitStep::advance(&[], policy, Some(&|_| Ok(())));
        assert_eq!(step.outcome, Some(CombineOutcome::Wait(Vec::new())));
        assert!(step.cleaned_up.is_empty());
        assert!(step.cleanup_errors.is_empty());
    }
}

// Future.wait uncaught error ordering, pinned to Dart 3.14
// (async/future.dart:529-559,579-600).
#[test]
fn wait_cleanup_errors_are_reported_before_the_combined_error() {
    // future.dart:542-546,581-585: Future.sync schedules each cleanup error
    // before the combined future completes with the first input error.
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(_) => Err(AsyncError::new(format!(
                "cleanup-{}",
                match value {
                    FutureValue::Int(v) => *v,
                    _ => 0,
                }
            ))),
            _ => Ok(()),
        }),
    );
    let uncaught = wait_uncaught_errors(&step);
    assert_eq!(
        uncaught.cleanup,
        vec![AsyncError::new("cleanup-1"), AsyncError::new("cleanup-2")]
    );
    assert_eq!(uncaught.combined, Some(AsyncError::new("boom")));
}

#[test]
fn wait_without_error_has_only_the_combined_outcome() {
    let step = WaitStep::advance(
        &[Some(ok(1)), Some(ok(2))],
        FutureCombineErrorPolicy::Wait,
        Some(&|_| Ok(())),
    );
    let uncaught = wait_uncaught_errors(&step);
    assert!(uncaught.cleanup.is_empty());
    assert_eq!(uncaught.combined, None);
}

#[test]
fn wait_without_input_errors_never_runs_cleanup() {
    // future.dart:538-548,579-586: cleanUp only runs when an error occurred;
    // a fully successful wait never invokes it.
    let step = WaitStep::advance(
        &[Some(ok(1)), Some(ok(2))],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(2) => Err(AsyncError::new("cleanup-only")),
            _ => Ok(()),
        }),
    );
    let uncaught = wait_uncaught_errors(&step);
    assert!(uncaught.cleanup.is_empty());
    assert_eq!(uncaught.combined, None);
    assert_eq!(
        step.outcome,
        Some(CombineOutcome::Wait(vec![int(1), int(2)]))
    );
}

// End-to-end uncaught reporting order, pinned to Dart 3.14
// (async/future.dart:529-600, schedule_microtask.dart:63-105).
#[test]
fn wait_cleanup_errors_are_reported_uncaught_before_the_combined_error() {
    // future.dart:542-546,581-585: each cleanup body runs inside Future.sync,
    // so its error is uncaught; the combined error follows in the same zone.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(1) => Err(AsyncError::new("cleanup-1")),
            FutureValue::Int(2) => Err(AsyncError::new("cleanup-2")),
            _ => Ok(()),
        }),
    );
    let uncaught = wait_uncaught_errors(&step);
    uncaught.report_cleanup_errors(&runtime);
    if let Some(combined) = &uncaught.combined {
        runtime.report_uncaught(combined.clone());
    }
    assert_eq!(
        runtime.unhandled_errors(),
        vec![
            AsyncError::new("cleanup-1"),
            AsyncError::new("cleanup-2"),
            AsyncError::new("boom"),
        ]
    );
}

#[test]
fn uncaught_reporting_keeps_insertion_order() {
    // future.dart:819-823: uncaught errors are reported in the order they
    // occur.
    let runtime = FutureRuntime::new();
    runtime.report_uncaught(AsyncError::new("first"));
    runtime.report_uncaught(AsyncError::new("second"));
    assert_eq!(
        runtime.unhandled_errors(),
        vec![AsyncError::new("first"), AsyncError::new("second")]
    );
}

#[test]
fn wait_delivery_reports_cleanup_errors_before_the_combined_future() {
    // future.dart:542-546,581-585: each cleanup body runs in its own
    // Future.sync, so its failure is uncaught before the combined future
    // completes with the first input error.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(_) => Err(AsyncError::new("cleanup")),
            _ => Ok(()),
        }),
    );
    let future = step.deliver(&runtime);
    runtime.pump(16);
    // Nobody listens to the combined future, so its error is uncaught too;
    // it is reported after the cleanup errors
    // (`future.dart:550-557,1336-1351`).
    assert_eq!(
        runtime.unhandled_errors(),
        vec![
            AsyncError::new("cleanup"),
            AsyncError::new("cleanup"),
            AsyncError::new("boom"),
        ]
    );
    assert_eq!(
        future.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
}

#[test]
fn wait_delivery_with_a_listener_keeps_only_cleanup_errors_uncaught() {
    // future.dart:819-823: a handled error is not reported as unhandled.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(_) => Err(AsyncError::new("cleanup")),
            _ => Ok(()),
        }),
    );
    let future = step.deliver(&runtime);
    let _handled = future.catch_error(&runtime, Rc::new(|_: &AsyncError| int(0)) as OnError);
    runtime.pump(16);
    assert_eq!(
        runtime.unhandled_errors(),
        vec![AsyncError::new("cleanup"), AsyncError::new("cleanup")]
    );
    assert_eq!(
        future.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
}

#[test]
fn wait_delivery_of_a_successful_wait_yields_the_value_list() {
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[Some(ok(1)), Some(ok(2))],
        FutureCombineErrorPolicy::Wait,
        None,
    );
    let future = step.deliver(&runtime);
    runtime.pump(16);
    assert_eq!(
        future.completion(),
        Some(Completion::Value(FutureValue::List(vec![int(1), int(2)])))
    );
    assert!(runtime.unhandled_errors().is_empty());
}

#[test]
fn wait_eager_delivery_reports_cleanup_errors_before_the_combined_error() {
    // future.dart:542-546,550-552,581-585: with eagerError the first error
    // completes the wait future immediately, after its cleanup bodies have
    // been scheduled as uncaught errors.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(Completion::Error(AsyncError::new("boom"))),
            None,
        ],
        FutureCombineErrorPolicy::WaitEager,
        Some(&|value| match value {
            FutureValue::Int(1) => Err(AsyncError::new("cleanup-1")),
            _ => Ok(()),
        }),
    );
    let future = step.deliver(&runtime);
    runtime.pump(16);
    assert_eq!(
        future.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
    assert_eq!(
        runtime.unhandled_errors(),
        vec![AsyncError::new("cleanup-1"), AsyncError::new("boom")]
    );
}

#[test]
fn wait_eager_delivery_ignores_later_values_and_cleanup() {
    // future.dart:550-552,579-586: eagerError completes on the first error;
    // pending inputs are not part of the delivered result.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            None,
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(3)),
        ],
        FutureCombineErrorPolicy::WaitEager,
        Some(&|_| Err(AsyncError::new("cleanup-late"))),
    );
    let future = step.deliver(&runtime);
    runtime.pump(16);
    assert_eq!(
        future.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
    assert_eq!(runtime.unhandled_errors(), vec![AsyncError::new("boom")]);
}

#[test]
fn wait_cleanup_errors_global_order_across_first_error_and_late_values() {
    // future.dart:538-548,579-586: cleanup for values already present runs
    // in source order when the first error fires; values completing later are
    // cleaned up in source order after that, still before the non-eager
    // combined error is delivered.
    let runtime = FutureRuntime::new();
    let step = WaitStep::advance(
        &[
            Some(ok(1)),
            Some(ok(0)),
            Some(Completion::Error(AsyncError::new("boom"))),
            Some(ok(2)),
            Some(ok(3)),
        ],
        FutureCombineErrorPolicy::Wait,
        Some(&|value| match value {
            FutureValue::Int(v) => Err(AsyncError::new(format!("cleanup-{v}"))),
            _ => Ok(()),
        }),
    );
    assert_eq!(
        step.outcome,
        Some(CombineOutcome::Error(AsyncError::new("boom")))
    );
    assert_eq!(
        step.cleanup_errors,
        vec![
            AsyncError::new("cleanup-1"),
            AsyncError::new("cleanup-0"),
            AsyncError::new("cleanup-2"),
            AsyncError::new("cleanup-3"),
        ]
    );
    let future = step.deliver(&runtime);
    let _handled = future.catch_error(&runtime, Rc::new(|_: &AsyncError| int(0)) as OnError);
    runtime.pump(16);
    assert_eq!(
        runtime.unhandled_errors(),
        vec![
            AsyncError::new("cleanup-1"),
            AsyncError::new("cleanup-0"),
            AsyncError::new("cleanup-2"),
            AsyncError::new("cleanup-3"),
        ]
    );
}
