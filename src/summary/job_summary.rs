//! Job summary (summary.json) per PLAN.md normative spec

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use super::failure::{ExitCode, FailureKind, FailureSubkind, Status};

/// Schema version for summary.json
pub const SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for summary.json
pub const SUMMARY_SCHEMA_ID: &str = "rch-xcode/summary@1";

/// Backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Xcodebuild,
    Mcp,
}

/// Artifact profile
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactProfile {
    Minimal,
    Rich,
}

/// Job summary (summary.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// Parent run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash)
    pub job_key: String,

    /// When the summary was created
    pub created_at: DateTime<Utc>,

    /// Job status
    pub status: Status,

    /// Failure kind (when status is not success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<FailureKind>,

    /// Failure subkind (optional additional detail)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_subkind: Option<FailureSubkind>,

    /// Stable exit code
    pub exit_code: i32,

    /// Backend process exit code (when available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_exit_code: Option<i32>,

    /// Backend termination signal (e.g., "SIGKILL")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_term_signal: Option<String>,

    /// Backend identity
    pub backend: Backend,

    /// Human-readable summary
    pub human_summary: String,

    /// Wall-clock job duration in milliseconds
    pub duration_ms: u64,

    /// Artifact profile produced
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_profile: Option<ArtifactProfile>,

    /// If result was served from cache
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_from_job_id: Option<String>,

    /// Integrity errors (when failure_subkind is INTEGRITY_MISMATCH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity_errors: Option<Vec<String>>,
}

impl JobSummary {
    /// Create a new success summary
    pub fn success(
        run_id: String,
        job_id: String,
        job_key: String,
        backend: Backend,
        duration_ms: u64,
    ) -> Self {
        Self {
            schema_version: SUMMARY_SCHEMA_VERSION,
            schema_id: SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            job_id,
            job_key,
            created_at: Utc::now(),
            status: Status::Success,
            failure_kind: None,
            failure_subkind: None,
            exit_code: ExitCode::Success.as_i32(),
            backend_exit_code: Some(0),
            backend_term_signal: None,
            backend,
            human_summary: "Build succeeded".to_string(),
            duration_ms,
            artifact_profile: None,
            cached_from_job_id: None,
            integrity_errors: None,
        }
    }

    /// Create a new failure summary
    pub fn failure(
        run_id: String,
        job_id: String,
        job_key: String,
        backend: Backend,
        failure_kind: FailureKind,
        failure_subkind: Option<FailureSubkind>,
        human_summary: String,
        duration_ms: u64,
    ) -> Self {
        let exit_code = failure_kind.exit_code();
        Self {
            schema_version: SUMMARY_SCHEMA_VERSION,
            schema_id: SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            job_id,
            job_key,
            created_at: Utc::now(),
            status: Status::Failed,
            failure_kind: Some(failure_kind),
            failure_subkind,
            exit_code: exit_code.as_i32(),
            backend_exit_code: None,
            backend_term_signal: None,
            backend,
            human_summary,
            duration_ms,
            artifact_profile: None,
            cached_from_job_id: None,
            integrity_errors: None,
        }
    }

    /// Create a rejection summary (classifier rejected)
    pub fn rejected(run_id: String, job_id: String, job_key: String, reason: String) -> Self {
        Self {
            schema_version: SUMMARY_SCHEMA_VERSION,
            schema_id: SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            job_id,
            job_key,
            created_at: Utc::now(),
            status: Status::Rejected,
            failure_kind: Some(FailureKind::ClassifierRejected),
            failure_subkind: None,
            exit_code: ExitCode::ClassifierRejected.as_i32(),
            backend_exit_code: None,
            backend_term_signal: None,
            backend: Backend::Xcodebuild, // Not actually used for rejected
            human_summary: reason,
            duration_ms: 0,
            artifact_profile: None,
            cached_from_job_id: None,
            integrity_errors: None,
        }
    }

    /// Create a cancelled summary
    pub fn cancelled(
        run_id: String,
        job_id: String,
        job_key: String,
        backend: Backend,
        duration_ms: u64,
    ) -> Self {
        Self {
            schema_version: SUMMARY_SCHEMA_VERSION,
            schema_id: SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            job_id,
            job_key,
            created_at: Utc::now(),
            status: Status::Cancelled,
            failure_kind: Some(FailureKind::Cancelled),
            failure_subkind: None,
            exit_code: ExitCode::Cancelled.as_i32(),
            backend_exit_code: None,
            backend_term_signal: None,
            backend,
            human_summary: "Job cancelled".to_string(),
            duration_ms,
            artifact_profile: None,
            cached_from_job_id: None,
            integrity_errors: None,
        }
    }

    /// Set the backend exit code
    pub fn with_backend_exit_code(mut self, code: i32) -> Self {
        self.backend_exit_code = Some(code);
        self
    }

    /// Set the backend termination signal
    pub fn with_backend_term_signal(mut self, signal: String) -> Self {
        self.backend_term_signal = Some(signal);
        self
    }

    /// Set the artifact profile
    pub fn with_artifact_profile(mut self, profile: ArtifactProfile) -> Self {
        self.artifact_profile = Some(profile);
        self
    }

    /// Set cached from job id
    pub fn with_cached_from(mut self, job_id: String) -> Self {
        self.cached_from_job_id = Some(job_id);
        self
    }

    /// Add integrity errors
    pub fn with_integrity_errors(mut self, errors: Vec<String>) -> Self {
        self.integrity_errors = Some(errors);
        self
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write to file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e))
        })?;
        fs::write(path, json)
    }

    /// Load from file
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        Self::from_json(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e)))
    }

    /// Get the exit code as ExitCode enum
    pub fn exit_code_enum(&self) -> Option<ExitCode> {
        ExitCode::from_i32(self.exit_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_summary() {
        let summary = JobSummary::success(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            5000,
        );

        assert_eq!(summary.status, Status::Success);
        assert_eq!(summary.exit_code, 0);
        assert!(summary.failure_kind.is_none());
        assert_eq!(summary.backend, Backend::Xcodebuild);
    }

    #[test]
    fn test_failure_summary() {
        let summary = JobSummary::failure(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            FailureKind::Xcodebuild,
            Some(FailureSubkind::TimeoutOverall),
            "Build timed out".to_string(),
            30000,
        );

        assert_eq!(summary.status, Status::Failed);
        assert_eq!(summary.exit_code, 50);
        assert_eq!(summary.failure_kind, Some(FailureKind::Xcodebuild));
        assert_eq!(summary.failure_subkind, Some(FailureSubkind::TimeoutOverall));
    }

    #[test]
    fn test_rejected_summary() {
        let summary = JobSummary::rejected(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "Unknown flag: --evil".to_string(),
        );

        assert_eq!(summary.status, Status::Rejected);
        assert_eq!(summary.exit_code, 10);
        assert_eq!(summary.failure_kind, Some(FailureKind::ClassifierRejected));
    }

    #[test]
    fn test_cancelled_summary() {
        let summary = JobSummary::cancelled(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Mcp,
            10000,
        );

        assert_eq!(summary.status, Status::Cancelled);
        assert_eq!(summary.exit_code, 80);
        assert_eq!(summary.failure_kind, Some(FailureKind::Cancelled));
    }

    #[test]
    fn test_serialization() {
        let summary = JobSummary::success(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            5000,
        );

        let json = summary.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/summary@1""#));
        assert!(json.contains(r#""status": "success""#));
        assert!(json.contains(r#""exit_code": 0"#));
    }

    #[test]
    fn test_deserialization() {
        let summary = JobSummary::failure(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            FailureKind::Ssh,
            None,
            "Connection refused".to_string(),
            100,
        );

        let json = summary.to_json().unwrap();
        let parsed = JobSummary::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, summary.run_id);
        assert_eq!(parsed.status, summary.status);
        assert_eq!(parsed.failure_kind, summary.failure_kind);
        assert_eq!(parsed.exit_code, summary.exit_code);
    }

    #[test]
    fn test_builder_pattern() {
        let summary = JobSummary::failure(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            FailureKind::Xcodebuild,
            None,
            "Build failed".to_string(),
            5000,
        )
        .with_backend_exit_code(65)
        .with_artifact_profile(ArtifactProfile::Minimal);

        assert_eq!(summary.backend_exit_code, Some(65));
        assert_eq!(summary.artifact_profile, Some(ArtifactProfile::Minimal));
    }

    #[test]
    fn test_write_and_read_file() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let summary = JobSummary::success(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            5000,
        );

        let path = dir.path().join("summary.json");
        summary.write_to_file(&path).unwrap();

        let loaded = JobSummary::from_file(&path).unwrap();
        assert_eq!(loaded.run_id, summary.run_id);
        assert_eq!(loaded.job_id, summary.job_id);
    }

    #[test]
    fn test_integrity_errors() {
        let summary = JobSummary::failure(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            FailureKind::Artifacts,
            Some(FailureSubkind::IntegrityMismatch),
            "Artifact verification failed".to_string(),
            1000,
        )
        .with_integrity_errors(vec![
            "file1.txt: hash mismatch".to_string(),
            "file2.txt: missing".to_string(),
        ]);

        let json = summary.to_json().unwrap();
        assert!(json.contains("integrity_errors"));
        assert!(json.contains("hash mismatch"));
    }

    #[test]
    fn test_cached_from() {
        let summary = JobSummary::success(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            Backend::Xcodebuild,
            100,
        )
        .with_cached_from("job-000".to_string());

        assert_eq!(summary.cached_from_job_id, Some("job-000".to_string()));
    }
}
