//! Deterministic Future runtime ported from Dart 3.14.0 (`dart:async`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths below
//! are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - a future holds one of four completion states: incomplete, pending
//!   completion, value, or error (`async/future_impl.dart:291-325`);
//! - listeners are kept only while the future is incomplete
//!   (`async/future_impl.dart:341-355`);
//! - a listener is one of four kinds: `then` (value only), `then` with an
//!   error handler, `catchError` (with an optional error test), and
//!   `whenComplete` (`async/future_impl.dart:121-142,163-184`);
//! - `then` without an error handler forwards the error unchanged
//!   (`async/future.dart:812-813`);
//! - `catchError` runs the error test first; a false test leaves the error
//!   and its stack trace untouched (`async/future.dart:836-846`);
//! - `whenComplete` runs its action on both paths and lets the original
//!   result through (`async/future.dart:877-926`);
//! - completing with an error before any listener is attached reports an
//!   unhandled error (`async/future.dart:819-823,1336-1351`);
//! - `Future.value` and `Completer.complete` propagate through the microtask
//!   queue instead of synchronously (`async/future_impl.dart:364-366`,
//!   `async/future.dart:1361-1365`).
//!
//! Opurom adaptation: the carried value is a plain data tree, and the
//! callbacks are host-side Rust bodies of the same kind the plugin bus and
//! the priority scheduler already use in this crate — the kernel interprets
//! no IR and reads no clock. Completions are delivered on the core microtask
//! queue, so a future chain has the same ordering on every host.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::microtask::{CallbackId, MicrotaskQueue};

/// A value carried by a future: plain data, no dynamic dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum FutureValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<FutureValue>),
    Map(Vec<(String, FutureValue)>),
}

/// The error carried by a failed future, with its stack trace
/// (`async/future_impl.dart:323-325`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncError {
    pub message: String,
    pub stack_trace: String,
}

impl AsyncError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            stack_trace: message.clone(),
            message,
        }
    }
}

/// The result a future completes with.
#[derive(Debug, Clone, PartialEq)]
pub enum Completion {
    Value(FutureValue),
    Error(AsyncError),
}

/// Future completion states (`async/future_impl.dart:291-325`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FutureState {
    /// Waiting for a result; the field holds the listener list.
    Incomplete,
    /// Completion was requested but its microtask has not run yet.
    PendingComplete,
    /// Completed with a value.
    Value,
    /// Completed with an error.
    Error,
}

/// Listener kinds (`async/future_impl.dart:121-142`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenerKind {
    /// `then` without an error handler: values only, errors forwarded.
    Then,
    /// `then` with an error handler: values and errors.
    ThenOnError,
    /// `catchError` without a test: every error is handled.
    CatchError,
    /// `catchError` with a test: the test decides.
    CatchErrorTest,
    /// `whenComplete`: the action runs on both paths.
    WhenComplete,
}

/// The value callback of a listener.
pub type OnValue = Rc<dyn Fn(FutureValue) -> FutureValue>;
/// The error callback of a listener.
pub type OnError = Rc<dyn Fn(&AsyncError) -> FutureValue>;
/// The error test of `catchError`.
pub type ErrorTest = Rc<dyn Fn(&AsyncError) -> bool>;

/// One registered listener (`async/future_impl.dart:121-285`).
#[derive(Clone)]
struct Listener {
    kind: ListenerKind,
    on_value: Option<OnValue>,
    on_error: Option<OnError>,
    test: Option<ErrorTest>,
    /// The future this listener completes.
    target: Rc<RefCell<FutureInner>>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("kind", &self.kind)
            .finish()
    }
}

impl Listener {
    fn handles_value(&self) -> bool {
        matches!(
            self.kind,
            ListenerKind::Then | ListenerKind::ThenOnError | ListenerKind::WhenComplete
        )
    }

    fn handles_error(&self) -> bool {
        matches!(
            self.kind,
            ListenerKind::ThenOnError | ListenerKind::CatchError | ListenerKind::CatchErrorTest
        )
    }
}

struct FutureInner {
    state: FutureState,
    /// The completion once resolved.
    completion: Option<Completion>,
    /// The listener list while incomplete (`async/future_impl.dart:341-355`).
    listeners: Vec<Listener>,
}

/// A future handle. Cloning shares the same underlying state, exactly like
/// the future Dart's `then` returns (`async/future.dart:824`).
#[derive(Clone)]
pub struct Future {
    inner: Rc<RefCell<FutureInner>>,
}

/// The shared runtime: the microtask queue every future completes on, plus
/// the errors nobody handled.
#[derive(Clone, Default)]
pub struct FutureRuntime {
    microtasks: Rc<RefCell<MicrotaskQueue>>,
    /// Completions waiting for their microtask, in scheduling order.
    pending: Rc<RefCell<VecDeque<(CallbackId, Rc<RefCell<FutureInner>>, Completion)>>>,
    /// Errors that were never handled, in completion order
    /// (`async/future.dart:819-823`).
    unhandled: Rc<RefCell<Vec<AsyncError>>>,
    next_id: Rc<RefCell<u64>>,
}

impl std::fmt::Debug for FutureRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FutureRuntime")
            .field("pending", &self.pending.borrow().len())
            .field("unhandled", &self.unhandled.borrow().len())
            .finish()
    }
}

impl FutureRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// The microtask queue the runtime completes futures on.
    pub fn microtasks(&self) -> Rc<RefCell<MicrotaskQueue>> {
        Rc::clone(&self.microtasks)
    }

    /// Errors reported without a handler, oldest first.
    pub fn unhandled_errors(&self) -> Vec<AsyncError> {
        self.unhandled.borrow().clone()
    }

    /// A future that completes with `value`. Dart completes such a future
    /// through the microtask queue (`async/future_impl.dart:364-366`).
    pub fn value(&self, value: FutureValue) -> Future {
        let future = Future::new();
        self.schedule(Rc::clone(&future.inner), Completion::Value(value));
        future
    }

    /// A future that completes with an error. Without a listener the error is
    /// reported as unhandled (`async/future.dart:1336-1351`).
    pub fn error(&self, error: AsyncError) -> Future {
        let future = Future::new();
        self.schedule(Rc::clone(&future.inner), Completion::Error(error));
        future
    }

    /// A future completed by the host: `complete` / `completeError` mirror
    /// Dart's `Completer` (`async/future.dart:1310,1356`).
    pub fn completer(&self) -> Completer {
        Completer {
            runtime: self.clone(),
            future: Future::new(),
            completed: Rc::new(RefCell::new(false)),
        }
    }

    /// Runs the queued microtasks and delivers the completions they carry.
    ///
    /// Ids that belong to another runtime are returned to the host, which
    /// resolves them; ids owned by this runtime are delivered here.
    pub fn pump(&self, limit: usize) -> Vec<CallbackId> {
        let ids = self.microtasks.borrow_mut().drain(limit);
        let mut foreign = Vec::new();
        for id in ids {
            let entry = {
                let mut pending = self.pending.borrow_mut();
                pending
                    .iter()
                    .position(|(queued, _, _)| *queued == id)
                    .map(|index| pending.remove(index))
                    .flatten()
            };
            match entry {
                Some((_, inner, completion)) => self.deliver(inner, completion),
                None => foreign.push(id),
            }
        }
        foreign
    }

    /// Schedules a completion on the microtask queue
    /// (`async/future_impl.dart:364-366`).
    fn schedule(&self, inner: Rc<RefCell<FutureInner>>, completion: Completion) {
        {
            let mut state = inner.borrow_mut();
            if state.state != FutureState::Incomplete {
                return;
            }
            state.state = FutureState::PendingComplete;
            state.completion = Some(completion.clone());
        }
        let id = self.allocate_id();
        self.pending
            .borrow_mut()
            .push_back((id, Rc::clone(&inner), completion));
        self.microtasks.borrow_mut().schedule(id);
    }

    fn allocate_id(&self) -> CallbackId {
        let mut next = self.next_id.borrow_mut();
        *next += 1;
        *next as CallbackId
    }

    /// Delivers a completion to the listeners
    /// (`async/future_impl.dart:395-420`).
    fn deliver(&self, inner: Rc<RefCell<FutureInner>>, completion: Completion) {
        let (completion, listeners) = {
            let mut state = inner.borrow_mut();
            if state.state != FutureState::PendingComplete {
                return;
            }
            state.state = match &completion {
                Completion::Value(_) => FutureState::Value,
                Completion::Error(_) => FutureState::Error,
            };
            (completion, std::mem::take(&mut state.listeners))
        };
        if listeners.is_empty() {
            // An error nobody listens for is unhandled
            // (`async/future.dart:819-823`).
            if let Completion::Error(error) = &completion {
                self.unhandled.borrow_mut().push(error.clone());
            }
            return;
        }
        for listener in listeners {
            self.notify(&listener, completion.clone());
        }
    }

    /// Applies one listener to a completion
    /// (`async/future_impl.dart:221-279`).
    fn notify(&self, listener: &Listener, completion: Completion) {
        match &completion {
            Completion::Value(value) => {
                if !listener.handles_value() {
                    // A `catchError` listener ignores values
                    // (`async/future_impl.dart:136-139`).
                    self.complete(listener.target.clone(), completion.clone());
                    return;
                }
                let produced = match &listener.on_value {
                    Some(on_value) => on_value(value.clone()),
                    None => value.clone(),
                };
                self.complete(listener.target.clone(), Completion::Value(produced));
            }
            Completion::Error(error) => {
                if listener.kind == ListenerKind::WhenComplete {
                    // `whenComplete` runs the same action on both paths and
                    // lets the original result through
                    // (`async/future.dart:898-909`).
                    if let Some(action) = &listener.on_value {
                        action(FutureValue::Null);
                    }
                    self.complete(listener.target.clone(), completion.clone());
                    return;
                }
                if !listener.handles_error() {
                    // `then` without an error handler forwards the error
                    // unchanged (`async/future.dart:812-813`).
                    self.complete(listener.target.clone(), completion.clone());
                    return;
                }
                let handled = match &listener.test {
                    Some(test) => test(error),
                    None => true,
                };
                if !handled {
                    // A false test leaves the error and its stack trace
                    // untouched (`async/future.dart:836-846`).
                    self.complete(listener.target.clone(), completion.clone());
                    return;
                }
                let produced = match &listener.on_error {
                    Some(on_error) => on_error(error),
                    None => FutureValue::Null,
                };
                self.complete(listener.target.clone(), Completion::Value(produced));
            }
        }
    }

    /// Completes a future with a result (`async/future_impl.dart:395-420`).
    fn complete(&self, inner: Rc<RefCell<FutureInner>>, completion: Completion) {
        {
            let mut state = inner.borrow_mut();
            if state.state != FutureState::Incomplete {
                return;
            }
            state.state = FutureState::PendingComplete;
            state.completion = Some(completion.clone());
        }
        self.deliver(inner, completion);
    }
}

impl std::fmt::Debug for Future {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Future")
            .field("state", &self.inner.borrow().state)
            .finish()
    }
}

impl Future {
    fn new() -> Future {
        Future {
            inner: Rc::new(RefCell::new(FutureInner {
                state: FutureState::Incomplete,
                completion: None,
                listeners: Vec::new(),
            })),
        }
    }

    /// Whether the future already carries a result
    /// (`async/future.dart:1358-1368`).
    pub fn is_completed(&self) -> bool {
        matches!(
            self.inner.borrow().state,
            FutureState::Value | FutureState::Error
        )
    }

    /// The completion, once available.
    pub fn completion(&self) -> Option<Completion> {
        self.inner.borrow().completion.clone()
    }

    /// Registers a value callback; errors are forwarded unchanged
    /// (`async/future.dart:824`).
    pub fn then(&self, runtime: &FutureRuntime, on_value: OnValue) -> Future {
        self.attach(runtime, ListenerKind::Then, Some(on_value), None, None)
    }

    /// Registers a value callback and an error callback
    /// (`async/future.dart:824`).
    pub fn then_on_error(
        &self,
        runtime: &FutureRuntime,
        on_value: OnValue,
        on_error: OnError,
    ) -> Future {
        self.attach(
            runtime,
            ListenerKind::ThenOnError,
            Some(on_value),
            Some(on_error),
            None,
        )
    }

    /// Registers an error handler that handles every error
    /// (`async/future.dart:875`).
    pub fn catch_error(&self, runtime: &FutureRuntime, on_error: OnError) -> Future {
        self.attach(
            runtime,
            ListenerKind::CatchError,
            None,
            Some(on_error),
            None,
        )
    }

    /// Registers an error handler guarded by a test; a false test leaves the
    /// error untouched (`async/future.dart:836-846`).
    pub fn catch_error_test(
        &self,
        runtime: &FutureRuntime,
        test: ErrorTest,
        on_error: OnError,
    ) -> Future {
        self.attach(
            runtime,
            ListenerKind::CatchErrorTest,
            None,
            Some(on_error),
            Some(test),
        )
    }

    /// Registers a completion action that runs on both paths
    /// (`async/future.dart:926`).
    pub fn when_complete(&self, runtime: &FutureRuntime, action: OnValue) -> Future {
        self.attach(
            runtime,
            ListenerKind::WhenComplete,
            Some(action),
            None,
            None,
        )
    }

    /// Builds a listener and attaches it, returning the future it completes.
    fn attach(
        &self,
        runtime: &FutureRuntime,
        kind: ListenerKind,
        on_value: Option<OnValue>,
        on_error: Option<OnError>,
        test: Option<ErrorTest>,
    ) -> Future {
        let target = Future::new();
        let listener = Listener {
            kind,
            on_value,
            on_error,
            test,
            target: Rc::clone(&target.inner),
        };
        {
            let mut state = self.inner.borrow_mut();
            match state.state {
                FutureState::Incomplete | FutureState::PendingComplete => {
                    state.listeners.push(listener);
                }
                FutureState::Value | FutureState::Error => {
                    // The future already completed: notify right away
                    // (`async/future_impl.dart:395-420`).
                    let completion = state.completion.clone();
                    drop(state);
                    if let Some(completion) = completion {
                        runtime.notify(&listener, completion);
                    }
                }
            }
        }
        target
    }
}

/// A completer mirroring Dart's `Completer` (`async/future.dart:1310,1356`).
#[derive(Clone)]
pub struct Completer {
    runtime: FutureRuntime,
    future: Future,
    completed: Rc<RefCell<bool>>,
}

impl std::fmt::Debug for Completer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Completer")
            .field("completed", &self.is_completed())
            .finish()
    }
}

impl Completer {
    /// Completes the future; calling it twice is an error in Dart
    /// (`async/future.dart:1307,1316`).
    pub fn complete(&self, value: FutureValue) -> bool {
        let mut completed = self.completed.borrow_mut();
        if *completed {
            return false;
        }
        *completed = true;
        drop(completed);
        self.runtime
            .schedule(Rc::clone(&self.future.inner), Completion::Value(value));
        true
    }

    /// Completes the future with an error; without a listener the error is
    /// unhandled (`async/future.dart:1336-1351`).
    pub fn complete_error(&self, error: AsyncError) -> bool {
        let mut completed = self.completed.borrow_mut();
        if *completed {
            return false;
        }
        *completed = true;
        drop(completed);
        self.runtime
            .schedule(Rc::clone(&self.future.inner), Completion::Error(error));
        true
    }

    pub fn future(&self) -> Future {
        self.future.clone()
    }

    pub fn is_completed(&self) -> bool {
        *self.completed.borrow()
    }
}

/// `Future.wait` / `Future.any` policy for how an error is reported
/// (`async/future.dart:517-600,656-670`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutureCombineErrorPolicy {
    /// `Future.wait` with `eagerError = false`: the first error is reported
    /// after every input completes (`future.dart:550-559,587-590`).
    Wait,
    /// `Future.wait` with `eagerError = true`: the first error completes the
    /// combined future immediately (`future.dart:550-552`).
    WaitEager,
    /// `Future.any`: the first completion (value or error) wins
    /// (`future.dart:656-670`).
    Any,
}

/// Result of combining futures.
#[derive(Debug, Clone, PartialEq)]
pub enum CombineOutcome {
    /// `Future.wait` success: one value per input, in source order
    /// (`future.dart:574-577`).
    Wait(Vec<FutureValue>),
    /// `Future.any` success: the first completed value.
    Any(FutureValue),
    /// The first error, with its stack trace (`future.dart:530-537,662-664`).
    Error(AsyncError),
}

/// Combines already-completed futures with deterministic, source-ordered
/// resolution. Inputs arrive as `Completion`s so the kernel never reads a
/// clock or subscribes to an external source.
///
/// Sources: `async/future.dart:517-600` (`wait`) and `:656-670` (`any`).
pub fn combine_completions(
    inputs: &[Completion],
    policy: FutureCombineErrorPolicy,
) -> CombineOutcome {
    match policy {
        FutureCombineErrorPolicy::Any => {
            // `any` attaches listeners in source order and the first one to
            // complete wins (`future.dart:656-670`).
            for input in inputs {
                match input {
                    Completion::Value(value) => return CombineOutcome::Any(value.clone()),
                    Completion::Error(error) => return CombineOutcome::Error(error.clone()),
                }
            }
            CombineOutcome::Wait(Vec::new())
        }
        FutureCombineErrorPolicy::Wait | FutureCombineErrorPolicy::WaitEager => {
            let first_error = inputs.iter().find_map(|input| match input {
                Completion::Error(error) => Some(error.clone()),
                Completion::Value(_) => None,
            });
            if let Some(error) = first_error {
                // The first error is kept, even when later inputs also fail
                // (`future.dart:530-537,553-558`).
                return CombineOutcome::Error(error);
            }
            CombineOutcome::Wait(
                inputs
                    .iter()
                    .map(|input| match input {
                        Completion::Value(value) => value.clone(),
                        Completion::Error(_) => FutureValue::Null,
                    })
                    .collect(),
            )
        }
    }
}

/// Whether an eager error may complete `wait` before later inputs finish
/// (`future.dart:550-552`). Kept as a separate predicate so hosts can drive
/// partially-completed input sets without duplicating the policy branch.
pub fn should_complete_wait_early(inputs: &[Completion], policy: FutureCombineErrorPolicy) -> bool {
    policy == FutureCombineErrorPolicy::WaitEager
        && inputs
            .iter()
            .any(|input| matches!(input, Completion::Error(_)))
}

/// The error that an eager `wait` reports at a given step, together with the
/// source position where its listener fired. `Future.wait` registers listeners
/// in source order, so the first failed input in that order wins even when a
/// later input completes first in wall-clock time
/// (`future.dart:528-537,550-559`).
pub fn wait_error_at(
    inputs: &[Option<Completion>],
    policy: FutureCombineErrorPolicy,
) -> Option<(usize, AsyncError)> {
    if policy != FutureCombineErrorPolicy::WaitEager {
        return None;
    }
    inputs
        .iter()
        .enumerate()
        .find_map(|(index, input)| match input {
            Some(Completion::Error(error)) => Some((index, error.clone())),
            _ => None,
        })
}

/// Deterministic result of a host-driven `Future.forEach` sequence
/// (`async/future.dart:672-699`): actions run in source order, and the first
/// error stops the loop (`:686-687`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForEachResult {
    /// Elements whose action completed successfully, in source order.
    pub processed: Vec<FutureValue>,
    /// The first error from an action, if any.
    pub error: Option<AsyncError>,
}

/// Drives one `forEach` action per element. The host supplies the outcome of
/// each action, so the kernel interprets no code and reads no clock.
/// Non-future return values and future values are discarded
/// (`future.dart:680-684`).
pub fn for_each_outcomes(elements: &[FutureValue], outcomes: &[Completion]) -> ForEachResult {
    let mut processed = Vec::new();
    for (element, outcome) in elements.iter().zip(outcomes.iter()) {
        match outcome {
            Completion::Value(_) => processed.push(element.clone()),
            Completion::Error(error) => {
                return ForEachResult {
                    processed,
                    error: Some(error.clone()),
                }
            }
        }
    }
    ForEachResult {
        processed,
        error: None,
    }
}
