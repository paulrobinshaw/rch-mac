//! Job Lifecycle and Idempotency Tests
//!
//! Tests for bead rch-mac-666.4: Job lifecycle state machine and idempotency.

use rch_xcode_lane::{MockWorker, Operation, RpcRequest, JobState};
use serde_json::json;

/// Helper to create an RPC request
fn make_request(op: Operation, protocol_version: i32, payload: serde_json::Value) -> RpcRequest {
    RpcRequest {
        protocol_version,
        op,
        request_id: format!("test-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()),
        payload,
    }
}

// =============================================================================
// Test 1: Happy path lifecycle (QUEUED state)
// =============================================================================

#[test]
fn test_submit_new_job_enters_queued() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-happy-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-test"
    }));

    let response = worker.handle_request(&request);

    assert!(response.ok, "submit should succeed");
    let payload = response.payload.unwrap();
    assert_eq!(payload["state"], "QUEUED", "new job should be QUEUED");
    assert_eq!(payload["job_id"], "job-happy-001");
}

#[test]
fn test_status_returns_job_state() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-status-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-test"
    }));
    worker.handle_request(&submit);

    // Check status
    let status = make_request(Operation::Status, 1, json!({"job_id": "job-status-001"}));
    let response = worker.handle_request(&status);

    assert!(response.ok);
    let payload = response.payload.unwrap();
    assert_eq!(payload["job_id"], "job-status-001");
    assert!(payload.get("state").is_some(), "status should include state");
    assert!(payload.get("created_at").is_some(), "status should include created_at");
}

// =============================================================================
// Test 2: Idempotent submit — same job_id + same job_key
// =============================================================================

#[test]
fn test_idempotent_submit_same_job_key_succeeds() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    // First submit
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-idem-001",
        "job_key": "key-same",
        "source_sha256": "sha256-test"
    }));
    let response1 = worker.handle_request(&request);
    assert!(response1.ok);

    // Second submit with same job_id and same job_key
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-idem-001",
        "job_key": "key-same",
        "source_sha256": "sha256-test"
    }));
    let response2 = worker.handle_request(&request);

    // Should succeed (idempotent)
    assert!(response2.ok, "idempotent submit should succeed");
    assert_eq!(
        response1.payload.unwrap()["job_id"],
        response2.payload.unwrap()["job_id"]
    );
}

#[test]
fn test_idempotent_submit_returns_existing_job() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    // First submit
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-idem-002",
        "job_key": "key-same",
        "source_sha256": "sha256-test"
    }));
    let response1 = worker.handle_request(&request);
    let created_at_1 = response1.payload.as_ref().unwrap()["created_at"].clone();

    // Second submit
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-idem-002",
        "job_key": "key-same",
        "source_sha256": "sha256-test"
    }));
    let response2 = worker.handle_request(&request);
    let created_at_2 = response2.payload.as_ref().unwrap()["created_at"].clone();

    // Should return same job (same created_at)
    assert_eq!(created_at_1, created_at_2, "should return existing job, not create new");
}

// =============================================================================
// Test 3: Submit with mismatched job_key — must reject
// =============================================================================

#[test]
fn test_submit_mismatched_job_key_rejects() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    // First submit with job_key=K1
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-mismatch-001",
        "job_key": "key-K1",
        "source_sha256": "sha256-test"
    }));
    let response = worker.handle_request(&request);
    assert!(response.ok);

    // Second submit with same job_id but different job_key=K2
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-mismatch-001",
        "job_key": "key-K2",
        "source_sha256": "sha256-test"
    }));
    let response = worker.handle_request(&request);

    // Must reject
    assert!(!response.ok, "mismatched job_key should be rejected");
    let error = response.error.unwrap();
    assert!(
        error.message.contains("job_key") || error.message.contains("different"),
        "error should mention job_key mismatch"
    );
}

// =============================================================================
// Test 4: Source not present — SOURCE_MISSING
// =============================================================================

#[test]
fn test_submit_source_missing() {
    let worker = MockWorker::new();
    // Don't store any source

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-nosource-001",
        "job_key": "key-abc",
        "source_sha256": "nonexistent-sha256"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    let error = response.error.unwrap();
    assert_eq!(error.code, "SOURCE_MISSING", "should return SOURCE_MISSING error code");
}

#[test]
fn test_source_missing_includes_sha256_in_error() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-nosource-002",
        "job_key": "key-abc",
        "source_sha256": "specific-sha256-value"
    }));
    let response = worker.handle_request(&request);

    let error = response.error.unwrap();
    assert!(error.data.is_some(), "SOURCE_MISSING should include data");
    let data = error.data.unwrap();
    assert!(
        data.contains_key("source_sha256"),
        "error data should include source_sha256"
    );
}

// =============================================================================
// Test 5: TOCTOU race — source evicted between has_source and submit
// =============================================================================

#[test]
fn test_toctou_source_evicted_after_has_source() {
    let worker = MockWorker::new();

    // Store source
    worker.store_source("sha256-toctou", vec![1, 2, 3]);

    // Check has_source
    let has_source = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-toctou"}));
    let response = worker.handle_request(&has_source);
    assert!(response.ok);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());

    // Evict source (simulating GC)
    worker.evict_source("sha256-toctou");

    // Submit should fail with SOURCE_MISSING
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-toctou-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-toctou"
    }));
    let response = worker.handle_request(&submit);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "SOURCE_MISSING");
}

#[test]
fn test_toctou_recovery_via_reupload() {
    let worker = MockWorker::new();

    // Store source
    worker.store_source("sha256-recover", vec![1, 2, 3]);

    // Evict source
    worker.evict_source("sha256-recover");

    // Submit fails
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-recover-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-recover"
    }));
    let response = worker.handle_request(&submit);
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "SOURCE_MISSING");

    // Re-upload source
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-recover",
        "content": "test content"
    }));
    let response = worker.handle_request(&upload);
    assert!(response.ok, "upload should succeed");

    // Retry submit should succeed
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-recover-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-recover"
    }));
    let response = worker.handle_request(&submit);
    assert!(response.ok, "submit should succeed after re-upload");
}

// =============================================================================
// Test 6: Cancellation lifecycle
// =============================================================================

#[test]
fn test_cancel_transitions_to_cancel_requested() {
    let worker = MockWorker::new();
    worker.store_source("sha256-cancel", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-cancel-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-cancel"
    }));
    worker.handle_request(&submit);

    // Cancel
    let cancel = make_request(Operation::Cancel, 1, json!({"job_id": "job-cancel-001"}));
    let response = worker.handle_request(&cancel);

    assert!(response.ok);
    let payload = response.payload.unwrap();
    assert_eq!(payload["state"], "CANCEL_REQUESTED");
}

#[test]
fn test_cancel_already_terminal_returns_current_state() {
    let worker = MockWorker::new();
    worker.store_source("sha256-term", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-term-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-term"
    }));
    worker.handle_request(&submit);

    // Cancel once (enters CANCEL_REQUESTED)
    let cancel = make_request(Operation::Cancel, 1, json!({"job_id": "job-term-001"}));
    worker.handle_request(&cancel);

    // Cancel again
    let cancel = make_request(Operation::Cancel, 1, json!({"job_id": "job-term-001"}));
    let response = worker.handle_request(&cancel);

    // Should still succeed
    assert!(response.ok);
}

// =============================================================================
// Test 7: Capacity exceeded — BUSY
// =============================================================================

#[test]
fn test_reserve_busy_when_at_capacity() {
    let worker = MockWorker::new();
    worker.set_capacity(1);

    // First reserve succeeds
    let reserve1 = make_request(Operation::Reserve, 1, json!({"run_id": "run-001"}));
    let response = worker.handle_request(&reserve1);
    assert!(response.ok, "first reserve should succeed");

    // Second reserve should fail with BUSY
    let reserve2 = make_request(Operation::Reserve, 1, json!({"run_id": "run-002"}));
    let response = worker.handle_request(&reserve2);

    assert!(!response.ok);
    let error = response.error.unwrap();
    assert_eq!(error.code, "BUSY", "should return BUSY error");

    // Must include retry_after_seconds
    assert!(error.data.is_some());
    let data = error.data.unwrap();
    assert!(data.contains_key("retry_after_seconds"), "BUSY must include retry_after_seconds");

    let retry_after = data["retry_after_seconds"].as_i64().unwrap();
    assert!(retry_after > 0, "retry_after_seconds must be positive");
}

#[test]
fn test_capacity_restored_after_release() {
    let worker = MockWorker::new();
    worker.set_capacity(1);

    // Reserve
    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-001"}));
    let response = worker.handle_request(&reserve);
    let lease_id = response.payload.unwrap()["lease_id"].as_str().unwrap().to_string();

    // At capacity
    let reserve2 = make_request(Operation::Reserve, 1, json!({"run_id": "run-002"}));
    let response = worker.handle_request(&reserve2);
    assert!(!response.ok);

    // Release
    let release = make_request(Operation::Release, 1, json!({"lease_id": lease_id}));
    worker.handle_request(&release);

    // Now should succeed
    let reserve3 = make_request(Operation::Reserve, 1, json!({"run_id": "run-003"}));
    let response = worker.handle_request(&reserve3);
    assert!(response.ok, "reserve should succeed after release");
}

// =============================================================================
// Test 8: has_source and upload_source interaction
// =============================================================================

#[test]
fn test_has_source_returns_false_for_unknown() {
    let worker = MockWorker::new();

    let request = make_request(Operation::HasSource, 1, json!({"source_sha256": "unknown-sha"}));
    let response = worker.handle_request(&request);

    assert!(response.ok);
    assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());
}

#[test]
fn test_has_source_returns_true_after_upload() {
    let worker = MockWorker::new();

    // Initially not present
    let check = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-new"}));
    let response = worker.handle_request(&check);
    assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());

    // Upload
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-new",
        "content": "test"
    }));
    worker.handle_request(&upload);

    // Now present
    let check = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-new"}));
    let response = worker.handle_request(&check);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
}

// =============================================================================
// Test 9: Job state getter API
// =============================================================================

#[test]
fn test_get_job_state_api() {
    let worker = MockWorker::new();
    worker.store_source("sha256-api", vec![1, 2, 3]);

    // No job yet
    assert!(worker.get_job_state("nonexistent").is_none());

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-api-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-api"
    }));
    worker.handle_request(&submit);

    // Now exists
    let state = worker.get_job_state("job-api-001");
    assert!(state.is_some());
    assert_eq!(state.unwrap(), JobState::Queued);
}

// =============================================================================
// Test 10: Missing required fields in payloads
// =============================================================================

#[test]
fn test_submit_missing_job_id() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Submit, 1, json!({
        "job_key": "key",
        "source_sha256": "sha"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_submit_missing_job_key() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-001",
        "source_sha256": "sha"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_submit_missing_source_sha256() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-001",
        "job_key": "key"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_status_missing_job_id() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Status, 1, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_cancel_missing_job_id() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Cancel, 1, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_has_source_missing_sha256() {
    let worker = MockWorker::new();

    let request = make_request(Operation::HasSource, 1, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

// =============================================================================
// Test 11: Nonexistent job operations
// =============================================================================

#[test]
fn test_status_nonexistent_job() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Status, 1, json!({"job_id": "does-not-exist"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_cancel_nonexistent_job() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Cancel, 1, json!({"job_id": "does-not-exist"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
}

#[test]
fn test_tail_nonexistent_job() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Tail, 1, json!({"job_id": "does-not-exist"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
}
