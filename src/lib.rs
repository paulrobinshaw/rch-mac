//! RCH Xcode Lane - Remote Xcode build/test gate
//!
//! This crate implements the RCH Xcode Lane, a remote build/test gate for
//! Apple-platform projects that routes safe, allowlisted Xcode commands
//! to macOS workers.

pub mod classifier;
pub mod protocol;
pub mod worker;

pub use classifier::{Classifier, ClassifierConfig, ClassifierResult};
pub use protocol::{RpcRequest, RpcResponse, Operation, ErrorCode, RpcError};
pub use worker::RpcHandler;
