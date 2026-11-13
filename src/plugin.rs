use crate::error::{panic_message, PluginError};
use crate::parallel_sched::{Clock, SystemClock};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

type Handler = dyn Fn(&str) -> Result<String, String> + Send + Sync + 'static;

enum Circuit {
    Closed(u32),
    Disabled,
    Open {
        retry_at: u64,
        cooldown: u64,
        trial: bool,
    },
}

struct Entry {
    handler: Arc<Handler>,
    circuit: Circuit,
}

/// Exact, deterministic counters observed on a [`PluginBus`].
///
/// `trips` counts closed circuits that reached the failure threshold; a trip
/// opens the circuit when a cooldown is configured and disables it otherwise,
/// so `openings <= trips`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginMetrics {
    /// Every invocation of [`PluginBus::call`], successful or not.
    pub calls: u64,
    /// Closed circuits that reached the failure threshold.
    pub trips: u64,
    /// Transitions into the open state (trips with a finite cooldown).
    pub openings: u64,
    /// Calls admitted through an open circuit after its cooldown elapsed.
    pub trials: u64,
    /// Admitted trials that succeeded and closed the circuit again.
    pub recoveries: u64,
}

pub struct PluginBus {
    failure_threshold: u32,
    cooldown: Option<u64>,
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<String, Entry>>,
    metrics: Mutex<PluginMetrics>,
}

impl PluginBus {
    /// A plugin is disabled once it fails `failure_threshold` times without a success.
    pub fn new(failure_threshold: u32) -> Self {
        PluginBus {
            failure_threshold: failure_threshold.max(1),
            cooldown: None,
            clock: Arc::new(SystemClock),
            entries: Mutex::new(HashMap::new()),
            metrics: Mutex::new(PluginMetrics::default()),
        }
    }

    /// Cooldown in injected clock ticks; finite cooldown enables recovery.
    pub fn with_cooldown(mut self, ticks: u64) -> Self {
        self.cooldown = Some(ticks);
        self
    }

    /// Logical clock for deterministic cooldown tests; configure before calls.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    fn record(&self, apply: impl FnOnce(&mut PluginMetrics)) {
        let mut metrics = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
        apply(&mut metrics);
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
                circuit: Circuit::Closed(0),
            },
        );
    }

    pub fn call(&self, name: &str, input: &str) -> Result<String, PluginError> {
        self.record(|m| m.calls += 1);
        let handler = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = entries.get_mut(name) else {
                return Err(PluginError::NotFound(name.to_owned()));
            };
            match &mut entry.circuit {
                Circuit::Disabled => return Err(PluginError::Disabled(name.to_owned())),
                Circuit::Open {
                    retry_at,
                    cooldown,
                    trial,
                } => {
                    let now = self.clock.now();
                    if *trial || now < *retry_at {
                        return Err(PluginError::CircuitOpen {
                            retry_after: retry_at.saturating_sub(now),
                        });
                    }
                    // Re-arm before releasing the lock: concurrent callers see a
                    // trial in flight and only this call reaches the handler.
                    *retry_at = now.saturating_add(*cooldown);
                    *trial = true;
                    self.record(|m| m.trials += 1);
                }
                Circuit::Closed(_) => {}
            }
            Arc::clone(&entry.handler)
        };
        let result = match catch_unwind(AssertUnwindSafe(|| handler(input))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(PluginError::Failed(message)),
            Err(panic) => Err(PluginError::Panicked(panic_message(panic))),
        };
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(name) {
            if result.is_ok() {
                if matches!(entry.circuit, Circuit::Open { .. }) {
                    self.record(|m| m.recoveries += 1);
                }
                entry.circuit = Circuit::Closed(0);
            } else if let Circuit::Closed(failures) = &mut entry.circuit {
                *failures = failures.saturating_add(1);
                if *failures >= self.failure_threshold {
                    self.record(|m| m.trips += 1);
                    entry.circuit = match self.cooldown {
                        Some(cooldown) => {
                            self.record(|m| m.openings += 1);
                            Circuit::Open {
                                retry_at: self.clock.now().saturating_add(cooldown),
                                cooldown,
                                trial: false,
                            }
                        }
                        None => Circuit::Disabled,
                    };
                }
            } else if let Circuit::Open { trial, .. } = &mut entry.circuit {
                *trial = false;
            }
        }
        result
    }

    pub fn is_disabled(&self, name: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .is_some_and(|entry| !matches!(entry.circuit, Circuit::Closed(_)))
    }

    /// Remaining cooldown ticks of an open circuit; None when the plugin is callable.
    pub fn retry_after(&self, name: &str) -> Option<u64> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(name).map(|entry| &entry.circuit) {
            Some(Circuit::Open { retry_at, .. }) => Some(retry_at.saturating_sub(self.clock.now())),
            _ => None,
        }
    }

    /// Snapshot of the exact current counters; deterministic, no estimation.
    pub fn metrics(&self) -> PluginMetrics {
        *self.metrics.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for PluginBus {
    fn default() -> Self {
        PluginBus::new(3)
    }
}
