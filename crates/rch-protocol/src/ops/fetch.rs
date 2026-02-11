//! Fetch operation types.
//!
//! Retrieve job artifacts as binary-framed response.

use serde::{Deserialize, Serialize};

/// Fetch request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchRequest {
    /// The job ID to fetch artifacts for.
    pub job_id: String,
}

/// Fetch response header (binary-framed, not wrapped in RpcResponse).
///
/// This is the JSON header line for a binary-framed response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponseHeader {
    /// Protocol version.
    pub protocol_version: i32,
    /// Request ID (echoed).
    pub request_id: String,
    /// Always true for success.
    pub ok: bool,
    /// The job ID.
    pub job_id: String,
    /// The manifest.json content.
    pub manifest: serde_json::Value,
    /// Stream metadata for the binary payload.
    pub stream: FetchStream,
}

/// Stream metadata for fetch response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchStream {
    /// Length of the binary payload in bytes.
    pub content_length: u64,
    /// SHA-256 hex digest of the raw streamed bytes.
    pub content_sha256: String,
    /// Compression algorithm (none or zstd).
    pub compression: String,
    /// Format of the stream (tar).
    pub format: String,
}
