//! Timeout enforcement for RCH Xcode Lane
//!
//! Implements host-side timeout enforcement per bead axz.20:
//! - `overall_seconds`: maximum wall-clock time per job
//! - `idle_log_seconds`: maximum time without new log output
//! - `connect_timeout_seconds`: SSH connection timeout (handled in transport)
//!
//! All timeout enforcement is host-driven. The worker has no independent
//! timeout watchdog. When a timeout occurs, the host sends a cancel RPC
//! with the appropriate reason (TIMEOUT_OVERALL or TIMEOUT_IDLE).

use std::time::{Duration, Instant};

/// Timeout configuration
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    /// Maximum wall-clock time per job (default: 1800 = 30 min)
    pub overall_seconds: u64,

    /// Maximum time without new log output (default: 300 = 5 min)
    pub idle_log_seconds: u64,

    /// SSH connection timeout (default: 30)
    pub connect_timeout_seconds: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            overall_seconds: 1800,
            idle_log_seconds: 300,
            connect_timeout_seconds: 30,
        }
    }
}

impl TimeoutConfig {
    /// Validate timeout configuration per PLAN.md bounds
    pub fn validate(&self) -> Result<(), TimeoutValidationError> {
        // overall_seconds must be in (0, 86400]
        if self.overall_seconds == 0 || self.overall_seconds > 86400 {
            return Err(TimeoutValidationError::OverallOutOfBounds {
                value: self.overall_seconds,
            });
        }

        // idle_log_seconds must be in (0, overall_seconds]
        if self.idle_log_seconds == 0 || self.idle_log_seconds > self.overall_seconds {
            return Err(TimeoutValidationError::IdleOutOfBounds {
                value: self.idle_log_seconds,
                max: self.overall_seconds,
            });
        }

        // connect_timeout_seconds must be in (0, 300]
        if self.connect_timeout_seconds == 0 || self.connect_timeout_seconds > 300 {
            return Err(TimeoutValidationError::ConnectOutOfBounds {
                value: self.connect_timeout_seconds,
            });
        }

        Ok(())
    }

    /// Create TimeoutConfig from effective config values
    pub fn from_config(
        overall: Option<u64>,
        idle: Option<u64>,
        connect: Option<u64>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            overall_seconds: overall.unwrap_or(defaults.overall_seconds),
            idle_log_seconds: idle.unwrap_or(defaults.idle_log_seconds),
            connect_timeout_seconds: connect.unwrap_or(defaults.connect_timeout_seconds),
        }
    }
}

/// Timeout validation errors
#[derive(Debug, thiserror::Error)]
pub enum TimeoutValidationError {
    #[error("overall_seconds must be in (0, 86400], got {value}")]
    OverallOutOfBounds { value: u64 },

    #[error("idle_log_seconds must be in (0, {max}], got {value}")]
    IdleOutOfBounds { value: u64, max: u64 },

    #[error("connect_timeout_seconds must be in (0, 300], got {value}")]
    ConnectOutOfBounds { value: u64 },
}

/// Timeout check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutStatus {
    /// No timeout has occurred
    Ok,
    /// Overall wall-clock timeout exceeded
    OverallTimeout,
    /// Idle log timeout exceeded
    IdleTimeout,
}

impl TimeoutStatus {
    /// Returns true if a timeout occurred
    pub fn is_timeout(&self) -> bool {
        !matches!(self, TimeoutStatus::Ok)
    }

    /// Get the failure subkind for this timeout
    pub fn failure_subkind(&self) -> Option<&'static str> {
        match self {
            TimeoutStatus::Ok => None,
            TimeoutStatus::OverallTimeout => Some("TIMEOUT_OVERALL"),
            TimeoutStatus::IdleTimeout => Some("TIMEOUT_IDLE"),
        }
    }
}

/// Timeout enforcer for a job execution
///
/// Tracks wall-clock time and activity to enforce overall and idle timeouts.
/// The enforcer does NOT send cancellation itself - it only checks for
/// timeout conditions. The caller is responsible for sending the cancel RPC.
#[derive(Debug)]
pub struct TimeoutEnforcer {
    /// Timeout configuration
    config: TimeoutConfig,

    /// When the job started
    start_time: Instant,

    /// Last activity timestamp (set externally from log streamer)
    last_activity: Instant,
}

impl TimeoutEnforcer {
    /// Create a new timeout enforcer
    pub fn new(config: TimeoutConfig) -> Self {
        let now = Instant::now();
        Self {
            config,
            start_time: now,
            last_activity: now,
        }
    }

    /// Create with default config
    pub fn with_defaults() -> Self {
        Self::new(TimeoutConfig::default())
    }

    /// Update the last activity timestamp
    ///
    /// Call this when new log data or events are received.
    pub fn record_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Update activity from an external timestamp
    ///
    /// This is used to sync with LogStreamer's activity tracking.
    pub fn sync_activity(&mut self, last_activity: Instant) {
        self.last_activity = last_activity;
    }

    /// Check for timeout conditions
    ///
    /// Returns the timeout status. If a timeout occurred, the caller
    /// should send a cancel RPC with the appropriate reason.
    pub fn check(&self) -> TimeoutStatus {
        let now = Instant::now();

        // Check overall timeout first
        let elapsed = now.duration_since(self.start_time);
        if elapsed > Duration::from_secs(self.config.overall_seconds) {
            return TimeoutStatus::OverallTimeout;
        }

        // Check idle timeout
        let idle_duration = now.duration_since(self.last_activity);
        if idle_duration > Duration::from_secs(self.config.idle_log_seconds) {
            return TimeoutStatus::IdleTimeout;
        }

        TimeoutStatus::Ok
    }

    /// Get elapsed time since job start
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get time since last activity
    pub fn idle_time(&self) -> Duration {
        self.last_activity.elapsed()
    }

    /// Get remaining time before overall timeout
    pub fn overall_remaining(&self) -> Duration {
        let overall = Duration::from_secs(self.config.overall_seconds);
        let elapsed = self.elapsed();
        overall.saturating_sub(elapsed)
    }

    /// Get remaining time before idle timeout
    pub fn idle_remaining(&self) -> Duration {
        let idle_limit = Duration::from_secs(self.config.idle_log_seconds);
        let idle_time = self.idle_time();
        idle_limit.saturating_sub(idle_time)
    }

    /// Reset the enforcer for a new job
    pub fn reset(&mut self) {
        let now = Instant::now();
        self.start_time = now;
        self.last_activity = now;
    }

    /// Get the config
    pub fn config(&self) -> &TimeoutConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_timeout_config_default() {
        let config = TimeoutConfig::default();
        assert_eq!(config.overall_seconds, 1800);
        assert_eq!(config.idle_log_seconds, 300);
        assert_eq!(config.connect_timeout_seconds, 30);
    }

    #[test]
    fn test_timeout_config_validation_valid() {
        let config = TimeoutConfig {
            overall_seconds: 1800,
            idle_log_seconds: 300,
            connect_timeout_seconds: 30,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_timeout_config_validation_overall_zero() {
        let config = TimeoutConfig {
            overall_seconds: 0,
            idle_log_seconds: 300,
            connect_timeout_seconds: 30,
        };
        assert!(matches!(
            config.validate(),
            Err(TimeoutValidationError::OverallOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_timeout_config_validation_overall_too_large() {
        let config = TimeoutConfig {
            overall_seconds: 86401,
            idle_log_seconds: 300,
            connect_timeout_seconds: 30,
        };
        assert!(matches!(
            config.validate(),
            Err(TimeoutValidationError::OverallOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_timeout_config_validation_idle_exceeds_overall() {
        let config = TimeoutConfig {
            overall_seconds: 100,
            idle_log_seconds: 200,
            connect_timeout_seconds: 30,
        };
        assert!(matches!(
            config.validate(),
            Err(TimeoutValidationError::IdleOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_timeout_config_validation_connect_zero() {
        let config = TimeoutConfig {
            overall_seconds: 1800,
            idle_log_seconds: 300,
            connect_timeout_seconds: 0,
        };
        assert!(matches!(
            config.validate(),
            Err(TimeoutValidationError::ConnectOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_timeout_config_validation_connect_too_large() {
        let config = TimeoutConfig {
            overall_seconds: 1800,
            idle_log_seconds: 300,
            connect_timeout_seconds: 301,
        };
        assert!(matches!(
            config.validate(),
            Err(TimeoutValidationError::ConnectOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_enforcer_no_timeout() {
        let config = TimeoutConfig {
            overall_seconds: 10,
            idle_log_seconds: 5,
            connect_timeout_seconds: 30,
        };
        let enforcer = TimeoutEnforcer::new(config);

        // Should be Ok immediately after creation
        assert_eq!(enforcer.check(), TimeoutStatus::Ok);
    }

    #[test]
    fn test_enforcer_idle_timeout() {
        let config = TimeoutConfig {
            overall_seconds: 10,
            idle_log_seconds: 1, // 1 second idle timeout
            connect_timeout_seconds: 30,
        };
        let enforcer = TimeoutEnforcer::new(config);

        // Wait for idle timeout
        sleep(Duration::from_millis(1100));

        // Should now be idle timeout
        assert_eq!(enforcer.check(), TimeoutStatus::IdleTimeout);
    }

    #[test]
    fn test_enforcer_activity_resets_idle() {
        let config = TimeoutConfig {
            overall_seconds: 10,
            idle_log_seconds: 1,
            connect_timeout_seconds: 30,
        };
        let mut enforcer = TimeoutEnforcer::new(config);

        // Wait a bit
        sleep(Duration::from_millis(500));

        // Record activity
        enforcer.record_activity();

        // Wait a bit more
        sleep(Duration::from_millis(600));

        // Should still be Ok because we recorded activity
        assert_eq!(enforcer.check(), TimeoutStatus::Ok);
    }

    #[test]
    fn test_timeout_status_is_timeout() {
        assert!(!TimeoutStatus::Ok.is_timeout());
        assert!(TimeoutStatus::OverallTimeout.is_timeout());
        assert!(TimeoutStatus::IdleTimeout.is_timeout());
    }

    #[test]
    fn test_timeout_status_failure_subkind() {
        assert_eq!(TimeoutStatus::Ok.failure_subkind(), None);
        assert_eq!(
            TimeoutStatus::OverallTimeout.failure_subkind(),
            Some("TIMEOUT_OVERALL")
        );
        assert_eq!(
            TimeoutStatus::IdleTimeout.failure_subkind(),
            Some("TIMEOUT_IDLE")
        );
    }

    #[test]
    fn test_from_config() {
        let config = TimeoutConfig::from_config(Some(600), Some(60), Some(15));
        assert_eq!(config.overall_seconds, 600);
        assert_eq!(config.idle_log_seconds, 60);
        assert_eq!(config.connect_timeout_seconds, 15);

        // Test with None uses defaults
        let config2 = TimeoutConfig::from_config(None, None, None);
        assert_eq!(config2.overall_seconds, 1800);
        assert_eq!(config2.idle_log_seconds, 300);
        assert_eq!(config2.connect_timeout_seconds, 30);
    }

    #[test]
    fn test_enforcer_reset() {
        let config = TimeoutConfig {
            overall_seconds: 1,
            idle_log_seconds: 1,
            connect_timeout_seconds: 30,
        };
        let mut enforcer = TimeoutEnforcer::new(config);

        // Wait for timeout
        sleep(Duration::from_millis(1100));
        assert!(enforcer.check().is_timeout());

        // Reset
        enforcer.reset();

        // Should be Ok again
        assert_eq!(enforcer.check(), TimeoutStatus::Ok);
    }
}
