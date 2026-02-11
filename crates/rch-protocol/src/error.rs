//! Error types for the RPC protocol.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Error codes returned in RPC error responses.
///
/// These codes are stable and used for automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Malformed JSON, missing required fields, or invalid field values.
    InvalidRequest,
    /// Protocol version is outside the supported range.
    UnsupportedProtocol,
    /// A required feature is not available on this worker.
    FeatureMissing,
    /// Worker is at capacity; retry after the specified delay.
    Busy,
    /// The job lease has expired.
    LeaseExpired,
    /// The referenced source bundle is not in the content-addressed store.
    SourceMissing,
    /// Job artifacts are no longer available for fetch.
    ArtifactsGone,
    /// Upload exceeds the maximum allowed size.
    PayloadTooLarge,
    /// Unknown operation requested.
    UnknownOperation,
    /// Job not found.
    JobNotFound,
    /// Job key mismatch (job_id exists with different job_key).
    JobKeyMismatch,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "INVALID_REQUEST"),
            Self::UnsupportedProtocol => write!(f, "UNSUPPORTED_PROTOCOL"),
            Self::FeatureMissing => write!(f, "FEATURE_MISSING"),
            Self::Busy => write!(f, "BUSY"),
            Self::LeaseExpired => write!(f, "LEASE_EXPIRED"),
            Self::SourceMissing => write!(f, "SOURCE_MISSING"),
            Self::ArtifactsGone => write!(f, "ARTIFACTS_GONE"),
            Self::PayloadTooLarge => write!(f, "PAYLOAD_TOO_LARGE"),
            Self::UnknownOperation => write!(f, "UNKNOWN_OPERATION"),
            Self::JobNotFound => write!(f, "JOB_NOT_FOUND"),
            Self::JobKeyMismatch => write!(f, "JOB_KEY_MISMATCH"),
        }
    }
}

/// RPC error response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Error code from the registry.
    pub code: ErrorCode,
    /// Human-readable, single-line error message.
    /// Must not contain secrets, filesystem paths outside job dirs, or stack traces.
    pub message: String,
    /// Optional machine-readable details (failing field, expected vs actual values).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// Create a new RPC error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Create a new RPC error with additional data.
    pub fn with_data(code: ErrorCode, message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create an INVALID_REQUEST error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    /// Create an UNSUPPORTED_PROTOCOL error.
    pub fn unsupported_protocol(version: i32, min: i32, max: i32) -> Self {
        Self::with_data(
            ErrorCode::UnsupportedProtocol,
            format!("protocol_version {} is outside supported range [{}, {}]", version, min, max),
            serde_json::json!({
                "requested": version,
                "min": min,
                "max": max
            }),
        )
    }

    /// Create an UNKNOWN_OPERATION error.
    pub fn unknown_operation(op: &str) -> Self {
        Self::with_data(
            ErrorCode::UnknownOperation,
            format!("unknown operation: {}", op),
            serde_json::json!({ "op": op }),
        )
    }

    /// Create a BUSY error with retry information.
    pub fn busy(retry_after_seconds: u32) -> Self {
        Self::with_data(
            ErrorCode::Busy,
            format!("worker is at capacity, retry after {} seconds", retry_after_seconds),
            serde_json::json!({ "retry_after_seconds": retry_after_seconds }),
        )
    }

    /// Create a FEATURE_MISSING error.
    pub fn feature_missing(feature: &str) -> Self {
        Self::with_data(
            ErrorCode::FeatureMissing,
            format!("feature '{}' is not available on this worker", feature),
            serde_json::json!({ "feature": feature }),
        )
    }

    /// Create a LEASE_EXPIRED error.
    pub fn lease_expired(lease_id: &str) -> Self {
        Self::with_data(
            ErrorCode::LeaseExpired,
            format!("lease '{}' has expired or does not exist", lease_id),
            serde_json::json!({ "lease_id": lease_id }),
        )
    }

    /// Create a SOURCE_MISSING error.
    pub fn source_missing(source_sha256: &str) -> Self {
        Self::with_data(
            ErrorCode::SourceMissing,
            format!("source bundle '{}' is not in the content-addressed store", source_sha256),
            serde_json::json!({ "source_sha256": source_sha256 }),
        )
    }

    /// Create an ARTIFACTS_GONE error.
    pub fn artifacts_gone(job_id: &str) -> Self {
        Self::with_data(
            ErrorCode::ArtifactsGone,
            format!("artifacts for job '{}' are no longer available", job_id),
            serde_json::json!({ "job_id": job_id }),
        )
    }

    /// Create a PAYLOAD_TOO_LARGE error.
    pub fn payload_too_large(size: u64, max: u64) -> Self {
        Self::with_data(
            ErrorCode::PayloadTooLarge,
            format!("payload size {} exceeds maximum {}", size, max),
            serde_json::json!({ "size": size, "max_bytes": max }),
        )
    }

    /// Create a JOB_NOT_FOUND error.
    pub fn job_not_found(job_id: &str) -> Self {
        Self::with_data(
            ErrorCode::JobNotFound,
            format!("job '{}' not found", job_id),
            serde_json::json!({ "job_id": job_id }),
        )
    }

    /// Create a JOB_KEY_MISMATCH error.
    pub fn job_key_mismatch(job_id: &str, expected: &str, actual: &str) -> Self {
        Self::with_data(
            ErrorCode::JobKeyMismatch,
            format!("job '{}' exists with different job_key", job_id),
            serde_json::json!({
                "job_id": job_id,
                "expected_job_key": expected,
                "actual_job_key": actual
            }),
        )
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}
