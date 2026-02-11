//! Mock Worker State Management
//!
//! Manages jobs, leases, and source bundles for the mock worker.

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Job state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    /// Job accepted, waiting to run
    Queued,
    /// Job is currently running
    Running,
    /// Job completed successfully
    Succeeded,
    /// Job failed
    Failed,
    /// Cancellation requested
    CancelRequested,
    /// Job was cancelled
    Cancelled,
}

impl JobState {
    /// Returns true if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Failed | JobState::Cancelled)
    }
}

/// Represents a job in the mock worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier
    pub job_id: String,
    /// SHA-256 hash of JCS(job_key_inputs)
    pub job_key: String,
    /// Source bundle SHA-256
    pub source_sha256: String,
    /// Lease ID this job belongs to
    pub lease_id: String,
    /// Current state
    pub state: JobState,
    /// State history for debugging
    pub state_history: Vec<(JobState, DateTime<Utc>)>,
    /// Exit code (when terminal)
    pub exit_code: Option<i32>,
    /// Log entries
    pub logs: Vec<LogEntry>,
    /// Cursor for log pagination
    pub log_cursor: usize,
    /// Whether artifacts exist
    pub artifacts_available: bool,
    /// Job creation time
    pub created_at: DateTime<Utc>,
    /// Last state change time
    pub updated_at: DateTime<Utc>,
}

impl Job {
    /// Create a new job in QUEUED state
    pub fn new(job_id: String, job_key: String, source_sha256: String, lease_id: String) -> Self {
        let now = Utc::now();
        Self {
            job_id,
            job_key,
            source_sha256,
            lease_id,
            state: JobState::Queued,
            state_history: vec![(JobState::Queued, now)],
            exit_code: None,
            logs: Vec::new(),
            log_cursor: 0,
            artifacts_available: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: JobState) {
        let now = Utc::now();
        self.state = new_state;
        self.state_history.push((new_state, now));
        self.updated_at = now;
    }

    /// Add a log entry
    pub fn add_log(&mut self, stream: &str, line: &str) {
        self.logs.push(LogEntry {
            timestamp: Utc::now(),
            stream: stream.to_string(),
            line: line.to_string(),
        });
    }
}

/// A log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub stream: String,
    pub line: String,
}

/// Represents a lease (capacity reservation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    /// Unique lease identifier
    pub lease_id: String,
    /// When the lease was granted
    pub created_at: DateTime<Utc>,
    /// When the lease expires
    pub expires_at: DateTime<Utc>,
    /// Associated run ID
    pub run_id: String,
}

impl Lease {
    /// Create a new lease with the given TTL in seconds
    pub fn new(lease_id: String, run_id: String, ttl_seconds: i64) -> Self {
        let now = Utc::now();
        Self {
            lease_id,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(ttl_seconds),
            run_id,
        }
    }

    /// Check if the lease has expired
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

/// Mock worker state container
#[derive(Debug, Default)]
pub struct MockState {
    /// Active leases by lease_id
    pub leases: HashMap<String, Lease>,
    /// Jobs by job_id
    pub jobs: HashMap<String, Job>,
    /// Source bundles by sha256 (content stored as bytes)
    pub sources: HashMap<String, Vec<u8>>,
    /// Artifacts by job_id (content stored as bytes)
    pub artifacts: HashMap<String, Vec<u8>>,
    /// Counter for generating unique IDs
    id_counter: u64,
}

impl MockState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self::default()
    }

    /// Generate a unique ID
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        format!("{}-{:08x}", prefix, self.id_counter)
    }

    /// Count active (non-expired) leases
    pub fn active_lease_count(&self) -> usize {
        self.leases.values().filter(|l| !l.is_expired()).count()
    }

    /// Get a job by ID
    pub fn get_job(&self, job_id: &str) -> Option<&Job> {
        self.jobs.get(job_id)
    }

    /// Get a mutable job by ID
    pub fn get_job_mut(&mut self, job_id: &str) -> Option<&mut Job> {
        self.jobs.get_mut(job_id)
    }

    /// Check if a source exists
    pub fn has_source(&self, sha256: &str) -> bool {
        self.sources.contains_key(sha256)
    }

    /// Store a source bundle
    pub fn store_source(&mut self, sha256: String, content: Vec<u8>) {
        self.sources.insert(sha256, content);
    }

    /// Remove a source (for GC simulation)
    pub fn evict_source(&mut self, sha256: &str) -> bool {
        self.sources.remove(sha256).is_some()
    }

    /// Store artifacts for a job
    pub fn store_artifacts(&mut self, job_id: String, content: Vec<u8>) {
        self.artifacts.insert(job_id, content);
    }

    /// Delete artifacts (for ARTIFACTS_GONE simulation)
    pub fn delete_artifacts(&mut self, job_id: &str) -> bool {
        self.artifacts.remove(job_id).is_some()
    }

    /// Get artifacts for a job
    pub fn get_artifacts(&self, job_id: &str) -> Option<&Vec<u8>> {
        self.artifacts.get(job_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_state_terminal() {
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Running.is_terminal());
        assert!(JobState::Succeeded.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(!JobState::CancelRequested.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
    }

    #[test]
    fn test_job_creation() {
        let job = Job::new(
            "job-001".to_string(),
            "key-abc".to_string(),
            "sha256-xyz".to_string(),
            "lease-001".to_string(),
        );

        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.state_history.len(), 1);
        assert!(!job.artifacts_available);
    }

    #[test]
    fn test_job_transition() {
        let mut job = Job::new(
            "job-001".to_string(),
            "key-abc".to_string(),
            "sha256-xyz".to_string(),
            "lease-001".to_string(),
        );

        job.transition(JobState::Running);
        assert_eq!(job.state, JobState::Running);
        assert_eq!(job.state_history.len(), 2);

        job.transition(JobState::Succeeded);
        assert_eq!(job.state, JobState::Succeeded);
        assert!(job.state.is_terminal());
    }

    #[test]
    fn test_lease_expiry() {
        let lease = Lease::new("lease-001".to_string(), "run-001".to_string(), 3600);
        assert!(!lease.is_expired());

        // Create an already-expired lease
        let mut expired = Lease::new("lease-002".to_string(), "run-002".to_string(), 0);
        expired.expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(expired.is_expired());
    }

    #[test]
    fn test_mock_state_sources() {
        let mut state = MockState::new();

        assert!(!state.has_source("sha256-abc"));
        state.store_source("sha256-abc".to_string(), vec![1, 2, 3]);
        assert!(state.has_source("sha256-abc"));

        assert!(state.evict_source("sha256-abc"));
        assert!(!state.has_source("sha256-abc"));
    }

    #[test]
    fn test_mock_state_id_generation() {
        let mut state = MockState::new();

        let id1 = state.next_id("job");
        let id2 = state.next_id("job");
        let id3 = state.next_id("lease");

        assert_ne!(id1, id2);
        assert!(id1.starts_with("job-"));
        assert!(id3.starts_with("lease-"));
    }
}
