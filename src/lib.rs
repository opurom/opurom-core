//! Opurom kernel: IR 0.1.0 and its interactive UI extensions, a bounded priority
//! scheduler, a deterministic cooperative scheduler, a multi-threaded
//! work-stealing scheduler, a fault-isolating plugin bus, a process-isolated
//! plugin host, a bounded resource cache, and a kernel-owned Unicode text
//! input state.

pub mod async_sched;
pub mod capabilities;
pub mod clamping_scroll;
pub mod collections;
pub mod convert;
pub mod dart_error;
pub mod drag_axis;
pub mod error;
pub mod future;
pub mod gestures;
pub mod host;
pub mod ir;
pub mod json_codec;
pub mod microtask;
pub mod parallel_sched;
pub mod plugin;
pub mod plugin_host;
pub mod render;
pub mod resource;
pub mod scale;
pub mod scheduler;
pub mod state;
pub mod stream;
pub mod text_buffer;
pub mod text_input;

pub use async_sched::{AsyncScheduler, SchedulingHook, SchedulingTask, TaskContext, TaskId};
pub use capabilities::{CapabilityHost, NATIVE_CAPABILITIES};
pub use clamping_scroll::{
    deceleration_rate, ClampingScrollSimulation, DEFAULT_FRICTION, INFLEXION, PHYSICAL_COEFF,
    TOLERANCE,
};
pub use collections::{
    add_all, fold, index_of, last_index_of, map, r#where as collect_where, reduce, remove_at,
    reversed, skip_take, sort_by, sublist, to_list, GuardedIterator, NoElementError, OrderedMap,
    OrderedSet,
};
pub use convert::{
    base64_decode, base64_encode, clamp_i64, convert_error_as_format, uri_encode_component,
    uri_encode_query_component, utf8_decode, utf8_encode, Base64Variant, ConvertError,
};
pub use dart_error::{
    check_not_null, check_valid_index, check_value_in_interval, DartError, DartException,
    DartStackTrace,
};
pub use drag_axis::{
    consider_fling, has_sufficient_distance, is_fling_gesture, primary_delta, primary_value,
    DragAxis,
};
pub use error::{CoreError, PluginError, SchedulerError};
pub use future::{
    combine_completions, for_each_outcomes, should_complete_wait_early, AsyncError, CombineOutcome,
    Completer, Completion, ErrorTest, ForEachResult, Future, FutureCombineErrorPolicy,
    FutureRuntime, FutureValue, OnError, OnValue,
};
pub use gestures::{
    ArenaEvent, ArenaStep, CancelReason, GestureArenaKernel, GestureKind, HitEntry, Point,
    PointerKind, RecognizerEvent, Velocity, PRESS_TIMEOUT_MS,
};
pub use host::{AdapterFault, Host};
pub use ir::{Action, Ir, Locale, Node, Op, Route, StateDecl, IR_VERSION};
pub use json_codec::{
    json_decode, json_encode, json_encode_pretty, json_error_as_format, JsonError,
};
pub use microtask::{CallbackId, FrameCallbacks, FramePhase, FramePump, FrameStep, MicrotaskQueue};
pub use parallel_sched::{
    cpu_bound_workload, measure_speedup, Clock, CounterClock, SchedulerMetrics, SpeedupMeasurement,
    SystemClock, WorkStealingScheduler, WorkStealingTaskHandle,
};
pub use plugin::{PluginBus, PluginMetrics};
pub use plugin_host::{BreakerState, PluginHost, PluginInfo, PluginStatus, DEFAULT_PLUGIN_TIMEOUT};
pub use render::RenderingHook;
pub use resource::{ResourceError, ResourceManager};
pub use scale::{
    compute_pan_slop, compute_scale_slop, DragStartBehavior, PointerLine, ScaleDetails,
    ScaleRecognizer, ScaleState, PAN_SLOP, SCALE_ACCEPT_RATIO, SCALE_SLOP,
};
pub use scheduler::{Scheduler, TaskHandle};
pub use state::{
    Event, FrameRecord, FrameReport, FrameSample, NetworkRecord, Patch, Runtime, Value,
    DEFAULT_RESOURCE_BUDGET, FRAME_RING, MEMORY_HISTORY_RING, NETWORK_RING,
};
pub use stream::{
    DataCallback, DoneCallback, ErrorCallback, Stream, StreamEvent, StreamHooks, StreamRuntime,
    StreamSubscription,
};
pub use text_buffer::{TextBuffer, TextBufferError};
pub use text_input::{TextInputError, TextInputState, DEFAULT_TEXT_INPUT_CAPACITY, TEXT_INPUT_BUS};
