use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    InvalidJson(String),
    InvalidIr(String),
    UnknownState(String),
    UnknownNode(u32),
    UnboundEvent { node_id: u32, event: String },
    TypeMismatch { name: String, expected: &'static str },
    Overflow { name: String },
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(m) => write!(f, "invalid IR JSON: {m}"),
            Self::InvalidIr(m) => write!(f, "invalid IR: {m}"),
            Self::UnknownState(name) => write!(f, "unknown state name: {name}"),
            Self::UnknownNode(id) => write!(f, "unknown node id: {id}"),
            Self::UnboundEvent { node_id, event } => {
                write!(f, "node {node_id} has no {event} event binding")
            }
            Self::TypeMismatch { name, expected } => {
                write!(f, "state {name} expects {expected}, got incompatible value")
            }
            Self::Overflow { name } => write!(f, "integer overflow while updating {name}"),
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
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "no plugin registered as {name}"),
            Self::Failed(m) => write!(f, "plugin failed: {m}"),
            Self::Panicked(m) => write!(f, "plugin panicked: {m}"),
            Self::Disabled(name) => write!(f, "plugin {name} is disabled after repeated failures"),
        }
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    ShuttingDown,
    TaskFailed,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShuttingDown => write!(f, "scheduler is shutting down"),
            Self::TaskFailed => write!(f, "task panicked or was dropped"),
        }
    }
}

impl std::error::Error for SchedulerError {}
