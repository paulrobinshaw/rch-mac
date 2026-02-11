//! Has-source operation handler (M2).
//!
//! Checks if a source bundle exists in the content-addressed store.
//! This is a stub that returns FEATURE_MISSING until implemented.

use rch_protocol::{ErrorCode, RpcError, RpcRequest};
use crate::config::WorkerConfig;

/// Handle the has_source operation.
pub fn handle(_request: &RpcRequest, _config: &WorkerConfig) -> Result<serde_json::Value, RpcError> {
    Err(RpcError::new(
        ErrorCode::FeatureMissing,
        "has_source operation not yet implemented (M2)",
    ))
}
