//! Failure taxonomy and stable exit codes per PLAN.md normative spec

use serde::{Deserialize, Serialize};

/// Job/run status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Job completed successfully
    Success,
    /// Job failed during execution
    Failed,
    /// Job was rejected by classifier (no execution)
    Rejected,
    /// Job was cancelled
    Cancelled,
}

impl Status {
    /// Get the default exit code for this status
    pub fn default_exit_code(&self) -> ExitCode {
        match self {
            Status::Success => ExitCode::Success,
            Status::Failed => ExitCode::Executor, // Default for failed, specific failure_kind may override
            Status::Rejected => ExitCode::ClassifierRejected,
            Status::Cancelled => ExitCode::Cancelled,
        }
    }

    /// Check if this is a terminal failure state
    pub fn is_failure(&self) -> bool {
        matches!(self, Status::Failed | Status::Rejected | Status::Cancelled)
    }
}

/// Failure kind - categorizes the cause of failure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureKind {
    /// Classifier rejected the invocation
    ClassifierRejected,
    /// SSH connection failure
    Ssh,
    /// File transfer failure
    Transfer,
    /// Executor failure (sandbox, process management)
    Executor,
    /// xcodebuild backend failure
    Xcodebuild,
    /// MCP backend failure
    Mcp,
    /// Artifact handling failure
    Artifacts,
    /// Job was cancelled
    Cancelled,
    /// Worker incompatible (feature/toolchain mismatch)
    WorkerIncompatible,
    /// Bundler failure (source bundling issues)
    Bundler,
    /// Attestation failure
    Attestation,
    /// Worker at capacity
    WorkerBusy,
}

impl FailureKind {
    /// Get the stable exit code for this failure kind
    pub fn exit_code(&self) -> ExitCode {
        match self {
            FailureKind::ClassifierRejected => ExitCode::ClassifierRejected,
            FailureKind::Ssh => ExitCode::Ssh,
            FailureKind::Transfer => ExitCode::Transfer,
            FailureKind::Executor => ExitCode::Executor,
            FailureKind::Xcodebuild => ExitCode::XcodebuildFailed,
            FailureKind::Mcp => ExitCode::McpFailed,
            FailureKind::Artifacts => ExitCode::ArtifactsFailed,
            FailureKind::Cancelled => ExitCode::Cancelled,
            FailureKind::WorkerBusy => ExitCode::WorkerBusy,
            FailureKind::WorkerIncompatible => ExitCode::WorkerIncompatible,
            FailureKind::Bundler => ExitCode::Bundler,
            FailureKind::Attestation => ExitCode::Attestation,
        }
    }

    /// Human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            FailureKind::ClassifierRejected => "Command rejected by classifier",
            FailureKind::Ssh => "SSH connection failed",
            FailureKind::Transfer => "File transfer failed",
            FailureKind::Executor => "Executor error",
            FailureKind::Xcodebuild => "xcodebuild failed",
            FailureKind::Mcp => "MCP backend failed",
            FailureKind::Artifacts => "Artifact processing failed",
            FailureKind::Cancelled => "Job cancelled",
            FailureKind::WorkerBusy => "Worker at capacity",
            FailureKind::WorkerIncompatible => "Worker incompatible",
            FailureKind::Bundler => "Source bundling failed",
            FailureKind::Attestation => "Attestation verification failed",
        }
    }
}

/// Failure subkind - optional additional detail
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureSubkind {
    /// Overall timeout exceeded
    TimeoutOverall,
    /// Idle timeout exceeded (no output)
    TimeoutIdle,
    /// Protocol error in communication
    ProtocolError,
    /// Size limit exceeded
    SizeExceeded,
    /// Integrity check failed
    IntegrityMismatch,
    /// Required feature missing
    FeatureMissing,
    /// Toolchain changed during execution
    ToolchainChanged,
    /// Protocol version mismatch
    ProtocolDrift,
}

impl FailureSubkind {
    /// Human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            FailureSubkind::TimeoutOverall => "Overall timeout exceeded",
            FailureSubkind::TimeoutIdle => "Idle timeout exceeded",
            FailureSubkind::ProtocolError => "Protocol error",
            FailureSubkind::SizeExceeded => "Size limit exceeded",
            FailureSubkind::IntegrityMismatch => "Integrity check failed",
            FailureSubkind::FeatureMissing => "Required feature missing",
            FailureSubkind::ToolchainChanged => "Toolchain changed",
            FailureSubkind::ProtocolDrift => "Protocol version mismatch",
        }
    }
}

/// Stable exit codes per PLAN.md normative spec
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ExitCode {
    /// Successful execution
    Success = 0,
    /// Classifier rejected the command
    ClassifierRejected = 10,
    /// SSH/connection failure
    Ssh = 20,
    /// File transfer failure
    Transfer = 30,
    /// Executor failure
    Executor = 40,
    /// xcodebuild failed
    XcodebuildFailed = 50,
    /// MCP backend failed
    McpFailed = 60,
    /// Artifact handling failed
    ArtifactsFailed = 70,
    /// Job was cancelled
    Cancelled = 80,
    /// Worker at capacity
    WorkerBusy = 90,
    /// Worker incompatible
    WorkerIncompatible = 91,
    /// Bundler failure
    Bundler = 92,
    /// Attestation failure
    Attestation = 93,
}

impl ExitCode {
    /// Get the integer value of the exit code
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    /// Create from integer value
    pub fn from_i32(code: i32) -> Option<Self> {
        match code {
            0 => Some(ExitCode::Success),
            10 => Some(ExitCode::ClassifierRejected),
            20 => Some(ExitCode::Ssh),
            30 => Some(ExitCode::Transfer),
            40 => Some(ExitCode::Executor),
            50 => Some(ExitCode::XcodebuildFailed),
            60 => Some(ExitCode::McpFailed),
            70 => Some(ExitCode::ArtifactsFailed),
            80 => Some(ExitCode::Cancelled),
            90 => Some(ExitCode::WorkerBusy),
            91 => Some(ExitCode::WorkerIncompatible),
            92 => Some(ExitCode::Bundler),
            93 => Some(ExitCode::Attestation),
            _ => None,
        }
    }

    /// Check if this exit code indicates success
    pub fn is_success(&self) -> bool {
        matches!(self, ExitCode::Success)
    }
}

impl Default for ExitCode {
    fn default() -> Self {
        ExitCode::Success
    }
}

/// Helper for aggregating exit codes across multiple steps
pub struct ExitCodeAggregator {
    has_rejected: bool,
    has_cancelled: bool,
    first_failure_code: Option<ExitCode>,
}

impl ExitCodeAggregator {
    /// Create a new aggregator
    pub fn new() -> Self {
        Self {
            has_rejected: false,
            has_cancelled: false,
            first_failure_code: None,
        }
    }

    /// Add a step's status and exit code to the aggregation
    pub fn add(&mut self, status: Status, exit_code: ExitCode) {
        match status {
            Status::Rejected => {
                self.has_rejected = true;
            }
            Status::Cancelled => {
                self.has_cancelled = true;
            }
            Status::Failed => {
                if self.first_failure_code.is_none() {
                    self.first_failure_code = Some(exit_code);
                }
            }
            Status::Success => {}
        }
    }

    /// Get the aggregated status
    pub fn status(&self) -> Status {
        if self.has_rejected {
            Status::Rejected
        } else if self.has_cancelled {
            Status::Cancelled
        } else if self.first_failure_code.is_some() {
            Status::Failed
        } else {
            Status::Success
        }
    }

    /// Get the aggregated exit code per PLAN.md rules
    pub fn exit_code(&self) -> ExitCode {
        if self.has_rejected {
            ExitCode::ClassifierRejected
        } else if self.has_cancelled {
            ExitCode::Cancelled
        } else if let Some(code) = self.first_failure_code {
            code
        } else {
            ExitCode::Success
        }
    }
}

impl Default for ExitCodeAggregator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_serialization() {
        assert_eq!(serde_json::to_string(&Status::Success).unwrap(), r#""success""#);
        assert_eq!(serde_json::to_string(&Status::Failed).unwrap(), r#""failed""#);
        assert_eq!(serde_json::to_string(&Status::Rejected).unwrap(), r#""rejected""#);
        assert_eq!(serde_json::to_string(&Status::Cancelled).unwrap(), r#""cancelled""#);
    }

    #[test]
    fn test_failure_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&FailureKind::ClassifierRejected).unwrap(),
            r#""CLASSIFIER_REJECTED""#
        );
        assert_eq!(
            serde_json::to_string(&FailureKind::Ssh).unwrap(),
            r#""SSH""#
        );
        assert_eq!(
            serde_json::to_string(&FailureKind::WorkerIncompatible).unwrap(),
            r#""WORKER_INCOMPATIBLE""#
        );
    }

    #[test]
    fn test_failure_subkind_serialization() {
        assert_eq!(
            serde_json::to_string(&FailureSubkind::TimeoutOverall).unwrap(),
            r#""TIMEOUT_OVERALL""#
        );
        assert_eq!(
            serde_json::to_string(&FailureSubkind::IntegrityMismatch).unwrap(),
            r#""INTEGRITY_MISMATCH""#
        );
    }

    #[test]
    fn test_exit_code_values() {
        assert_eq!(ExitCode::Success.as_i32(), 0);
        assert_eq!(ExitCode::ClassifierRejected.as_i32(), 10);
        assert_eq!(ExitCode::Ssh.as_i32(), 20);
        assert_eq!(ExitCode::Transfer.as_i32(), 30);
        assert_eq!(ExitCode::Executor.as_i32(), 40);
        assert_eq!(ExitCode::XcodebuildFailed.as_i32(), 50);
        assert_eq!(ExitCode::McpFailed.as_i32(), 60);
        assert_eq!(ExitCode::ArtifactsFailed.as_i32(), 70);
        assert_eq!(ExitCode::Cancelled.as_i32(), 80);
        assert_eq!(ExitCode::WorkerBusy.as_i32(), 90);
        assert_eq!(ExitCode::WorkerIncompatible.as_i32(), 91);
        assert_eq!(ExitCode::Bundler.as_i32(), 92);
        assert_eq!(ExitCode::Attestation.as_i32(), 93);
    }

    #[test]
    fn test_exit_code_from_i32() {
        assert_eq!(ExitCode::from_i32(0), Some(ExitCode::Success));
        assert_eq!(ExitCode::from_i32(10), Some(ExitCode::ClassifierRejected));
        assert_eq!(ExitCode::from_i32(50), Some(ExitCode::XcodebuildFailed));
        assert_eq!(ExitCode::from_i32(999), None);
    }

    #[test]
    fn test_failure_kind_exit_code_mapping() {
        assert_eq!(FailureKind::ClassifierRejected.exit_code(), ExitCode::ClassifierRejected);
        assert_eq!(FailureKind::Ssh.exit_code(), ExitCode::Ssh);
        assert_eq!(FailureKind::Transfer.exit_code(), ExitCode::Transfer);
        assert_eq!(FailureKind::Executor.exit_code(), ExitCode::Executor);
        assert_eq!(FailureKind::Xcodebuild.exit_code(), ExitCode::XcodebuildFailed);
        assert_eq!(FailureKind::Mcp.exit_code(), ExitCode::McpFailed);
        assert_eq!(FailureKind::Artifacts.exit_code(), ExitCode::ArtifactsFailed);
        assert_eq!(FailureKind::Cancelled.exit_code(), ExitCode::Cancelled);
        assert_eq!(FailureKind::WorkerBusy.exit_code(), ExitCode::WorkerBusy);
        assert_eq!(FailureKind::WorkerIncompatible.exit_code(), ExitCode::WorkerIncompatible);
        assert_eq!(FailureKind::Bundler.exit_code(), ExitCode::Bundler);
        assert_eq!(FailureKind::Attestation.exit_code(), ExitCode::Attestation);
    }

    #[test]
    fn test_aggregator_all_success() {
        let mut agg = ExitCodeAggregator::new();
        agg.add(Status::Success, ExitCode::Success);
        agg.add(Status::Success, ExitCode::Success);

        assert_eq!(agg.status(), Status::Success);
        assert_eq!(agg.exit_code(), ExitCode::Success);
    }

    #[test]
    fn test_aggregator_rejected_takes_priority() {
        let mut agg = ExitCodeAggregator::new();
        agg.add(Status::Success, ExitCode::Success);
        agg.add(Status::Failed, ExitCode::XcodebuildFailed);
        agg.add(Status::Rejected, ExitCode::ClassifierRejected);
        agg.add(Status::Cancelled, ExitCode::Cancelled);

        assert_eq!(agg.status(), Status::Rejected);
        assert_eq!(agg.exit_code(), ExitCode::ClassifierRejected);
    }

    #[test]
    fn test_aggregator_cancelled_over_failed() {
        let mut agg = ExitCodeAggregator::new();
        agg.add(Status::Success, ExitCode::Success);
        agg.add(Status::Failed, ExitCode::XcodebuildFailed);
        agg.add(Status::Cancelled, ExitCode::Cancelled);

        assert_eq!(agg.status(), Status::Cancelled);
        assert_eq!(agg.exit_code(), ExitCode::Cancelled);
    }

    #[test]
    fn test_aggregator_first_failure_code() {
        let mut agg = ExitCodeAggregator::new();
        agg.add(Status::Success, ExitCode::Success);
        agg.add(Status::Failed, ExitCode::Transfer);
        agg.add(Status::Failed, ExitCode::XcodebuildFailed);

        assert_eq!(agg.status(), Status::Failed);
        assert_eq!(agg.exit_code(), ExitCode::Transfer);
    }

    #[test]
    fn test_status_is_failure() {
        assert!(!Status::Success.is_failure());
        assert!(Status::Failed.is_failure());
        assert!(Status::Rejected.is_failure());
        assert!(Status::Cancelled.is_failure());
    }

    #[test]
    fn test_exit_code_is_success() {
        assert!(ExitCode::Success.is_success());
        assert!(!ExitCode::ClassifierRejected.is_success());
        assert!(!ExitCode::XcodebuildFailed.is_success());
    }
}
