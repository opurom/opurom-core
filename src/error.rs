use crate::resource::ResourceError;
use crate::text_input::TextInputError;
use std::any::Any;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    InvalidJson(String),
    InvalidIr(String),
    UnknownState(String),
    UnknownNode(u32),
    NoTextBufferInput(u32),
    UnboundEvent {
        node_id: u32,
        event: String,
    },
    TypeMismatch {
        name: String,
        expected: &'static str,
    },
    IndexOutOfBounds {
        name: String,
        index: i64,
        len: usize,
    },
    /// An `@locale` patch failed validation (`IR_ADDITIONS.md` §15.4).
    InvalidLocale {
        message: String,
    },
    Overflow {
        name: String,
    },
    Plugin(PluginError),
    Resource(ResourceError),
    TextInput(TextInputError),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(m) => write!(f, "invalid IR JSON: {m}"),
            Self::InvalidIr(m) => write!(f, "invalid IR: {m}"),
            Self::UnknownState(name) => write!(f, "unknown state name: {name}"),
            Self::UnknownNode(id) => write!(f, "unknown node id: {id}"),
            Self::NoTextBufferInput(id) => write!(f, "node {id} has no text buffer input"),
            Self::UnboundEvent { node_id, event } => {
                write!(f, "node {node_id} has no {event} event binding")
            }
            Self::TypeMismatch { name, expected } => {
                write!(f, "state {name} expects {expected}, got incompatible value")
            }
            Self::IndexOutOfBounds { name, index, len } => {
                write!(
                    f,
                    "index {index} is out of bounds for list {name} of length {len}"
                )
            }
            Self::InvalidLocale { message } => write!(f, "@locale {message}"),
            Self::Overflow { name } => write!(f, "integer overflow while updating {name}"),
            Self::Plugin(error) => write!(f, "plugin error: {error}"),
            Self::Resource(error) => write!(f, "resource error: {error}"),
            Self::TextInput(error) => write!(f, "text input error: {error}"),
        }
    }
}

impl std::error::Error for CoreError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    NotFound(String),
    Failed(String),
    Panicked(String),
    Disabled(String),
    Io(String),
    InvalidManifest(String),
    Spawn(String),
    Timeout(String),
    ProtocolViolation(String),
    Crashed(String),
    NotAllowed(String),
    UnknownCapability(String),
    AmbiguousCapability(String),
    /// A declared dependency is absent from the registry, so the plugin was
    /// not spawned (`PLUGIN_PROTOCOL.md` dependency order).
    DependencyFailed {
        plugin: String,
        dependency: String,
    },
    CircuitOpen {
        retry_after: u64,
    },
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "no plugin registered as {name}"),
            Self::Failed(m) => write!(f, "plugin failed: {m}"),
            Self::Panicked(m) => write!(f, "plugin panicked: {m}"),
            Self::Disabled(name) => write!(f, "plugin {name} is disabled after repeated failures"),
            Self::Io(m) => write!(f, "plugin I/O error: {m}"),
            Self::InvalidManifest(m) => write!(f, "invalid plugin manifest: {m}"),
            Self::Spawn(m) => write!(f, "cannot start plugin: {m}"),
            Self::Timeout(m) => write!(f, "plugin timed out: {m}"),
            Self::ProtocolViolation(m) => write!(f, "plugin protocol violation: {m}"),
            Self::Crashed(m) => write!(f, "plugin crashed: {m}"),
            Self::NotAllowed(name) => write!(f, "capability {name} is not in the app allowlist"),
            Self::UnknownCapability(name) => write!(f, "no loaded plugin declares {name}"),
            Self::AmbiguousCapability(name) => {
                write!(f, "more than one active plugin declares {name}")
            }
            Self::DependencyFailed { plugin, dependency } => write!(
                f,
                "plugin {plugin} was not started: its dependency {dependency} is not loaded"
            ),
            Self::CircuitOpen { retry_after } => write!(
                f,
                "plugin circuit is open; retry after {retry_after} clock ticks"
            ),
        }
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    ShuttingDown,
    TaskFailed,
    TaskPanicked(String),
    QueueFull { capacity: usize },
    CircuitOpen { retry_after: u64 },
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShuttingDown => write!(f, "scheduler is shutting down"),
            Self::TaskFailed => write!(f, "task panicked or was dropped"),
            Self::TaskPanicked(message) => write!(f, "task panicked: {message}"),
            Self::QueueFull { capacity } => {
                write!(f, "task queue is full ({capacity} pending tasks)")
            }
            Self::CircuitOpen { retry_after } => write!(
                f,
                "scheduler circuit is open; retry after {retry_after} clock ticks"
            ),
        }
    }
}

impl std::error::Error for SchedulerError {}

pub(crate) fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
