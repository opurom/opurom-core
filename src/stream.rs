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
//! - during an error or done callback a terminal event waits for cancellation;
//!   an explicit cancel clears that state and suppresses the pending done
//!   (`async/stream_impl.dart:69-76,392-395,429-445`);
//! - `asFuture` overwrites the terminal handlers: done completes with a
//!   supplied value, error cancels first and then completes with the original
//!   error and stack trace (`async/stream_impl.dart:243-270`);
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

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::future::{AsyncError, Future, FutureRuntime, FutureValue};
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
    /// `_STATE_WAIT_FOR_CANCEL`: the terminal event is awaiting the cancel
    /// future and must be discarded if the listener cancels meanwhile
    /// (`async/stream_impl.dart:69-76,392-395,429-445`).
    waiting_for_cancel: bool,
    /// A done event has already been delivered; later events are dropped
    /// (`stream_impl.dart:53-55,424-445`).
    closed: bool,
    /// Set when `cancel()` is called while the controller is still delivering
    /// a callback. Dart defers such cancellation until the callback returns
    /// (`stream_impl.dart:407-421`); the kernel finalizes it at the end of
    /// the delivery step (`stream_impl.dart:69-76`).
    pending_cancel: Rc<Cell<bool>>,
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
            waiting_for_cancel: false,
            closed: false,
            pending_cancel: Rc::new(Cell::new(false)),
            on_data,
            on_error,
            on_done,
            buffered: VecDeque::new(),
        }
    }

    /// Delivers one event and reports whether `cancelOnError` must cancel the
    /// subscription afterwards. `_sendError` sets the wait-for-cancel state
    /// before invoking the handler but still reports the error exactly once
    /// (`stream_impl.dart:386-421`); the caller applies the state change so
    /// this method can keep taking `&self`.
    fn deliver(&self, event: &StreamEvent) -> bool {
        // `_sendDone` runs its callback while WAIT_FOR_CANCEL is set
        // (`stream_impl.dart:429-435`); other canceled subscriptions drop
        // events (`stream_impl.dart:308-325`).
        if self.state.canceled && !self.waiting_for_cancel {
            return false;
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
            }
            StreamEvent::Done => {
                // `sendDone` reports only while WAIT_FOR_CANCEL is set
                // (`stream_impl.dart:429-435`), and only once.
                if self.closed || self.state.canceled && !self.waiting_for_cancel {
                    return false;
                }
                if let Some(callback) = &self.on_done {
                    callback();
                }
            }
        }
        matches!(event, StreamEvent::Error(_)) && self.state.cancel_on_error
    }
}

/// A subscription handle returned by [`Stream::listen`].
#[derive(Debug, Clone)]
pub struct StreamSubscription {
    id: u32,
    inner: Rc<RefCell<StreamInner>>,
    pending_cancel: Rc<Cell<bool>>,
}

impl StreamSubscription {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn pause(&self) {
        let (hook, input_hook, propagate) = {
            let mut inner = self.inner.borrow_mut();
            let mut propagate = false;
            if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
                propagate = subscription.state.pause_count == 0;
                subscription.state.pause_count += 1;
            }
            (
                inner.hooks.on_pause.clone(),
                inner.hooks.on_pause_input.clone(),
                propagate,
            )
        };
        if propagate {
            if let Some(input_hook) = input_hook {
                input_hook();
            }
        }
        if let Some(hook) = hook {
            hook();
        }
    }

    pub fn resume(&self) {
        let (buffered, hook, still_paused, resume_input) = {
            let mut inner = self.inner.borrow_mut();
            let Some(subscription) = inner.subscriptions.get_mut(&self.id) else {
                return;
            };
            subscription.state.pause_count = subscription.state.pause_count.saturating_sub(1);
            let still_paused = subscription.state.pause_count > 0;
            // `_onResume` runs only after the last nested pause is released
            // (`stream_impl.dart:215-229`); the pipe then resumes its source
            // subscription (`stream_pipe.dart:164-170`).
            let resume_input = !still_paused;
            (
                std::mem::take(&mut subscription.buffered),
                inner.hooks.on_resume.clone(),
                still_paused,
                resume_input,
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
        let mut cancel_hook = None;
        for event in buffered {
            let cancel_after_error = self.deliver(&event);
            if cancel_after_error {
                // A buffered error triggers the same cancelOnError transition
                // as a live one (`stream_impl.dart:407-421`): later events in
                // the buffer are discarded.
                let mut inner = self.inner.borrow_mut();
                if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
                    subscription.state.canceled = true;
                    subscription.buffered.clear();
                }
                cancel_hook = inner.take_cancel_hook_if_last();
                break;
            }
        }
        let still_active = {
            let inner = self.inner.borrow();
            inner
                .subscriptions
                .get(&self.id)
                .is_some_and(|subscription| !subscription.state.canceled)
        };
        if resume_input && still_active {
            if let Some(resume_input) = {
                let inner = self.inner.borrow();
                inner.hooks.on_resume_input.clone()
            } {
                resume_input();
            }
        }
        if let Some(hook) = cancel_hook {
            hook();
        }
        if let Some(hook) = hook {
            hook();
        }
        self.finalize_pending_cancels();
    }

    fn finalize_pending_cancels(&self) {
        Stream {
            inner: Rc::clone(&self.inner),
        }
        .finalize_pending_cancels()
    }

    /// Bridges terminal events to a `Future` like `StreamSubscription.asFuture`
    /// (`async/stream_impl.dart:243-270`). Done completes with `value`; an
    /// error cancels the subscription and completes with that error, keeping
    /// its stack trace byte for byte.
    pub fn as_future(&self, runtime: &FutureRuntime, value: FutureValue) -> Future {
        let completer = runtime.completer_with_error_handler();
        let done_completer = completer.clone();
        let error_completer = completer.clone();
        let cancel_self = self.clone();
        let mut overrides = self.inner.borrow_mut();
        if let Some(subscription) = overrides.subscriptions.get_mut(&self.id) {
            subscription.on_done = Some(Rc::new(move || {
                let _ = done_completer.complete(value.clone());
            }));
            subscription.on_error = Some(Rc::new(move |error| {
                cancel_self.cancel();
                let _ = error_completer.complete_error(error.clone());
            }));
        }
        drop(overrides);
        completer.future()
    }

    pub fn cancel(&self) {
        let on_cancel = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                // Re-entrant cancel during callback delivery; finalize after
                // the current delivery step releases the controller borrow.
                self.pending_cancel.set(true);
                return;
            };
            let was_subscribed = !inner.subscriptions.is_empty();
            if let Some(subscription) = inner.subscriptions.get_mut(&self.id) {
                subscription.state.canceled = true;
                subscription.waiting_for_cancel = false;
                subscription.buffered.clear();
            }
            inner.subscriptions.remove(&self.id);
            let no_subscribers = inner.subscriptions.values().all(|s| s.state.canceled);
            if was_subscribed && no_subscribers {
                inner.hooks.on_cancel.take()
            } else {
                None
            }
        };
        self.pending_cancel.set(false);
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }

    fn deliver(&self, event: &StreamEvent) -> bool {
        let subscription = self.inner.borrow().subscriptions.get(&self.id).cloned();
        subscription
            .as_ref()
            .is_some_and(|subscription| subscription.deliver(event))
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
    #[doc(hidden)]
    pub on_pause_input: Option<Rc<dyn Fn()>>,
    pub on_resume: Option<Rc<dyn Fn()>>,
    #[doc(hidden)]
    pub on_resume_input: Option<Rc<dyn Fn()>>,
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
    fn take_cancel_hook_if_last(&mut self) -> Option<Rc<dyn Fn()>> {
        if self.subscriptions.values().all(|s| s.state.canceled) {
            self.hooks.on_cancel.take()
        } else {
            None
        }
    }

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

    fn finalize_pending_cancels(&self) {
        let pending: Vec<(u32, Rc<Cell<bool>>)> = {
            let inner = self.inner.borrow();
            inner
                .subscriptions
                .iter()
                .filter(|(_, subscription)| subscription.pending_cancel.get())
                .map(|(id, subscription)| (*id, Rc::clone(&subscription.pending_cancel)))
                .collect()
        };
        for (id, flag) in pending {
            flag.set(false);
            StreamSubscription {
                id,
                inner: Rc::clone(&self.inner),
                pending_cancel: flag,
            }
            .cancel();
        }
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
        let pending_cancel = {
            let state = self.inner.borrow();
            Rc::clone(&state.subscriptions.get(&id).unwrap().pending_cancel)
        };
        let handle = StreamSubscription {
            id,
            inner: Rc::clone(&self.inner),
            pending_cancel,
        };
        for event in pre_listen {
            let _ = handle.deliver(&event);
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
                        // The source subscription never auto-cancels on
                        // error. The forwarding subscription owns the
                        // downstream cancelOnError policy and cancels this
                        // source through its onCancel hook
                        // (`stream_pipe.dart:134-145,164-179`).
                        ListenOptions::default(),
                    );
                    let source_subscription = subscription;
                    let pause_source = source_subscription.clone();
                    let resume_source = source_subscription.clone();
                    let cancel_source = source_subscription;
                    let mut state = cancel_target.inner.borrow_mut();
                    state.hooks.on_pause_input = Some(Rc::new(move || pause_source.pause()));
                    state.hooks.on_resume_input = Some(Rc::new(move || resume_source.resume()));
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
            let Some(existing) = inner.subscriptions.get(&id).cloned() else {
                continue;
            };
            let pending_done = matches!(event, StreamEvent::Done)
                && existing.waiting_for_cancel
                && !existing.pending_cancel.get();
            if existing.closed || existing.state.canceled && !pending_done {
                continue;
            }
            if existing.state.pause_count > 0 {
                // Paused listeners buffer without applying cancelOnError; the
                // transition happens when the buffered error is delivered
                // (`stream_controller.dart:141-143`, `stream_impl.dart:407-421`).
                inner
                    .subscriptions
                    .get_mut(&id)
                    .unwrap()
                    .buffered
                    .push_back(event.clone());
                continue;
            }
            let error_cancels =
                matches!(event, StreamEvent::Error(_)) && existing.state.cancel_on_error;
            let is_done = matches!(event, StreamEvent::Done);
            if !existing.state.canceled || existing.waiting_for_cancel {
                existing.deliver(&event);
            }
            if error_cancels {
                // `_sendError` marks wait-for-cancel before invoking the
                // handler (`stream_impl.dart:407-416`). A cancel fired from
                // the handler clears that flag and suppresses the done event
                // (`stream_impl.dart:69-76,232-240,392-395`).
                let canceled_during_callback = inner
                    .subscriptions
                    .get(&id)
                    .is_some_and(|subscription| subscription.pending_cancel.get());
                if let Some(subscription) = inner.subscriptions.get_mut(&id) {
                    if canceled_during_callback || subscription.state.cancel_on_error {
                        // An explicit cancel clears WAIT_FOR_CANCEL. A plain
                        // cancelOnError subscription has no pending done: its
                        // cancel future is already the terminal transition.
                        subscription.waiting_for_cancel = false;
                    } else {
                        subscription.waiting_for_cancel = true;
                    }
                    subscription.state.canceled = true;
                    subscription.buffered.clear();
                }
            } else if is_done {
                if let Some(subscription) = inner.subscriptions.get_mut(&id) {
                    subscription.closed = true;
                    subscription.state.canceled = true;
                    subscription.waiting_for_cancel = false;
                    subscription.buffered.clear();
                }
            }
        }
        drop(inner);
        stream.finalize_pending_cancels();
    }

    /// Pumps the microtask queue used by async stream controllers
    /// (`stream_controller.dart:154-159`).
    pub fn pump(&self, limit: usize) -> Vec<CallbackId> {
        self.microtasks.borrow_mut().drain(limit)
    }
}
