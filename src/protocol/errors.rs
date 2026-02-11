//! RPC Error Code Registry
//!
//! Defines the standard error codes as specified in PLAN.md § Error codes.

use super::envelope::RpcErrorPayload;

/// Standard error codes for worker RPC responses
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Malformed JSON, missing fields
    InvalidRequest,
    /// No protocol version intersection
    UnsupportedProtocol,
    /// Required feature absent from worker
    FeatureMissing,
    /// Worker at capacity; retry later
    Busy,
    /// Job lease timed out
    LeaseExpired,
    /// Referenced source bundle not in store
    SourceMissing,
    /// Job artifacts no longer available for fetch
    ArtifactsGone,
    /// Upload exceeds size limit
    PayloadTooLarge,
}

impl ErrorCode {
    /// Returns the string representation of the error code
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "INVALID_REQUEST",
            ErrorCode::UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
            ErrorCode::FeatureMissing => "FEATURE_MISSING",
            ErrorCode::Busy => "BUSY",
            ErrorCode::LeaseExpired => "LEASE_EXPIRED",
            ErrorCode::SourceMissing => "SOURCE_MISSING",
            ErrorCode::ArtifactsGone => "ARTIFACTS_GONE",
            ErrorCode::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Worker RPC error type
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Unsupported protocol version {version}; supported range: [{min}, {max}]")]
    UnsupportedProtocol { version: i32, min: i32, max: i32 },

    #[error("Required feature missing: {0}")]
    FeatureMissing(String),

    #[error("Worker busy; retry after {retry_after_seconds} seconds")]
    Busy { retry_after_seconds: u32 },

    #[error("Lease expired for job {0}")]
    LeaseExpired(String),

    #[error("Source bundle not found: {0}")]
    SourceMissing(String),

    #[error("Artifacts no longer available for job {0}")]
    ArtifactsGone(String),

    #[error("Payload exceeds size limit: {size} bytes > {limit} bytes")]
    PayloadTooLarge { size: u64, limit: u64 },
}

impl RpcError {
    /// Returns the error code for this error
    pub fn code(&self) -> ErrorCode {
        match self {
            RpcError::InvalidRequest(_) => ErrorCode::InvalidRequest,
            RpcError::UnsupportedProtocol { .. } => ErrorCode::UnsupportedProtocol,
            RpcError::FeatureMissing(_) => ErrorCode::FeatureMissing,
            RpcError::Busy { .. } => ErrorCode::Busy,
            RpcError::LeaseExpired(_) => ErrorCode::LeaseExpired,
            RpcError::SourceMissing(_) => ErrorCode::SourceMissing,
            RpcError::ArtifactsGone(_) => ErrorCode::ArtifactsGone,
            RpcError::PayloadTooLarge { .. } => ErrorCode::PayloadTooLarge,
        }
    }

    /// Convert to an RPC error payload
    pub fn to_payload(&self) -> RpcErrorPayload {
        let mut payload = RpcErrorPayload::new(self.code().as_str(), self.to_string());

        // Add machine-readable data for specific error types
        match self {
            RpcError::UnsupportedProtocol { version, min, max } => {
                payload = payload
                    .with_data("protocol_version", serde_json::json!(version))
                    .with_data("supported_min", serde_json::json!(min))
                    .with_data("supported_max", serde_json::json!(max));
            }
            RpcError::Busy { retry_after_seconds } => {
                payload = payload.with_data("retry_after_seconds", serde_json::json!(retry_after_seconds));
            }
            RpcError::PayloadTooLarge { size, limit } => {
                payload = payload
                    .with_data("size", serde_json::json!(size))
                    .with_data("limit", serde_json::json!(limit));
            }
            _ => {}
        }

        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_str() {
        assert_eq!(ErrorCode::InvalidRequest.as_str(), "INVALID_REQUEST");
        assert_eq!(ErrorCode::Busy.as_str(), "BUSY");
    }

    #[test]
    fn test_rpc_error_to_payload() {
        let err = RpcError::Busy { retry_after_seconds: 30 };
        let payload = err.to_payload();

        assert_eq!(payload.code, "BUSY");
        assert!(payload.message.contains("30 seconds"));
        assert!(payload.data.is_some());
        assert_eq!(
            payload.data.as_ref().unwrap().get("retry_after_seconds"),
            Some(&serde_json::json!(30))
        );
    }

    #[test]
    fn test_unsupported_protocol_payload() {
        let err = RpcError::UnsupportedProtocol {
            version: 3,
            min: 1,
            max: 2,
        };
        let payload = err.to_payload();

        assert_eq!(payload.code, "UNSUPPORTED_PROTOCOL");
        assert!(payload.message.contains("protocol version 3"));
        assert!(payload.message.contains("[1, 2]"));
    }
}
