//! Status operation types.
//!
//! Query job state and artifact pointers.

use serde::{Deserialize, Serialize};
use super::submit::JobState;

/// Status request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {
    /// The job ID to query.
    pub job_id: String,
}

/// Status response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// The job ID.
    pub job_id: String,
    /// Current job state.
    pub state: JobState,
    /// Job key (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_key: Option<String>,
    /// Whether artifacts are available for fetch.
    #[serde(default)]
    pub artifacts_available: bool,
    /// Relative path to build.log if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_log_path: Option<String>,
    /// Relative path to result.xcresult if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcresult_path: Option<String>,
}
