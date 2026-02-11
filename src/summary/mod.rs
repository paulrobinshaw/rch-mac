//! Summary and failure taxonomy for RCH Xcode Lane
//!
//! Implements summary.json and run_summary.json per PLAN.md normative spec.

mod failure;
mod job_summary;
mod run_summary;

pub use failure::{ExitCode, FailureKind, FailureSubkind, Status};
pub use job_summary::{ArtifactProfile, Backend, JobSummary, SUMMARY_SCHEMA_ID, SUMMARY_SCHEMA_VERSION};
pub use run_summary::{RunSummary, RUN_SUMMARY_SCHEMA_ID, RUN_SUMMARY_SCHEMA_VERSION};
