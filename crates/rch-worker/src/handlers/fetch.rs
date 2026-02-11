//! Fetch operation handler (M4).
//!
//! Returns job artifacts as a binary-framed response.
//! This is a stub that returns FEATURE_MISSING until implemented.

use rch_protocol::{ErrorCode, RpcError, RpcRequest};
use crate::config::WorkerConfig;

/// Handle the fetch operation.
pub fn handle(_request: &RpcRequest, _config: &WorkerConfig) -> Result<serde_json::Value, RpcError> {
    Err(RpcError::new(
        ErrorCode::FeatureMissing,
        "fetch operation not yet implemented (M4)",
    ))
}
