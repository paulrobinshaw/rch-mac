//! Fetch operation handler.
//!
//! Returns job artifacts as a binary-framed response.
//!
//! Note: For the mock worker, we return a stub response indicating artifacts
//! are available but don't actually stream any binary data. In a real
//! implementation, this would output a JSON header line followed by the
//! tar archive of artifacts.

use rch_protocol::{RpcError, RpcRequest, ErrorCode, ops::{FetchRequest, JobState}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the fetch operation.
///
/// Note: Fetch uses binary framing and bypasses the normal RpcResponse envelope.
/// For the mock worker, we return a simplified response.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: FetchRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid fetch request: {}", e)))?;

    // Process any scheduled transitions
    state.process_transitions(&req.job_id);

    // Look up job
    let job = state.get_job(&req.job_id)
        .ok_or_else(|| RpcError::job_not_found(&req.job_id))?;

    // Check if job is complete
    if !job.state.is_terminal() {
        return Err(RpcError::new(
            ErrorCode::InvalidRequest,
            format!("job '{}' is not complete (state: {:?})", req.job_id, job.state),
        ));
    }

    // For mock, we simulate artifacts being available for successful jobs
    // but gone for failed/cancelled jobs (simulating cleanup)
    if job.state == JobState::Failed || job.state == JobState::Cancelled {
        return Err(RpcError::artifacts_gone(&req.job_id));
    }

    // Return a mock response indicating what would be fetched
    // In a real implementation, this would output the binary-framed response
    let response = serde_json::json!({
        "job_id": job.job_id,
        "job_key": job.job_key,
        "artifacts_available": true,
        "manifest": {
            "schema_version": 1,
            "schema_id": "rch-xcode/manifest@1",
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "entries": [
                { "path": "summary.json", "size": 256, "type": "file", "sha256": "mock-summary-sha256" },
                { "path": "build.log", "size": 1024, "type": "file", "sha256": "mock-log-sha256" }
            ],
            "artifact_root_sha256": "mock-root-sha256"
        },
        "note": "Mock fetch response - real implementation would stream binary tar"
    });

    Ok(response)
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
    fn test_fetch_completed_job() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Succeeded);

        let request = RpcRequest {
            protocol_version: 1,
            op: "fetch".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        assert!(result.get("artifacts_available").is_some());
        assert!(result.get("manifest").is_some());
    }

    #[test]
    fn test_fetch_running_job_fails() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Running);

        let request = RpcRequest {
            protocol_version: 1,
            op: "fetch".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
    }

    #[test]
    fn test_fetch_failed_job_artifacts_gone() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.set_job_state("job-001", JobState::Failed);

        let request = RpcRequest {
            protocol_version: 1,
            op: "fetch".to_string(),
            request_id: "test-003".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::ArtifactsGone);
    }

    #[test]
    fn test_fetch_not_found() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "fetch".to_string(),
            request_id: "test-004".to_string(),
            payload: serde_json::json!({ "job_id": "unknown-job" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::JobNotFound);
    }
}
