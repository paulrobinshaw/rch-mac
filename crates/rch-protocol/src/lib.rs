//! RCH Protocol Types
//!
//! Defines the JSON RPC envelope for host↔worker communication.

pub mod error;
pub mod request;
pub mod response;
pub mod ops;

pub use error::{ErrorCode, RpcError};
pub use request::RpcRequest;
pub use response::RpcResponse;

/// Protocol version used for probe requests (sentinel value).
pub const PROTOCOL_VERSION_PROBE: i32 = 0;

/// Minimum protocol version supported by this implementation.
pub const PROTOCOL_MIN: i32 = 1;

/// Maximum protocol version supported by this implementation.
pub const PROTOCOL_MAX: i32 = 1;

/// Current lane version string.
pub const LANE_VERSION: &str = "0.1.0";
