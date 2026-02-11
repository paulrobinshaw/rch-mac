//! RCH Xcode Lane Protocol Types
//!
//! This module defines the JSON RPC envelope types for host-worker communication
//! as specified in PLAN.md § Host↔Worker protocol.

pub mod envelope;
pub mod errors;

pub use envelope::{RpcRequest, RpcResponse, Operation};
pub use errors::{ErrorCode, RpcError};
