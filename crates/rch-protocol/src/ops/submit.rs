//! Submit operation types.
//!
//! Job submission and initial status.

use serde::{Deserialize, Serialize};

/// Submit request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRequest {
    /// The lease ID (required if worker requires leases).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    /// The job specification (job.json content).
    pub job: JobSpec,
}

/// Job specification (job.json schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// Schema version.
    pub schema_version: i32,
    /// Schema identifier.
    pub schema_id: String,
    /// Parent run identifier.
    pub run_id: String,
    /// Unique job identifier.
    pub job_id: String,
    /// Action type.
    pub action: String,
    /// Canonical key-object for caching/attestation.
    pub job_key_inputs: serde_json::Value,
    /// SHA-256 hex of JCS(job_key_inputs).
    pub job_key: String,
    /// Merged effective config snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_config: Option<serde_json::Value>,
    /// When this job spec was created.
    pub created_at: String,
}

/// Submit response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitResponse {
    /// The job ID (echoed).
    pub job_id: String,
    /// Initial job state.
    pub state: JobState,
}

/// Job state enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    /// Job is pending execution.
    Queued,
    /// Job is actively executing.
    Running,
    /// Cancellation requested but still running.
    CancelRequested,
    /// Job completed successfully.
    Succeeded,
    /// Job completed with failure.
    Failed,
    /// Job was cancelled.
    Cancelled,
}

impl JobState {
    /// Check if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}
