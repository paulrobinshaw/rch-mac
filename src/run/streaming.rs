//! Log streaming and activity tracking for RCH Xcode Lane
//!
//! Implements cursor-based log streaming via the tail RPC and activity tracking
//! for idle watchdog support (per PLAN.md and rch-mac-axz.13).
//!
//! ## Usage
//!
//! The LogStreamer manages the tail loop and exposes `last_activity_timestamp()`
//! for timeout enforcement (axz.20). Activity is defined as:
//! - New log bytes in log_chunk
//! - New events in events array
//!
//! Empty responses do NOT count as activity.
//!
//! ## Fallback mode
//!
//! If the worker does not advertise `tail` in its features list, the streamer
//! operates in fallback mode using periodic `status` checks. This prevents
//! silent hangs but provides no streaming output.

use std::time::{Duration, Instant};

/// Configuration for log streaming
#[derive(Debug, Clone)]
pub struct LogStreamerConfig {
    /// Poll interval for tail requests (recommended: 1-2 seconds)
    pub poll_interval: Duration,

    /// Maximum bytes to request per tail call (optional)
    pub max_bytes_per_request: Option<u64>,

    /// Maximum events to request per tail call (optional)
    pub max_events_per_request: Option<u32>,
}

impl Default for LogStreamerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            max_bytes_per_request: None,
            max_events_per_request: None,
        }
    }
}

impl LogStreamerConfig {
    /// Create a new config with the given poll interval
    pub fn with_poll_interval(poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            ..Default::default()
        }
    }
}

/// Stream mode based on worker capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Worker supports tail RPC - use cursor-based streaming
    Tail,
    /// Worker lacks tail feature - fall back to periodic status checks
    StatusFallback,
}

/// Update from a single poll iteration
#[derive(Debug, Clone)]
pub struct StreamUpdate {
    /// New log content (if any)
    pub log_chunk: Option<String>,

    /// New structured events (if any)
    pub events: Vec<serde_json::Value>,

    /// Whether this update contained activity (new data)
    pub had_activity: bool,

    /// Whether the stream is complete (job finished)
    pub complete: bool,
}

impl StreamUpdate {
    /// Create an empty update (no new data)
    pub fn empty() -> Self {
        Self {
            log_chunk: None,
            events: Vec::new(),
            had_activity: false,
            complete: false,
        }
    }

    /// Create a complete update (stream ended)
    pub fn completed() -> Self {
        Self {
            log_chunk: None,
            events: Vec::new(),
            had_activity: false,
            complete: true,
        }
    }
}

/// Error during log streaming
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// RPC error from tail/status call
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Job not found
    #[error("Job not found: {0}")]
    JobNotFound(String),

    /// Unexpected job state
    #[error("Unexpected job state: {0}")]
    UnexpectedState(String),
}

/// Log streamer state
///
/// Manages the tail loop and tracks activity for idle watchdog support.
#[derive(Debug)]
pub struct LogStreamer {
    /// Configuration
    config: LogStreamerConfig,

    /// Stream mode (Tail or StatusFallback)
    mode: StreamMode,

    /// Current cursor for tail requests (None = start from beginning)
    cursor: Option<String>,

    /// Timestamp of last activity (new log bytes or events)
    last_activity: Instant,

    /// Whether the stream is complete
    complete: bool,

    /// Total bytes received
    total_bytes: u64,

    /// Total events received
    total_events: u64,
}

impl LogStreamer {
    /// Create a new log streamer
    ///
    /// The `has_tail_feature` flag determines the stream mode:
    /// - true: Use cursor-based tail RPC
    /// - false: Fall back to periodic status checks
    pub fn new(config: LogStreamerConfig, has_tail_feature: bool) -> Self {
        let mode = if has_tail_feature {
            StreamMode::Tail
        } else {
            StreamMode::StatusFallback
        };

        Self {
            config,
            mode,
            cursor: None,
            last_activity: Instant::now(),
            complete: false,
            total_bytes: 0,
            total_events: 0,
        }
    }

    /// Get the stream mode
    pub fn mode(&self) -> StreamMode {
        self.mode
    }

    /// Get the timestamp of last activity
    ///
    /// This is exposed for timeout enforcement (axz.20). The idle watchdog
    /// compares current time against this timestamp; if the gap exceeds
    /// `idle_log_seconds`, the job should be cancelled with TIMEOUT_IDLE.
    pub fn last_activity_timestamp(&self) -> Instant {
        self.last_activity
    }

    /// Get duration since last activity
    pub fn time_since_activity(&self) -> Duration {
        self.last_activity.elapsed()
    }

    /// Check if the stream is complete
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Get the current cursor
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Get total bytes received
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Get total events received
    pub fn total_events(&self) -> u64 {
        self.total_events
    }

    /// Get recommended sleep duration before next poll
    pub fn poll_interval(&self) -> Duration {
        self.config.poll_interval
    }

    /// Process a tail response
    ///
    /// Updates internal state and returns a StreamUpdate.
    /// Call this with the response from `RpcClient::tail()`.
    pub fn process_tail_response(
        &mut self,
        next_cursor: Option<String>,
        log_chunk: Option<String>,
        events: Vec<serde_json::Value>,
    ) -> StreamUpdate {
        // Check for activity
        let log_bytes = log_chunk.as_ref().map(|c| c.len()).unwrap_or(0);
        let event_count = events.len();
        let had_activity = log_bytes > 0 || event_count > 0;

        // Update activity timestamp if we got new data
        if had_activity {
            self.last_activity = Instant::now();
            self.total_bytes += log_bytes as u64;
            self.total_events += event_count as u64;
        }

        // Update cursor
        let complete = next_cursor.is_none();
        self.cursor = next_cursor;
        self.complete = complete;

        StreamUpdate {
            log_chunk,
            events,
            had_activity,
            complete,
        }
    }

    /// Process a status response (for fallback mode)
    ///
    /// Returns a StreamUpdate based on job state.
    /// In fallback mode, we only know when the job finishes, not the logs.
    pub fn process_status_response(&mut self, _job_state: &str, is_terminal: bool) -> StreamUpdate {
        if is_terminal {
            self.complete = true;
            StreamUpdate::completed()
        } else {
            // Job still running - no log data in fallback mode
            StreamUpdate::empty()
        }
    }

    /// Mark activity manually (e.g., when job starts)
    ///
    /// This resets the idle watchdog timer.
    pub fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Reset the streamer for a new stream
    pub fn reset(&mut self) {
        self.cursor = None;
        self.last_activity = Instant::now();
        self.complete = false;
        self.total_bytes = 0;
        self.total_events = 0;
    }

    /// Build tail request parameters
    ///
    /// Returns (cursor, max_bytes, max_events) tuple for the RPC call.
    pub fn tail_request_params(&self) -> (Option<String>, Option<u64>, Option<u32>) {
        (
            self.cursor.clone(),
            self.config.max_bytes_per_request,
            self.config.max_events_per_request,
        )
    }
}

/// Determines if a job state is terminal
pub fn is_terminal_state(state: &str) -> bool {
    matches!(state, "SUCCEEDED" | "FAILED" | "CANCELLED")
}

/// Check if worker has tail feature from probe capabilities
pub fn has_tail_feature(capabilities: &serde_json::Value) -> bool {
    capabilities
        .get("features")
        .and_then(|f| f.as_array())
        .map(|arr| arr.iter().any(|v| v.as_str() == Some("tail")))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_log_streamer_creation_with_tail() {
        let config = LogStreamerConfig::default();
        let streamer = LogStreamer::new(config, true);

        assert_eq!(streamer.mode(), StreamMode::Tail);
        assert!(!streamer.is_complete());
        assert!(streamer.cursor().is_none());
    }

    #[test]
    fn test_log_streamer_creation_fallback() {
        let config = LogStreamerConfig::default();
        let streamer = LogStreamer::new(config, false);

        assert_eq!(streamer.mode(), StreamMode::StatusFallback);
    }

    #[test]
    fn test_process_tail_response_with_data() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        let update = streamer.process_tail_response(
            Some("cursor-100".to_string()),
            Some("Build started\n".to_string()),
            vec![json!({"stage": "compile", "kind": "start"})],
        );

        assert!(update.had_activity);
        assert!(!update.complete);
        assert_eq!(update.log_chunk, Some("Build started\n".to_string()));
        assert_eq!(update.events.len(), 1);
        assert_eq!(streamer.cursor(), Some("cursor-100"));
        assert_eq!(streamer.total_bytes(), 14);
        assert_eq!(streamer.total_events(), 1);
    }

    #[test]
    fn test_process_tail_response_empty() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        // First, add some data to set an initial activity time
        let initial_activity = streamer.last_activity_timestamp();

        // Process an empty response (no new data)
        std::thread::sleep(Duration::from_millis(10));
        let update = streamer.process_tail_response(
            Some("cursor-100".to_string()),
            None,
            vec![],
        );

        assert!(!update.had_activity);
        assert!(!update.complete);
        // Activity timestamp should NOT have updated (empty response)
        assert_eq!(streamer.last_activity_timestamp(), initial_activity);
    }

    #[test]
    fn test_process_tail_response_complete() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        let update = streamer.process_tail_response(
            None, // next_cursor = null means complete
            Some("Final line\n".to_string()),
            vec![],
        );

        assert!(update.had_activity);
        assert!(update.complete);
        assert!(streamer.is_complete());
        assert!(streamer.cursor().is_none());
    }

    #[test]
    fn test_process_status_response_running() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, false);

        let update = streamer.process_status_response("RUNNING", false);

        assert!(!update.had_activity);
        assert!(!update.complete);
        assert!(!streamer.is_complete());
    }

    #[test]
    fn test_process_status_response_terminal() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, false);

        let update = streamer.process_status_response("SUCCEEDED", true);

        assert!(!update.had_activity);
        assert!(update.complete);
        assert!(streamer.is_complete());
    }

    #[test]
    fn test_time_since_activity() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        // Initially should be very small
        assert!(streamer.time_since_activity() < Duration::from_millis(100));

        // Wait a bit
        std::thread::sleep(Duration::from_millis(50));

        // Time since activity should have increased
        assert!(streamer.time_since_activity() >= Duration::from_millis(50));

        // Add activity
        streamer.mark_activity();

        // Time since activity should reset
        assert!(streamer.time_since_activity() < Duration::from_millis(10));
    }

    #[test]
    fn test_has_tail_feature() {
        let capabilities_with_tail = json!({
            "protocol_min": 1,
            "protocol_max": 1,
            "features": ["tail", "fetch", "has_source"]
        });

        let capabilities_without_tail = json!({
            "protocol_min": 1,
            "protocol_max": 1,
            "features": ["fetch", "has_source"]
        });

        let capabilities_no_features = json!({
            "protocol_min": 1,
            "protocol_max": 1
        });

        assert!(has_tail_feature(&capabilities_with_tail));
        assert!(!has_tail_feature(&capabilities_without_tail));
        assert!(!has_tail_feature(&capabilities_no_features));
    }

    #[test]
    fn test_is_terminal_state() {
        assert!(is_terminal_state("SUCCEEDED"));
        assert!(is_terminal_state("FAILED"));
        assert!(is_terminal_state("CANCELLED"));
        assert!(!is_terminal_state("RUNNING"));
        assert!(!is_terminal_state("QUEUED"));
        assert!(!is_terminal_state("CANCEL_REQUESTED"));
    }

    #[test]
    fn test_config_with_poll_interval() {
        let config = LogStreamerConfig::with_poll_interval(Duration::from_millis(500));
        assert_eq!(config.poll_interval, Duration::from_millis(500));
    }

    #[test]
    fn test_streamer_reset() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        // Add some state
        streamer.process_tail_response(
            Some("cursor-100".to_string()),
            Some("Log data\n".to_string()),
            vec![json!({"event": 1})],
        );

        assert_eq!(streamer.cursor(), Some("cursor-100"));
        assert!(streamer.total_bytes() > 0);
        assert!(streamer.total_events() > 0);

        // Reset
        streamer.reset();

        assert!(streamer.cursor().is_none());
        assert_eq!(streamer.total_bytes(), 0);
        assert_eq!(streamer.total_events(), 0);
        assert!(!streamer.is_complete());
    }

    #[test]
    fn test_tail_request_params() {
        let config = LogStreamerConfig {
            poll_interval: Duration::from_secs(1),
            max_bytes_per_request: Some(65536),
            max_events_per_request: Some(100),
        };
        let mut streamer = LogStreamer::new(config, true);

        // Initial params (no cursor)
        let (cursor, max_bytes, max_events) = streamer.tail_request_params();
        assert!(cursor.is_none());
        assert_eq!(max_bytes, Some(65536));
        assert_eq!(max_events, Some(100));

        // After processing a response with cursor
        streamer.process_tail_response(Some("cursor-50".to_string()), None, vec![]);

        let (cursor, _, _) = streamer.tail_request_params();
        assert_eq!(cursor, Some("cursor-50".to_string()));
    }

    #[test]
    fn test_activity_on_log_chunk_only() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        let update = streamer.process_tail_response(
            Some("cursor-1".to_string()),
            Some("Some logs".to_string()),
            vec![], // No events
        );

        assert!(update.had_activity);
        assert_eq!(streamer.total_bytes(), 9);
        assert_eq!(streamer.total_events(), 0);
    }

    #[test]
    fn test_activity_on_events_only() {
        let config = LogStreamerConfig::default();
        let mut streamer = LogStreamer::new(config, true);

        let update = streamer.process_tail_response(
            Some("cursor-1".to_string()),
            None, // No log chunk
            vec![json!({"event": 1}), json!({"event": 2})],
        );

        assert!(update.had_activity);
        assert_eq!(streamer.total_bytes(), 0);
        assert_eq!(streamer.total_events(), 2);
    }
}
