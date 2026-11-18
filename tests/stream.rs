//! Regression tests for the Dart 3.14 Stream runtime, pinned to
//! `flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb
//! (Dart SDK 3.14.0).

use opurom_core::{
    AsyncError, DataCallback, DoneCallback, ErrorCallback, FutureValue, StreamEvent, StreamHooks,
    StreamRuntime,
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
fn closed_stream_ignores_new_events() {
    // broadcast_stream_controller.dart:266-275: close marks the controller
    // closed and sends done to current listeners.
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
    runtime.add(&stream, StreamEvent::Data(int(1)));
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
