//! Worker Harness Implementation
//!
//! This module implements the worker-side components for the RCH Xcode Lane,
//! including the RPC entrypoint invoked via SSH forced-command.

pub mod capabilities;
pub mod executor_env;
pub mod probe;
pub mod rpc;

pub use capabilities::{
    Capabilities, Capacity, Limits, MacOSInfo, Runtime, Simulator, ToolingVersions, XcodeVersion,
};
pub use executor_env::{EnvFilter, EnvOverride, ExecutorEnv};
pub use rpc::RpcHandler;
