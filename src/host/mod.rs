//! Host-Side Components
//!
//! Implements the host-side logic for communicating with workers,
//! managing runs and jobs, and collecting artifacts.

pub mod rpc;
pub mod transport;

pub use rpc::{RpcClient, RpcClientConfig, RpcResult, RpcError};
pub use transport::{Transport, MockTransport, SshTransport};
