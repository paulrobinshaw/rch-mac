//! Run and job state machine implementation
//!
//! Implements the state machines for runs and jobs per PLAN.md:
//! - Run states: QUEUED → RUNNING → {SUCCEEDED | FAILED | CANCELLED}
//! - Job states: QUEUED → RUNNING → {SUCCEEDED | FAILED | CANCELLED}
//!   with CANCEL_REQUESTED as intermediate state

mod job_state;
mod run_state;

pub use job_state::{JobState, JobStateData, JobStateError};
pub use run_state::{CurrentStep, RunState, RunStateData, RunStateError};

use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, Ordering};

/// Global sequence counter for ordering events within a single machine
static SEQUENCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Get the next sequence number for ordering
pub fn next_seq() -> u64 {
    SEQUENCE_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Get current UTC timestamp in RFC 3339 format
pub fn now_rfc3339() -> DateTime<Utc> {
    Utc::now()
}

/// Check if a state is terminal (no further transitions possible)
pub trait TerminalState {
    fn is_terminal(&self) -> bool;
}
