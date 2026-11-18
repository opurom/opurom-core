//! Regression tests for the Dart 3.14 Stream runtime, pinned to
//! `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb
//! (Dart SDK 3.14.0).

use opurom_core::{
    AsyncError, Completion, DartError, DataCallback, DoneCallback, ErrorCallback, FutureRuntime,
    FutureValue, StreamEvent, StreamHooks, StreamRuntime,
};
use std::cell::RefCell;
use std::rc::Rc;

fn int(value: i64) -> FutureValue {
    FutureValue::Int(value)
}

#[test]
fn single_subscription_buffers_until_listen() {
    // stream_controller.dart:97-98: a controller buffers events until a
    // subscriber registers.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Data(int(1)));
    runtime.add(&stream, StreamEvent::Data(int(2)));

    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    assert_eq!(*seen.borrow(), vec![int(1), int(2)]);
}

#[test]
fn broadcast_drops_events_without_listeners() {
    // stream_controller.dart:132-139: broadcast events with no listener are
    // dropped, not buffered.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Data(int(1)));

    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    assert!(seen.borrow().is_empty());
}

#[test]
fn broadcast_delivers_to_every_current_listener() {
    // stream_controller.dart:134-135: events go to all listeners subscribed
    // at the time of add.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let first = Rc::new(RefCell::new(Vec::new()));
    let second = Rc::new(RefCell::new(Vec::new()));
    let first_clone = Rc::clone(&first);
    let second_clone = Rc::clone(&second);
    stream.listen(
        Some(Rc::new(move |value| first_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    stream.listen(
        Some(Rc::new(move |value| second_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    runtime.add(&stream, StreamEvent::Data(int(9)));
    assert_eq!(*first.borrow(), vec![int(9)]);
    assert_eq!(*second.borrow(), vec![int(9)]);
}

#[test]
fn paused_listener_buffers_while_other_listeners_continue() {
    // stream_controller.dart:141-143: only the paused subscription buffers.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let paused_seen = Rc::new(RefCell::new(Vec::new()));
    let other_seen = Rc::new(RefCell::new(Vec::new()));
    let paused_clone = Rc::clone(&paused_seen);
    let other_clone = Rc::clone(&other_seen);
    let paused = stream.listen(
        Some(Rc::new(move |value| paused_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    stream.listen(
        Some(Rc::new(move |value| other_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    paused.pause();
    runtime.add(&stream, StreamEvent::Data(int(1)));
    assert!(paused_seen.borrow().is_empty());
    assert_eq!(*other_seen.borrow(), vec![int(1)]);
    paused.resume();
    assert_eq!(*paused_seen.borrow(), vec![int(1)]);
}

#[test]
fn error_and_done_events_reach_callbacks() {
    // stream_controller.dart:134-135, stream_controller.dart:254-260:
    // errors and done are delivered like data.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    let errors = Rc::new(RefCell::new(Vec::new()));
    let done_count = Rc::new(RefCell::new(0));
    let errors_clone = Rc::clone(&errors);
    let done_clone = Rc::clone(&done_count);
    stream.listen(
        None,
        Some(
            Rc::new(move |error: &AsyncError| errors_clone.borrow_mut().push(error.clone()))
                as ErrorCallback,
        ),
        Some(Rc::new(move || *done_clone.borrow_mut() += 1) as DoneCallback),
    );
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("bad")));
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(errors.borrow().len(), 1);
    assert_eq!(*done_count.borrow(), 1);
}

#[test]
fn hooks_fire_on_first_listen_and_last_cancel() {
    // stream_controller.dart:168-171: onListen on the first subscriber,
    // onCancel when there are none left.
    let runtime = StreamRuntime::new();
    let listened = Rc::new(RefCell::new(0));
    let canceled = Rc::new(RefCell::new(0));
    let listened_clone = Rc::clone(&listened);
    let canceled_clone = Rc::clone(&canceled);
    let hooks = StreamHooks {
        on_listen: Some(Rc::new(move || *listened_clone.borrow_mut() += 1)),
        on_cancel: Some(Rc::new(move || *canceled_clone.borrow_mut() += 1)),
        ..StreamHooks::default()
    };
    let stream = runtime.broadcast_controller(hooks);
    let first = stream.listen(None, None, None);
    let second = stream.listen(None, None, None);
    assert_eq!(*listened.borrow(), 1);
    first.cancel();
    assert_eq!(*canceled.borrow(), 0);
    second.cancel();
    assert_eq!(*canceled.borrow(), 1);
}

#[test]
fn map_transforms_each_data_event() {
    // stream_pipe.dart:237-253: map converts each data event once per
    // listener.
    let runtime = StreamRuntime::new();
    let source = runtime.controller(StreamHooks::default());
    let mapped = source.map(Rc::new(|value| match value {
        FutureValue::Int(v) => int(v * 2),
        other => other,
    }));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    mapped.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    runtime.add(&source, StreamEvent::Data(int(2)));
    runtime.add(&source, StreamEvent::Data(int(3)));
    assert_eq!(*seen.borrow(), vec![int(4), int(6)]);
}

#[test]
fn where_drops_failing_events() {
    // stream_pipe.dart:213-231: where drops data failing the test.
    let runtime = StreamRuntime::new();
    let source = runtime.controller(StreamHooks::default());
    let filtered = source.r#where(Rc::new(
        |value| matches!(value, FutureValue::Int(v) if *v > 3),
    ));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    filtered.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    for value in [1, 4, 2, 5] {
        runtime.add(&source, StreamEvent::Data(int(value)));
    }
    assert_eq!(*seen.borrow(), vec![int(4), int(5)]);
}

#[test]
fn distinct_drops_adjacent_duplicates_only() {
    // stream_pipe.dart:485-530: distinct compares only adjacent events.
    let runtime = StreamRuntime::new();
    let source = runtime.controller(StreamHooks::default());
    let distinct = source.distinct();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    distinct.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    for value in [2, 2, 6, 6, 8, 2] {
        runtime.add(&source, StreamEvent::Data(int(value)));
    }
    assert_eq!(*seen.borrow(), vec![int(2), int(6), int(8), int(2)]);
}

#[test]
fn a_cancelled_subscription_stops_receiving() {
    // stream_controller.dart:141-143: cancel ends the subscription.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    let subscription = stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    runtime.add(&stream, StreamEvent::Data(int(1)));
    subscription.cancel();
    runtime.add(&stream, StreamEvent::Data(int(2)));
    assert_eq!(*seen.borrow(), vec![int(1)]);
}

#[test]
fn closed_stream_rejects_new_events() {
    // broadcast_stream_controller.dart:247-257: adding after close throws a
    // state error; the listener never sees the rejected event.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    runtime.add(&stream, StreamEvent::Done);
    let error = runtime.try_add(&stream, StreamEvent::Data(int(1)));
    assert_eq!(
        error.err(),
        Some(DartError::State(
            "Cannot add new events after calling close".to_owned()
        ))
    );
    assert!(seen.borrow().is_empty());
}

#[test]
fn broadcast_errors_drop_without_listeners() {
    // stream_controller.dart:132-139: addError without a listener is dropped.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("lost")));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let errors_clone = Rc::clone(&errors);
    stream.listen(
        None,
        Some(
            Rc::new(move |error: &AsyncError| errors_clone.borrow_mut().push(error.clone()))
                as ErrorCallback,
        ),
        None,
    );
    assert!(errors.borrow().is_empty());
}

#[test]
fn cancelling_during_pause_discards_buffered_events() {
    // stream_controller.dart:141-143: cancel ends the subscription, so
    // buffered events are no longer delivered on resume.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    let subscription = stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    subscription.pause();
    runtime.add(&stream, StreamEvent::Data(int(1)));
    subscription.cancel();
    subscription.resume();
    assert!(seen.borrow().is_empty());
}

#[test]
fn error_events_bypass_where_filters() {
    // stream_pipe.dart:220-231: _WhereStream forwards error/done unchanged;
    // only data is tested.
    let runtime = StreamRuntime::new();
    let source = runtime.controller(StreamHooks::default());
    let filtered = source.r#where(Rc::new(|_| false));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let errors_clone = Rc::clone(&errors);
    filtered.listen(
        None,
        Some(
            Rc::new(move |error: &AsyncError| errors_clone.borrow_mut().push(error.clone()))
                as ErrorCallback,
        ),
        None,
    );
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Error(AsyncError::new("passthrough")));
    assert!(errors
        .borrow()
        .iter()
        .any(|error| error.message == "passthrough"));
}

#[test]
fn nested_pause_needs_matching_resumes() {
    // stream_impl.dart:204-229: pause count is nested; one resume does not
    // unpause a doubly-paused subscription.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    let subscription = stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    subscription.pause();
    subscription.pause();
    runtime.add(&stream, StreamEvent::Data(int(1)));
    subscription.resume();
    assert!(seen.borrow().is_empty());
    subscription.resume();
    assert_eq!(*seen.borrow(), vec![int(1)]);
}

#[test]
fn pause_after_cancel_is_a_no_op() {
    // stream_impl.dart:204-205,215-216: a canceled subscription ignores
    // pause and resume.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let subscription = stream.listen(None, None, None);
    subscription.cancel();
    subscription.pause();
    subscription.resume();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = Rc::clone(&seen);
    stream.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    runtime.add(&stream, StreamEvent::Data(int(1)));
    assert_eq!(*seen.borrow(), vec![int(1)]);
}

#[test]
fn done_is_delivered_to_each_listener_exactly_once() {
    // broadcast_stream_controller.dart:404-413: _sendDone closes every
    // current listener.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let counts = Rc::new(RefCell::new(vec![0, 0]));
    for index in 0..2 {
        let counts = Rc::clone(&counts);
        stream.listen(
            None,
            None,
            Some(Rc::new(move || counts.borrow_mut()[index] += 1) as DoneCallback),
        );
    }
    runtime.add(&stream, StreamEvent::Done);
    runtime.add(&stream, StreamEvent::Done); // closed controller ignores it
    assert_eq!(*counts.borrow(), vec![1, 1]);
}

#[test]
fn broadcast_event_order_is_independent_per_listener() {
    // stream_controller.dart:160-166: each subscription receives all events
    // in order, although listeners may interleave.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let first = Rc::new(RefCell::new(Vec::new()));
    let second = Rc::new(RefCell::new(Vec::new()));
    let first_clone = Rc::clone(&first);
    let second_clone = Rc::clone(&second);
    stream.listen(
        Some(Rc::new(move |value| first_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    stream.listen(
        Some(Rc::new(move |value| second_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    for value in [1, 2, 3, 4] {
        runtime.add(&stream, StreamEvent::Data(int(value)));
    }
    assert_eq!(*first.borrow(), vec![int(1), int(2), int(3), int(4)]);
    assert_eq!(*second.borrow(), vec![int(1), int(2), int(3), int(4)]);
}

// cancelOnError and post-error ordering, pinned to Dart 3.14
// (flutter-golden HEAD ab2912035e160d9cad6306df307576da6128ebdb).
use opurom_core::ListenOptions;

#[test]
fn cancel_on_error_delivers_error_then_stops() {
    // stream_impl.dart:386-421: the error callback runs once, then the
    // subscription is canceled; later data is not delivered.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    stream.listen_with(
        Some(
            Rc::new(move |_: FutureValue| data_events.borrow_mut().push("data".to_owned()))
                as DataCallback,
        ),
        Some(Rc::new(move |error: &AsyncError| {
            error_events
                .borrow_mut()
                .push(format!("error:{}", error.message))
        }) as ErrorCallback),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
        ListenOptions {
            cancel_on_error: true,
        },
    );
    runtime.add(&stream, StreamEvent::Data(int(1)));
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&stream, StreamEvent::Data(int(2)));
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["data", "error:boom"]);
}

#[test]
fn cancel_on_error_false_keeps_receiving_after_error() {
    // stream_impl.dart:417-421: without the flag, the subscription stays
    // open and continues to receive later events.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    stream.listen(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(
            Rc::new(move |_: &AsyncError| error_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
    );
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("soft")));
    runtime.add(&stream, StreamEvent::Data(int(1)));
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["error", "data", "done"]);
}

#[test]
fn broadcast_cancel_on_error_is_per_subscription() {
    // stream_controller.dart:141-143: each subscription is independent, so
    // one listener's auto-cancel must not affect another.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let strict = Rc::new(RefCell::new(Vec::<String>::new()));
    let lenient = Rc::new(RefCell::new(Vec::<String>::new()));
    let strict_events = Rc::clone(&strict);
    let lenient_events = Rc::clone(&lenient);
    stream.listen_with(
        None,
        Some(Rc::new(move |_| {
            strict_events.borrow_mut().push("error".to_owned())
        })),
        None,
        ListenOptions {
            cancel_on_error: true,
        },
    );
    stream.listen(
        None,
        Some(
            Rc::new(move |_: &AsyncError| lenient_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        None,
    );
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("later")));
    assert_eq!(*strict.borrow(), vec!["error"]);
    assert_eq!(*lenient.borrow(), vec!["error", "error"]);
}

#[test]
fn cancel_on_error_with_pause_buffers_then_suppresses() {
    // stream_impl.dart:69-76,407-421: a paused listener buffers the error;
    // after resume the error is delivered and then cancels the subscription.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let subscription = stream.listen_with(
        Some(Rc::new(move |_| {
            data_events.borrow_mut().push("data".to_owned())
        })),
        Some(Rc::new(move |_| {
            error_events.borrow_mut().push("error".to_owned())
        })),
        None,
        ListenOptions {
            cancel_on_error: true,
        },
    );
    subscription.pause();
    runtime.add(&stream, StreamEvent::Data(int(1)));
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    subscription.resume();
    runtime.add(&stream, StreamEvent::Data(int(2)));
    assert_eq!(*events.borrow(), vec!["data", "error"]);
}

#[test]
fn manual_cancel_before_error_suppresses_error_callback() {
    // stream_impl.dart:392-395: a subscription canceled before the error is
    // sent must not report it.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let errors = Rc::new(RefCell::new(Vec::new()));
    let errors_clone = Rc::clone(&errors);
    let subscription = stream.listen(
        None,
        Some(
            Rc::new(move |error: &AsyncError| errors_clone.borrow_mut().push(error.clone()))
                as ErrorCallback,
        ),
        None,
    );
    subscription.cancel();
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("suppressed")));
    assert!(errors.borrow().is_empty());
}

#[test]
fn done_after_error_without_cancel_is_suppressed_by_closed_controller() {
    // broadcast_stream_controller.dart:266-275: close marks the controller
    // closed, so a second done is ignored even if the listener stays open.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let counts = Rc::new(RefCell::new(0));
    let counts_clone = Rc::clone(&counts);
    stream.listen(
        None,
        None,
        Some(Rc::new(move || *counts_clone.borrow_mut() += 1) as DoneCallback),
    );
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("soft")));
    runtime.add(&stream, StreamEvent::Done);
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(*counts.borrow(), 1);
}

// Pipe cancellation propagation, pinned to Dart 3.14
// (async/stream_pipe.dart:213-253,485-530, async/stream_impl.dart:386-421).
#[test]
fn pipe_cancel_on_error_stops_upstream_after_delivery() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let mapped = source.map(Rc::new(|value| value));
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    mapped.listen_with(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(
            Rc::new(move |_: &AsyncError| error_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
        ListenOptions {
            cancel_on_error: true,
        },
    );
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&source, StreamEvent::Data(int(2)));
    runtime.add(&source, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["data", "error"]);
}

#[test]
fn pipe_cancel_on_error_false_keeps_upstream_open() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let filtered = source.r#where(Rc::new(|_| true));
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    filtered.listen_with(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(
            Rc::new(move |_: &AsyncError| error_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        None,
        ListenOptions::default(),
    );
    runtime.add(&source, StreamEvent::Error(AsyncError::new("soft")));
    runtime.add(&source, StreamEvent::Data(int(1)));
    assert_eq!(*events.borrow(), vec!["error", "data"]);
}

#[test]
fn chained_pipes_propagate_cancel_to_the_source() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let pipeline = source.map(Rc::new(|value| value)).distinct();
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    pipeline.listen_with(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(
            Rc::new(move |_: &AsyncError| error_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        None,
        ListenOptions {
            cancel_on_error: true,
        },
    );
    runtime.add(&source, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&source, StreamEvent::Data(int(1)));
    assert_eq!(*events.borrow(), vec!["error"]);
}

// Paused pipe subscriptions buffer upstream events and forward them in order
// after the last matching resume (`stream_impl.dart:204-229`,
// `stream_pipe.dart:164-170`).
#[test]
fn paused_pipe_buffers_and_forwards_in_order_after_resume() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let mapped = source.map(Rc::new(|value| value));
    let events = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let events_clone = Rc::clone(&events);
    let subscription = mapped.listen(
        Some(Rc::new(move |value| events_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    subscription.pause();
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Data(int(2)));
    assert!(events.borrow().is_empty());
    subscription.resume();
    assert_eq!(*events.borrow(), vec![int(1), int(2)]);
    runtime.add(&source, StreamEvent::Data(int(3)));
    assert_eq!(*events.borrow(), vec![int(1), int(2), int(3)]);
}

#[test]
fn nested_pause_on_pipe_resumes_upstream_only_after_last_resume() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let mapped = source.map(Rc::new(|value| value));
    let events = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let events_clone = Rc::clone(&events);
    let subscription = mapped.listen(
        Some(Rc::new(move |value| events_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    subscription.pause();
    subscription.pause();
    runtime.add(&source, StreamEvent::Data(int(1)));
    subscription.resume();
    assert!(events.borrow().is_empty());
    subscription.resume();
    assert_eq!(*events.borrow(), vec![int(1)]);
}

#[test]
fn paused_pipe_cancel_on_error_reports_buffered_error_before_cancel() {
    let runtime = StreamRuntime::new();
    let source = runtime.broadcast_controller(StreamHooks::default());
    let mapped = source.map(Rc::new(|value| value));
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let subscription = mapped.listen_with(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(
            Rc::new(move |_: &AsyncError| error_events.borrow_mut().push("error".to_owned()))
                as ErrorCallback,
        ),
        None,
        ListenOptions {
            cancel_on_error: true,
        },
    );
    subscription.pause();
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&source, StreamEvent::Data(int(2)));
    subscription.resume();
    runtime.add(&source, StreamEvent::Data(int(3)));
    assert_eq!(*events.borrow(), vec!["data", "error"]);
}

// Cancellation and done ordering, pinned to Dart 3.14
// (async/stream_impl.dart:69-76,232-240,386-445).
#[test]
fn cancel_from_error_callback_suppresses_done() {
    // stream_impl.dart:392-395,429-432: an explicit cancel during the error
    // callback clears WAIT_FOR_CANCEL, so the later done must be suppressed.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    let subscription: Rc<RefCell<Option<opurom_core::StreamSubscription>>> =
        Rc::new(RefCell::new(None));
    let subscription_clone = Rc::clone(&subscription);
    *subscription.borrow_mut() = Some(stream.listen_with(
        None,
        Some(Rc::new(move |error: &AsyncError| {
            error_events.borrow_mut().push(error.message.clone());
            if let Some(subscription) = subscription_clone.borrow().as_ref() {
                subscription.cancel();
            }
        }) as ErrorCallback),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
        ListenOptions::default(),
    ));
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["boom"]);
}

#[test]
fn cancel_on_error_closes_after_error_without_done() {
    // stream_impl.dart:407-421,467-483: cancelOnError reports the error once
    // and cancels the subscription; a following done is dropped.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    stream.listen_with(
        None,
        Some(Rc::new(move |error: &AsyncError| {
            error_events
                .borrow_mut()
                .push(format!("error:{}", error.message))
        }) as ErrorCallback),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
        ListenOptions {
            cancel_on_error: true,
        },
    );
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    runtime.add(&stream, StreamEvent::Done);
    runtime.add(&stream, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["error:boom"]);
}

// StreamSubscription.asFuture, pinned to Dart 3.14
// (async/stream_impl.dart:243-270).
#[test]
fn as_future_completes_with_value_on_done() {
    let runtime = StreamRuntime::new();
    let future_runtime = FutureRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let subscription = stream.listen(None, None, None);
    let future = subscription.as_future(&future_runtime, int(42));
    assert!(!future.is_completed());
    runtime.add(&stream, StreamEvent::Done);
    future_runtime.pump(16);
    assert_eq!(future.completion(), Some(Completion::Value(int(42))));
}

#[test]
fn as_future_cancels_then_completes_with_error() {
    let runtime = StreamRuntime::new();
    let future_runtime = FutureRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let subscription = stream.listen(None, None, None);
    let future = subscription.as_future(&future_runtime, int(0));
    let original = AsyncError::new("boom");
    runtime.add(&stream, StreamEvent::Error(original.clone()));
    future_runtime.pump(16);
    assert_eq!(
        future.completion(),
        Some(Completion::Error(original.clone()))
    );
    assert!(future_runtime.unhandled_errors().is_empty());
    runtime.add(&stream, StreamEvent::Done);
    future_runtime.pump(16);
    assert_eq!(future.completion(), Some(Completion::Error(original)));
}

#[test]
fn as_future_error_preserves_stack_trace() {
    let runtime = StreamRuntime::new();
    let future_runtime = FutureRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let subscription = stream.listen(None, None, None);
    let future = subscription.as_future(&future_runtime, int(0));
    let original = AsyncError {
        message: "boom".to_owned(),
        stack_trace: "frame-a\nframe-b".to_owned(),
    };
    runtime.add(&stream, StreamEvent::Error(original.clone()));
    future_runtime.pump(16);
    assert_eq!(future.completion(), Some(Completion::Error(original)));
}

// asFuture independence across listeners, pinned to Dart 3.14
// (async/stream_impl.dart:243-270, stream_controller.dart:141-143).
#[test]
fn as_future_is_independent_per_listener() {
    let runtime = StreamRuntime::new();
    let future_runtime = FutureRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let first_subscription = stream.listen(None, None, None);
    let second_subscription = stream.listen(None, None, None);
    let first = first_subscription.as_future(&future_runtime, int(1));
    let second = second_subscription.as_future(&future_runtime, int(2));
    runtime.add(&stream, StreamEvent::Data(int(9)));
    runtime.add(&stream, StreamEvent::Done);
    future_runtime.pump(16);
    assert_eq!(first.completion(), Some(Completion::Value(int(1))));
    assert_eq!(second.completion(), Some(Completion::Value(int(2))));
}

#[test]
fn as_future_error_completes_only_its_own_future() {
    let runtime = StreamRuntime::new();
    let future_runtime = FutureRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let first_subscription = stream.listen(None, None, None);
    let second_subscription = stream.listen(None, None, None);
    let first = first_subscription.as_future(&future_runtime, int(1));
    let _second = second_subscription.as_future(&future_runtime, int(2));
    runtime.add(&stream, StreamEvent::Error(AsyncError::new("boom")));
    future_runtime.pump(16);
    assert_eq!(
        first.completion(),
        Some(Completion::Error(AsyncError::new("boom")))
    );
    assert!(first_subscription.id() != second_subscription.id());
    assert!(future_runtime.unhandled_errors().is_empty());
}

// Single-subscription protocol boundary, pinned to Dart 3.14
// (async/stream_controller.dart:440-452,509-519,688-696).
#[test]
fn single_subscription_rejects_a_second_listen() {
    // stream_controller.dart:688-696: a single-subscription controller
    // accepts one listener only.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    let _first = stream.listen(None, None, None);
    let error = stream.try_listen_with(None, None, None, ListenOptions::default());
    assert_eq!(
        error.err(),
        Some(DartError::State(
            "Stream has already been listened to.".to_owned()
        ))
    );
}

#[test]
fn single_subscription_rejects_a_listen_after_cancel() {
    // stream_controller.dart:509-519: after a listener was canceled the
    // controller is no longer in its initial state, so a later listen fails.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    let first = stream.listen(None, None, None);
    first.cancel();
    let error = stream.try_listen_with(None, None, None, ListenOptions::default());
    assert!(matches!(error, Err(DartError::State(_))));
}

#[test]
fn broadcast_accepts_many_listeners() {
    // stream_controller.dart:266-275: a broadcast controller keeps one
    // subscription per listener and never rejects a later listen.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let first = stream.try_listen_with(None, None, None, ListenOptions::default());
    let second = stream.try_listen_with(None, None, None, ListenOptions::default());
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_ne!(first.unwrap().id(), second.unwrap().id());
}

#[test]
fn pipe_over_an_already_listened_source_reports_the_refusal() {
    // stream_controller.dart:688-696: a pipe subscribes to its source, so a
    // source that already has a listener refuses the pipe's subscription.
    let runtime = StreamRuntime::new();
    let source = runtime.controller(StreamHooks::default());
    let _first = source.listen(None, None, None);
    let mapped = source.map(Rc::new(|value| value));
    let error = mapped.try_listen_with(None, None, None, ListenOptions::default());
    assert!(matches!(error, Err(DartError::State(_))));
}

// Close boundary, pinned to Dart 3.14
// (async/stream_controller.dart:581-587,628-656,688-696,
//  async/broadcast_stream_controller.dart:201-209,247-257).
#[test]
fn single_subscription_add_after_close_is_a_state_error() {
    // stream_controller.dart:581-587,616-618: a single-subscription
    // controller refuses events after close.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Done);
    let error = runtime.try_add(&stream, StreamEvent::Data(int(1)));
    assert_eq!(
        error.err(),
        Some(DartError::State(
            "Cannot add event after closing".to_owned()
        ))
    );
}

#[test]
fn closing_twice_is_allowed() {
    // stream_controller.dart:628-656: close may be called more than once;
    // only the first call has an effect.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    let done_count = Rc::new(RefCell::new(0));
    let done_clone = Rc::clone(&done_count);
    stream.listen(
        None,
        None,
        Some(Rc::new(move || *done_clone.borrow_mut() += 1) as DoneCallback),
    );
    runtime.add(&stream, StreamEvent::Done);
    assert!(runtime.try_add(&stream, StreamEvent::Done).is_ok());
    assert_eq!(*done_count.borrow(), 1);
}

#[test]
fn listen_after_close_receives_only_done() {
    // broadcast_stream_controller.dart:201-209: a closed broadcast
    // controller hands out a done-only subscription.
    let runtime = StreamRuntime::new();
    let stream = runtime.broadcast_controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Done);
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    let subscription = stream.try_listen_with(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        None,
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
        ListenOptions::default(),
    );
    assert!(subscription.is_ok());
    assert_eq!(*events.borrow(), vec!["done"]);
}

#[test]
fn single_subscription_listen_after_close_still_gets_done() {
    // stream_controller.dart:649-656: closing without a listener queues a
    // pending done event that the first listener receives.
    let runtime = StreamRuntime::new();
    let stream = runtime.controller(StreamHooks::default());
    runtime.add(&stream, StreamEvent::Done);
    let done_count = Rc::new(RefCell::new(0));
    let done_clone = Rc::clone(&done_count);
    let subscription = stream.try_listen_with(
        None,
        None,
        Some(Rc::new(move || *done_clone.borrow_mut() += 1) as DoneCallback),
        ListenOptions::default(),
    );
    assert!(subscription.is_ok());
    assert_eq!(*done_count.borrow(), 1);
}

// StreamController.addStream ordering, pinned to Dart 3.14
// (async/stream_controller.dart:590-603,913-938,
//  async/broadcast_stream_controller.dart:280-303).
#[test]
fn add_stream_forwards_source_order_then_closes() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let done_count = Rc::new(RefCell::new(0));
    let seen_clone = Rc::clone(&seen);
    let done_clone = Rc::clone(&done_count);
    target.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        Some(Rc::new(move || *done_clone.borrow_mut() += 1) as DoneCallback),
    );
    let future = runtime.add_stream(&target, &source, false);
    assert!(future.is_ok());
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Data(int(2)));
    runtime.add(&source, StreamEvent::Done);
    assert_eq!(*seen.borrow(), vec![int(1), int(2)]);
    assert_eq!(*done_count.borrow(), 1);
}

#[test]
fn add_stream_cancel_on_error_forwards_first_error_then_closes() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let data_events = Rc::clone(&events);
    let error_events = Rc::clone(&events);
    let done_events = Rc::clone(&events);
    target.listen(
        Some(Rc::new(move |_| data_events.borrow_mut().push("data".to_owned())) as DataCallback),
        Some(Rc::new(move |error: &AsyncError| {
            error_events.borrow_mut().push(error.message.clone())
        }) as ErrorCallback),
        Some(Rc::new(move || done_events.borrow_mut().push("done".to_owned())) as DoneCallback),
    );
    let future = runtime.add_stream(&target, &source, true);
    assert!(future.is_ok());
    runtime.add(&source, StreamEvent::Error(AsyncError::new("first")));
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Error(AsyncError::new("second")));
    assert_eq!(*events.borrow(), vec!["first", "done"]);
}

#[test]
fn add_stream_without_cancel_forwards_all_errors() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let errors = Rc::new(RefCell::new(Vec::<String>::new()));
    let errors_clone = Rc::clone(&errors);
    target.listen(
        None,
        Some(Rc::new(move |error: &AsyncError| {
            errors_clone.borrow_mut().push(error.message.clone())
        }) as ErrorCallback),
        None,
    );
    let _ = runtime.add_stream(&target, &source, false);
    runtime.add(&source, StreamEvent::Error(AsyncError::new("first")));
    runtime.add(&source, StreamEvent::Error(AsyncError::new("second")));
    assert_eq!(*errors.borrow(), vec!["first", "second"]);
}

#[test]
fn add_stream_after_close_is_rejected() {
    // stream_controller.dart:581-587: addStream refuses a closed controller.
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    runtime.add(&target, StreamEvent::Done);
    let error = runtime.add_stream(&target, &source, false);
    assert!(matches!(error, Err(DartError::State(_))));
}

#[test]
fn direct_add_during_add_stream_is_rejected() {
    // stream_controller.dart:528-552: while addStream is active, a direct
    // add is a state error and never reaches target listeners.
    let runtime = StreamRuntime::new();
    let target = runtime.controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let seen_clone = Rc::clone(&seen);
    target.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    let _ = runtime.add_stream(&target, &source, false);
    let error = runtime.try_add(&target, StreamEvent::Data(int(99)));
    assert_eq!(
        error.err(),
        Some(DartError::State(
            "Cannot add event while adding a stream".to_owned()
        ))
    );
    runtime.add(&source, StreamEvent::Data(int(1)));
    assert_eq!(*seen.borrow(), vec![int(1)]);
}

#[test]
fn paused_target_pauses_the_add_stream_source() {
    // stream_controller.dart:970-986: a paused single-subscription controller
    // pauses its addStream subscription until resumed.
    let runtime = StreamRuntime::new();
    let target = runtime.controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let seen = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let seen_clone = Rc::clone(&seen);
    let subscription = target.listen(
        Some(Rc::new(move |value| seen_clone.borrow_mut().push(value)) as DataCallback),
        None,
        None,
    );
    let _ = runtime.add_stream(&target, &source, false);
    subscription.pause();
    runtime.add(&source, StreamEvent::Data(int(1)));
    assert!(seen.borrow().is_empty());
    subscription.resume();
    assert_eq!(*seen.borrow(), vec![int(1)]);
}

// Broadcast addStream snapshot and close ordering, pinned to Dart 3.14
// (async/broadcast_stream_controller.dart:247-276,266-303,404-413).
#[test]
fn broadcast_add_stream_delivers_to_listeners_subscribed_before_source_event() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let first = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let second = Rc::new(RefCell::new(Vec::<FutureValue>::new()));
    let first_clone = Rc::clone(&first);
    let second_clone = Rc::clone(&second);
    target.listen(
        Some(Rc::new(move |v| first_clone.borrow_mut().push(v)) as DataCallback),
        None,
        None,
    );
    let _ = runtime.add_stream(&target, &source, false);
    target.listen(
        Some(Rc::new(move |v| second_clone.borrow_mut().push(v)) as DataCallback),
        None,
        None,
    );
    runtime.add(&source, StreamEvent::Data(int(1)));
    // The first listener was present when addStream subscribed, so it forwards
    // the source event through the controller; the later listener is still a
    // current target listener and receives the same forwarded event.
    assert_eq!(*first.borrow(), vec![int(1)]);
    assert_eq!(*second.borrow(), vec![int(1)]);
}

#[test]
fn broadcast_direct_add_during_add_stream_has_official_message() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let _ = runtime.add_stream(&target, &source, false);
    let error = runtime.try_add(&target, StreamEvent::Data(int(1)));
    assert_eq!(
        error.err(),
        Some(DartError::State(
            "Cannot add new events while doing an addStream".to_owned()
        ))
    );
}

#[test]
fn broadcast_close_during_add_stream_is_refused() {
    let runtime = StreamRuntime::new();
    let target = runtime.broadcast_controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let _ = runtime.add_stream(&target, &source, false);
    let error = runtime.try_close(&target);
    assert!(matches!(error, Err(DartError::State(_))));
    runtime.add(&source, StreamEvent::Done);
    assert!(runtime.try_close(&target).is_ok());
}

#[test]
fn single_subscription_add_stream_completion_future_order() {
    let runtime = StreamRuntime::new();
    let target = runtime.controller(StreamHooks::default());
    let source = runtime.broadcast_controller(StreamHooks::default());
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let done_events = Rc::clone(&events);
    target.listen(
        None,
        None,
        Some(
            Rc::new(move || done_events.borrow_mut().push("target-done".to_owned()))
                as DoneCallback,
        ),
    );
    let future = runtime.add_stream(&target, &source, false).unwrap();
    assert!(!future.is_completed());
    runtime.add(&source, StreamEvent::Data(int(1)));
    runtime.add(&source, StreamEvent::Done);
    assert_eq!(*events.borrow(), vec!["target-done"]);
    // The addStream future is completed after source done is forwarded
    // (stream_controller.dart:965-967); completion is observable even before
    // the host drains the microtask queue in this data model.
    assert_eq!(
        future.completion(),
        Some(Completion::Value(FutureValue::Null))
    );
}
