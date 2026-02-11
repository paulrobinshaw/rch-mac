//! Mock Worker Implementation
//!
//! Implements a configurable mock worker for testing the RCH Xcode Lane protocol.
//! Supports all RPC operations with failure injection for testing error paths.
//!
//! # Usage Modes
//!
//! - **In-process library**: Direct function calls for unit tests
//! - **Standalone binary**: Via SSH forced-command for integration tests
//!
//! # Operations
//!
//! - `probe`: Return configurable capabilities
//! - `reserve`: Request lease, track active leases
//! - `release`: Release lease by ID
//! - `submit`: Accept job, validate job_key, track state
//! - `status`: Return job state and pointers
//! - `tail`: Return log chunks with cursor pagination
//! - `cancel`: Request job cancellation
//! - `fetch`: Return artifacts as binary-framed response
//! - `has_source`: Check source existence
//! - `upload_source`: Accept source bundle upload

mod failure;
mod state;
mod worker;

pub use failure::{FailureConfig, FailureInjector};
pub use state::{Job, JobState, Lease, MockState};
pub use worker::MockWorker;
