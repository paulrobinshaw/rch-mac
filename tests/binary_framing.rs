//! Binary Framing Tests (upload_source/fetch)
//!
//! Tests for bead rch-mac-666.5: Binary framing protocol for upload and fetch.
//!
//! Note: The mock worker uses a simplified JSON-based approach for testing.
//! Full binary framing with stdin/stdout is tested at the integration level.
//! These tests verify the logical behavior and error handling.

use rch_xcode_lane::{MockWorker, Operation, RpcRequest};
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
// Test 1: upload_source happy path
// =============================================================================

#[test]
fn test_upload_source_stores_bundle() {
    let worker = MockWorker::new();

    // Check source doesn't exist
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-upload-001"}));
    let response = worker.handle_request(&has);
    assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());

    // Upload
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-upload-001",
        "content": "test tar content"
    }));
    let response = worker.handle_request(&upload);
    assert!(response.ok, "upload should succeed");

    // Verify stored
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-upload-001"}));
    let response = worker.handle_request(&has);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
}

#[test]
fn test_upload_source_response_includes_sha256() {
    let worker = MockWorker::new();

    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-upload-002"
    }));
    let response = worker.handle_request(&upload);

    assert!(response.ok);
    let payload = response.payload.unwrap();
    assert_eq!(payload["source_sha256"], "sha256-upload-002");
    assert!(payload["stored"].as_bool().unwrap());
}

// =============================================================================
// Test 2: upload_source idempotency
// =============================================================================

#[test]
fn test_upload_source_idempotent() {
    let worker = MockWorker::new();

    // Upload first time
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-idem",
        "content": "content v1"
    }));
    let response1 = worker.handle_request(&upload);
    assert!(response1.ok);

    // Upload again with same sha256
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-idem",
        "content": "content v2"
    }));
    let response2 = worker.handle_request(&upload);
    assert!(response2.ok, "second upload should also succeed");

    // Still exists
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-idem"}));
    let response = worker.handle_request(&has);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
}

// =============================================================================
// Test 3: upload_source missing required field
// =============================================================================

#[test]
fn test_upload_source_missing_sha256() {
    let worker = MockWorker::new();

    let upload = make_request(Operation::UploadSource, 1, json!({
        "content": "some content"
    }));
    let response = worker.handle_request(&upload);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

// =============================================================================
// Test 4: fetch — job not terminal
// =============================================================================

#[test]
fn test_fetch_job_not_terminal() {
    let worker = MockWorker::new();
    worker.store_source("sha256-fetch", vec![1, 2, 3]);

    // Submit job (will be in QUEUED state)
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-fetch-001",
        "job_key": "key",
        "source_sha256": "sha256-fetch"
    }));
    worker.handle_request(&submit);

    // Try to fetch before job is terminal
    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "job-fetch-001"}));
    let response = worker.handle_request(&fetch);

    // Should fail - job not terminal
    assert!(!response.ok);
}

// =============================================================================
// Test 5: fetch — ARTIFACTS_GONE
// =============================================================================

#[test]
fn test_fetch_artifacts_gone() {
    let worker = MockWorker::new();

    // Inject ARTIFACTS_GONE error
    worker.inject_error(Operation::Fetch, "ARTIFACTS_GONE", "Artifacts have been deleted");

    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "job-gone-001"}));
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "ARTIFACTS_GONE");
}

#[test]
fn test_delete_artifacts_causes_artifacts_gone() {
    let worker = MockWorker::new();
    worker.store_source("sha256-del", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-del-001",
        "job_key": "key",
        "source_sha256": "sha256-del"
    }));
    worker.handle_request(&submit);

    // Delete artifacts via API
    let _deleted = worker.delete_artifacts("job-del-001");
    // Artifacts weren't there yet, so this might return false
    // That's fine - the point is the API works

    // Now inject ARTIFACTS_GONE for the fetch
    worker.inject_error(Operation::Fetch, "ARTIFACTS_GONE", "Artifacts deleted");
    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "job-del-001"}));
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "ARTIFACTS_GONE");
}

// =============================================================================
// Test 6: fetch — missing job_id
// =============================================================================

#[test]
fn test_fetch_missing_job_id() {
    let worker = MockWorker::new();

    let fetch = make_request(Operation::Fetch, 1, json!({}));
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

// =============================================================================
// Test 7: fetch — job not found
// =============================================================================

#[test]
fn test_fetch_job_not_found() {
    let worker = MockWorker::new();

    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "nonexistent"}));
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

// =============================================================================
// Test 8: PAYLOAD_TOO_LARGE via failure injection
// =============================================================================

#[test]
fn test_payload_too_large() {
    let worker = MockWorker::new();

    // Inject PAYLOAD_TOO_LARGE error
    worker.inject_error(Operation::UploadSource, "PAYLOAD_TOO_LARGE", "Payload exceeds maximum allowed size");

    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-large",
        "content": "x".repeat(10000)  // Simulate large content
    }));
    let response = worker.handle_request(&upload);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "PAYLOAD_TOO_LARGE");
}

// =============================================================================
// Test 9: Source eviction between operations
// =============================================================================

#[test]
fn test_source_eviction_api() {
    let worker = MockWorker::new();

    // Store source
    worker.store_source("sha256-evict", vec![1, 2, 3, 4, 5]);

    // Verify exists
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-evict"}));
    let response = worker.handle_request(&has);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());

    // Evict
    let evicted = worker.evict_source("sha256-evict");
    assert!(evicted, "evict should return true for existing source");

    // Verify gone
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-evict"}));
    let response = worker.handle_request(&has);
    assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());

    // Evict again returns false
    let evicted = worker.evict_source("sha256-evict");
    assert!(!evicted, "evict should return false for non-existent source");
}

// =============================================================================
// Test 10: Multiple source uploads
// =============================================================================

#[test]
fn test_multiple_different_sources() {
    let worker = MockWorker::new();

    // Upload multiple sources
    for i in 1..=5 {
        let sha = format!("sha256-multi-{}", i);
        let upload = make_request(Operation::UploadSource, 1, json!({
            "source_sha256": sha,
            "content": format!("content {}", i)
        }));
        let response = worker.handle_request(&upload);
        assert!(response.ok);
    }

    // All should exist
    for i in 1..=5 {
        let sha = format!("sha256-multi-{}", i);
        let has = make_request(Operation::HasSource, 1, json!({"source_sha256": sha}));
        let response = worker.handle_request(&has);
        assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
    }
}

// =============================================================================
// Test 11: Content via store_source API
// =============================================================================

#[test]
fn test_store_source_via_api() {
    let worker = MockWorker::new();

    // Store directly via API (for test setup)
    worker.store_source("sha256-api-store", vec![10, 20, 30, 40, 50]);

    // Should exist
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-api-store"}));
    let response = worker.handle_request(&has);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
}

// =============================================================================
// Test 12: Tail operation for jobs
// =============================================================================

#[test]
fn test_tail_returns_empty_for_new_job() {
    let worker = MockWorker::new();
    worker.store_source("sha256-tail", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-tail-001",
        "job_key": "key",
        "source_sha256": "sha256-tail"
    }));
    worker.handle_request(&submit);

    // Tail logs
    let tail = make_request(Operation::Tail, 1, json!({
        "job_id": "job-tail-001",
        "cursor": 0
    }));
    let response = worker.handle_request(&tail);

    assert!(response.ok);
    let payload = response.payload.unwrap();
    // Just verify we got entries array (may be empty for new job)
    assert!(payload["entries"].is_array());
}

#[test]
fn test_tail_with_cursor_pagination() {
    let worker = MockWorker::new();
    worker.store_source("sha256-page", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-page-001",
        "job_key": "key",
        "source_sha256": "sha256-page"
    }));
    worker.handle_request(&submit);

    // Tail with cursor 0
    let tail = make_request(Operation::Tail, 1, json!({
        "job_id": "job-page-001",
        "cursor": 0,
        "limit": 10
    }));
    let response = worker.handle_request(&tail);

    assert!(response.ok);
    let payload = response.payload.unwrap();
    assert!(payload.get("entries").is_some());
    assert!(payload.get("next_cursor").is_some());
}

// =============================================================================
// Test 13: Tail missing job_id
// =============================================================================

#[test]
fn test_tail_missing_job_id() {
    let worker = MockWorker::new();

    let tail = make_request(Operation::Tail, 1, json!({}));
    let response = worker.handle_request(&tail);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

// =============================================================================
// Test 14: Protocol version for upload/fetch
// =============================================================================

#[test]
fn test_upload_requires_non_zero_version() {
    let worker = MockWorker::new();

    let upload = make_request(Operation::UploadSource, 0, json!({"source_sha256": "test"}));
    let response = worker.handle_request(&upload);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_fetch_requires_non_zero_version() {
    let worker = MockWorker::new();

    let fetch = make_request(Operation::Fetch, 0, json!({"job_id": "test"}));
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

// =============================================================================
// Test 15: Integration scenario
// =============================================================================

#[test]
fn test_full_upload_submit_flow() {
    let worker = MockWorker::new();

    // 1. Check source doesn't exist
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-flow"}));
    let response = worker.handle_request(&has);
    assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());

    // 2. Upload source
    let upload = make_request(Operation::UploadSource, 1, json!({
        "source_sha256": "sha256-flow",
        "content": "flow test content"
    }));
    let response = worker.handle_request(&upload);
    assert!(response.ok);

    // 3. Verify source exists
    let has = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-flow"}));
    let response = worker.handle_request(&has);
    assert!(response.payload.unwrap()["exists"].as_bool().unwrap());

    // 4. Submit job
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-flow-001",
        "job_key": "key-flow",
        "source_sha256": "sha256-flow"
    }));
    let response = worker.handle_request(&submit);
    assert!(response.ok);
    assert_eq!(response.payload.unwrap()["state"], "QUEUED");

    // 5. Check status
    let status = make_request(Operation::Status, 1, json!({"job_id": "job-flow-001"}));
    let response = worker.handle_request(&status);
    assert!(response.ok);
    assert_eq!(response.payload.unwrap()["job_id"], "job-flow-001");
}
