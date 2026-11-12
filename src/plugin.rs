use crate::error::PluginError;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

type Handler = dyn Fn(&str) -> Result<String, String> + Send + Sync + 'static;

struct Entry {
    handler: Arc<Handler>,
    failures: u32,
    disabled: bool,
}

pub struct PluginBus {
    failure_threshold: u32,
    entries: Mutex<HashMap<String, Entry>>,
}

impl PluginBus {
    /// A plugin is disabled once it fails `failure_threshold` times without a success.
    pub fn new(failure_threshold: u32) -> Self {
        PluginBus {
            failure_threshold: failure_threshold.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn register<F>(&self, name: impl Into<String>, handler: F)
    where
        F: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(
            name.into(),
            Entry {
                handler: Arc::new(handler),
                failures: 0,
                disabled: false,
            },
        );
    }

    pub fn call(&self, name: &str, input: &str) -> Result<String, PluginError> {
        let (handler, disabled) = {
            let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            match entries.get(name) {
                None => return Err(PluginError::NotFound(name.to_owned())),
                Some(entry) => (Arc::clone(&entry.handler), entry.disabled),
            }
        };
        if disabled {
            return Err(PluginError::Disabled(name.to_owned()));
        }
        let result = match catch_unwind(AssertUnwindSafe(|| handler(input))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(PluginError::Failed(message)),
            Err(panic) => Err(PluginError::Panicked(panic_message(panic))),
        };
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(name) {
            if result.is_ok() {
                entry.failures = 0;
            } else {
                entry.failures += 1;
                if entry.failures >= self.failure_threshold {
                    entry.disabled = true;
                }
            }
        }
        result
    }

    pub fn is_disabled(&self, name: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .is_some_and(|entry| entry.disabled)
    }
}

impl Default for PluginBus {
    fn default() -> Self {
        PluginBus::new(3)
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
