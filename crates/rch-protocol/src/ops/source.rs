//! Source store operation types.
//!
//! has_source and upload_source for content-addressed source bundles.

use serde::{Deserialize, Serialize};

/// Has-source request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HasSourceRequest {
    /// SHA-256 hex digest of the source bundle.
    pub source_sha256: String,
}

/// Has-source response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HasSourceResponse {
    /// Whether the source exists in the store.
    pub exists: bool,
}

/// Upload-source request payload.
///
/// The actual binary content follows the JSON header line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSourceRequest {
    /// SHA-256 hex digest of the source bundle.
    pub source_sha256: String,
    /// Stream metadata for the binary payload.
    pub stream: UploadStream,
}

/// Stream metadata for upload_source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStream {
    /// Length of the binary payload in bytes.
    pub content_length: u64,
    /// SHA-256 hex digest of the raw streamed bytes.
    pub content_sha256: String,
    /// Compression algorithm.
    #[serde(default)]
    pub compression: String,
    /// Format of the stream.
    #[serde(default)]
    pub format: String,
}

/// Upload-source response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSourceResponse {
    /// Whether the upload was accepted.
    pub accepted: bool,
    /// The source SHA-256 (echoed).
    pub source_sha256: String,
}
