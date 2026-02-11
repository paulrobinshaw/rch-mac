//! RCH Xcode Lane Worker
//!
//! The worker is a binary that runs on macOS workers (Mac mini).
//! It implements the stdin/stdout JSON RPC protocol for executing
//! Xcode build/test jobs.
//!
//! This crate can be used in two modes:
//! - **Standalone binary**: invoked via SSH for production use
//! - **In-process library**: for unit and integration testing with mock state

pub mod config;
pub mod handlers;
pub mod mock_state;
pub mod rpc;
pub mod source_store;

pub use config::WorkerConfig;
pub use mock_state::MockState;
pub use rpc::RpcHandler;
pub use source_store::{SourceStore, SourceMetadata, StoreError};
