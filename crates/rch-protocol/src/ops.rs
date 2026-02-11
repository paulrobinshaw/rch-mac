//! Operation-specific types.

pub mod probe;

pub use probe::{ProbeRequest, ProbeResponse, Capabilities, XcodeInfo, SimulatorRuntime, Capacity};

/// Known operation names.
pub mod names {
    pub const PROBE: &str = "probe";
    pub const RESERVE: &str = "reserve";
    pub const RELEASE: &str = "release";
    pub const SUBMIT: &str = "submit";
    pub const STATUS: &str = "status";
    pub const TAIL: &str = "tail";
    pub const CANCEL: &str = "cancel";
    pub const HAS_SOURCE: &str = "has_source";
    pub const UPLOAD_SOURCE: &str = "upload_source";
    pub const FETCH: &str = "fetch";
}
