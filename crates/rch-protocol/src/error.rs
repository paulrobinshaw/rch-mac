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
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}
