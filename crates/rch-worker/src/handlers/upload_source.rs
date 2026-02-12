//! Upload-source operation handler.
//!
//! Accepts a binary-framed source bundle upload.
//!
//! Note: For the mock worker, we don't actually read the binary payload.
//! In a real implementation, this would read `content_length` bytes from stdin
//! after the JSON header line and verify the SHA-256 digest.

use rch_protocol::{RpcError, RpcRequest, ops::{UploadSourceRequest, UploadSourceResponse}};
use crate::config::WorkerConfig;
use crate::mock_state::MockState;

/// Handle the upload_source operation.
pub fn handle(request: &RpcRequest, config: &WorkerConfig, state: &MockState) -> Result<serde_json::Value, RpcError> {
    // Parse request
    let req: UploadSourceRequest = serde_json::from_value(request.payload.clone())
        .map_err(|e| RpcError::invalid_request(format!("invalid upload_source request: {}", e)))?;

    // Check size limit
    if req.stream.content_length > config.max_upload_bytes {
        return Err(RpcError::payload_too_large(
            req.stream.content_length,
            config.max_upload_bytes,
        ));
    }

    // For the mock worker, we just record the source as existing
    // In a real implementation, we would:
    // 1. Read content_length bytes from stdin
    // 2. Verify content_sha256 matches
    // 3. Decompress if needed
    // 4. Verify source_sha256 matches the decompressed content
    // 5. Store atomically (write-to-temp-then-rename)

    // Idempotent: if source already exists, that's fine
    if !state.has_source(&req.source_sha256) {
        state.add_source(
            req.source_sha256.clone(),
            req.stream.content_sha256.clone(),
            req.stream.content_length,
        );
    }

    let response = UploadSourceResponse {
        accepted: true,
        source_sha256: req.source_sha256,
        upload_id: None,    // Full upload completed, no resumption needed
        next_offset: None,  // Full upload completed, no resumption needed
    };

    serde_json::to_value(response).map_err(|e| {
        RpcError::invalid_request(format!("failed to serialize response: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_upload_request(source_sha256: &str, content_length: u64) -> RpcRequest {
        RpcRequest {
            protocol_version: 1,
            op: "upload_source".to_string(),
            request_id: "test-001".to_string(),
            payload: serde_json::json!({
                "source_sha256": source_sha256,
                "stream": {
                    "content_length": content_length,
                    "content_sha256": "deadbeef",
                    "compression": "none",
                    "format": "tar"
                }
            }),
        }
    }

    #[test]
    fn test_upload_source_accepts() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        let request = make_upload_request("abc123", 1024);

        let result = handle(&request, &config, &state).unwrap();
        let response: UploadSourceResponse = serde_json::from_value(result).unwrap();

        assert!(response.accepted);
        assert_eq!(response.source_sha256, "abc123");
        assert!(state.has_source("abc123"));
    }

    #[test]
    fn test_upload_source_idempotent() {
        let config = WorkerConfig::default();
        let state = MockState::new();

        // Pre-add the source
        state.add_source("abc123".to_string(), "xyz".to_string(), 500);

        let request = make_upload_request("abc123", 1024);

        // Should succeed even though source already exists
        let result = handle(&request, &config, &state).unwrap();
        let response: UploadSourceResponse = serde_json::from_value(result).unwrap();

        assert!(response.accepted);
    }

    #[test]
    fn test_upload_source_too_large() {
        let mut config = WorkerConfig::default();
        config.max_upload_bytes = 1000;
        let state = MockState::new();

        let request = make_upload_request("abc123", 2000);

        let result = handle(&request, &config, &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, rch_protocol::ErrorCode::PayloadTooLarge);
    }
}
