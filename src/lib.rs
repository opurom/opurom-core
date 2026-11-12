//! OPUROM kernel, round 2: IR v0.1 + R2 extensions (multi-action events,
//! `valueFrom`), a bounded priority scheduler, a fault-isolating plugin bus,
//! a process-isolated plugin host and a bounded resource cache.

pub mod error;
pub mod host;
pub mod ir;
pub mod plugin;
pub mod plugin_host;
pub mod resource;
pub mod scheduler;
pub mod state;

pub use error::{CoreError, PluginError, SchedulerError};
pub use host::{AdapterFault, Host};
pub use ir::{Action, Ir, Node, Op, StateDecl, IR_VERSION};
pub use plugin::PluginBus;
pub use plugin_host::{PluginHost, PluginInfo, PluginStatus, DEFAULT_PLUGIN_TIMEOUT};
pub use resource::{ResourceError, ResourceManager};
pub use scheduler::{Scheduler, TaskHandle};
pub use state::{Event, Patch, Runtime, Value, DEFAULT_RESOURCE_BUDGET};
