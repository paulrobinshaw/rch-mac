//! M7 Integration Tests: Resumable Uploads
//!
//! Tests for rch-mac-b7s.4: Resumable uploads
//!
//! Per PLAN.md M7 Hardening:
//! - Upload resumption with upload_id and offset tracking
//! - Worker advertises "upload_resumable" feature

use rch_protocol::ops::{UploadSourceRequest, UploadSourceResponse, UploadStream, ResumeInfo};

// === ResumeInfo Tests ===

#[test]
fn test_resume_info_serialization() {
    let resume = ResumeInfo {
        upload_id: "upload-abc123".to_string(),
        offset: 1048576, // 1MB
    };

    let json = serde_json::to_value(&resume).unwrap();

    assert_eq!(json["upload_id"], "upload-abc123");
    assert_eq!(json["offset"], 1048576);
}

#[test]
fn test_resume_info_deserialization() {
    let json = r#"{"upload_id": "upload-xyz789", "offset": 2097152}"#;

    let resume: ResumeInfo = serde_json::from_str(json).unwrap();

    assert_eq!(resume.upload_id, "upload-xyz789");
    assert_eq!(resume.offset, 2097152);
}

// === UploadSourceRequest with Resume ===

#[test]
fn test_upload_source_request_without_resume() {
    let request = UploadSourceRequest {
        source_sha256: "abc123def456".to_string(),
        stream: UploadStream {
            content_length: 1024,
            content_sha256: "deadbeef".to_string(),
            compression: "none".to_string(),
            format: "tar".to_string(),
        },
        resume: None,
    };

    let json = serde_json::to_value(&request).unwrap();

    // resume field should be absent when None (skip_serializing_if)
    assert!(json.get("resume").is_none());
    assert_eq!(json["source_sha256"], "abc123def456");
    assert_eq!(json["stream"]["content_length"], 1024);
}

#[test]
fn test_upload_source_request_with_resume() {
    let request = UploadSourceRequest {
        source_sha256: "abc123def456".to_string(),
        stream: UploadStream {
            content_length: 2048,
            content_sha256: "cafebabe".to_string(),
            compression: "zstd".to_string(),
            format: "tar".to_string(),
        },
        resume: Some(ResumeInfo {
            upload_id: "upload-resume-001".to_string(),
            offset: 1024,
        }),
    };

    let json = serde_json::to_value(&request).unwrap();

    assert!(json.get("resume").is_some());
    assert_eq!(json["resume"]["upload_id"], "upload-resume-001");
    assert_eq!(json["resume"]["offset"], 1024);
    // Remaining bytes = content_length - offset
    assert_eq!(json["stream"]["content_length"], 2048);
}

#[test]
fn test_upload_source_request_deserialization_without_resume() {
    let json = r#"{
        "source_sha256": "sha256hex",
        "stream": {
            "content_length": 4096,
            "content_sha256": "streamshasum",
            "compression": "none",
            "format": "tar"
        }
    }"#;

    let request: UploadSourceRequest = serde_json::from_str(json).unwrap();

    assert_eq!(request.source_sha256, "sha256hex");
    assert!(request.resume.is_none());
}

#[test]
fn test_upload_source_request_deserialization_with_resume() {
    let json = r#"{
        "source_sha256": "sha256hex",
        "stream": {
            "content_length": 8192,
            "content_sha256": "streamshasum",
            "compression": "zstd",
            "format": "tar"
        },
        "resume": {
            "upload_id": "resume-upload-id",
            "offset": 4096
        }
    }"#;

    let request: UploadSourceRequest = serde_json::from_str(json).unwrap();

    assert_eq!(request.source_sha256, "sha256hex");
    assert!(request.resume.is_some());
    let resume = request.resume.unwrap();
    assert_eq!(resume.upload_id, "resume-upload-id");
    assert_eq!(resume.offset, 4096);
}

// === UploadSourceResponse with Resumption Fields ===

#[test]
fn test_upload_source_response_complete_upload() {
    let response = UploadSourceResponse {
        accepted: true,
        source_sha256: "completed-sha256".to_string(),
        upload_id: None,
        next_offset: None,
    };

    let json = serde_json::to_value(&response).unwrap();

    assert_eq!(json["accepted"], true);
    assert_eq!(json["source_sha256"], "completed-sha256");
    // Optional fields should be absent when None
    assert!(json.get("upload_id").is_none());
    assert!(json.get("next_offset").is_none());
}

#[test]
fn test_upload_source_response_partial_upload() {
    let response = UploadSourceResponse {
        accepted: true,
        source_sha256: "partial-sha256".to_string(),
        upload_id: Some("partial-upload-id".to_string()),
        next_offset: Some(2048),
    };

    let json = serde_json::to_value(&response).unwrap();

    assert_eq!(json["accepted"], true);
    assert_eq!(json["source_sha256"], "partial-sha256");
    assert_eq!(json["upload_id"], "partial-upload-id");
    assert_eq!(json["next_offset"], 2048);
}

#[test]
fn test_upload_source_response_deserialization_complete() {
    let json = r#"{
        "accepted": true,
        "source_sha256": "source-hash-123"
    }"#;

    let response: UploadSourceResponse = serde_json::from_str(json).unwrap();

    assert!(response.accepted);
    assert_eq!(response.source_sha256, "source-hash-123");
    assert!(response.upload_id.is_none());
    assert!(response.next_offset.is_none());
}

#[test]
fn test_upload_source_response_deserialization_partial() {
    let json = r#"{
        "accepted": true,
        "source_sha256": "source-hash-456",
        "upload_id": "resumable-id",
        "next_offset": 1048576
    }"#;

    let response: UploadSourceResponse = serde_json::from_str(json).unwrap();

    assert!(response.accepted);
    assert_eq!(response.source_sha256, "source-hash-456");
    assert_eq!(response.upload_id, Some("resumable-id".to_string()));
    assert_eq!(response.next_offset, Some(1048576));
}

// === Worker Feature Advertisement ===

#[test]
fn test_worker_config_advertises_resumable() {
    use rch_worker::config::WorkerConfig;

    let config = WorkerConfig::default();

    assert!(
        config.features.contains(&"upload_resumable".to_string()),
        "Worker should advertise upload_resumable feature"
    );
}

#[test]
fn test_worker_config_has_upload_source() {
    use rch_worker::config::WorkerConfig;

    let config = WorkerConfig::default();

    // Both upload_source and upload_resumable should be present
    assert!(config.features.contains(&"upload_source".to_string()));
    assert!(config.features.contains(&"upload_resumable".to_string()));
}
