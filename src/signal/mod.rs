//! Signal handling for graceful shutdown (SIGINT/SIGTERM)
//!
//! Implements the host signal handling per PLAN.md normative spec.
//!
//! On receiving SIGINT or SIGTERM:
//! 1. Send cancel RPC for all RUNNING jobs in current run
//! 2. Wait up to grace period (10 seconds) for cancellation acknowledgement
//! 3. Persist run_state.json with state=CANCELLED and run_summary.json
//! 4. Exit with code 80 (CANCELLED)
//!
//! On double-SIGINT: exit immediately but still persist run_state.json

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default grace period for cancellation acknowledgement
pub const DEFAULT_GRACE_PERIOD_SECONDS: u64 = 10;

/// Exit code for cancelled runs
pub const EXIT_CODE_CANCELLED: i32 = 80;

/// Signal handler state
#[derive(Debug)]
pub struct SignalState {
    /// First signal received (cancellation initiated)
    cancel_requested: AtomicBool,
    /// Second signal received (immediate exit requested)
    immediate_exit: AtomicBool,
    /// Signal count (for tracking double-SIGINT)
    signal_count: AtomicU8,
    /// Running job IDs to cancel
    running_jobs: Mutex<Vec<String>>,
    /// Run ID for state persistence
    run_id: Mutex<Option<String>>,
    /// Artifact directory for state persistence
    artifact_dir: Mutex<Option<PathBuf>>,
    /// Grace period for cancellation
    grace_period: Duration,
}

impl SignalState {
    /// Create a new signal state with default grace period
    pub fn new() -> Self {
        Self::with_grace_period(Duration::from_secs(DEFAULT_GRACE_PERIOD_SECONDS))
    }

    /// Create a new signal state with custom grace period
    pub fn with_grace_period(grace_period: Duration) -> Self {
        Self {
            cancel_requested: AtomicBool::new(false),
            immediate_exit: AtomicBool::new(false),
            signal_count: AtomicU8::new(0),
            running_jobs: Mutex::new(Vec::new()),
            run_id: Mutex::new(None),
            artifact_dir: Mutex::new(None),
            grace_period,
        }
    }

    /// Check if cancellation has been requested
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    /// Check if immediate exit has been requested (double-SIGINT)
    pub fn is_immediate_exit(&self) -> bool {
        self.immediate_exit.load(Ordering::SeqCst)
    }

    /// Get the number of signals received
    pub fn signal_count(&self) -> u8 {
        self.signal_count.load(Ordering::SeqCst)
    }

    /// Handle a signal (SIGINT/SIGTERM)
    ///
    /// Returns the appropriate action to take
    pub fn handle_signal(&self) -> SignalAction {
        let count = self.signal_count.fetch_add(1, Ordering::SeqCst);

        if count == 0 {
            // First signal: initiate cancellation
            self.cancel_requested.store(true, Ordering::SeqCst);
            SignalAction::InitiateCancellation
        } else if count == 1 {
            // Second signal: immediate exit
            self.immediate_exit.store(true, Ordering::SeqCst);
            SignalAction::ImmediateExit
        } else {
            // Third+ signal: ignore
            SignalAction::Ignore
        }
    }

    /// Register a running job for potential cancellation
    pub fn register_job(&self, job_id: String) {
        if let Ok(mut jobs) = self.running_jobs.lock() {
            jobs.push(job_id);
        }
    }

    /// Unregister a job (completed or cancelled)
    pub fn unregister_job(&self, job_id: &str) {
        if let Ok(mut jobs) = self.running_jobs.lock() {
            jobs.retain(|id| id != job_id);
        }
    }

    /// Get the list of running job IDs
    pub fn get_running_jobs(&self) -> Vec<String> {
        self.running_jobs.lock().map(|j| j.clone()).unwrap_or_default()
    }

    /// Set the run ID for state persistence
    pub fn set_run_id(&self, run_id: String) {
        if let Ok(mut id) = self.run_id.lock() {
            *id = Some(run_id);
        }
    }

    /// Get the run ID
    pub fn get_run_id(&self) -> Option<String> {
        self.run_id.lock().ok().and_then(|id| id.clone())
    }

    /// Set the artifact directory for state persistence
    pub fn set_artifact_dir(&self, path: PathBuf) {
        if let Ok(mut dir) = self.artifact_dir.lock() {
            *dir = Some(path);
        }
    }

    /// Get the artifact directory
    pub fn get_artifact_dir(&self) -> Option<PathBuf> {
        self.artifact_dir.lock().ok().and_then(|dir| dir.clone())
    }

    /// Get the grace period duration
    pub fn grace_period(&self) -> Duration {
        self.grace_period
    }

    /// Reset the signal state (for testing)
    pub fn reset(&self) {
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.immediate_exit.store(false, Ordering::SeqCst);
        self.signal_count.store(0, Ordering::SeqCst);
        if let Ok(mut jobs) = self.running_jobs.lock() {
            jobs.clear();
        }
        if let Ok(mut id) = self.run_id.lock() {
            *id = None;
        }
        if let Ok(mut dir) = self.artifact_dir.lock() {
            *dir = None;
        }
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

/// Action to take after receiving a signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// First signal: initiate graceful cancellation
    InitiateCancellation,
    /// Second signal: exit immediately (but still persist state)
    ImmediateExit,
    /// Third+ signal: ignore
    Ignore,
}

/// Signal handler that manages the signal state
pub struct SignalHandler {
    state: Arc<SignalState>,
}

impl SignalHandler {
    /// Create a new signal handler
    pub fn new() -> Self {
        Self {
            state: Arc::new(SignalState::new()),
        }
    }

    /// Create a new signal handler with custom state
    pub fn with_state(state: Arc<SignalState>) -> Self {
        Self { state }
    }

    /// Get a reference to the signal state
    pub fn state(&self) -> Arc<SignalState> {
        Arc::clone(&self.state)
    }

    /// Install the signal handlers
    ///
    /// This sets up handlers for SIGINT and SIGTERM.
    /// Must be called once at program startup.
    pub fn install(&self) -> Result<(), ctrlc::Error> {
        let state = Arc::clone(&self.state);
        ctrlc::set_handler(move || {
            let action = state.handle_signal();
            match action {
                SignalAction::InitiateCancellation => {
                    eprintln!("\nReceived interrupt signal, initiating graceful shutdown...");
                }
                SignalAction::ImmediateExit => {
                    eprintln!("\nReceived second interrupt, exiting immediately...");
                }
                SignalAction::Ignore => {
                    // Ignore additional signals
                }
            }
        })
    }

    /// Wait for cancellation with grace period
    ///
    /// Returns true if we should exit immediately (double-SIGINT),
    /// false if grace period expired normally.
    pub fn wait_for_cancellation(&self) -> bool {
        let start = Instant::now();
        let grace_period = self.state.grace_period();

        while start.elapsed() < grace_period {
            if self.state.is_immediate_exit() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        false
    }
}

impl Default for SignalHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Cancellation coordinator that manages the cancellation workflow
pub struct CancellationCoordinator {
    state: Arc<SignalState>,
}

impl CancellationCoordinator {
    /// Create a new cancellation coordinator with the given state
    pub fn new(state: Arc<SignalState>) -> Self {
        Self { state }
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.state.is_cancel_requested()
    }

    /// Check if immediate exit has been requested
    pub fn should_exit_immediately(&self) -> bool {
        self.state.is_immediate_exit()
    }

    /// Register a job for cancellation tracking
    pub fn register_job(&self, job_id: &str) {
        self.state.register_job(job_id.to_string());
    }

    /// Unregister a completed job
    pub fn unregister_job(&self, job_id: &str) {
        self.state.unregister_job(job_id);
    }

    /// Get the job IDs that need to be cancelled
    pub fn jobs_to_cancel(&self) -> Vec<String> {
        self.state.get_running_jobs()
    }

    /// Set the run context for state persistence
    pub fn set_run_context(&self, run_id: &str, artifact_dir: PathBuf) {
        self.state.set_run_id(run_id.to_string());
        self.state.set_artifact_dir(artifact_dir);
    }

    /// Get the run ID
    pub fn run_id(&self) -> Option<String> {
        self.state.get_run_id()
    }

    /// Get the artifact directory
    pub fn artifact_dir(&self) -> Option<PathBuf> {
        self.state.get_artifact_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_state_initial() {
        let state = SignalState::new();
        assert!(!state.is_cancel_requested());
        assert!(!state.is_immediate_exit());
        assert_eq!(state.signal_count(), 0);
    }

    #[test]
    fn test_first_signal_initiates_cancellation() {
        let state = SignalState::new();
        let action = state.handle_signal();

        assert_eq!(action, SignalAction::InitiateCancellation);
        assert!(state.is_cancel_requested());
        assert!(!state.is_immediate_exit());
        assert_eq!(state.signal_count(), 1);
    }

    #[test]
    fn test_second_signal_requests_immediate_exit() {
        let state = SignalState::new();

        state.handle_signal(); // First
        let action = state.handle_signal(); // Second

        assert_eq!(action, SignalAction::ImmediateExit);
        assert!(state.is_cancel_requested());
        assert!(state.is_immediate_exit());
        assert_eq!(state.signal_count(), 2);
    }

    #[test]
    fn test_third_signal_ignored() {
        let state = SignalState::new();

        state.handle_signal(); // First
        state.handle_signal(); // Second
        let action = state.handle_signal(); // Third

        assert_eq!(action, SignalAction::Ignore);
        assert_eq!(state.signal_count(), 3);
    }

    #[test]
    fn test_job_registration() {
        let state = SignalState::new();

        state.register_job("job-1".to_string());
        state.register_job("job-2".to_string());

        let jobs = state.get_running_jobs();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.contains(&"job-1".to_string()));
        assert!(jobs.contains(&"job-2".to_string()));
    }

    #[test]
    fn test_job_unregistration() {
        let state = SignalState::new();

        state.register_job("job-1".to_string());
        state.register_job("job-2".to_string());
        state.unregister_job("job-1");

        let jobs = state.get_running_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(jobs.contains(&"job-2".to_string()));
    }

    #[test]
    fn test_run_context() {
        let state = SignalState::new();

        state.set_run_id("run-123".to_string());
        state.set_artifact_dir(PathBuf::from("/tmp/artifacts"));

        assert_eq!(state.get_run_id(), Some("run-123".to_string()));
        assert_eq!(state.get_artifact_dir(), Some(PathBuf::from("/tmp/artifacts")));
    }

    #[test]
    fn test_reset() {
        let state = SignalState::new();

        state.handle_signal();
        state.register_job("job-1".to_string());
        state.set_run_id("run-123".to_string());

        state.reset();

        assert!(!state.is_cancel_requested());
        assert_eq!(state.signal_count(), 0);
        assert!(state.get_running_jobs().is_empty());
        assert!(state.get_run_id().is_none());
    }

    #[test]
    fn test_grace_period() {
        let state = SignalState::with_grace_period(Duration::from_secs(5));
        assert_eq!(state.grace_period(), Duration::from_secs(5));
    }

    #[test]
    fn test_cancellation_coordinator() {
        let state = Arc::new(SignalState::new());
        let coordinator = CancellationCoordinator::new(Arc::clone(&state));

        assert!(!coordinator.is_cancelled());

        coordinator.register_job("job-1");
        coordinator.set_run_context("run-123", PathBuf::from("/tmp"));

        // Simulate signal
        state.handle_signal();

        assert!(coordinator.is_cancelled());
        assert_eq!(coordinator.jobs_to_cancel(), vec!["job-1".to_string()]);
        assert_eq!(coordinator.run_id(), Some("run-123".to_string()));
    }

    #[test]
    fn test_default_grace_period() {
        let state = SignalState::new();
        assert_eq!(
            state.grace_period(),
            Duration::from_secs(DEFAULT_GRACE_PERIOD_SECONDS)
        );
    }
}
