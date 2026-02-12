//! M7 Integration Tests: Resumable Uploads
//!
//! Tests for rch-mac-b7s.4: Resumable uploads
//!
//! Per PLAN.md:
//! - When worker advertises feature `upload_resumable`:
//! - `upload_source` payload includes optional `resume` object with `upload_id` and `offset`
//! - Worker responds with `next_offset` for resumable uploads
//! - Enables recovery from interrupted large bundle uploads

use std::sync::Arc;

use rch_xcode_lane::host::rpc::RpcClient;
use rch_xcode_lane::host::resumable::{ResumeRequest, UploadSession};
use rch_xcode_lane::host::transport::MockTransport;
use rch_xcode_lane::worker::capabilities::features;

// === Feature Detection Tests ===

#[test]
fn test_upload_resumable_feature_constant() {
    assert_eq!(features::UPLOAD_RESUMABLE, "upload_resumable");
}

// === Upload Session Tests ===

#[test]
fn test_upload_session_creation() {
    let session = UploadSession::new(
        "upload-001".to_string(),
        "sha256-abc".to_string(),
        1000,
    );

    assert_eq!(session.upload_id, "upload-001");
    assert_eq!(session.source_sha256, "sha256-abc");
    assert_eq!(session.content_length, 1000);
    assert_eq!(session.offset, 0);
    assert!(!session.is_complete());
}

#[test]
fn test_upload_session_progress() {
    let mut session = UploadSession::new(
        "upload-001".to_string(),
        "sha256-abc".to_string(),
        1000,
    );

    // Initial state
    assert_eq!(session.remaining(), 1000);
    assert!(!session.is_complete());

    // Partial upload
    session.advance(400);
    assert_eq!(session.offset, 400);
    assert_eq!(session.remaining(), 600);
    assert!(!session.is_complete());

    // More progress
    session.advance(400);
    assert_eq!(session.offset, 800);
    assert_eq!(session.remaining(), 200);

    // Complete
    session.advance(200);
    assert_eq!(session.offset, 1000);
    assert_eq!(session.remaining(), 0);
    assert!(session.is_complete());
}

#[test]
fn test_upload_session_advance_overflow_protection() {
    let mut session = UploadSession::new(
        "upload-001".to_string(),
        "sha256-abc".to_string(),
        100,
    );

    // Advance beyond content_length (should saturate)
    session.advance(200);
    assert_eq!(session.offset, 200);
    assert!(session.is_complete());
    assert_eq!(session.remaining(), 0);
}

// === Resume Request/Response Serialization Tests ===

#[test]
fn test_resume_request_serialization() {
    let request = ResumeRequest {
        upload_id: "upload-abc-123".to_string(),
        offset: 51200,
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("upload-abc-123"));
    assert!(json.contains("51200"));

    let parsed: ResumeRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.upload_id, "upload-abc-123");
    assert_eq!(parsed.offset, 51200);
}

// === RPC Client Resumable Upload Tests ===

#[test]
fn test_upload_source_basic() {
    let transport = MockTransport::new();
    let mut client = RpcClient::new(Arc::new(transport));

    client.probe().unwrap();

    let content = vec![1u8, 2, 3, 4, 5];
    let result = client.upload_source("sha256-test", &content);
    assert!(result.is_ok());
}

#[test]
fn test_upload_source_resumable_fresh_upload() {
    let transport = MockTransport::new();
    let mut client = RpcClient::new(Arc::new(transport));

    client.probe().unwrap();

    let content = vec![0u8; 1000];
    let result = client.upload_source_resumable("sha256-test", &content, None);

    let response = result.unwrap();
    assert!(response.complete);
    assert!(response.upload_id.is_some());
    assert_eq!(response.next_offset, 1000);
}

#[test]
fn test_upload_source_resumable_with_resume() {
    let transport = MockTransport::new();
    let mut client = RpcClient::new(Arc::new(transport));

    client.probe().unwrap();

    // First upload (partial - simulate getting upload_id back)
    let content = vec![0u8; 1000];
    let result1 = client.upload_source_resumable("sha256-partial", &content, None);
    let response1 = result1.unwrap();
    let upload_id = response1.upload_id.clone().unwrap();

    // Resume from where we left off (simulated - in reality this would be after interruption)
    let resume = ResumeRequest {
        upload_id: upload_id.clone(),
        offset: response1.next_offset,
    };

    // Upload again with resume request (should work even if already complete)
    let result2 = client.upload_source_resumable("sha256-partial", &content, Some(resume));
    let response2 = result2.unwrap();
    assert!(response2.complete);
}

#[test]
fn test_upload_source_already_complete_resume() {
    let transport = MockTransport::new();
    let mut client = RpcClient::new(Arc::new(transport));

    client.probe().unwrap();

    let content = vec![0u8; 100];

    // Resume with offset at end of content
    let resume = ResumeRequest {
        upload_id: "upload-already-done".to_string(),
        offset: 100, // Already at the end
    };

    let result = client.upload_source_resumable("sha256-done", &content, Some(resume));
    let response = result.unwrap();
    assert!(response.complete);
    assert_eq!(response.next_offset, 100);
}

// === Upload Session Store Tests ===

#[test]
fn test_upload_session_store_operations() {
    use rch_xcode_lane::host::resumable::UploadSessionStore;

    let store = UploadSessionStore::new();

    // Create session
    let session1 = store.get_or_create("upload-001", "sha256-abc", 1000);
    assert_eq!(session1.upload_id, "upload-001");
    assert_eq!(session1.offset, 0);

    // Get same session
    let session2 = store.get_or_create("upload-001", "sha256-abc", 1000);
    assert_eq!(session2.upload_id, "upload-001");

    // Update session
    let mut session = store.get("upload-001").unwrap();
    session.advance(500);
    store.update(session);

    let retrieved = store.get("upload-001").unwrap();
    assert_eq!(retrieved.offset, 500);

    // Remove session
    let removed = store.remove("upload-001");
    assert!(removed.is_some());
    assert!(store.get("upload-001").is_none());
}

#[test]
fn test_upload_session_store_find_by_source() {
    use rch_xcode_lane::host::resumable::UploadSessionStore;

    let store = UploadSessionStore::new();

    store.get_or_create("upload-001", "sha256-abc", 1000);
    store.get_or_create("upload-002", "sha256-def", 2000);

    let found = store.find_by_source("sha256-abc");
    assert!(found.is_some());
    assert_eq!(found.unwrap().upload_id, "upload-001");

    let not_found = store.find_by_source("sha256-xyz");
    assert!(not_found.is_none());
}

// === Upload ID Generation Tests ===

#[test]
fn test_generate_upload_id_uniqueness() {
    use std::collections::HashSet;
    use rch_xcode_lane::host::resumable::generate_upload_id;

    let mut ids = HashSet::new();

    for _ in 0..100 {
        let id = generate_upload_id();
        assert!(id.starts_with("upload-"), "ID should start with 'upload-': {}", id);
        assert!(ids.insert(id), "Generated duplicate upload ID");
    }

    assert_eq!(ids.len(), 100);
}

// === Mock Worker Resumable Upload Tests ===

#[test]
fn test_mock_worker_upload_with_session() {
    use rch_xcode_lane::mock::MockWorker;
    use rch_xcode_lane::protocol::{RpcRequest, Operation};
    use serde_json::json;

    let worker = MockWorker::new();

    // Start upload
    let request1 = RpcRequest {
        protocol_version: 1,
        op: Operation::UploadSource,
        request_id: "req-001".to_string(),
        payload: json!({
            "source_sha256": "sha256-test",
            "stream": {
                "content_length": 1000,
                "content_sha256": "sha256-test",
                "compression": "none",
                "format": "tar"
            }
        }),
    };

    let response1 = worker.handle_request(&request1);
    assert!(response1.ok);

    let payload1 = response1.payload.unwrap();
    assert!(payload1.get("upload_id").is_some());
    assert!(payload1.get("next_offset").is_some());
}

#[test]
fn test_mock_worker_upload_resume() {
    use rch_xcode_lane::mock::MockWorker;
    use rch_xcode_lane::protocol::{RpcRequest, Operation};
    use serde_json::json;

    let worker = MockWorker::new();

    // First upload - this will complete in mock (simulates full content received)
    let request1 = RpcRequest {
        protocol_version: 1,
        op: Operation::UploadSource,
        request_id: "req-001".to_string(),
        payload: json!({
            "source_sha256": "sha256-partial",
            "stream": {
                "content_length": 1000,
                "content_sha256": "sha256-partial",
                "compression": "none",
                "format": "tar"
            }
        }),
    };

    let response1 = worker.handle_request(&request1);
    assert!(response1.ok);

    let payload1 = response1.payload.unwrap();
    let upload_id = payload1["upload_id"].as_str().unwrap();
    let next_offset = payload1["next_offset"].as_u64().unwrap();

    // In the mock, the first upload completes successfully
    // Verify it returned the resumable upload response fields
    assert_eq!(next_offset, 1000);
    assert!(payload1["complete"].as_bool().unwrap());
    assert!(!upload_id.is_empty());

    // Verify the source was stored
    let has_source_request = RpcRequest {
        protocol_version: 1,
        op: Operation::HasSource,
        request_id: "req-check".to_string(),
        payload: json!({"source_sha256": "sha256-partial"}),
    };
    let has_source_response = worker.handle_request(&has_source_request);
    assert!(has_source_response.ok);
    let exists = has_source_response.payload.unwrap()["exists"].as_bool().unwrap();
    assert!(exists, "Source should be stored after complete upload");
}

// === Error Handling Tests ===

#[test]
fn test_upload_payload_too_large() {
    use rch_xcode_lane::mock::MockWorker;
    use rch_xcode_lane::protocol::{RpcRequest, Operation};
    use serde_json::json;

    let worker = MockWorker::new();
    worker.set_max_upload_bytes(100);

    let request = RpcRequest {
        protocol_version: 1,
        op: Operation::UploadSource,
        request_id: "req-001".to_string(),
        payload: json!({
            "source_sha256": "sha256-big",
            "stream": {
                "content_length": 1000, // Exceeds 100 byte limit
                "content_sha256": "sha256-big",
                "compression": "none",
                "format": "tar"
            }
        }),
    };

    let response = worker.handle_request(&request);
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "PAYLOAD_TOO_LARGE");
}
