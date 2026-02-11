//! Worker Harness Implementation
//!
//! This module implements the worker-side components for the RCH Xcode Lane,
//! including the RPC entrypoint invoked via SSH forced-command.

pub mod rpc;
pub mod probe;

pub use rpc::RpcHandler;
