//! Tail operation handler (M2).
//!
//! Returns log chunks with cursor-based pagination.
//! This is a stub that returns FEATURE_MISSING until implemented.

use rch_protocol::{ErrorCode, RpcError, RpcRequest};
use crate::config::WorkerConfig;

/// Handle the tail operation.
pub fn handle(_request: &RpcRequest, _config: &WorkerConfig) -> Result<serde_json::Value, RpcError> {
    Err(RpcError::new(
        ErrorCode::FeatureMissing,
        "tail operation not yet implemented (M2)",
    ))
}
