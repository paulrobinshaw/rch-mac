//! Tail operation handler.
//!
//! Returns log chunks with cursor-based pagination.

use rch_protocol::{RpcError, RpcRequest, ops::{TailRequest, TailResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the tail operation.
pub fn handle(request: &RpcRequest, _config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: TailRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid tail request: {}", e)))?;

    // Process any scheduled transitions
    state.process_transitions(&req.job_id);

    // Look up job
    let job = state.get_job(&req.job_id)
        .ok_or_else(|| RpcError::job_not_found(&req.job_id))?;

    // Parse cursor (byte offset)
    let cursor: usize = req.cursor
        .as_ref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    // Get log chunk from cursor
    let (log_chunk, new_cursor) = state.get_job_log(&req.job_id, cursor)
        .ok_or_else(|| RpcError::job_not_found(&req.job_id))?;

    // Apply max_bytes limit if specified
    let log_chunk = if let Some(max) = req.max_bytes {
        if log_chunk.len() > max as usize {
            log_chunk[..max as usize].to_string()
        } else {
            log_chunk
        }
    } else {
        log_chunk
    };

    // Determine next_cursor: null if job is terminal and we've read all logs
    let next_cursor = if job.state.is_terminal() && new_cursor == cursor {
        None // No more logs coming
    } else {
        Some(new_cursor.to_string())
    };

    let response = TailResponse {
        job_id: req.job_id,
        next_cursor,
        log_chunk: if log_chunk.is_empty() { None } else { Some(log_chunk) },
        events: Vec::new(), // Mock doesn't emit events yet
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
    fn test_tail_returns_logs() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.append_job_log("job-001", "Line 1\nLine 2\n");

        let request = RpcRequest {
            protocol_version: 1,
            op: "tail".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result = handle(&request, &config, &state).unwrap();
        let response: TailResponse = serde_json::from_value(result).unwrap();

        assert_eq!(response.job_id, "job-001");
        assert!(response.log_chunk.is_some());
        assert!(response.log_chunk.unwrap().contains("Line 1"));
    }

    #[test]
    fn test_tail_with_cursor() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let spec = make_job_spec("job-001");
        state.create_job(spec);
        state.append_job_log("job-001", "ABCDEFGHIJ");

        // First tail from start
        let request1 = RpcRequest {
            protocol_version: 1,
            op: "tail".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({ "job_id": "job-001" }),
        };

        let result1 = handle(&request1, &config, &state).unwrap();
        let response1: TailResponse = serde_json::from_value(result1).unwrap();
        let cursor = response1.next_cursor.unwrap();

        // Append more logs
        state.append_job_log("job-001", "KLMNO");

        // Tail from cursor
        let request2 = RpcRequest {
            protocol_version: 1,
            op: "tail".to_string(),
            request_id: "test-002".to_string(),
            payload: serde_json::json!({ "job_id": "job-001", "cursor": cursor }),
        };

        let result2 = handle(&request2, &config, &state).unwrap();
        let response2: TailResponse = serde_json::from_value(result2).unwrap();

        assert_eq!(response2.log_chunk.unwrap(), "KLMNO");
    }

    #[test]
    fn test_tail_not_found() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = RpcRequest {
            protocol_version: 1,
            op: "tail".to_string(),
            request_id: "test-003".to_string(),
            payload: serde_json::json!({ "job_id": "unknown-job" }),
        };

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::JobNotFound);
    }
}
