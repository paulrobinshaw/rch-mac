//! Run state machine
//!
//! Run states: QUEUED → RUNNING → {SUCCEEDED | FAILED | CANCELLED}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use super::{next_seq, now_rfc3339, TerminalState};

/// Schema version for run_state.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/run_state@1";

/// Run state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunState {
    /// Run is queued, waiting to start
    Queued,
    /// Run is actively executing
    Running,
    /// Run completed successfully
    Succeeded,
    /// Run failed
    Failed,
    /// Run was cancelled
    Cancelled,
}

impl TerminalState for RunState {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled
        )
    }
}

impl RunState {
    /// Check if transition from this state to target is valid
    pub fn can_transition_to(&self, target: RunState) -> bool {
        match (self, target) {
            // From QUEUED
            (RunState::Queued, RunState::Running) => true,
            (RunState::Queued, RunState::Cancelled) => true,
            (RunState::Queued, RunState::Failed) => true, // Can fail before starting

            // From RUNNING
            (RunState::Running, RunState::Succeeded) => true,
            (RunState::Running, RunState::Failed) => true,
            (RunState::Running, RunState::Cancelled) => true,

            // Terminal states cannot transition
            _ => false,
        }
    }
}

/// Current step information for a running run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentStep {
    /// Step index (0-based)
    pub index: usize,

    /// Job ID for this step
    pub job_id: String,

    /// Action being performed (build, test)
    pub action: String,
}

/// Run state artifact data (run_state.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStateData {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// Run identifier
    pub run_id: String,

    /// Current state
    pub state: RunState,

    /// When the run was created
    pub created_at: DateTime<Utc>,

    /// When the state was last updated
    pub updated_at: DateTime<Utc>,

    /// Current step being executed (None if not running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<CurrentStep>,

    /// Monotonic sequence counter for ordering
    pub seq: u64,
}

/// Errors for run state operations
#[derive(Debug, thiserror::Error)]
pub enum RunStateError {
    #[error("Invalid state transition from {from:?} to {to:?}")]
    InvalidTransition { from: RunState, to: RunState },

    #[error("Run is in terminal state {0:?}")]
    TerminalState(RunState),

    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

impl RunStateData {
    /// Create a new run in QUEUED state
    pub fn new(run_id: String) -> Self {
        let now = now_rfc3339();
        Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            run_id,
            state: RunState::Queued,
            created_at: now,
            updated_at: now,
            current_step: None,
            seq: next_seq(),
        }
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: RunState) -> Result<(), RunStateError> {
        if !self.state.can_transition_to(new_state) {
            return Err(RunStateError::InvalidTransition {
                from: self.state,
                to: new_state,
            });
        }

        self.state = new_state;
        self.updated_at = now_rfc3339();
        self.seq = next_seq();

        // Clear current step if entering terminal state
        if new_state.is_terminal() {
            self.current_step = None;
        }

        Ok(())
    }

    /// Start the run (QUEUED → RUNNING)
    pub fn start(&mut self) -> Result<(), RunStateError> {
        self.transition(RunState::Running)
    }

    /// Mark run as succeeded
    pub fn succeed(&mut self) -> Result<(), RunStateError> {
        self.transition(RunState::Succeeded)
    }

    /// Mark run as failed
    pub fn fail(&mut self) -> Result<(), RunStateError> {
        self.transition(RunState::Failed)
    }

    /// Cancel the run
    pub fn cancel(&mut self) -> Result<(), RunStateError> {
        self.transition(RunState::Cancelled)
    }

    /// Set the current step being executed
    pub fn set_current_step(&mut self, step: CurrentStep) {
        self.current_step = Some(step);
        self.updated_at = now_rfc3339();
        self.seq = next_seq();
    }

    /// Clear the current step
    pub fn clear_current_step(&mut self) {
        self.current_step = None;
        self.updated_at = now_rfc3339();
        self.seq = next_seq();
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
    pub fn write_to_file(&self, path: &Path) -> Result<(), RunStateError> {
        let json = self.to_json()?;

        // Write to temp file first
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, &json)?;

        // Atomic rename
        fs::rename(&temp_path, path)?;

        Ok(())
    }

    /// Load from file
    pub fn from_file(path: &Path) -> Result<Self, RunStateError> {
        let json = fs::read_to_string(path)?;
        Ok(Self::from_json(&json)?)
    }

    /// Write to run directory as run_state.json
    pub fn write_to_run_dir(&self, run_dir: &Path) -> Result<(), RunStateError> {
        let path = run_dir.join("run_state.json");
        self.write_to_file(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_run_state() {
        let state = RunStateData::new("run-123".to_string());
        assert_eq!(state.run_id, "run-123");
        assert_eq!(state.state, RunState::Queued);
        assert_eq!(state.schema_version, SCHEMA_VERSION);
        assert!(state.current_step.is_none());
    }

    #[test]
    fn test_valid_transitions() {
        let mut state = RunStateData::new("run-123".to_string());

        // QUEUED → RUNNING
        assert!(state.start().is_ok());
        assert_eq!(state.state, RunState::Running);

        // RUNNING → SUCCEEDED
        assert!(state.succeed().is_ok());
        assert_eq!(state.state, RunState::Succeeded);
    }

    #[test]
    fn test_invalid_transition() {
        let mut state = RunStateData::new("run-123".to_string());

        // Cannot go directly from QUEUED to SUCCEEDED
        let result = state.transition(RunState::Succeeded);
        assert!(result.is_err());
    }

    #[test]
    fn test_terminal_state_no_transition() {
        let mut state = RunStateData::new("run-123".to_string());
        state.start().unwrap();
        state.succeed().unwrap();

        // Cannot transition from terminal state
        let result = state.transition(RunState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn test_current_step() {
        let mut state = RunStateData::new("run-123".to_string());
        state.start().unwrap();

        state.set_current_step(CurrentStep {
            index: 0,
            job_id: "job-456".to_string(),
            action: "build".to_string(),
        });

        assert!(state.current_step.is_some());
        assert_eq!(state.current_step.as_ref().unwrap().index, 0);
        assert_eq!(state.current_step.as_ref().unwrap().action, "build");
    }

    #[test]
    fn test_terminal_clears_current_step() {
        let mut state = RunStateData::new("run-123".to_string());
        state.start().unwrap();
        state.set_current_step(CurrentStep {
            index: 0,
            job_id: "job-456".to_string(),
            action: "build".to_string(),
        });

        state.succeed().unwrap();
        assert!(state.current_step.is_none());
    }

    #[test]
    fn test_serialization() {
        let state = RunStateData::new("run-123".to_string());
        let json = state.to_json().unwrap();

        assert!(json.contains("\"run_id\": \"run-123\""));
        assert!(json.contains("\"state\": \"QUEUED\""));
        assert!(json.contains("\"schema_version\": 1"));
    }

    #[test]
    fn test_deserialization() {
        let state = RunStateData::new("run-123".to_string());
        let json = state.to_json().unwrap();

        let parsed = RunStateData::from_json(&json).unwrap();
        assert_eq!(parsed.run_id, state.run_id);
        assert_eq!(parsed.state, state.state);
    }

    #[test]
    fn test_seq_increments() {
        let state1 = RunStateData::new("run-1".to_string());
        let state2 = RunStateData::new("run-2".to_string());

        assert!(state2.seq > state1.seq);
    }

    #[test]
    fn test_cancel_from_queued() {
        let mut state = RunStateData::new("run-123".to_string());
        assert!(state.cancel().is_ok());
        assert_eq!(state.state, RunState::Cancelled);
    }

    #[test]
    fn test_cancel_from_running() {
        let mut state = RunStateData::new("run-123".to_string());
        state.start().unwrap();
        assert!(state.cancel().is_ok());
        assert_eq!(state.state, RunState::Cancelled);
    }

    #[test]
    fn test_fail_from_queued() {
        let mut state = RunStateData::new("run-123".to_string());
        assert!(state.fail().is_ok());
        assert_eq!(state.state, RunState::Failed);
    }

    #[test]
    fn test_write_and_read_file() {
        let state = RunStateData::new("run-123".to_string());
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_run_state.json");

        state.write_to_file(&path).unwrap();

        let loaded = RunStateData::from_file(&path).unwrap();
        assert_eq!(loaded.run_id, state.run_id);
        assert_eq!(loaded.state, state.state);

        // Cleanup
        let _ = fs::remove_file(&path);
    }
}
