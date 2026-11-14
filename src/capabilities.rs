//! Explicit native-capability grants in front of the process-isolated host.
//!
//! Registration reads a manifest only; `invoke` lazily loads the plugin after
//! the catalog, grant and declaration checks. A denial therefore never spawns.

use crate::error::PluginError;
use crate::parallel_sched::Clock;
use crate::plugin_host::{declared_plugin, BreakerState, PluginHost, PluginInfo};
use serde_json::Value as Json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The seven native capabilities. Grants are exact strings from this list.
pub const NATIVE_CAPABILITIES: [&str; 7] = [
    "camera.takePhoto",
    "geolocation.getCurrentPosition",
    "bluetooth.scan",
    "notification.show",
    "payment.request",
    "share.send",
    "network.request",
];

#[derive(Clone)]
struct Registration {
    name: String,
    dir: PathBuf,
    capabilities: Vec<String>,
}

pub struct CapabilityHost {
    grants: HashSet<String>,
    inner: PluginHost,
    registrations: Mutex<Vec<Registration>>,
    loaded: Mutex<HashSet<String>>,
}

impl CapabilityHost {
    pub fn new<I, S>(grants: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let grants: HashSet<String> = grants.into_iter().map(Into::into).collect();
        let inner = PluginHost::new(grants.iter().cloned());
        Self {
            grants,
            inner,
            registrations: Mutex::new(Vec::new()),
            loaded: Mutex::new(HashSet::new()),
        }
    }

    pub fn with_failure_threshold(mut self, threshold: u32) -> Self {
        self.inner = self.inner.with_failure_threshold(threshold);
        self
    }

    pub fn with_cooldown(mut self, ticks: u64) -> Self {
        self.inner = self.inner.with_cooldown(ticks);
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.inner = self.inner.with_clock(clock);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.with_timeout(timeout);
        self
    }

    /// Validates the manifest without starting the executable.
    pub fn register(&self, dir: impl AsRef<Path>) -> Result<Vec<String>, PluginError> {
        let declaration = declared_plugin(dir.as_ref())?;
        let registration = Registration {
            name: declaration.name.clone(),
            dir: declaration.dir,
            capabilities: declaration.capabilities.clone(),
        };
        let mut registrations = lock(&self.registrations);
        registrations.retain(|item| item.name != registration.name);
        registrations.push(registration);
        lock(&self.loaded).remove(&declaration.name);
        Ok(declaration.capabilities)
    }

    pub fn invoke(&self, capability: &str, params: Json) -> Result<Json, PluginError> {
        if !NATIVE_CAPABILITIES.iter().any(|item| *item == capability) {
            return Err(PluginError::UnknownCapability(capability.to_owned()));
        }
        if !self.grants.contains(capability) {
            return Err(PluginError::NotAllowed(capability.to_owned()));
        }
        let matches: Vec<Registration> = lock(&self.registrations)
            .iter()
            .filter(|item| item.capabilities.iter().any(|cap| cap == capability))
            .cloned()
            .collect();
        match matches.len() {
            0 => return Err(PluginError::UnknownCapability(capability.to_owned())),
            1 => {}
            _ => return Err(PluginError::AmbiguousCapability(capability.to_owned())),
        }
        self.ensure_loaded(&matches[0])?;
        self.inner.invoke(capability, params)
    }

    fn ensure_loaded(&self, registration: &Registration) -> Result<(), PluginError> {
        let mut loaded = lock(&self.loaded);
        if loaded.contains(&registration.name) {
            return Ok(());
        }
        self.inner.load(&registration.dir)?;
        loaded.insert(registration.name.clone());
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<PluginInfo>, PluginError> {
        self.inner.list()
    }

    pub fn consecutive_failures(&self, plugin: &str) -> u32 {
        self.inner.consecutive_failures(plugin)
    }

    pub fn circuit_open(&self, plugin: &str) -> bool {
        self.inner.circuit_open(plugin)
    }

    pub fn breaker_state(&self, plugin: &str) -> Option<BreakerState> {
        self.inner.breaker_state(plugin)
    }

    pub fn retry_after(&self, plugin: &str) -> Option<u64> {
        self.inner.retry_after(plugin)
    }

    /// Kills and reaps every lazily loaded plugin process.
    pub fn shutdown(&self) {
        self.inner.shutdown();
        lock(&self.loaded).clear();
    }
}

impl Default for CapabilityHost {
    fn default() -> Self {
        Self::new(Vec::<String>::new())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
