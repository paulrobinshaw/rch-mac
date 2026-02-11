//! Run summary (run_summary.json) per PLAN.md normative spec

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use super::failure::{ExitCode, ExitCodeAggregator, Status};
use super::job_summary::JobSummary;

/// Schema version for run_summary.json
pub const RUN_SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for run_summary.json
pub const RUN_SUMMARY_SCHEMA_ID: &str = "rch-xcode/run_summary@1";

/// Run summary (run_summary.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// Run identifier
    pub run_id: String,

    /// When the summary was created
    pub created_at: DateTime<Utc>,

    /// Aggregated status
    pub status: Status,

    /// Aggregated exit code
    pub exit_code: i32,

    /// Total steps in the run
    pub step_count: usize,

    /// Count of steps with status=success
    pub steps_succeeded: usize,

    /// Count of steps with status=failed
    pub steps_failed: usize,

    /// Count of steps with status=cancelled
    pub steps_cancelled: usize,

    /// Count of steps skipped due to early-abort
    pub steps_skipped: usize,

    /// Count of steps with status=rejected
    pub steps_rejected: usize,

    /// Wall-clock duration of the entire run in milliseconds
    pub duration_ms: u64,

    /// Human-readable summary
    pub human_summary: String,
}

impl RunSummary {
    /// Create a new run summary by aggregating job summaries
    pub fn from_job_summaries(run_id: String, summaries: &[JobSummary], duration_ms: u64) -> Self {
        let mut aggregator = ExitCodeAggregator::new();
        let mut steps_succeeded = 0;
        let mut steps_failed = 0;
        let mut steps_cancelled = 0;
        let mut steps_rejected = 0;

        for summary in summaries {
            let exit_code = ExitCode::from_i32(summary.exit_code).unwrap_or(ExitCode::Executor);
            aggregator.add(summary.status, exit_code);

            match summary.status {
                Status::Success => steps_succeeded += 1,
                Status::Failed => steps_failed += 1,
                Status::Cancelled => steps_cancelled += 1,
                Status::Rejected => steps_rejected += 1,
            }
        }

        let status = aggregator.status();
        let exit_code = aggregator.exit_code();
        let step_count = summaries.len();

        let human_summary = Self::generate_human_summary(
            status,
            step_count,
            steps_succeeded,
            steps_failed,
            steps_cancelled,
            steps_rejected,
        );

        Self {
            schema_version: RUN_SUMMARY_SCHEMA_VERSION,
            schema_id: RUN_SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            created_at: Utc::now(),
            status,
            exit_code: exit_code.as_i32(),
            step_count,
            steps_succeeded,
            steps_failed,
            steps_cancelled,
            steps_skipped: 0, // Updated separately if needed
            steps_rejected,
            duration_ms,
            human_summary,
        }
    }

    /// Create an empty run summary for runs with no steps
    pub fn empty(run_id: String) -> Self {
        Self {
            schema_version: RUN_SUMMARY_SCHEMA_VERSION,
            schema_id: RUN_SUMMARY_SCHEMA_ID.to_string(),
            run_id,
            created_at: Utc::now(),
            status: Status::Success,
            exit_code: ExitCode::Success.as_i32(),
            step_count: 0,
            steps_succeeded: 0,
            steps_failed: 0,
            steps_cancelled: 0,
            steps_skipped: 0,
            steps_rejected: 0,
            duration_ms: 0,
            human_summary: "No steps executed".to_string(),
        }
    }

    /// Set the number of skipped steps
    pub fn with_skipped_steps(mut self, count: usize) -> Self {
        self.steps_skipped = count;
        self.step_count += count;
        self.human_summary = Self::generate_human_summary(
            self.status,
            self.step_count,
            self.steps_succeeded,
            self.steps_failed,
            self.steps_cancelled,
            self.steps_rejected,
        );
        self
    }

    /// Generate a human-readable summary
    fn generate_human_summary(
        status: Status,
        step_count: usize,
        steps_succeeded: usize,
        steps_failed: usize,
        steps_cancelled: usize,
        steps_rejected: usize,
    ) -> String {
        match status {
            Status::Success => {
                if step_count == 1 {
                    "Run succeeded".to_string()
                } else {
                    format!("Run succeeded: {}/{} steps passed", steps_succeeded, step_count)
                }
            }
            Status::Failed => {
                if step_count == 1 {
                    "Run failed".to_string()
                } else {
                    format!(
                        "Run failed: {} succeeded, {} failed, {} cancelled",
                        steps_succeeded, steps_failed, steps_cancelled
                    )
                }
            }
            Status::Cancelled => {
                format!("Run cancelled: {} step(s) cancelled", steps_cancelled)
            }
            Status::Rejected => {
                format!("Run rejected: {} step(s) rejected by classifier", steps_rejected)
            }
        }
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
    use super::super::failure::FailureKind;
    use super::super::job_summary::Backend;
    use super::*;

    fn make_success_summary(job_id: &str) -> JobSummary {
        JobSummary::success(
            "run-123".to_string(),
            job_id.to_string(),
            format!("key-{}", job_id),
            Backend::Xcodebuild,
            1000,
        )
    }

    fn make_failed_summary(job_id: &str) -> JobSummary {
        JobSummary::failure(
            "run-123".to_string(),
            job_id.to_string(),
            format!("key-{}", job_id),
            Backend::Xcodebuild,
            FailureKind::Xcodebuild,
            None,
            "Build failed".to_string(),
            1000,
        )
    }

    fn make_rejected_summary(job_id: &str) -> JobSummary {
        JobSummary::rejected(
            "run-123".to_string(),
            job_id.to_string(),
            format!("key-{}", job_id),
            "Rejected".to_string(),
        )
    }

    fn make_cancelled_summary(job_id: &str) -> JobSummary {
        JobSummary::cancelled(
            "run-123".to_string(),
            job_id.to_string(),
            format!("key-{}", job_id),
            Backend::Xcodebuild,
            1000,
        )
    }

    #[test]
    fn test_all_success() {
        let summaries = vec![make_success_summary("job1"), make_success_summary("job2")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 2000);

        assert_eq!(run.status, Status::Success);
        assert_eq!(run.exit_code, 0);
        assert_eq!(run.step_count, 2);
        assert_eq!(run.steps_succeeded, 2);
        assert_eq!(run.steps_failed, 0);
    }

    #[test]
    fn test_one_failure() {
        let summaries = vec![make_success_summary("job1"), make_failed_summary("job2")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 2000);

        assert_eq!(run.status, Status::Failed);
        assert_eq!(run.exit_code, 50); // XcodebuildFailed
        assert_eq!(run.steps_succeeded, 1);
        assert_eq!(run.steps_failed, 1);
    }

    #[test]
    fn test_rejected_takes_priority() {
        let summaries = vec![
            make_success_summary("job1"),
            make_failed_summary("job2"),
            make_rejected_summary("job3"),
            make_cancelled_summary("job4"),
        ];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 4000);

        assert_eq!(run.status, Status::Rejected);
        assert_eq!(run.exit_code, 10); // ClassifierRejected
        assert_eq!(run.steps_rejected, 1);
    }

    #[test]
    fn test_cancelled_over_failed() {
        let summaries = vec![
            make_success_summary("job1"),
            make_failed_summary("job2"),
            make_cancelled_summary("job3"),
        ];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 3000);

        assert_eq!(run.status, Status::Cancelled);
        assert_eq!(run.exit_code, 80);
    }

    #[test]
    fn test_first_failure_code_used() {
        // Need to create failures with different exit codes
        let mut summaries = vec![make_success_summary("job1")];

        let ssh_failure = JobSummary::failure(
            "run-123".to_string(),
            "job2".to_string(),
            "key-job2".to_string(),
            Backend::Xcodebuild,
            FailureKind::Ssh,
            None,
            "SSH failed".to_string(),
            1000,
        );
        summaries.push(ssh_failure);

        summaries.push(make_failed_summary("job3")); // XcodebuildFailed = 50

        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 3000);

        assert_eq!(run.status, Status::Failed);
        assert_eq!(run.exit_code, 20); // SSH = 20 (first failure)
    }

    #[test]
    fn test_empty_run() {
        let run = RunSummary::empty("run-123".to_string());

        assert_eq!(run.status, Status::Success);
        assert_eq!(run.exit_code, 0);
        assert_eq!(run.step_count, 0);
    }

    #[test]
    fn test_with_skipped_steps() {
        let summaries = vec![make_success_summary("job1")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 1000)
            .with_skipped_steps(2);

        assert_eq!(run.step_count, 3); // 1 executed + 2 skipped
        assert_eq!(run.steps_skipped, 2);
    }

    #[test]
    fn test_serialization() {
        let summaries = vec![make_success_summary("job1")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 1000);

        let json = run.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/run_summary@1""#));
        assert!(json.contains(r#""status": "success""#));
    }

    #[test]
    fn test_deserialization() {
        let summaries = vec![make_success_summary("job1"), make_failed_summary("job2")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 2000);

        let json = run.to_json().unwrap();
        let parsed = RunSummary::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, run.run_id);
        assert_eq!(parsed.status, run.status);
        assert_eq!(parsed.exit_code, run.exit_code);
        assert_eq!(parsed.step_count, run.step_count);
    }

    #[test]
    fn test_write_and_read_file() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let summaries = vec![make_success_summary("job1")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 1000);

        let path = dir.path().join("run_summary.json");
        run.write_to_file(&path).unwrap();

        let loaded = RunSummary::from_file(&path).unwrap();
        assert_eq!(loaded.run_id, run.run_id);
    }

    #[test]
    fn test_human_summary_single_step() {
        let summaries = vec![make_success_summary("job1")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 1000);

        assert_eq!(run.human_summary, "Run succeeded");
    }

    #[test]
    fn test_human_summary_multiple_steps() {
        let summaries = vec![make_success_summary("job1"), make_success_summary("job2")];
        let run = RunSummary::from_job_summaries("run-123".to_string(), &summaries, 2000);

        assert_eq!(run.human_summary, "Run succeeded: 2/2 steps passed");
    }
}
