//! Job state machine
//!
//! Job states: QUEUED → RUNNING → {SUCCEEDED | FAILED | CANCELLED}
//! with CANCEL_REQUESTED as intermediate state

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use super::{next_seq, now_rfc3339, TerminalState};

/// Schema version for job_state.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/job_state@1";

/// Job state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    /// Job is queued, waiting to start
    Queued,
    /// Job is actively executing
    Running,
    /// Cancellation has been requested (intermediate state)
    CancelRequested,
    /// Job completed successfully
    Succeeded,
    /// Job failed
    Failed,
    /// Job was cancelled
    Cancelled,
}

impl TerminalState for JobState {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        )
    }
}

impl JobState {
    /// Check if transition from this state to target is valid
    pub fn can_transition_to(&self, target: JobState) -> bool {
        match (self, target) {
            // From QUEUED
            (JobState::Queued, JobState::Running) => true,
            (JobState::Queued, JobState::Cancelled) => true,
            (JobState::Queued, JobState::Failed) => true, // Can fail before starting

            // From RUNNING
            (JobState::Running, JobState::Succeeded) => true,
            (JobState::Running, JobState::Failed) => true,
            (JobState::Running, JobState::CancelRequested) => true,
            (JobState::Running, JobState::Cancelled) => true, // Direct cancel if worker responds immediately

            // From CANCEL_REQUESTED
            (JobState::CancelRequested, JobState::Cancelled) => true,
            (JobState::CancelRequested, JobState::Failed) => true, // Can fail while cancelling
            (JobState::CancelRequested, JobState::Succeeded) => true, // Completed before cancel took effect

            // Terminal states cannot transition
            _ => false,
        }
    }

    /// Check if this is a cancel-related state
    pub fn is_cancelling(&self) -> bool {
        matches!(self, JobState::CancelRequested | JobState::Cancelled)
    }
}

/// Job state artifact data (job_state.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStateData {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// Run identifier (parent run)
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash of job inputs)
    pub job_key: String,

    /// Current state
    pub state: JobState,

    /// When the job was created
    pub created_at: DateTime<Utc>,

    /// When the state was last updated
    pub updated_at: DateTime<Utc>,

    /// Monotonic sequence counter for ordering
    pub seq: u64,
}

/// Errors for job state operations
#[derive(Debug, thiserror::Error)]
pub enum JobStateError {
    #[error("Invalid state transition from {from:?} to {to:?}")]
    InvalidTransition { from: JobState, to: JobState },

    #[error("Job is in terminal state {0:?}")]
    TerminalState(JobState),

    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

impl JobStateData {
    /// Create a new job in QUEUED state
    pub fn new(run_id: String, job_id: String, job_key: String) -> Self {
        let now = now_rfc3339();
        Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            run_id,
            job_id,
            job_key,
            state: JobState::Queued,
            created_at: now,
            updated_at: now,
            seq: next_seq(),
        }
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: JobState) -> Result<(), JobStateError> {
        if !self.state.can_transition_to(new_state) {
            return Err(JobStateError::InvalidTransition {
                from: self.state,
                to: new_state,
            });
        }

        self.state = new_state;
        self.updated_at = now_rfc3339();
        self.seq = next_seq();

        Ok(())
    }

    /// Start the job (QUEUED → RUNNING)
    pub fn start(&mut self) -> Result<(), JobStateError> {
        self.transition(JobState::Running)
    }

    /// Mark job as succeeded
    pub fn succeed(&mut self) -> Result<(), JobStateError> {
        self.transition(JobState::Succeeded)
    }

    /// Mark job as failed
    pub fn fail(&mut self) -> Result<(), JobStateError> {
        self.transition(JobState::Failed)
    }

    /// Request cancellation (RUNNING → CANCEL_REQUESTED)
    pub fn request_cancel(&mut self) -> Result<(), JobStateError> {
        self.transition(JobState::CancelRequested)
    }

    /// Complete cancellation
    pub fn cancel(&mut self) -> Result<(), JobStateError> {
        self.transition(JobState::Cancelled)
    }

    /// Check if job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Check if cancellation is in progress
    pub fn is_cancelling(&self) -> bool {
        self.state.is_cancelling()
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write atomically to file (write-then-rename)
    pub fn write_to_file(&self, path: &Path) -> Result<(), JobStateError> {
        let json = self.to_json()?;

        // Write to temp file first
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, &json)?;

        // Atomic rename
        fs::rename(&temp_path, path)?;

        Ok(())
    }

    /// Load from file
    pub fn from_file(path: &Path) -> Result<Self, JobStateError> {
        let json = fs::read_to_string(path)?;
        Ok(Self::from_json(&json)?)
    }

    /// Write to job directory as job_state.json
    /// Path: <run_dir>/steps/<action>/<job_id>/job_state.json
    pub fn write_to_job_dir(&self, job_dir: &Path) -> Result<(), JobStateError> {
        let path = job_dir.join("job_state.json");
        self.write_to_file(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_job_state() {
        let state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        assert_eq!(state.run_id, "run-123");
        assert_eq!(state.job_id, "job-456");
        assert_eq!(state.job_key, "key-789");
        assert_eq!(state.state, JobState::Queued);
        assert_eq!(state.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn test_valid_transitions() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        // QUEUED → RUNNING
        assert!(state.start().is_ok());
        assert_eq!(state.state, JobState::Running);

        // RUNNING → SUCCEEDED
        assert!(state.succeed().is_ok());
        assert_eq!(state.state, JobState::Succeeded);
    }

    #[test]
    fn test_cancel_flow() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        state.start().unwrap();
        assert!(state.request_cancel().is_ok());
        assert_eq!(state.state, JobState::CancelRequested);
        assert!(state.is_cancelling());

        assert!(state.cancel().is_ok());
        assert_eq!(state.state, JobState::Cancelled);
        assert!(state.is_terminal());
    }

    #[test]
    fn test_cancel_requested_can_succeed() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        state.start().unwrap();
        state.request_cancel().unwrap();

        // Job completed before cancel took effect
        assert!(state.succeed().is_ok());
        assert_eq!(state.state, JobState::Succeeded);
    }

    #[test]
    fn test_cancel_requested_can_fail() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        state.start().unwrap();
        state.request_cancel().unwrap();

        // Job failed during cancellation
        assert!(state.fail().is_ok());
        assert_eq!(state.state, JobState::Failed);
    }

    #[test]
    fn test_invalid_transition() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        // Cannot go directly from QUEUED to SUCCEEDED
        let result = state.transition(JobState::Succeeded);
        assert!(result.is_err());
    }

    #[test]
    fn test_terminal_state_no_transition() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        state.start().unwrap();
        state.succeed().unwrap();

        // Cannot transition from terminal state
        let result = state.transition(JobState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialization() {
        let state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        let json = state.to_json().unwrap();

        assert!(json.contains("\"job_id\": \"job-456\""));
        assert!(json.contains("\"state\": \"QUEUED\""));
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"job_key\": \"key-789\""));
    }

    #[test]
    fn test_deserialization() {
        let state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        let json = state.to_json().unwrap();

        let parsed = JobStateData::from_json(&json).unwrap();
        assert_eq!(parsed.job_id, state.job_id);
        assert_eq!(parsed.state, state.state);
        assert_eq!(parsed.job_key, state.job_key);
    }

    #[test]
    fn test_seq_increments() {
        let state1 = JobStateData::new(
            "run-1".to_string(),
            "job-1".to_string(),
            "key-1".to_string(),
        );
        let state2 = JobStateData::new(
            "run-2".to_string(),
            "job-2".to_string(),
            "key-2".to_string(),
        );

        assert!(state2.seq > state1.seq);
    }

    #[test]
    fn test_direct_cancel_from_running() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        state.start().unwrap();

        // Worker responds immediately with cancellation
        assert!(state.cancel().is_ok());
        assert_eq!(state.state, JobState::Cancelled);
    }

    #[test]
    fn test_write_and_read_file() {
        let state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_job_state.json");

        state.write_to_file(&path).unwrap();

        let loaded = JobStateData::from_file(&path).unwrap();
        assert_eq!(loaded.job_id, state.job_id);
        assert_eq!(loaded.state, state.state);
        assert_eq!(loaded.job_key, state.job_key);

        // Cleanup
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_fail_from_queued() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        // Job can fail before starting (e.g., source bundle error)
        assert!(state.fail().is_ok());
        assert_eq!(state.state, JobState::Failed);
    }

    #[test]
    fn test_cancel_from_queued() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        // Job can be cancelled before starting
        assert!(state.cancel().is_ok());
        assert_eq!(state.state, JobState::Cancelled);
    }
}
