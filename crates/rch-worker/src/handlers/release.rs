//! Release operation handler.
//!
//! Releases a previously acquired lease.
//! This operation is idempotent: releasing an unknown or expired lease returns ok.

use rch_protocol::{RpcError, RpcRequest, ops::{ReleaseRequest, ReleaseResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the release operation.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: ReleaseRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid release request: {}", e)))?;

    // Release is idempotent - always succeeds
    let was_valid = state.release_lease(&req.lease_id);

    let response = ReleaseResponse {
        released: was_valid,
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_valid_lease() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        // Create a lease first
        let lease = state.create_lease(3600);

        let request = RpcRequest {
            protocol_version: 1,
            op: "release".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "lease_id": lease.lease_id }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: ReleaseResponse = serde_json::from_value(result).unwrap();

        assert!(response.released);
        assert!(!state.is_lease_valid(&lease.lease_id));
    }

    #[test]
    fn test_release_unknown_lease_succeeds() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "release".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "lease_id": "unknown-lease" }),
        };

        // Idempotent: releasing unknown lease succeeds
        let result = handle(&request, &config, &state).unwrap();
        let response: ReleaseResponse = serde_json::from_value(result).unwrap();

        assert!(!response.released); // wasn't actually released, but no error
    }

    #[test]
    fn test_release_is_idempotent() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let lease = state.create_lease(3600);
        let request = RpcRequest {
            protocol_version: 1,
            op: "release".to_string(),
            request_id: "test-003".to_string(),
            payload: serde_json::json!({ "lease_id": lease.lease_id }),
        };

        // First release
        let result1 = handle(&request, &config, &state).unwrap();
        let response1: ReleaseResponse = serde_json::from_value(result1).unwrap();
        assert!(response1.released);

        // Second release (idempotent)
        let result2 = handle(&request, &config, &state).unwrap();
        let response2: ReleaseResponse = serde_json::from_value(result2).unwrap();
        assert!(!response2.released); // already released
    }
}
