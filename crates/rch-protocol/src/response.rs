//! RPC response types.

use serde::{Deserialize, Serialize};
use crate::error::RpcError;

/// RPC response envelope.
///
/// All worker operations emit a single JSON response on stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Protocol version (echoed from request, or 0 for probe).
    pub protocol_version: i32,
    /// Request ID echoed from the request.
    pub request_id: String,
    /// Whether the operation succeeded.
    pub ok: bool,
    /// Success payload (present when ok=true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Error details (present when ok=false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// Create a success response.
    pub fn success(protocol_version: i32, request_id: String, payload: serde_json::Value) -> Self {
        Self {
            protocol_version,
            request_id,
            ok: true,
            payload: Some(payload),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(protocol_version: i32, request_id: String, error: RpcError) -> Self {
        Self {
            protocol_version,
            request_id,
            ok: false,
            payload: None,
            error: Some(error),
        }
    }
}
