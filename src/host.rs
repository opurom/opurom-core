use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFault(pub String);

impl fmt::Display for AdapterFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "adapter error: {}", self.0)
    }
}

impl std::error::Error for AdapterFault {}

/// Standalone adapter-failure policy: 3 consecutive errors degrade to the headless fallback.
pub struct Host {
    threshold: u32,
    consecutive_errors: u32,
    degraded: bool,
}

impl Host {
    pub fn new() -> Self {
        Host::with_threshold(3)
    }

    pub fn with_threshold(threshold: u32) -> Self {
        Host {
            threshold: threshold.max(1),
            consecutive_errors: 0,
            degraded: false,
        }
    }

    pub fn degraded(&self) -> bool {
        self.degraded
    }

    pub fn consecutive_errors(&self) -> u32 {
        self.consecutive_errors
    }

    pub fn active_adapter(&self) -> &'static str {
        if self.degraded {
            "headless"
        } else {
            "primary"
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_errors = 0;
    }

    /// Returns true when this error is the one that trips the circuit breaker.
    pub fn record_error(&mut self) -> bool {
        if self.degraded {
            return false;
        }
        self.consecutive_errors += 1;
        if self.consecutive_errors >= self.threshold {
            self.degraded = true;
            return true;
        }
        false
    }

    /// While degraded, the primary adapter is not invoked at all.
    pub fn run<T, E, P, F>(&mut self, primary: P, fallback: F) -> Result<T, E>
    where
        P: FnOnce() -> Result<T, E>,
        F: FnOnce() -> Result<T, E>,
    {
        if self.degraded {
            return fallback();
        }
        match primary() {
            Ok(value) => {
                self.record_success();
                Ok(value)
            }
            Err(error) => {
                self.record_error();
                Err(error)
            }
        }
    }
}

impl Default for Host {
    fn default() -> Self {
        Host::new()
    }
}
