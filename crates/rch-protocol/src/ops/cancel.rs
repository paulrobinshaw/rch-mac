//! Cancel operation types.
//!
//! Best-effort job cancellation.

use serde::{Deserialize, Serialize};
use super::submit::JobState;

/// Cancel request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    /// The job ID to cancel.
    pub job_id: String,
}

/// Cancel response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResponse {
    /// The job ID.
    pub job_id: String,
    /// New job state after cancel request.
    pub state: JobState,
    /// Whether cancellation was acknowledged.
    pub acknowledged: bool,
}
