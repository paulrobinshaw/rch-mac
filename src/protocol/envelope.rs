//! RPC Envelope Types
//!
//! Defines the JSON RPC request/response envelope as specified in PLAN.md.
//!
//! Protocol: Single JSON request on stdin → single JSON response on stdout.
//! Maps directly to SSH forced-command entrypoint.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported RPC operations
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Returns protocol range/features + capabilities.json
    /// MUST accept protocol_version: 0 exclusively for this op
    Probe,
    /// Requests a lease for a run (capacity reservation)
    Reserve,
    /// Releases a lease early
    Release,
    /// Accepts job.json, returns ACK and initial status
    Submit,
    /// Returns current job status and pointers to logs/artifacts
    Status,
    /// Returns next chunk of logs/events given a cursor
    Tail,
    /// Requests best-effort cancellation
    Cancel,
    /// Returns job artifacts as binary-framed response
    Fetch,
    /// Returns {exists: bool} for a given source_sha256
    HasSource,
    /// Accepts a source bundle upload for a given source_sha256
    UploadSource,
}

impl Operation {
    /// Returns true if this operation accepts protocol_version: 0
    pub fn accepts_version_zero(&self) -> bool {
        matches!(self, Operation::Probe)
    }
}

/// RPC Request envelope
///
/// All worker operations accept this envelope format on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Protocol version (selected by host after probe)
    /// probe requests MUST use protocol_version: 0
    pub protocol_version: i32,

    /// Operation to perform
    pub op: Operation,

    /// Caller-chosen request ID for correlation and retries
    /// MUST be unique per RPC request (per host process)
    pub request_id: String,

    /// Operation-specific payload
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// RPC Response envelope
///
/// All worker operations emit this envelope format on stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Protocol version (selected by host after probe)
    /// For probe responses, this MUST be 0
    pub protocol_version: i32,

    /// Echoed request ID for correlation
    pub request_id: String,

    /// Whether the operation succeeded
    pub ok: bool,

    /// Operation-specific payload (present when ok=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,

    /// Error details (present when ok=false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcErrorPayload>,
}

impl RpcResponse {
    /// Create a successful response
    pub fn success(protocol_version: i32, request_id: String, payload: serde_json::Value) -> Self {
        Self {
            protocol_version,
            request_id,
            ok: true,
            payload: Some(payload),
            error: None,
        }
    }

    /// Create an error response
    pub fn error(protocol_version: i32, request_id: String, error: RpcErrorPayload) -> Self {
        Self {
            protocol_version,
            request_id,
            ok: false,
            payload: None,
            error: Some(error),
        }
    }
}

/// Error payload structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcErrorPayload {
    /// Error code from the registry
    pub code: String,

    /// Human-readable, single-line error message
    /// MUST NOT include secrets, file system paths outside job directory, or stack traces
    pub message: String,

    /// Optional machine-readable details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<HashMap<String, serde_json::Value>>,
}

impl RpcErrorPayload {
    /// Create a new error payload
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    /// Add machine-readable data to the error
    pub fn with_data(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.data.get_or_insert_with(HashMap::new).insert(key.into(), value);
        self
    }
}

/// Binary framing stream metadata
///
/// Used for upload_source and fetch operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMetadata {
    /// Length of the raw bytes following the JSON header
    pub content_length: u64,

    /// SHA-256 hex digest of the raw streamed bytes
    pub content_sha256: String,

    /// Compression: none | zstd
    #[serde(default = "default_compression")]
    pub compression: String,

    /// Format: tar
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_compression() -> String {
    "none".to_string()
}

fn default_format() -> String {
    "tar".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_version_zero() {
        assert!(Operation::Probe.accepts_version_zero());
        assert!(!Operation::Submit.accepts_version_zero());
        assert!(!Operation::Status.accepts_version_zero());
    }

    #[test]
    fn test_request_parsing() {
        let json = r#"{
            "protocol_version": 0,
            "op": "probe",
            "request_id": "req-001",
            "payload": {}
        }"#;

        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.protocol_version, 0);
        assert_eq!(req.op, Operation::Probe);
        assert_eq!(req.request_id, "req-001");
    }

    #[test]
    fn test_response_success() {
        let resp = RpcResponse::success(
            1,
            "req-001".to_string(),
            serde_json::json!({"status": "ok"})
        );

        assert!(resp.ok);
        assert!(resp.payload.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_response_error() {
        let err = RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id")
            .with_data("field", serde_json::json!("job_id"));

        let resp = RpcResponse::error(1, "req-002".to_string(), err);

        assert!(!resp.ok);
        assert!(resp.payload.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }
}
