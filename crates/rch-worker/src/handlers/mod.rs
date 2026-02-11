//! Operation handlers for the worker RPC.
//!
//! Each operation has its own handler module that processes requests
//! and returns responses.

pub mod probe;
pub mod reserve;
pub mod release;
pub mod submit;
pub mod status;
pub mod tail;
pub mod cancel;
pub mod has_source;
pub mod upload_source;
pub mod fetch;
