//! Failure Injection for Mock Worker
//!
//! Supports configurable failure injection for testing error paths.

use std::collections::HashMap;
use std::time::Duration;

use crate::protocol::envelope::Operation;

/// Failure configuration for an operation
#[derive(Debug, Clone)]
pub struct FailureConfig {
    /// Error code to return (if any)
    pub error_code: Option<String>,
    /// Error message to return
    pub error_message: Option<String>,
    /// Delay to add before responding
    pub delay: Option<Duration>,
    /// Number of times to fail before succeeding (None = always fail)
    pub fail_count: Option<u32>,
    /// Retry-after seconds for BUSY errors
    pub retry_after_seconds: Option<u32>,
}

impl FailureConfig {
    /// Create a config that returns an error
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_code: Some(code.into()),
            error_message: Some(message.into()),
            delay: None,
            fail_count: None,
            retry_after_seconds: None,
        }
    }

    /// Create a BUSY error with retry-after
    pub fn busy(retry_after_seconds: u32) -> Self {
        Self {
            error_code: Some("BUSY".to_string()),
            error_message: Some("Worker at capacity".to_string()),
            delay: None,
            fail_count: None,
            retry_after_seconds: Some(retry_after_seconds),
        }
    }

    /// Create a config that just adds delay
    pub fn delay(duration: Duration) -> Self {
        Self {
            error_code: None,
            error_message: None,
            delay: Some(duration),
            fail_count: None,
            retry_after_seconds: None,
        }
    }

    /// Set the number of times to fail before succeeding
    pub fn with_fail_count(mut self, count: u32) -> Self {
        self.fail_count = Some(count);
        self
    }
}

/// Failure injector for the mock worker
#[derive(Debug, Default)]
pub struct FailureInjector {
    /// Per-operation failure configs
    configs: HashMap<Operation, FailureConfig>,
    /// Call counts per operation (for fail_count tracking)
    call_counts: HashMap<Operation, u32>,
}

impl FailureInjector {
    /// Create a new failure injector
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject a failure for an operation
    pub fn inject(&mut self, op: Operation, config: FailureConfig) {
        self.configs.insert(op.clone(), config);
        self.call_counts.insert(op, 0);
    }

    /// Inject an error for an operation
    pub fn inject_error(&mut self, op: Operation, code: impl Into<String>, message: impl Into<String>) {
        self.inject(op, FailureConfig::error(code, message));
    }

    /// Inject a delay for an operation
    pub fn inject_delay(&mut self, op: Operation, delay: Duration) {
        self.inject(op, FailureConfig::delay(delay));
    }

    /// Clear all failure injections
    pub fn clear(&mut self) {
        self.configs.clear();
        self.call_counts.clear();
    }

    /// Clear failure injection for a specific operation
    pub fn clear_op(&mut self, op: &Operation) {
        self.configs.remove(op);
        self.call_counts.remove(op);
    }

    /// Check if a failure should occur for an operation
    /// Returns the failure config if one should occur, None otherwise
    pub fn check(&mut self, op: &Operation) -> Option<&FailureConfig> {
        if let Some(config) = self.configs.get(op) {
            let count = self.call_counts.entry(op.clone()).or_insert(0);
            *count += 1;

            // Check if we should still fail based on fail_count
            if let Some(fail_limit) = config.fail_count {
                if *count > fail_limit {
                    return None; // Exceeded fail count, succeed now
                }
            }

            Some(config)
        } else {
            None
        }
    }

    /// Get the delay for an operation (if any)
    pub fn get_delay(&self, op: &Operation) -> Option<Duration> {
        self.configs.get(op).and_then(|c| c.delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_config_error() {
        let config = FailureConfig::error("TEST_ERROR", "Test message");
        assert_eq!(config.error_code, Some("TEST_ERROR".to_string()));
        assert_eq!(config.error_message, Some("Test message".to_string()));
    }

    #[test]
    fn test_failure_config_busy() {
        let config = FailureConfig::busy(30);
        assert_eq!(config.error_code, Some("BUSY".to_string()));
        assert_eq!(config.retry_after_seconds, Some(30));
    }

    #[test]
    fn test_failure_injector_basic() {
        let mut injector = FailureInjector::new();

        // No failure configured
        assert!(injector.check(&Operation::Probe).is_none());

        // Configure failure
        injector.inject_error(Operation::Submit, "SOURCE_MISSING", "Source not found");

        // Should return failure
        let config = injector.check(&Operation::Submit);
        assert!(config.is_some());
        assert_eq!(config.unwrap().error_code, Some("SOURCE_MISSING".to_string()));
    }

    #[test]
    fn test_failure_injector_fail_count() {
        let mut injector = FailureInjector::new();

        // Fail twice, then succeed
        injector.inject(
            Operation::Reserve,
            FailureConfig::busy(10).with_fail_count(2),
        );

        // First two calls should fail
        assert!(injector.check(&Operation::Reserve).is_some());
        assert!(injector.check(&Operation::Reserve).is_some());

        // Third call should succeed
        assert!(injector.check(&Operation::Reserve).is_none());
    }

    #[test]
    fn test_failure_injector_clear() {
        let mut injector = FailureInjector::new();

        injector.inject_error(Operation::Submit, "ERROR", "msg");
        assert!(injector.check(&Operation::Submit).is_some());

        injector.clear_op(&Operation::Submit);
        assert!(injector.check(&Operation::Submit).is_none());
    }
}
