//! RPC request types.

use serde::{Deserialize, Serialize};

/// RPC request envelope.
///
/// All worker operations accept a single JSON request on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Protocol version (selected by host after probe).
    /// For probe requests, this MUST be 0.
    pub protocol_version: i32,
    /// Operation name.
    pub op: String,
    /// Caller-chosen request ID for correlation.
    pub request_id: String,
    /// Operation-specific payload.
    pub payload: serde_json::Value,
}

impl RpcRequest {
    /// Check if this request includes a binary stream.
    pub fn has_stream(&self) -> bool {
        self.payload.get("stream").is_some()
    }

    /// Get the stream metadata if present.
    pub fn stream_info(&self) -> Option<StreamInfo> {
        self.payload.get("stream").and_then(|s| serde_json::from_value(s.clone()).ok())
    }
}

/// Binary stream metadata for framed requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Length of the binary payload in bytes.
    pub content_length: u64,
    /// SHA-256 hex digest of the raw streamed bytes.
    pub content_sha256: String,
    /// Compression algorithm (none or zstd).
    #[serde(default)]
    pub compression: Compression,
    /// Format of the stream (tar).
    #[serde(default)]
    pub format: StreamFormat,
}

/// Compression algorithm for binary streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    #[default]
    None,
    Zstd,
}

/// Format of binary streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamFormat {
    #[default]
    Tar,
}
