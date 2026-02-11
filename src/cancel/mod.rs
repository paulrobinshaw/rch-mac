//! Cancellation support for host-initiated and CLI-initiated cancellation
//!
//! Implements cancellation per PLAN.md normative spec.
//!
//! Cancel flows:
//! - CLI: `rch xcode cancel <run_id|job_id>` with reason=USER
//! - Signal handling (SIGINT/SIGTERM): reason=SIGNAL
//! - Timeout: reason=TIMEOUT_OVERALL or TIMEOUT_IDLE

use std::sync::Arc;

use crate::host::rpc::{CancelReason, CancelResponse, RpcClient, RpcError};
use crate::signal::{CancellationCoordinator, SignalState};
use crate::state::{JobState, JobStateData, JobStateError, RunState, RunStateData};

/// Cancellation result for a single job
#[derive(Debug)]
pub struct JobCancellation {
    /// Job ID
    pub job_id: String,
    /// Whether the cancel RPC succeeded
    pub rpc_success: bool,
    /// The resulting state from worker (if RPC succeeded)
    pub worker_state: Option<String>,
    /// Whether the job was already in a terminal state
    pub already_terminal: bool,
    /// Error message if RPC failed
    pub error: Option<String>,
}

impl JobCancellation {
    /// Create a successful cancellation result
    pub fn success(job_id: String, response: CancelResponse) -> Self {
        Self {
            job_id,
            rpc_success: true,
            worker_state: Some(response.state),
            already_terminal: response.already_terminal,
            error: None,
        }
    }

    /// Create a failed cancellation result
    pub fn failure(job_id: String, error: RpcError) -> Self {
        Self {
            job_id,
            rpc_success: false,
            worker_state: None,
            already_terminal: false,
            error: Some(error.to_string()),
        }
    }

    /// Create a result for a job that was already terminal locally
    pub fn already_terminal(job_id: String, state: JobState) -> Self {
        Self {
            job_id,
            rpc_success: true,
            worker_state: Some(format!("{:?}", state)),
            already_terminal: true,
            error: None,
        }
    }
}

/// Cancellation result for a run (multiple jobs)
#[derive(Debug)]
pub struct RunCancellation {
    /// Run ID
    pub run_id: String,
    /// Results for each job
    pub jobs: Vec<JobCancellation>,
    /// Number of jobs successfully cancelled
    pub cancelled_count: usize,
    /// Number of jobs that failed to cancel
    pub failed_count: usize,
    /// Number of jobs already in terminal state
    pub already_terminal_count: usize,
}

impl RunCancellation {
    /// Create a new run cancellation result
    pub fn new(run_id: String) -> Self {
        Self {
            run_id,
            jobs: Vec::new(),
            cancelled_count: 0,
            failed_count: 0,
            already_terminal_count: 0,
        }
    }

    /// Add a job cancellation result
    pub fn add(&mut self, result: JobCancellation) {
        if result.already_terminal {
            self.already_terminal_count += 1;
        } else if result.rpc_success {
            self.cancelled_count += 1;
        } else {
            self.failed_count += 1;
        }
        self.jobs.push(result);
    }

    /// Check if all cancellations succeeded (or were already terminal)
    pub fn all_succeeded(&self) -> bool {
        self.failed_count == 0
    }

    /// Get a human-readable summary
    pub fn summary(&self) -> String {
        let total = self.jobs.len();
        if total == 0 {
            return format!("Run {} has no jobs to cancel", self.run_id);
        }

        let mut parts = Vec::new();
        if self.cancelled_count > 0 {
            parts.push(format!("{} cancelled", self.cancelled_count));
        }
        if self.already_terminal_count > 0 {
            parts.push(format!("{} already complete", self.already_terminal_count));
        }
        if self.failed_count > 0 {
            parts.push(format!("{} failed", self.failed_count));
        }

        format!("Run {}: {} ({})", self.run_id, parts.join(", "), total)
    }
}

/// Cancellation manager that orchestrates the cancellation workflow
pub struct CancellationManager {
    /// Optional RPC client for sending cancel RPCs
    rpc_client: Option<Arc<RpcClient>>,
    /// Signal state for coordination with signal handler
    signal_state: Option<Arc<SignalState>>,
}

impl CancellationManager {
    /// Create a new cancellation manager with no dependencies (for testing)
    pub fn new() -> Self {
        Self {
            rpc_client: None,
            signal_state: None,
        }
    }

    /// Create a cancellation manager with RPC client
    pub fn with_rpc_client(rpc_client: Arc<RpcClient>) -> Self {
        Self {
            rpc_client: Some(rpc_client),
            signal_state: None,
        }
    }

    /// Create a cancellation manager with signal state
    pub fn with_signal_state(signal_state: Arc<SignalState>) -> Self {
        Self {
            rpc_client: None,
            signal_state: Some(signal_state),
        }
    }

    /// Create a cancellation manager with both RPC client and signal state
    pub fn with_both(rpc_client: Arc<RpcClient>, signal_state: Arc<SignalState>) -> Self {
        Self {
            rpc_client: Some(rpc_client),
            signal_state: Some(signal_state),
        }
    }

    /// Get a cancellation coordinator (for polling cancellation status)
    pub fn coordinator(&self) -> Option<CancellationCoordinator> {
        self.signal_state
            .as_ref()
            .map(|s| CancellationCoordinator::new(Arc::clone(s)))
    }

    /// Cancel a single job by ID
    ///
    /// If RPC client is available, sends cancel RPC to worker.
    /// If signal state is available, unregisters the job.
    pub fn cancel_job(&self, job_id: &str, reason: CancelReason) -> JobCancellation {
        // Unregister from signal state if available
        if let Some(ref state) = self.signal_state {
            state.unregister_job(job_id);
        }

        // Send cancel RPC if client is available
        if let Some(ref client) = self.rpc_client {
            match client.cancel(job_id, Some(reason)) {
                Ok(response) => JobCancellation::success(job_id.to_string(), response),
                Err(e) => JobCancellation::failure(job_id.to_string(), e),
            }
        } else {
            // No RPC client - just report success (local-only cancellation)
            JobCancellation {
                job_id: job_id.to_string(),
                rpc_success: true,
                worker_state: None,
                already_terminal: false,
                error: None,
            }
        }
    }

    /// Cancel all running jobs in a run
    ///
    /// Gets the list of running jobs from signal state (if available)
    /// or uses the provided job IDs.
    pub fn cancel_run(&self, run_id: &str, job_ids: &[String], reason: CancelReason) -> RunCancellation {
        let mut result = RunCancellation::new(run_id.to_string());

        for job_id in job_ids {
            let job_result = self.cancel_job(job_id, reason);
            result.add(job_result);
        }

        result
    }

    /// Cancel all jobs tracked by the signal state
    pub fn cancel_all_tracked(&self, reason: CancelReason) -> Vec<JobCancellation> {
        let job_ids = self
            .signal_state
            .as_ref()
            .map(|s| s.get_running_jobs())
            .unwrap_or_default();

        job_ids
            .iter()
            .map(|job_id| self.cancel_job(job_id, reason))
            .collect()
    }
}

impl Default for CancellationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Update job state to cancelled with proper transitions
pub fn update_job_state_cancelled(
    state: &mut JobStateData,
) -> Result<(), JobStateError> {
    // Handle based on current state
    match state.state {
        JobState::Queued | JobState::Running => {
            // Can go directly to Cancelled
            state.cancel()
        }
        JobState::CancelRequested => {
            // Already requested, complete the cancellation
            state.cancel()
        }
        JobState::Cancelled | JobState::Succeeded | JobState::Failed => {
            // Already terminal, nothing to do
            Ok(())
        }
    }
}

/// Update run state to cancelled
pub fn update_run_state_cancelled(
    state: &mut RunStateData,
) -> Result<(), crate::state::RunStateError> {
    // Handle based on current state
    match state.state {
        RunState::Queued | RunState::Running => {
            state.cancel()
        }
        RunState::Cancelled | RunState::Succeeded | RunState::Failed => {
            // Already terminal, nothing to do
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_cancellation_success() {
        let response = CancelResponse {
            job_id: "job-123".to_string(),
            state: "CANCELLED".to_string(),
            already_terminal: false,
        };
        let result = JobCancellation::success("job-123".to_string(), response);

        assert!(result.rpc_success);
        assert_eq!(result.worker_state, Some("CANCELLED".to_string()));
        assert!(!result.already_terminal);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_job_cancellation_already_terminal() {
        let result = JobCancellation::already_terminal("job-123".to_string(), JobState::Succeeded);

        assert!(result.rpc_success);
        assert!(result.already_terminal);
    }

    #[test]
    fn test_run_cancellation() {
        let mut run = RunCancellation::new("run-123".to_string());

        let job1 = JobCancellation {
            job_id: "job-1".to_string(),
            rpc_success: true,
            worker_state: Some("CANCELLED".to_string()),
            already_terminal: false,
            error: None,
        };
        run.add(job1);

        let job2 = JobCancellation::already_terminal("job-2".to_string(), JobState::Succeeded);
        run.add(job2);

        assert_eq!(run.cancelled_count, 1);
        assert_eq!(run.already_terminal_count, 1);
        assert_eq!(run.failed_count, 0);
        assert!(run.all_succeeded());
    }

    #[test]
    fn test_run_cancellation_with_failure() {
        let mut run = RunCancellation::new("run-123".to_string());

        let job1 = JobCancellation {
            job_id: "job-1".to_string(),
            rpc_success: false,
            worker_state: None,
            already_terminal: false,
            error: Some("Connection failed".to_string()),
        };
        run.add(job1);

        assert_eq!(run.failed_count, 1);
        assert!(!run.all_succeeded());
    }

    #[test]
    fn test_cancellation_manager_no_deps() {
        let manager = CancellationManager::new();
        let result = manager.cancel_job("job-123", CancelReason::User);

        // Without RPC client, should succeed locally
        assert!(result.rpc_success);
    }

    #[test]
    fn test_cancellation_manager_with_signal_state() {
        let signal_state = Arc::new(SignalState::new());
        signal_state.register_job("job-1".to_string());
        signal_state.register_job("job-2".to_string());

        let manager = CancellationManager::with_signal_state(Arc::clone(&signal_state));

        let results = manager.cancel_all_tracked(CancelReason::Signal);
        assert_eq!(results.len(), 2);

        // Jobs should be unregistered
        assert!(signal_state.get_running_jobs().is_empty());
    }

    #[test]
    fn test_run_cancellation_summary() {
        let mut run = RunCancellation::new("run-123".to_string());

        run.add(JobCancellation {
            job_id: "job-1".to_string(),
            rpc_success: true,
            worker_state: Some("CANCELLED".to_string()),
            already_terminal: false,
            error: None,
        });
        run.add(JobCancellation::already_terminal("job-2".to_string(), JobState::Succeeded));

        let summary = run.summary();
        assert!(summary.contains("1 cancelled"));
        assert!(summary.contains("1 already complete"));
    }

    #[test]
    fn test_update_job_state_cancelled() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        state.start().unwrap();

        update_job_state_cancelled(&mut state).unwrap();
        assert_eq!(state.state, JobState::Cancelled);
    }

    #[test]
    fn test_update_job_state_already_cancelled() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        state.start().unwrap();
        state.cancel().unwrap();

        // Should succeed (no-op)
        update_job_state_cancelled(&mut state).unwrap();
        assert_eq!(state.state, JobState::Cancelled);
    }

    #[test]
    fn test_update_job_state_from_cancel_requested() {
        let mut state = JobStateData::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        state.start().unwrap();
        state.request_cancel().unwrap();

        update_job_state_cancelled(&mut state).unwrap();
        assert_eq!(state.state, JobState::Cancelled);
    }
}
