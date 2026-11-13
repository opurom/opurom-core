//! OPUROM kernel: IR v0.1 and its interactive UI extensions, a bounded priority
//! scheduler, a deterministic cooperative scheduler, a multi-threaded
//! work-stealing scheduler, a fault-isolating plugin bus, a process-isolated
//! plugin host, and a bounded resource cache.

pub mod async_sched;
pub mod capabilities;
pub mod error;
pub mod host;
pub mod ir;
pub mod parallel_sched;
pub mod plugin;
pub mod plugin_host;
pub mod resource;
pub mod scheduler;
pub mod state;

pub use async_sched::{AsyncScheduler, SchedulingHook, SchedulingTask, TaskContext, TaskId};
pub use capabilities::{CapabilityHost, NATIVE_CAPABILITIES};
pub use error::{CoreError, PluginError, SchedulerError};
pub use host::{AdapterFault, Host};
pub use ir::{Action, Ir, Node, Op, StateDecl, IR_VERSION};
pub use parallel_sched::{
    cpu_bound_workload, measure_speedup, Clock, CounterClock, SpeedupMeasurement, SystemClock,
    WorkStealingScheduler, WorkStealingTaskHandle,
};
pub use plugin::{PluginBus, PluginMetrics};
pub use plugin_host::{BreakerState, PluginHost, PluginInfo, PluginStatus, DEFAULT_PLUGIN_TIMEOUT};
pub use resource::{ResourceError, ResourceManager};
pub use scheduler::{Scheduler, TaskHandle};
pub use state::{Event, Patch, Runtime, Value, DEFAULT_RESOURCE_BUDGET};
