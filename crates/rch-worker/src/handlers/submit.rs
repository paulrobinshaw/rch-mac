//! Submit operation handler (M2).
//!
//! Accepts a job.json and starts execution.
//! This is a stub that returns FEATURE_MISSING until implemented.

use rch_protocol::{ErrorCode, RpcError, RpcRequest};
use crate::config::WorkerConfig;

/// Handle the submit operation.
pub fn handle(_request: &RpcRequest, _config: &WorkerConfig) -> Result<serde_json::Value, RpcError> {
    Err(RpcError::new(
        ErrorCode::FeatureMissing,
        "submit operation not yet implemented (M2)",
    ))
}
