//! Reserve operation handler (M6).
//!
//! Requests a worker lease for capacity reservation.
//! This is a stub that returns FEATURE_MISSING until implemented.

use rch_protocol::{ErrorCode, RpcError, RpcRequest};
use crate::config::WorkerConfig;

/// Handle the reserve operation.
pub fn handle(_request: &RpcRequest, _config: &WorkerConfig) -> Result<serde_json::Value, RpcError> {
    Err(RpcError::new(
        ErrorCode::FeatureMissing,
        "reserve operation not yet implemented (M6)",
    ))
}
