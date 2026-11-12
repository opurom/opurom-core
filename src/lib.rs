//! Opurom kernel, initial release vertical slice: IR 0.1.0 reactive state, a bounded
//! priority scheduler, a fault-isolating plugin bus and adapter degradation.

pub mod error;
pub mod host;
pub mod ir;
pub mod plugin;
pub mod scheduler;
pub mod state;

pub use error::{CoreError, PluginError, SchedulerError};
pub use host::{AdapterFault, Host};
pub use ir::{Action, Ir, Node, Op, StateDecl, IR_VERSION};
pub use plugin::PluginBus;
pub use scheduler::{Scheduler, TaskHandle};
pub use state::{Event, Patch, Runtime, Value};
