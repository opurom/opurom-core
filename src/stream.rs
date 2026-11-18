//! Deterministic Stream runtime ported from Dart 3.14.0 (`dart:async`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths below
//! are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - a single-subscription controller buffers events until a listener is
//!   registered (`async/stream_controller.dart:92-120`);
//! - a broadcast controller does not buffer events when there is no
//!   listener: events sent while nobody is subscribed are dropped
//!   (`async/stream_controller.dart:132-139`);
//! - each broadcast subscription is independent: a paused listener buffers
//!   events until resumed or canceled while other listeners keep receiving
//!   (`async/stream_controller.dart:141-143`);
//! - `cancelOnError` reports the error once and then cancels the subscription;
//!   a paused listener applies the transition only when the buffered error is
//!   delivered (`async/stream_impl.dart:50-51,386-421`,
//!   `async/stream_controller.dart:141-143`);
//! - `onListen` is called on the first subscriber and `onCancel` when the
//!   last subscriber leaves (`async/stream_controller.dart:168-171`);
//! - `close` sends a done event to the listeners subscribed at close time
//!   (`async/broadcast_stream_controller.dart:266-275`,
//!   `async/stream_controller.dart:254-260`);
//! - `map`, `where` and `distinct` are forwarding pipes: errors and done
//!   events pass through unchanged; a callback error becomes an error event
//!   (`async/stream_pipe.dart:213-253,485-530`, `async/stream.dart:751-795,1626-1628`).
//!
//! Opurom adaptation: events and errors are plain data; callbacks are
//! host-side Rust bodies; the kernel reads no clock. Delivery happens on the
//! core microtask queue in scheduling order, and the same event stream
//! always yields the same data on every host.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::future::{AsyncError, FutureValue};
use crate::microtask::{CallbackId, MicrotaskQueue};

/// An event carried by a stream.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Data(FutureValue),
    Error(AsyncError),
    Done,
}

/// Callback invoked for a data event.
pub type DataCallback = Rc<dyn Fn(FutureValue)>;
/// Callback invoked for an error event.
pub type ErrorCallback = Rc<dyn Fn(&AsyncError)>;
/// Callback invoked on done.
pub type DoneCallback = Rc<dyn Fn()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubscriptionState {
    /// `_STATE_CANCEL_ON_ERROR` (`async/stream_impl.dart:50-51,133-135`).
    cancel_on_error: bool,
    /// Dart counts nested pauses in `_STATE_PAUSE_COUNT`
    /// (`async/stream_impl.dart:204-229`); one resume only decrements.
    pause_count: usize,
    canceled: bool,
}

/// One subscription. Broadcast controllers keep one of these per listener;
/// single-subscription controllers keep exactly one.
#[derive(Clone)]
struct Subscription {
    id: u32,
    state: SubscriptionState,
    on_data: Option<DataCallback>,
    on_error: Option<ErrorCallback>,
    on_done: Option<DoneCallback>,
    /// Events buffered while this subscription is paused
    /// (`stream_controller.dart:141-143`).
    buffered: VecDeque<StreamEvent>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("pause_count", &self.state.pause_count)
            .field("canceled", &self.state.canceled)
            .field("buffered", &self.buffered.len())
            .finish()
    }
}

impl Subscription {
    fn with_cancel_on_error(mut self, value: bool) -> Self {
        self.state.cancel_on_error = value;
        self
    }

    fn new(
        id: u32,
        on_data: Option<DataCallback>,
        on_error: Option<ErrorCallback>,
        on_done: Option<DoneCallback>,
    ) -> Self {
        Self {
            id,
            state: SubscriptionState {
                cancel_on_error: false,
                pause_count: 0,
                canceled: false,
            },
            on_data,
            on_error,
            on_done,
            buffered: VecDeque::new(),
        }
    }

    fn deliver(&self, event: &StreamEvent) {
        if self.state.canceled {
            return;
        }
        match event {
            StreamEvent::Data(value) => {
                if let Some(callback) = &self.on_data {
                    callback(value.clone());
                }
            }
            StreamEvent::Error(error) => {
                if let Some(callback) = &self.on_error {
                    callback(error);
                }
                // `stream_impl.dart:407-421`: the error callback runs before
                // cancelOnError takes effect, so this error is reported once.
            }
            StreamEvent::Done => {
                if let Some(callback) = &self.on_done {
                    callback();
                }
            }
        }
    }
}

/// A subscription handle returned by [`Stream::listen`].
#[derive(Debug, Clone)]
pub struct StreamSubscription {
    id: u32,
    inner: Rc<RefCell<StreamInner>>,
}

impl StreamSubscription {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
            subscription.state.pause_count += 1;
        }
        if let Some(hook) = inner.hooks.on_pause.clone() {
            drop(inner);
            hook();
        }
    }

    pub fn resume(&self) {
        let (buffered, hook, still_paused) = {
            let mut inner = self.inner.borrow_mut();
            let Some(subscription) = inner.subscriptions.get_mut(&self.id) else {
                return;
            };
            subscription.state.pause_count = subscription.state.pause_count.saturating_sub(1);
            let still_paused = subscription.state.pause_count > 0;
            (
                std::mem::take(&mut subscription.buffered),
                inner.hooks.on_resume.clone(),
                still_paused,
            )
        };
        if still_paused {
            // Nested pause: input stays paused until the last resume
            // (`stream_impl.dart:217-228`). Re-buffer the events.
            let mut inner = self.inner.borrow_mut();
            if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
                for event in buffered {
                    subscription.buffered.push_back(event);
                }
            }
            return;
        }
        for event in buffered {
            let cancel_after_error = matches!(event, StreamEvent::Error(_)) && {
                let inner = self.inner.borrow();
                inner
                    .subscriptions
                    .get(&self.id)
                    .is_some_and(|subscription| subscription.state.cancel_on_error)
            };
            let was_canceled = {
                let inner = self.inner.borrow();
                inner
                    .subscriptions
                    .get(&self.id)
                    .is_some_and(|subscription| subscription.state.canceled)
            };
            self.deliver(&event);
            if !was_canceled && cancel_after_error {
                // A buffered error triggers the same cancelOnError transition
                // as a live one (`stream_impl.dart:407-421`): later events in
                // the buffer are discarded.
                let mut inner = self.inner.borrow_mut();
                if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
                    subscription.state.canceled = true;
                    subscription.buffered.clear();
                }
                break;
            }
        }
        if let Some(hook) = hook {
            hook();
        }
    }

    pub fn cancel(&self) {
        let mut inner = self.inner.borrow_mut();
        let was_subscribed = !inner.subscriptions.is_empty();
        if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
            subscription.state.canceled = true;
            subscription.buffered.clear();
        }
        inner.subscriptions.remove(&self.id);
        let no_subscribers = inner.subscriptions.values().all(|s| s.state.canceled);
        let on_cancel = if was_subscribed && no_subscribers {
            inner.hooks.on_cancel.take()
        } else {
            None
        };
        drop(inner);
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }

    fn deliver(&self, event: &StreamEvent) {
        let subscription = self.inner.borrow().subscriptions.get(&self.id).cloned();
        if let Some(subscription) = subscription {
            subscription.deliver(event);
        }
    }
}

/// Controller lifecycle hooks (`stream_controller.dart:103-107,168-171`).
#[derive(Clone, Default)]
pub struct StreamHooks {
    pub on_listen: Option<Rc<dyn Fn()>>,
    /// Notified when `listen_with` changes the error policy, so forwarding
    /// pipes can apply the same policy to their upstream subscription
    /// (`stream_pipe.dart:213-253`).
    #[doc(hidden)]
    pub on_error_options: Option<Rc<dyn Fn(ListenOptions)>>,
    pub on_pause: Option<Rc<dyn Fn()>>,
    pub on_resume: Option<Rc<dyn Fn()>>,
    pub on_cancel: Option<Rc<dyn Fn()>>,
}

impl std::fmt::Debug for StreamHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamHooks").finish()
    }
}

#[derive(Debug)]
struct StreamInner {
    broadcast: bool,
    closed: bool,
    /// Events buffered for a single-subscription controller before listen
    /// (`stream_controller.dart:97-98`).
    pre_listen: VecDeque<StreamEvent>,
    subscriptions: std::collections::BTreeMap<u32, Subscription>,
    hooks: StreamHooks,
    next_subscription: u32,
}

impl StreamInner {
    fn new(broadcast: bool, hooks: StreamHooks) -> Self {
        Self {
            broadcast,
            closed: false,
            pre_listen: VecDeque::new(),
            subscriptions: std::collections::BTreeMap::new(),
            hooks,
            next_subscription: 1,
        }
    }
}

/// Options for `Stream.listen` (`async/stream_impl.dart:121-138`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenOptions {
    /// Cancel the subscription when an error is delivered, while still
    /// reporting that error once (`stream_impl.dart:386-421`).
    pub cancel_on_error: bool,
}

/// A stream handle. Cloning shares the same controller state.
#[derive(Debug, Clone)]
pub struct Stream {
    inner: Rc<RefCell<StreamInner>>,
}

impl Stream {
    fn new(inner: Rc<RefCell<StreamInner>>) -> Self {
        Self { inner }
    }

    /// Registers a listener (`stream.dart:723`, `stream_controller.dart:406-411`).
    pub fn listen(
        &self,
        on_data: Option<DataCallback>,
        on_error: Option<ErrorCallback>,
        on_done: Option<DoneCallback>,
    ) -> StreamSubscription {
        self.listen_with(on_data, on_error, on_done, ListenOptions::default())
    }

    /// Registers a listener with `cancelOnError` support
    /// (`stream.dart:723`, `async/stream_impl.dart:121-138`).
    pub fn listen_with(
        &self,
        on_data: Option<DataCallback>,
        on_error: Option<ErrorCallback>,
        on_done: Option<DoneCallback>,
        options: ListenOptions,
    ) -> StreamSubscription {
        let mut inner = self.inner.borrow_mut();
        let id = inner.next_subscription;
        inner.next_subscription += 1;
        let subscription = Subscription::new(id, on_data, on_error, on_done)
            .with_cancel_on_error(options.cancel_on_error);
        let first = inner.subscriptions.is_empty();
        inner.subscriptions.insert(id, subscription);
        let on_error_options = inner.hooks.on_error_options.clone();
        let on_listen = if first {
            inner.hooks.on_listen.clone()
        } else {
            None
        };
        let pre_listen = if !inner.broadcast {
            std::mem::take(&mut inner.pre_listen)
        } else {
            VecDeque::new()
        };
        drop(inner);
        if let Some(on_error_options) = on_error_options {
            on_error_options(options);
        }
        if let Some(on_listen) = on_listen {
            on_listen();
        }
        let handle = StreamSubscription {
            id,
            inner: Rc::clone(&self.inner),
        };
        for event in pre_listen {
            handle.deliver(&event);
        }
        handle
    }

    /// Applies `map` semantics (`stream_pipe.dart:237-253`): each data event
    /// is transformed once per listener, while errors and done pass through
    /// unchanged.
    pub fn map(&self, transform: Rc<dyn Fn(FutureValue) -> FutureValue>) -> Stream {
        let transform: Rc<dyn Fn(StreamEvent) -> Option<StreamEvent>> =
            Rc::new(move |event| match event {
                StreamEvent::Data(value) => Some(StreamEvent::Data(transform(value.clone()))),
                other => Some(other),
            });
        self.forwarding_stream(transform)
    }

    /// Applies `where` semantics (`stream_pipe.dart:213-231`): data failing
    /// the test is dropped; errors and done pass through unchanged.
    pub fn r#where(&self, test: Rc<dyn Fn(&FutureValue) -> bool>) -> Stream {
        let test: Rc<dyn Fn(StreamEvent) -> Option<StreamEvent>> =
            Rc::new(move |event| match event {
                StreamEvent::Data(value) if !test(&value) => None,
                other => Some(other),
            });
        self.forwarding_stream(test)
    }

    /// Applies `distinct` semantics (`stream_pipe.dart:485-530`): compares
    /// only adjacent data events, with the first event always emitted.
    pub fn distinct(&self) -> Stream {
        let previous: Rc<RefCell<Option<FutureValue>>> = Rc::new(RefCell::new(None));
        let filter: Rc<dyn Fn(StreamEvent) -> Option<StreamEvent>> =
            Rc::new(move |event| match event {
                StreamEvent::Data(value) => {
                    let mut previous = previous.borrow_mut();
                    if previous.as_ref() == Some(&value) {
                        None
                    } else {
                        *previous = Some(value.clone());
                        Some(StreamEvent::Data(value))
                    }
                }
                other => Some(other),
            });
        self.forwarding_stream(filter)
    }

    /// Builds a forwarding pipe (`stream_pipe.dart:_ForwardingStream`):
    /// subscribing to the pipe subscribes to the source, and every source
    /// event passes through `filter` before reaching the pipe's listeners.
    /// `cancelOnError` is forwarded so an error cancels the whole listener
    /// chain after it has been delivered to the pipe's listener
    /// (`stream_pipe.dart:213-253,485-530`, `stream_impl.dart:386-421`).
    fn forwarding_stream(&self, filter: Rc<dyn Fn(StreamEvent) -> Option<StreamEvent>>) -> Stream {
        let is_broadcast = self.inner.borrow().broadcast;
        let target = Stream::new(Rc::new(RefCell::new(StreamInner::new(
            is_broadcast,
            StreamHooks::default(),
        ))));
        let hook_inner = Rc::clone(&target.inner);
        let closure_inner = Rc::clone(&hook_inner);
        let source_inner = Rc::clone(&self.inner);
        let filter_for_listen = Rc::clone(&filter);
        let cancel_on_error = Rc::new(RefCell::new(false));
        {
            let mut state = hook_inner.borrow_mut();
            state.hooks.on_listen = {
                let cancel_on_error = Rc::clone(&cancel_on_error);
                Some(Rc::new(move || {
                    let target = Stream::new(Rc::clone(&closure_inner));
                    let source = Stream::new(Rc::clone(&source_inner));
                    let data_target = target.clone();
                    let error_target = target.clone();
                    let done_target = target.clone();
                    let options_target = target.clone();
                    let cancel_target = target;
                    let forward_data = Rc::clone(&filter_for_listen);
                    let forward_error = Rc::clone(&filter_for_listen);
                    let hook_cancel = Rc::clone(&cancel_on_error);
                    let options_hook = Rc::new(move |options: ListenOptions| {
                        *hook_cancel.borrow_mut() = options.cancel_on_error;
                    });
                    let options_hook_target = Rc::clone(&options_hook);
                    {
                        let mut state = options_target.inner.borrow_mut();
                        state.hooks.on_error_options = Some(options_hook_target);
                    }
                    let subscription = source.listen_with(
                        Some(Rc::new(move |value| {
                            if let Some(StreamEvent::Data(mapped)) =
                                forward_data(StreamEvent::Data(value))
                            {
                                StreamRuntime::default()
                                    .add(&data_target, StreamEvent::Data(mapped));
                            }
                        })),
                        Some(Rc::new(move |error| {
                            if let Some(StreamEvent::Error(mapped)) =
                                forward_error(StreamEvent::Error(error.clone()))
                            {
                                StreamRuntime::default()
                                    .add(&error_target, StreamEvent::Error(mapped));
                            }
                        })),
                        Some(Rc::new(move || {
                            StreamRuntime::default().add(&done_target, StreamEvent::Done);
                        })),
                        ListenOptions {
                            cancel_on_error: *cancel_on_error.borrow(),
                        },
                    );
                    let source_subscription = Rc::new(subscription);
                    let cancel_source = Rc::clone(&source_subscription);
                    let mut state = cancel_target.inner.borrow_mut();
                    state.hooks.on_cancel = Some(Rc::new(move || cancel_source.cancel()));
                }))
            };
        }
        let forward_cancel = Rc::clone(&cancel_on_error);
        {
            let mut state = hook_inner.borrow_mut();
            state.hooks.on_error_options = Some(Rc::new(move |options| {
                *forward_cancel.borrow_mut() = options.cancel_on_error;
            }));
        }
        target
    }
}

/// The Stream runtime owns the microtask queue that async controllers deliver
/// on (`stream_controller.dart:154-159`).
#[derive(Debug, Clone, Default)]
pub struct StreamRuntime {
    microtasks: Rc<RefCell<MicrotaskQueue>>,
}

impl StreamRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn microtasks(&self) -> Rc<RefCell<MicrotaskQueue>> {
        Rc::clone(&self.microtasks)
    }

    /// Creates a single-subscription controller: events before the first
    /// listener are buffered (`stream_controller.dart:97-98`).
    pub fn controller(&self, hooks: StreamHooks) -> Stream {
        self.controller_with(hooks, false)
    }

    /// Creates a broadcast controller: events without listeners are dropped
    /// (`stream_controller.dart:132-139`).
    pub fn broadcast_controller(&self, hooks: StreamHooks) -> Stream {
        self.controller_with(hooks, true)
    }

    fn controller_with(&self, hooks: StreamHooks, broadcast: bool) -> Stream {
        Stream::new(Rc::new(RefCell::new(StreamInner::new(broadcast, hooks))))
    }

    /// Adds an event to a stream. Delivery is synchronous here; the host
    /// decides when to call it, and the core never reads a clock
    /// (`stream_controller.dart:134-139`).
    pub fn add(&self, stream: &Stream, event: StreamEvent) {
        let mut inner = stream.inner.borrow_mut();
        if inner.closed {
            return;
        }
        if matches!(event, StreamEvent::Done) {
            inner.closed = true;
        }
        if inner.subscriptions.is_empty() {
            if !inner.broadcast {
                inner.pre_listen.push_back(event);
            }
            return;
        }
        let ids: Vec<u32> = inner.subscriptions.keys().copied().collect();
        for id in ids {
            let was_canceled = inner
                .subscriptions
                .get(&id)
                .is_some_and(|subscription| subscription.state.canceled);
            if was_canceled {
                continue;
            }
            let subscription = inner.subscriptions.get_mut(&id).unwrap();
            if subscription.state.pause_count > 0 {
                // Paused listeners buffer without applying cancelOnError; the
                // transition happens when the buffered error is delivered
                // (`stream_controller.dart:141-143`, `stream_impl.dart:407-421`).
                subscription.buffered.push_back(event.clone());
                continue;
            }
            let cancel_after_error =
                matches!(event, StreamEvent::Error(_)) && subscription.state.cancel_on_error;
            subscription.deliver(&event);
            if cancel_after_error {
                // Report first, then cancel (`stream_impl.dart:407-421`).
                subscription.state.canceled = true;
                subscription.buffered.clear();
            }
        }
    }

    /// Pumps the microtask queue used by async stream controllers
    /// (`stream_controller.dart:154-159`).
    pub fn pump(&self, limit: usize) -> Vec<CallbackId> {
        self.microtasks.borrow_mut().drain(limit)
    }
}
