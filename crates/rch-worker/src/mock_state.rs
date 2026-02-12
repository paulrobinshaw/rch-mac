//! Mock worker state for testing.
//!
//! Provides in-memory state management for leases, jobs, and source bundles.
//! Used for both unit tests (in-process) and integration tests (via mock binary).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rch_protocol::ops::{JobSpec, JobState};

/// Thread-safe mock worker state.
#[derive(Debug, Clone)]
pub struct MockState {
    inner: Arc<RwLock<MockStateInner>>,
}

#[derive(Debug)]
struct MockStateInner {
    /// Active leases by lease_id.
    leases: HashMap<String, Lease>,
    /// Jobs by job_id.
    jobs: HashMap<String, MockJob>,
    /// Source bundles by source_sha256.
    sources: HashMap<String, SourceEntry>,
    /// Failure injection configuration.
    failure_injection: FailureInjection,
    /// Counter for generating unique IDs.
    id_counter: u64,
}

/// A worker lease.
#[derive(Debug, Clone)]
pub struct Lease {
    pub lease_id: String,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl Lease {
    /// Check if this lease has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

/// A mock job with its state and log buffer.
#[derive(Debug, Clone)]
pub struct MockJob {
    pub job_id: String,
    pub job_key: String,
    pub run_id: String,
    pub state: JobState,
    pub spec: JobSpec,
    pub log_buffer: String,
    pub log_cursor: usize,
    pub created_at: Instant,
    /// Scheduled state transitions (for simulating async execution).
    pub transitions: Vec<ScheduledTransition>,
    /// Associated lease ID (M6 feature: lease-based backstop).
    pub lease_id: Option<String>,
}

/// A scheduled state transition for mock jobs.
#[derive(Debug, Clone)]
pub struct ScheduledTransition {
    pub to_state: JobState,
    pub at: Instant,
}

/// A source bundle entry.
#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub source_sha256: String,
    pub content_sha256: String,
    pub size: u64,
    pub created_at: Instant,
}

/// Failure injection configuration.
#[derive(Debug, Clone, Default)]
pub struct FailureInjection {
    /// Return BUSY for reserve with this retry_after.
    pub reserve_busy: Option<u32>,
    /// Return LEASE_EXPIRED for submit.
    pub lease_expired: bool,
    /// Return SOURCE_MISSING for submit.
    pub source_missing: bool,
    /// Delay in milliseconds per operation.
    pub delays: HashMap<String, u64>,
    /// Force specific job outcomes.
    pub force_job_state: Option<JobState>,
}

impl Default for MockState {
    fn default() -> Self {
        Self::new()
    }
}

impl MockState {
    /// Create a new mock state.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MockStateInner {
                leases: HashMap::new(),
                jobs: HashMap::new(),
                sources: HashMap::new(),
                failure_injection: FailureInjection::default(),
                id_counter: 0,
            })),
        }
    }

    /// Generate a unique ID.
    fn generate_id(&self, prefix: &str) -> String {
        let mut inner = self.inner.write().unwrap();
        inner.id_counter += 1;
        format!("{}-{:08x}", prefix, inner.id_counter)
    }

    /// Configure failure injection.
    pub fn set_failure_injection(&self, injection: FailureInjection) {
        let mut inner = self.inner.write().unwrap();
        inner.failure_injection = injection;
    }

    /// Get current failure injection config.
    pub fn failure_injection(&self) -> FailureInjection {
        let inner = self.inner.read().unwrap();
        inner.failure_injection.clone()
    }

    // === Lease Management ===

    /// Create a new lease.
    pub fn create_lease(&self, ttl_seconds: u32) -> Lease {
        let lease_id = self.generate_id("lease");
        let lease = Lease {
            lease_id: lease_id.clone(),
            created_at: Instant::now(),
            ttl: Duration::from_secs(ttl_seconds as u64),
        };
        let mut inner = self.inner.write().unwrap();
        inner.leases.insert(lease_id, lease.clone());
        lease
    }

    /// Get a lease by ID.
    pub fn get_lease(&self, lease_id: &str) -> Option<Lease> {
        let inner = self.inner.read().unwrap();
        inner.leases.get(lease_id).cloned()
    }

    /// Release a lease (idempotent).
    pub fn release_lease(&self, lease_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap();
        inner.leases.remove(lease_id).is_some()
    }

    /// Check if a lease is valid (exists and not expired).
    pub fn is_lease_valid(&self, lease_id: &str) -> bool {
        let inner = self.inner.read().unwrap();
        inner
            .leases
            .get(lease_id)
            .map(|l| !l.is_expired())
            .unwrap_or(false)
    }

    /// Get count of active (non-expired) leases.
    pub fn active_lease_count(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner.leases.values().filter(|l| !l.is_expired()).count()
    }

    // === Job Management ===

    /// Create a new job from a JobSpec.
    pub fn create_job(&self, spec: JobSpec) -> MockJob {
        self.create_job_with_lease(spec, None)
    }

    /// Create a new job with an optional lease association (M6 feature).
    pub fn create_job_with_lease(&self, spec: JobSpec, lease_id: Option<String>) -> MockJob {
        let job = MockJob {
            job_id: spec.job_id.clone(),
            job_key: spec.job_key.clone(),
            run_id: spec.run_id.clone(),
            state: JobState::Queued,
            spec,
            log_buffer: String::new(),
            log_cursor: 0,
            created_at: Instant::now(),
            transitions: Vec::new(),
            lease_id,
        };
        let mut inner = self.inner.write().unwrap();
        inner.jobs.insert(job.job_id.clone(), job.clone());
        job
    }

    /// Get a job by ID.
    pub fn get_job(&self, job_id: &str) -> Option<MockJob> {
        let inner = self.inner.read().unwrap();
        inner.jobs.get(job_id).cloned()
    }

    /// Update job state.
    pub fn set_job_state(&self, job_id: &str, state: JobState) -> Option<JobState> {
        let mut inner = self.inner.write().unwrap();
        inner.jobs.get_mut(job_id).map(|job| {
            let old = job.state;
            job.state = state;
            old
        })
    }

    /// Append to job log.
    pub fn append_job_log(&self, job_id: &str, content: &str) {
        let mut inner = self.inner.write().unwrap();
        if let Some(job) = inner.jobs.get_mut(job_id) {
            job.log_buffer.push_str(content);
        }
    }

    /// Get job log from cursor.
    pub fn get_job_log(&self, job_id: &str, cursor: usize) -> Option<(String, usize)> {
        let inner = self.inner.read().unwrap();
        inner.jobs.get(job_id).map(|job| {
            let chunk = if cursor < job.log_buffer.len() {
                job.log_buffer[cursor..].to_string()
            } else {
                String::new()
            };
            let new_cursor = job.log_buffer.len();
            (chunk, new_cursor)
        })
    }

    /// Schedule job transitions for mock async execution.
    pub fn schedule_transitions(&self, job_id: &str, transitions: Vec<(JobState, Duration)>) {
        let mut inner = self.inner.write().unwrap();
        if let Some(job) = inner.jobs.get_mut(job_id) {
            let now = Instant::now();
            job.transitions = transitions
                .into_iter()
                .map(|(state, delay)| ScheduledTransition {
                    to_state: state,
                    at: now + delay,
                })
                .collect();
        }
    }

    /// Process scheduled transitions (call this periodically or before status checks).
    pub fn process_transitions(&self, job_id: &str) {
        let mut inner = self.inner.write().unwrap();
        if let Some(job) = inner.jobs.get_mut(job_id) {
            let now = Instant::now();
            while let Some(transition) = job.transitions.first() {
                if transition.at <= now {
                    job.state = transition.to_state;
                    job.transitions.remove(0);
                } else {
                    break;
                }
            }
        }
    }

    /// Get count of running jobs.
    pub fn running_job_count(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner
            .jobs
            .values()
            .filter(|j| matches!(j.state, JobState::Running | JobState::CancelRequested))
            .count()
    }

    // === Source Store ===

    /// Check if a source exists.
    pub fn has_source(&self, source_sha256: &str) -> bool {
        let inner = self.inner.read().unwrap();
        inner.sources.contains_key(source_sha256)
    }

    /// Add a source to the store.
    pub fn add_source(&self, source_sha256: String, content_sha256: String, size: u64) {
        let entry = SourceEntry {
            source_sha256: source_sha256.clone(),
            content_sha256,
            size,
            created_at: Instant::now(),
        };
        let mut inner = self.inner.write().unwrap();
        inner.sources.insert(source_sha256, entry);
    }

    /// Get a source entry.
    pub fn get_source(&self, source_sha256: &str) -> Option<SourceEntry> {
        let inner = self.inner.read().unwrap();
        inner.sources.get(source_sha256).cloned()
    }

    // === Cleanup ===

    /// Clean up expired leases.
    pub fn cleanup_expired_leases(&self) -> usize {
        let mut inner = self.inner.write().unwrap();
        let expired: Vec<_> = inner
            .leases
            .iter()
            .filter(|(_, l)| l.is_expired())
            .map(|(id, _)| id.clone())
            .collect();
        let count = expired.len();
        for id in expired {
            inner.leases.remove(&id);
        }
        count
    }

    /// Cancel RUNNING jobs whose lease has expired (M6 lease-based backstop).
    ///
    /// This is a safety net for host crashes: if the host dies without releasing
    /// the lease or cancelling jobs, the worker will eventually auto-cancel them.
    pub fn cancel_orphaned_jobs(&self) -> Vec<String> {
        let mut cancelled = Vec::new();
        let mut inner = self.inner.write().unwrap();

        // Find jobs that are running and have an expired or missing lease
        let jobs_to_cancel: Vec<String> = inner
            .jobs
            .iter()
            .filter(|(_, job)| {
                // Only cancel running or queued jobs
                matches!(job.state, JobState::Running | JobState::Queued | JobState::CancelRequested) &&
                // Only if they have a lease
                job.lease_id.is_some()
            })
            .filter(|(_, job)| {
                // Check if the associated lease is expired or missing
                if let Some(ref lease_id) = job.lease_id {
                    !inner
                        .leases
                        .get(lease_id)
                        .map(|l| !l.is_expired())
                        .unwrap_or(false)
                } else {
                    false
                }
            })
            .map(|(id, _)| id.clone())
            .collect();

        // Cancel the jobs
        for job_id in jobs_to_cancel {
            if let Some(job) = inner.jobs.get_mut(&job_id) {
                job.state = JobState::Cancelled;
                job.log_buffer.push_str(&format!(
                    "\n=== Job {} cancelled: lease expired (backstop) ===\n",
                    job_id
                ));
                cancelled.push(job_id);
            }
        }

        cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lease_lifecycle() {
        let state = MockState::new();

        // Create lease
        let lease = state.create_lease(3600);
        assert!(!lease.is_expired());
        assert!(state.is_lease_valid(&lease.lease_id));

        // Release lease
        assert!(state.release_lease(&lease.lease_id));
        assert!(!state.is_lease_valid(&lease.lease_id));

        // Idempotent release
        assert!(!state.release_lease(&lease.lease_id));
    }

    #[test]
    fn test_source_store() {
        let state = MockState::new();

        let sha256 = "abc123";
        assert!(!state.has_source(sha256));

        state.add_source(sha256.to_string(), "def456".to_string(), 1024);
        assert!(state.has_source(sha256));

        let entry = state.get_source(sha256).unwrap();
        assert_eq!(entry.size, 1024);
    }
}
