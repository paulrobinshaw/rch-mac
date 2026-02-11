//! Cancel operation handler.
//!
//! Signals executor to terminate the job.

use rch_protocol::{RpcError, RpcRequest, ops::{CancelRequest, CancelResponse, JobState}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the cancel operation.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: CancelRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid cancel request: {}", e)))?;

    // Process any scheduled transitions
    state.process_transitions(&req.job_id);

    // Look up job
    let job = state.get_job(&req.job_id)
        .ok_or_else(|| RpcError::job_not_found(&req.job_id))?;

    // Determine new state based on current state
    let (new_state, acknowledged) = match job.state {
        JobState::Queued | JobState::Running => {
            // Transition to CANCEL_REQUESTED, then immediately to CANCELLED for mock
            state.set_job_state(&req.job_id, JobState::CancelRequested);
            state.set_job_state(&req.job_id, JobState::Cancelled);
            state.append_job_log(&req.job_id, "\n=== Job cancelled ===\n");
            (JobState::Cancelled, true)
        }
        JobState::CancelRequested => {
            // Already cancelling
            (JobState::CancelRequested, true)
        }
        JobState::Succeeded | JobState::Failed | JobState::Cancelled => {
            // Already terminal - can't cancel
            (job.state, false)
        }
    };

    let response = CancelResponse {
        job_id: req.job_id,
        state: new_state,
        acknowledged,
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rch_protocol::ops::JobSpec;

    fn make_job_spec(job_id: &str) -> JobSpec {
        JobSpec {
            schema_version: 1,
            schema_id: "rch-xcode/job@1".to_string(),
            run_id: "run-001".to_string(),
            job_id: job_id.to_string(),
            action: "build".to_string(),
            job_key_inputs: serde_json::json!({}),
            job_key: "key-001".to_string(),
            effective_config: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_cancel_running_job() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Running);

        let request = RpcRequest {
            protocol_version: 1,
            op: "cancel".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: CancelResponse = serde_json::from_value(result).unwrap();

        assert!(response.acknowledged);
        assert_eq!(response.state, JobState::Cancelled);
    }

    #[test]
    fn test_cancel_completed_job() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Succeeded);

        let request = RpcRequest {
            protocol_version: 1,
            op: "cancel".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: CancelResponse = serde_json::from_value(result).unwrap();

        assert!(!response.acknowledged); // Can't cancel completed job
        assert_eq!(response.state, JobState::Succeeded);
    }

    #[test]
    fn test_cancel_not_found() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "cancel".to_string(),
            request_id: "test-003".to_string(),
            payload: serde_json::json!({ "job_id": "unknown-job" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::JobNotFound);
    }
}
