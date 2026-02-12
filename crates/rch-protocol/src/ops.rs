//! Operation-specific types.

pub mod probe;
pub mod reserve;
pub mod submit;
pub mod status;
pub mod tail;
pub mod cancel;
pub mod source;
pub mod fetch;

pub use probe::{ProbeRequest, ProbeResponse, Capabilities, XcodeInfo, SimulatorRuntime, Capacity};
pub use reserve::{ReserveRequest, ReserveResponse, ReleaseRequest, ReleaseResponse};
pub use submit::{SubmitRequest, SubmitResponse, JobSpec, JobState};
pub use status::{StatusRequest, StatusResponse};
pub use tail::{TailRequest, TailResponse};
pub use cancel::{CancelRequest, CancelResponse};
pub use source::{HasSourceRequest, HasSourceResponse, UploadSourceRequest, UploadSourceResponse, UploadStream, ResumeInfo};
pub use fetch::{FetchRequest, FetchResponseHeader, FetchStream};

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
