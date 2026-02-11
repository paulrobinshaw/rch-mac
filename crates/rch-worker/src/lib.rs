//! RCH Xcode Lane Worker
//!
//! The worker is a binary that runs on macOS workers (Mac mini).
//! It implements the stdin/stdout JSON RPC protocol for executing
//! Xcode build/test jobs.

pub mod config;
pub mod handlers;
pub mod rpc;

pub use config::WorkerConfig;
pub use rpc::RpcHandler;
