//! Has-source operation handler.
//!
//! Checks if a source bundle exists in the content-addressed store.

use rch_protocol::{RpcError, RpcRequest, ops::{HasSourceRequest, HasSourceResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the has_source operation.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: HasSourceRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid has_source request: {}", e)))?;

    let exists = state.has_source(&req.source_sha256);

    let response = HasSourceResponse { exists };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_source_exists() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        state.add_source("abc123".to_string(), "def456".to_string(), 1024);

        let request = RpcRequest {
            protocol_version: 1,
            op: "has_source".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "source_sha256": "abc123" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: HasSourceResponse = serde_json::from_value(result).unwrap();

        assert!(response.exists);
    }

    #[test]
    fn test_has_source_not_exists() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "has_source".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "source_sha256": "unknown" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: HasSourceResponse = serde_json::from_value(result).unwrap();

        assert!(!response.exists);
    }
}
