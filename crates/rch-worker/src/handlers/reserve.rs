//! Reserve operation handler.
//!
//! Requests a worker lease for capacity reservation.

use rch_protocol::{RpcError, RpcRequest, ops::{ReserveRequest, ReserveResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Default lease TTL in seconds (30 minutes).
const DEFAULT_LEASE_TTL: u32 = 1800;

/// Handle the reserve operation.
pub fn handle(request: &RpcRequest, config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Check for failure injection
    let injection = state.failure_injection();
    if let Some(retry_after) = injection.reserve_busy {
        return Err(RpcError::busy(retry_after));
    }

    // Check capacity
    let active_leases = state.active_lease_count();
    if active_leases >= config.max_concurrent_jobs as usize {
        return Err(RpcError::busy(30)); // suggest retry after 30 seconds
    }

    // Parse request
    let req: ReserveRequest = serde_json::from_value(request.payload.clone())
        .unwrap_or_default();

    // Create lease
    let ttl = req.ttl_seconds.unwrap_or(DEFAULT_LEASE_TTL);
    let lease = state.create_lease(ttl);

    let response = ReserveResponse {
        lease_id: lease.lease_id,
        ttl_seconds: ttl,
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> RpcRequest {
        RpcRequest {
            protocol_version: 1,
            op: "reserve".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn test_reserve_creates_lease() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        let request = make_request();

        let result = handle(&request, &config, &state).unwrap();
        let response: ReserveResponse = serde_json::from_value(result).unwrap();

        assert!(!response.lease_id.is_empty());
        assert_eq!(response.ttl_seconds, DEFAULT_LEASE_TTL);
        assert!(state.is_lease_valid(&response.lease_id));
    }

    #[test]
    fn test_reserve_with_custom_ttl() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        let request = RpcRequest {
            protocol_version: 1,
            op: "reserve".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "ttl_seconds": 600 }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: ReserveResponse = serde_json::from_value(result).unwrap();

        assert_eq!(response.ttl_seconds, 600);
    }

    #[test]
    fn test_reserve_busy_when_at_capacity() {
        let mut config = WorkerConfig::default();
        config.max_concurrent_jobs = 1;
        let state = MockState::new();

        // First reserve succeeds
        let request = make_request();
        let _ = handle(&request, &config, &state).unwrap();

        // Second reserve fails with BUSY
        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::Busy);
    }

    #[test]
    fn test_reserve_busy_with_injection() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        state.set_failure_injection(crate::mock_state::FailureInjection {
            reserve_busy: Some(60),
            ..Default::default()
        });

        let request = make_request();
        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::Busy);
    }
}
