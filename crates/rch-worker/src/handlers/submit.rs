//! Submit operation handler.
//!
//! Accepts a job.json and starts execution.

use std::time::Duration;
use rch_protocol::{RpcError, RpcRequest, ops::{SubmitRequest, SubmitResponse, JobState}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the submit operation.
pub fn handle(request: &RpcRequest, config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: SubmitRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid submit request: {}", e)))?;

    let injection = state.failure_injection();

    // Validate lease if provided
    if let Some(ref lease_id) = req.lease_id {
        if injection.lease_expired || !state.is_lease_valid(lease_id) {
            return Err(RpcError::lease_expired(lease_id));
        }
    }

    // Validate source exists
    if let Some(source_sha256) = req.job.job_key_inputs.get("source_sha256").and_then(|v| v.as_str()) {
        if injection.source_missing || !state.has_source(source_sha256) {
            return Err(RpcError::source_missing(source_sha256));
        }
    }

    // Check for existing job with same job_id
    if let Some(existing) = state.get_job(&req.job.job_id) {
        // Same job_id, check job_key
        if existing.job_key != req.job.job_key {
            return Err(RpcError::job_key_mismatch(
                &req.job.job_id,
                &existing.job_key,
                &req.job.job_key,
            ));
        }
        // Same job_id and job_key - return existing state (idempotent)
        return Ok(serde_json::to_value(SubmitResponse {
            job_id: existing.job_id,
            state: existing.state,
        }).unwrap());
    }

    // Check capacity
    let running_count = state.running_job_count();
    if running_count >= config.max_concurrent_jobs as usize {
        return Err(RpcError::busy(30));
    }

    // Create job
    let job = state.create_job(req.job.clone());

    // Schedule mock state transitions (QUEUED -> RUNNING -> SUCCEEDED)
    // For the mock worker, we simulate quick transitions
    let final_state = injection.force_job_state.unwrap_or(JobState::Succeeded);
    state.schedule_transitions(&job.job_id, vec![
        (JobState::Running, Duration::from_millis(10)),
        (final_state, Duration::from_millis(100)),
    ]);

    // Add some mock log output
    state.append_job_log(&job.job_id, &format!(
        "=== Job {} started ===\n\
         run_id: {}\n\
         job_key: {}\n\
         action: {}\n",
        job.job_id, job.run_id, job.job_key, req.job.action
    ));

    let response = SubmitResponse {
        job_id: job.job_id,
        state: JobState::Queued,
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rch_protocol::ops::JobSpec;

    fn make_job_spec(job_id: &str, job_key: &str) -> JobSpec {
        JobSpec {
            schema_version: 1,
            schema_id: "rch-xcode/job@1".to_string(),
            run_id: "run-001".to_string(),
            job_id: job_id.to_string(),
            action: "build".to_string(),
            job_key_inputs: serde_json::json!({
                "source_sha256": "abc123"
            }),
            job_key: job_key.to_string(),
            effective_config: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn make_request(job: JobSpec) -> RpcRequest {
        RpcRequest {
            protocol_version: 1,
            op: "submit".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job": job }),
        }
    }

    #[test]
    fn test_submit_creates_job() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        // Add source to store
        state.add_source("abc123".to_string(), "def456".to_string(), 1024);

        let job = make_job_spec("job-001", "key-001");
        let request = make_request(job);

        let result = handle(&request, &config, &state).unwrap();
        let response: SubmitResponse = serde_json::from_value(result).unwrap();

        assert_eq!(response.job_id, "job-001");
        assert_eq!(response.state, JobState::Queued);
    }

    #[test]
    fn test_submit_idempotent_same_key() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        state.add_source("abc123".to_string(), "def456".to_string(), 1024);

        let job = make_job_spec("job-001", "key-001");
        let request = make_request(job.clone());

        // First submit
        let _ = handle(&request, &config, &state).unwrap();

        // Second submit with same job_id and job_key - should succeed (idempotent)
        let result = handle(&request, &config, &state);
        assert!(result.is_ok());
    }

    #[test]
    fn test_submit_rejects_different_key() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        state.add_source("abc123".to_string(), "def456".to_string(), 1024);

        let job1 = make_job_spec("job-001", "key-001");
        let request1 = make_request(job1);
        let _ = handle(&request1, &config, &state).unwrap();

        // Submit with same job_id but different job_key
        let job2 = make_job_spec("job-001", "key-002");
        let request2 = make_request(job2);
        let result = handle(&request2, &config, &state);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::JobKeyMismatch);
    }

    #[test]
    fn test_submit_source_missing() {
        let config = WorkerConfig::default();
        let state = MockState::new();
        // Don't add source to store

        let job = make_job_spec("job-001", "key-001");
        let request = make_request(job);

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::SourceMissing);
    }
}
