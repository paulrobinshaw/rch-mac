//! Protocol Version Negotiation Tests
//!
//! Tests for bead rch-mac-666.2: Protocol version bootstrap and negotiation.

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
// Test 1: Probe with protocol_version=0 (sentinel)
// =============================================================================

#[test]
fn test_probe_with_version_zero_returns_capabilities() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 0, json!({}));

    let response = worker.handle_request(&request);

    // Must succeed
    assert!(response.ok, "probe request should succeed");
    assert_eq!(response.protocol_version, 0, "probe response must echo protocol_version: 0");

    // Must include required fields
    let payload = response.payload.expect("probe response must have payload");
    assert!(payload.get("protocol_min").is_some(), "must include protocol_min");
    assert!(payload.get("protocol_max").is_some(), "must include protocol_max");
    assert!(payload.get("features").is_some(), "must include features");

    // Type checking
    assert!(payload["protocol_min"].is_number(), "protocol_min must be a number");
    assert!(payload["protocol_max"].is_number(), "protocol_max must be a number");
    assert!(payload["features"].is_array(), "features must be an array");
}

#[test]
fn test_probe_includes_all_capability_fields() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 0, json!({}));

    let response = worker.handle_request(&request);
    let payload = response.payload.expect("probe response must have payload");

    // Check all expected capability fields
    assert_eq!(payload["kind"], "probe");
    assert!(payload.get("schema_id").is_some(), "must include schema_id");
    assert!(payload.get("macos").is_some(), "must include macos info");
    assert!(payload.get("xcode_versions").is_some(), "must include xcode_versions");
    assert!(payload.get("capacity").is_some(), "must include capacity");

    // Check nested structure
    let macos = &payload["macos"];
    assert!(macos.get("version").is_some(), "macos must include version");
    assert!(macos.get("arch").is_some(), "macos must include arch");

    let capacity = &payload["capacity"];
    assert!(capacity.get("max_concurrent_jobs").is_some(), "capacity must include max_concurrent_jobs");
}

// =============================================================================
// Test 2: Non-probe op with protocol_version=0 must fail
// =============================================================================

#[test]
fn test_submit_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Submit, 0, json!({
        "job_id": "test-job",
        "job_key": "test-key",
        "source_sha256": "abc123"
    }));

    let response = worker.handle_request(&request);

    assert!(!response.ok, "submit with version 0 should fail");
    let error = response.error.expect("should have error");
    assert_eq!(error.code, "UNSUPPORTED_PROTOCOL", "error code should be UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_status_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Status, 0, json!({"job_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_tail_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Tail, 0, json!({"job_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_cancel_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Cancel, 0, json!({"job_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_fetch_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Fetch, 0, json!({"job_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_has_source_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::HasSource, 0, json!({"source_sha256": "abc"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_upload_source_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::UploadSource, 0, json!({"source_sha256": "abc"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_reserve_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Reserve, 0, json!({"run_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_release_with_version_zero_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Release, 0, json!({"lease_id": "test"}));

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

// =============================================================================
// Test 3: Probe with non-zero version must fail
// =============================================================================

#[test]
fn test_probe_with_version_one_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 1, json!({}));

    let response = worker.handle_request(&request);

    assert!(!response.ok, "probe with version 1 should fail");
    let error = response.error.expect("should have error");
    assert_eq!(error.code, "UNSUPPORTED_PROTOCOL");
}

#[test]
fn test_probe_with_version_negative_fails() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, -1, json!({}));

    let response = worker.handle_request(&request);

    assert!(!response.ok, "probe with negative version should fail");
    assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
}

// =============================================================================
// Test 4: Version selection - operations within supported range
// =============================================================================

#[test]
fn test_operation_with_supported_version_succeeds() {
    let worker = MockWorker::new();

    // First probe to get the supported range
    let probe_request = make_request(Operation::Probe, 0, json!({}));
    let probe_response = worker.handle_request(&probe_request);
    let payload = probe_response.payload.unwrap();
    let protocol_max = payload["protocol_max"].as_i64().unwrap() as i32;

    // Now make a request with that version
    let request = make_request(Operation::Reserve, protocol_max, json!({"run_id": "test"}));
    let response = worker.handle_request(&request);

    assert!(response.ok, "operation with supported version should succeed");
    assert_eq!(response.protocol_version, protocol_max);
}

#[test]
fn test_operation_with_version_below_min_fails() {
    let worker = MockWorker::new();

    // The default worker supports [1, 1], so version 0 for non-probe should fail
    // (already tested above), but let's test version > max
    let request = make_request(Operation::Reserve, 999, json!({"run_id": "test"}));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    let error = response.error.unwrap();
    assert_eq!(error.code, "UNSUPPORTED_PROTOCOL");

    // Should include min/max in data
    assert!(error.data.is_some());
    let data = error.data.unwrap();
    assert!(data.contains_key("min"));
    assert!(data.contains_key("max"));
}

// =============================================================================
// Test 5: Feature detection via probe
// =============================================================================

#[test]
fn test_probe_returns_expected_features() {
    let worker = MockWorker::new();
    let request = make_request(Operation::Probe, 0, json!({}));

    let response = worker.handle_request(&request);
    let payload = response.payload.unwrap();
    let features = payload["features"].as_array().unwrap();

    // Default worker should support these features
    let feature_strings: Vec<&str> = features.iter()
        .filter_map(|v| v.as_str())
        .collect();

    assert!(feature_strings.contains(&"probe"), "should support probe");
    assert!(feature_strings.contains(&"tail"), "should support tail");
}

// =============================================================================
// Test 6: Protocol version consistency
// =============================================================================

#[test]
fn test_response_echoes_request_version() {
    let worker = MockWorker::new();

    // Reserve with version 1
    let request = make_request(Operation::Reserve, 1, json!({"run_id": "test"}));
    let response = worker.handle_request(&request);

    assert!(response.ok);
    assert_eq!(response.protocol_version, 1, "response should echo request version");
}

#[test]
fn test_error_response_includes_request_version() {
    let worker = MockWorker::new();

    // Submit without source (will fail with SOURCE_MISSING)
    let request = make_request(Operation::Submit, 1, json!({
        "job_id": "test",
        "job_key": "key",
        "source_sha256": "nonexistent"
    }));
    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.protocol_version, 1, "error response should echo request version");
}

// =============================================================================
// Test 7: Request ID correlation
// =============================================================================

#[test]
fn test_response_echoes_request_id() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 0,
        op: Operation::Probe,
        request_id: "unique-correlation-id-12345".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);

    assert!(response.ok);
    assert_eq!(response.request_id, "unique-correlation-id-12345");
}

#[test]
fn test_error_response_echoes_request_id() {
    let worker = MockWorker::new();

    let request = RpcRequest {
        protocol_version: 999,  // Invalid version
        op: Operation::Reserve,
        request_id: "error-correlation-id".to_string(),
        payload: json!({}),
    };

    let response = worker.handle_request(&request);

    assert!(!response.ok);
    assert_eq!(response.request_id, "error-correlation-id");
}

// =============================================================================
// Test 8: JSON handling
// =============================================================================

#[test]
fn test_handle_json_valid_request() {
    let worker = MockWorker::new();

    let json_request = r#"{"protocol_version":0,"op":"probe","request_id":"json-test","payload":{}}"#;
    let json_response = worker.handle_json(json_request).expect("should parse");

    // Parse the response
    let response: serde_json::Value = serde_json::from_str(&json_response).unwrap();
    assert!(response["ok"].as_bool().unwrap());
    assert_eq!(response["request_id"], "json-test");
}

#[test]
fn test_handle_json_invalid_json() {
    let worker = MockWorker::new();

    let result = worker.handle_json("not valid json");
    assert!(result.is_err(), "should fail to parse invalid JSON");
}
