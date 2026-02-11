//! Status operation handler.
//!
//! Returns current job status and artifact pointers.

use rch_protocol::{RpcError, RpcRequest, ops::{StatusRequest, StatusResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the status operation.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: StatusRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid status request: {}", e)))?;

    // Process any scheduled transitions
    state.process_transitions(&req.job_id);

    // Look up job
    let job = state.get_job(&req.job_id)
        .ok_or_else(|| RpcError::job_not_found(&req.job_id))?;

    let job_id = job.job_id.clone();
    let response = StatusResponse {
        job_id: job_id.clone(),
        state: job.state,
        job_key: Some(job.job_key),
        artifacts_available: job.state.is_terminal(),
        build_log_path: if job.state.is_terminal() {
            Some(format!("{}/build.log", job_id))
        } else {
            None
        },
        xcresult_path: if job.state.is_terminal() && job.spec.action == "test" {
            Some(format!("{}/result.xcresult", job_id))
        } else {
            None
        },
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rch_protocol::ops::{JobSpec, JobState};

    fn make_job_spec(job_id: &str) -> JobSpec {
        JobSpec {
            schema_version: 1,
            schema_id: "rch-xcode/job@1".to_string(),
            run_id: "run-001".to_string(),
            job_id: job_id.to_string(),
            action: "test".to_string(),
            job_key_inputs: serde_json::json!({}),
            job_key: "key-001".to_string(),
            effective_config: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_status_returns_job_state() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        // Create a job
        let spec = make_job_spec("job-001");
        state.create_job(spec);

        let request = RpcRequest {
            protocol_version: 1,
            op: "status".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: StatusResponse = serde_json::from_value(result).unwrap();

        assert_eq!(response.job_id, "job-001");
        assert_eq!(response.state, JobState::Queued);
    }

    #[test]
    fn test_status_not_found() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "status".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "job_id": "unknown-job" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::JobNotFound);
    }

    #[test]
    fn test_status_artifacts_available_when_terminal() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Succeeded);

        let request = RpcRequest {
            protocol_version: 1,
            op: "status".to_string(),
            request_id: "test-003".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: StatusResponse = serde_json::from_value(result).unwrap();

        assert!(response.artifacts_available);
        assert!(response.build_log_path.is_some());
        assert!(response.xcresult_path.is_some()); // because action is "test"
    }
}
