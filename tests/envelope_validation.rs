//! RPC Envelope Validation Tests
//!
//! Tests for bead rch-mac-666.3: JSON RPC envelope structure validation.

use rch_xcode_lane::{MockWorker, Operation, RpcRequest};
use serde_json::json;

/// Helper to create an RPC request
fn make_request(op: Operation, protocol_version: i32, payload: serde_json::Value) -> RpcRequest {
    RpcRequest {
        protocol_version,
        op,
        request_id: "test-req-001".to_string(),
        payload,
    }
}

// =============================================================================
// Test 1: Valid request structure
// =============================================================================

#[test]
fn test_valid_request_returns_all_envelope_fields() {
    let worker = MockWorker::new();
    worker.store_source("sha256-test", vec![1, 2, 3]);

    // Submit a job
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-001",
        "job_key": "key-abc",
        "source_sha256": "sha256-test"
    }));

    let response = worker.handle_request(&request);

    // Verify all envelope fields
    assert_eq!(response.protocol_version, 1, "response should have protocol_version");
    assert_eq!(response.request_id, "test-req-001", "response should echo request_id");
    assert!(response.ok, "response should be ok");
    assert!(response.payload.is_some(), "successful response should have payload");
    assert!(response.error.is_none(), "successful response should not have error");
}

#[test]
fn test_probe_response_structure() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 0, json!({}));

    let response = worker.handle_request(&request);

    assert_eq!(response.protocol_version, 0);
    assert!(response.ok);
    assert!(response.payload.is_some());

    // Payload should be an object with expected fields
    let payload = response.payload.unwrap();
    assert!(payload.is_object());
}

// =============================================================================
// Test 2: Missing required fields
// Note: These tests use raw JSON parsing to simulate malformed requests
// =============================================================================

#[test]
fn test_missing_protocol_version_fails() {
    // Raw JSON without protocol_version
    let json_str = r#"{"op":"probe","request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    // Should fail to parse
    assert!(result.is_err(), "request without protocol_version should fail to parse");
}

#[test]
fn test_missing_op_fails() {
    let json_str = r#"{"protocol_version":0,"request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "request without op should fail to parse");
}

#[test]
fn test_missing_request_id_fails() {
    let json_str = r#"{"protocol_version":0,"op":"probe","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "request without request_id should fail to parse");
}

#[test]
fn test_missing_payload_uses_default() {
    // payload has #[serde(default)] so missing payload should work
    let json_str = r#"{"protocol_version":0,"op":"probe","request_id":"test"}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    // Should succeed with default empty payload
    assert!(result.is_ok(), "missing payload should use default");
    let request = result.unwrap();
    assert!(request.payload.is_null() || request.payload.is_object());
}

// =============================================================================
// Test 3: Wrong types
// =============================================================================

#[test]
fn test_protocol_version_string_instead_of_int_fails() {
    let json_str = r#"{"protocol_version":"1","op":"probe","request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "string protocol_version should fail");
}

#[test]
fn test_op_int_instead_of_string_fails() {
    let json_str = r#"{"protocol_version":0,"op":123,"request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "int op should fail");
}

#[test]
fn test_request_id_null_fails() {
    let json_str = r#"{"protocol_version":0,"op":"probe","request_id":null,"payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "null request_id should fail");
}

// =============================================================================
// Test 4: Unknown op
// =============================================================================

#[test]
fn test_unknown_op_fails() {
    let json_str = r#"{"protocol_version":0,"op":"destroy","request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "unknown op 'destroy' should fail to parse");
}

#[test]
fn test_misspelled_op_fails() {
    let json_str = r#"{"protocol_version":0,"op":"probes","request_id":"test","payload":{}}"#;
    let result: Result<RpcRequest, _> = serde_json::from_str(json_str);

    assert!(result.is_err(), "misspelled op should fail");
}

// =============================================================================
// Test 5: Error response structure
// =============================================================================

#[test]
fn test_error_response_has_required_fields() {
    let worker = MockWorker::new();

    // Trigger an error by using invalid protocol version
    let request = make_request(Operation::Reserve, 999, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok, "should be error response");
    assert!(response.payload.is_none(), "error response should not have payload");
    assert!(response.error.is_some(), "error response should have error");

    let error = response.error.unwrap();
    assert!(!error.code.is_empty(), "error.code should be present");
    assert!(!error.message.is_empty(), "error.message should be present");
}

#[test]
fn test_error_message_is_single_line() {
    let worker = MockWorker::new();

    // Trigger an error
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "test",
        "job_key": "key",
        "source_sha256": "nonexistent"
    }));
    let response = worker.handle_request(&request);

    let error = response.error.unwrap();
    assert!(!error.message.contains('\n'), "error message should be single-line");
    assert!(!error.message.contains('\r'), "error message should not contain carriage return");
}

#[test]
fn test_error_message_no_stack_traces() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Status, 1, json!({"job_id": "nonexistent"}));
    let response = worker.handle_request(&request);

    let error = response.error.unwrap();
    // Stack traces typically contain "at " or ".rs:" patterns
    assert!(!error.message.contains(".rs:"), "error message should not contain stack traces");
    assert!(!error.message.contains(" at "), "error message should not contain 'at' from stack traces");
}

#[test]
fn test_error_data_is_object_or_absent() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Reserve, 999, json!({}));
    let response = worker.handle_request(&request);

    let error = response.error.unwrap();
    if let Some(data) = error.data {
        // data should be an object (HashMap), verify by checking it has keys
        assert!(data.keys().next().is_some() || data.is_empty(), "error.data should be an object");
    }
    // If data is None, that's also valid
}

// =============================================================================
// Test 6: Error code registry coverage
// =============================================================================

#[test]
fn test_error_code_invalid_request() {
    let worker = MockWorker::new();

    // Missing required field
    let request = make_request(Operation::Submit, 1, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "INVALID_REQUEST");
}

#[test]
fn test_error_code_unsupported_protocol() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Reserve, 999, json!({}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_error_code_busy() {
    let worker = MockWorker::new();
    worker.set_capacity(1);

    // First reserve succeeds
    let request = make_request(Operation::Reserve, 1, json!({"run_id": "run-1"}));
    worker.handle_request(&request);

    // Second reserve should fail with BUSY
    let request = make_request(Operation::Reserve, 1, json!({"run_id": "run-2"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    let error = response.error.unwrap();
    assert_eq!(error.code, "BUSY");

    // BUSY MUST include retry_after_seconds
    assert!(error.data.is_some(), "BUSY error must include data");
    let data = error.data.unwrap();
    assert!(data.contains_key("retry_after_seconds"), "BUSY must include retry_after_seconds");
}

#[test]
fn test_error_code_source_missing() {
    let worker = MockWorker::new();

    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "job-001",
        "job_key": "key",
        "source_sha256": "nonexistent"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "SOURCE_MISSING");
}

#[test]
fn test_error_code_artifacts_gone() {
    let worker = MockWorker::new();

    // Set up a completed job without artifacts
    worker.store_source("sha256-test", vec![1, 2, 3]);
    let submit = make_request(Operation::Submit, 1, json!({
        "job_id": "job-001",
        "job_key": "key",
        "source_sha256": "sha256-test"
    }));
    worker.handle_request(&submit);

    // Inject ARTIFACTS_GONE error for testing
    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "job-001"}));
    worker.inject_error(Operation::Fetch, "ARTIFACTS_GONE", "Artifacts have been deleted");
    let response = worker.handle_request(&fetch);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "ARTIFACTS_GONE");
}

#[test]
fn test_error_code_via_failure_injection() {
    let worker = MockWorker::new();

    // Test FEATURE_MISSING via injection
    worker.inject_error(Operation::Tail, "FEATURE_MISSING", "Feature not supported");
    let request = make_request(Operation::Tail, 1, json!({"job_id": "test"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "FEATURE_MISSING");

    // Clear and test LEASE_EXPIRED
    worker.clear_failures();
    worker.inject_error(Operation::Submit, "LEASE_EXPIRED", "Lease has expired");
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "test", "job_key": "key", "source_sha256": "sha"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "LEASE_EXPIRED");

    // Test PAYLOAD_TOO_LARGE
    worker.clear_failures();
    worker.inject_error(Operation::UploadSource, "PAYLOAD_TOO_LARGE", "Payload exceeds limit");
    let request = make_request(Operation::UploadSource, 1, json!({"source_sha256": "test"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "PAYLOAD_TOO_LARGE");
}

// =============================================================================
// Test 7: request_id echo
// =============================================================================

#[test]
fn test_request_id_uuid_echoed() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);
    assert_eq!(response.request_id, "550e8400-e29b-41d4-a716-446655440000");
}

#[test]
fn test_request_id_special_chars_echoed() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: "req-with-special-chars!@#$%^&*()".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);
    assert_eq!(response.request_id, "req-with-special-chars!@#$%^&*()");
}

#[test]
fn test_request_id_unicode_echoed() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: "req-日本語-emoji-🎉".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);
    assert_eq!(response.request_id, "req-日本語-emoji-🎉");
}

#[test]
fn test_request_id_long_string_echoed() {
    let worker = MockWorker::new();

    // Create a long request ID (256 characters)
    let long_id = "x".repeat(256);
    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: long_id.clone(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);
    assert_eq!(response.request_id, long_id);
}

#[test]
fn test_request_id_empty_echoed() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: "".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);
    assert_eq!(response.request_id, "");
}

// =============================================================================
// Test 8: Extra fields in request (forward compatibility)
// =============================================================================

#[test]
fn test_extra_fields_ignored() {
    let worker = MockWorker::new();

    // Parse JSON with extra field
    let json_str = r#"{
        "protocol_version": 0,
        "op": "probe",
        "request_id": "test",
        "payload": {},
        "extra_field": true,
        "another_field": {"nested": 123}
    }"#;

    let request: RpcRequest = serde_json::from_str(json_str).expect("should parse with extra fields");
    let response = worker.handle_request(&request);

    assert!(response.ok, "should succeed despite extra fields");
}

#[test]
fn test_extra_fields_in_payload_ignored() {
    let worker = MockWorker::new();

    // Extra fields in payload
    let request = make_request(Operation::Probe, 0, json!({
        "expected_field": "value",
        "extra_field": true
    }));

    let response = worker.handle_request(&request);
    assert!(response.ok, "should succeed with extra fields in payload");
}

// =============================================================================
// Test 9: Response JSON serialization
// =============================================================================

#[test]
fn test_response_serializes_to_valid_json() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 0, json!({}));

    let response = worker.handle_request(&request);
    let json_str = serde_json::to_string(&response).expect("response should serialize");

    // Should be parseable
    let _: serde_json::Value = serde_json::from_str(&json_str).expect("should be valid JSON");
}

#[test]
fn test_error_response_serializes_correctly() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Reserve, 999, json!({}));

    let response = worker.handle_request(&request);
    let json_str = serde_json::to_string(&response).expect("should serialize");

    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["ok"], false);
    assert!(parsed.get("error").is_some());
    assert!(parsed.get("payload").is_none(), "error response should not serialize payload");
}
