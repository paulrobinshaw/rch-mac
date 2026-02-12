//! M6 Integration Tests: Worker Leases
//!
//! Tests for rch-mac-0bw.2: Worker leases (reserve/release)
//!
//! Per PLAN.md:
//! - If worker advertises feature "lease": host calls reserve once per run before submitting
//! - Includes lease_id on each submit
//! - Release is idempotent (unknown/expired lease → ok: true)
//! - Capacity: reserve fails with WORKER_BUSY + retry_after_seconds when at capacity
//! - Lease-based backstop: worker auto-cancels RUNNING jobs whose lease expires

use std::time::Duration;

use rch_protocol::ops::{JobSpec, JobState};
use rch_protocol::{RpcRequest, ErrorCode};
use rch_worker::config::WorkerConfig;
use rch_worker::mock_state::MockState;
use rch_worker::handlers::{reserve, release, submit, probe};

fn make_job_spec(job_id: &str) -> JobSpec {
    JobSpec {
        schema_version: 1,
        schema_id: "rch-xcode/job@1".to_string(),
        run_id: "run-001".to_string(),
        job_id: job_id.to_string(),
        action: "build".to_string(),
        job_key_inputs: serde_json::json!({
            "source_sha256": "test-source"
        }),
        job_key: format!("key-{}", job_id),
        effective_config: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

// === Feature Advertisement Tests ===

#[test]
fn test_worker_advertises_lease_feature() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    let result = probe::handle(&config, &state).unwrap();
    let features: Vec<String> = result
        .get("features")
        .and_then(|f| serde_json::from_value(f.clone()).ok())
        .unwrap_or_default();

    assert!(
        features.contains(&"lease".to_string()),
        "Worker should advertise 'lease' feature. Features: {:?}",
        features
    );
}

// === Reserve Operation Tests ===

#[test]
fn test_reserve_creates_lease() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    let request = RpcRequest {
        protocol_version: 1,
        op: "reserve".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({}),
    };

    let result = reserve::handle(&request, &config, &state).unwrap();

    let lease_id = result.get("lease_id").and_then(|v| v.as_str()).unwrap();
    assert!(!lease_id.is_empty(), "Lease ID should not be empty");

    let ttl = result.get("ttl_seconds").and_then(|v| v.as_u64()).unwrap();
    assert!(ttl > 0, "TTL should be positive");

    assert!(state.is_lease_valid(lease_id), "Created lease should be valid");
}

#[test]
fn test_reserve_with_custom_ttl() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    let request = RpcRequest {
        protocol_version: 1,
        op: "reserve".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({ "ttl_seconds": 7200 }),
    };

    let result = reserve::handle(&request, &config, &state).unwrap();

    let ttl = result.get("ttl_seconds").and_then(|v| v.as_u64()).unwrap();
    assert_eq!(ttl, 7200, "TTL should match requested value");
}

#[test]
fn test_reserve_busy_at_capacity() {
    let mut config = WorkerConfig::default();
    config.max_concurrent_jobs = 1;
    let state = MockState::new();

    let request = RpcRequest {
        protocol_version: 1,
        op: "reserve".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({}),
    };

    // First reserve succeeds
    let _ = reserve::handle(&request, &config, &state).unwrap();

    // Second reserve fails with BUSY
    let result = reserve::handle(&request, &config, &state);
    assert!(result.is_err(), "Second reserve should fail at capacity");

    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::Busy, "Error should be BUSY");
}

// === Release Operation Tests ===

#[test]
fn test_release_valid_lease() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    // Create a lease
    let lease = state.create_lease(3600);

    let request = RpcRequest {
        protocol_version: 1,
        op: "release".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({ "lease_id": lease.lease_id }),
    };

    let result = release::handle(&request, &config, &state).unwrap();

    let released = result.get("released").and_then(|v| v.as_bool()).unwrap();
    assert!(released, "Release should indicate it was released");
    assert!(!state.is_lease_valid(&lease.lease_id), "Lease should be invalid after release");
}

#[test]
fn test_release_is_idempotent() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    let lease = state.create_lease(3600);

    let request = RpcRequest {
        protocol_version: 1,
        op: "release".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({ "lease_id": lease.lease_id }),
    };

    // First release
    let result1 = release::handle(&request, &config, &state).unwrap();
    assert!(result1.get("released").and_then(|v| v.as_bool()).unwrap());

    // Second release succeeds (idempotent) but indicates not released
    let result2 = release::handle(&request, &config, &state).unwrap();
    assert!(!result2.get("released").and_then(|v| v.as_bool()).unwrap());
}

#[test]
fn test_release_unknown_lease_succeeds() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    let request = RpcRequest {
        protocol_version: 1,
        op: "release".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({ "lease_id": "nonexistent-lease-id" }),
    };

    // Should succeed (idempotent)
    let result = release::handle(&request, &config, &state).unwrap();
    assert!(!result.get("released").and_then(|v| v.as_bool()).unwrap());
}

// === Submit with Lease Tests ===

#[test]
fn test_submit_with_valid_lease() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    // Add source
    state.add_source("test-source".to_string(), "content-sha".to_string(), 1024);

    // Create a lease
    let lease = state.create_lease(3600);

    let job = make_job_spec("job-001");
    let request = RpcRequest {
        protocol_version: 1,
        op: "submit".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({
            "job": job,
            "lease_id": lease.lease_id
        }),
    };

    let result = submit::handle(&request, &config, &state).unwrap();

    let job_id = result.get("job_id").and_then(|v| v.as_str()).unwrap();
    assert_eq!(job_id, "job-001");
}

#[test]
fn test_submit_with_expired_lease_fails() {
    let config = WorkerConfig::default();
    let state = MockState::new();

    // Add source
    state.add_source("test-source".to_string(), "content-sha".to_string(), 1024);

    // Inject lease_expired failure
    state.set_failure_injection(rch_worker::mock_state::FailureInjection {
        lease_expired: true,
        ..Default::default()
    });

    let job = make_job_spec("job-001");
    let request = RpcRequest {
        protocol_version: 1,
        op: "submit".to_string(),
        request_id: "req-001".to_string(),
        payload: serde_json::json!({
            "job": job,
            "lease_id": "some-lease"
        }),
    };

    let result = submit::handle(&request, &config, &state);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::LeaseExpired);
}

// === Lease-Based Backstop Tests ===

#[test]
fn test_cancel_orphaned_jobs_on_expired_lease() {
    let state = MockState::new();

    // Create a lease with very short TTL (we'll simulate expiry)
    let lease = state.create_lease(1);  // 1 second TTL

    // Create a job associated with the lease
    let spec = make_job_spec("job-orphan");
    let job = state.create_job_with_lease(spec, Some(lease.lease_id.clone()));

    // Transition to running
    state.set_job_state(&job.job_id, JobState::Running);

    // Verify job is running
    let job_before = state.get_job(&job.job_id).unwrap();
    assert_eq!(job_before.state, JobState::Running);

    // Wait for lease to expire
    std::thread::sleep(Duration::from_millis(1100));

    // Run the backstop cleanup
    let cancelled = state.cancel_orphaned_jobs();

    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0], "job-orphan");

    // Verify job is cancelled
    let job_after = state.get_job(&job.job_id).unwrap();
    assert_eq!(job_after.state, JobState::Cancelled);
}

#[test]
fn test_jobs_without_lease_not_cancelled() {
    let state = MockState::new();

    // Create a job without a lease
    let spec = make_job_spec("job-no-lease");
    let job = state.create_job(spec);
    state.set_job_state(&job.job_id, JobState::Running);

    // Run the backstop cleanup
    let cancelled = state.cancel_orphaned_jobs();

    assert!(cancelled.is_empty(), "Job without lease should not be cancelled");

    let job_after = state.get_job(&job.job_id).unwrap();
    assert_eq!(job_after.state, JobState::Running);
}

#[test]
fn test_jobs_with_valid_lease_not_cancelled() {
    let state = MockState::new();

    // Create a lease with long TTL
    let lease = state.create_lease(3600);

    // Create a job associated with the lease
    let spec = make_job_spec("job-valid");
    let job = state.create_job_with_lease(spec, Some(lease.lease_id.clone()));
    state.set_job_state(&job.job_id, JobState::Running);

    // Run the backstop cleanup
    let cancelled = state.cancel_orphaned_jobs();

    assert!(cancelled.is_empty(), "Job with valid lease should not be cancelled");

    let job_after = state.get_job(&job.job_id).unwrap();
    assert_eq!(job_after.state, JobState::Running);
}

// === Run State Lease Tracking Tests ===

mod run_state_tests {
    use rch_xcode_lane::state::RunStateData;

    #[test]
    fn test_set_lease() {
        let mut state = RunStateData::new("run-001".to_string());

        state.set_lease("lease-123".to_string(), 3600);

        assert!(state.lease.is_some());
        let lease = state.lease.as_ref().unwrap();
        assert_eq!(lease.lease_id, "lease-123");
        assert_eq!(lease.ttl_seconds, 3600);
        assert!(!lease.renewed);
        assert_eq!(lease.renewal_count, 0);
    }

    #[test]
    fn test_mark_lease_renewed() {
        let mut state = RunStateData::new("run-001".to_string());
        state.set_lease("lease-123".to_string(), 3600);

        state.mark_lease_renewed();

        let lease = state.lease.as_ref().unwrap();
        assert!(lease.renewed);
        assert_eq!(lease.renewal_count, 1);

        state.mark_lease_renewed();
        let lease = state.lease.as_ref().unwrap();
        assert_eq!(lease.renewal_count, 2);
    }

    #[test]
    fn test_clear_lease() {
        let mut state = RunStateData::new("run-001".to_string());
        state.set_lease("lease-123".to_string(), 3600);

        state.clear_lease();

        assert!(state.lease.is_none());
    }

    #[test]
    fn test_lease_id_accessor() {
        let mut state = RunStateData::new("run-001".to_string());

        assert!(state.lease_id().is_none());

        state.set_lease("lease-xyz".to_string(), 3600);

        assert_eq!(state.lease_id(), Some("lease-xyz"));
    }

    #[test]
    fn test_lease_serialization() {
        let mut state = RunStateData::new("run-001".to_string());
        state.set_lease("lease-123".to_string(), 3600);
        state.mark_lease_renewed();

        let json = state.to_json().unwrap();

        assert!(json.contains("\"lease_id\": \"lease-123\""));
        assert!(json.contains("\"ttl_seconds\": 3600"));
        assert!(json.contains("\"renewed\": true"));
        assert!(json.contains("\"renewal_count\": 1"));
    }

    #[test]
    fn test_no_lease_no_json_field() {
        let state = RunStateData::new("run-001".to_string());

        let json = state.to_json().unwrap();

        // lease field should be skipped when None
        assert!(!json.contains("\"lease\":"));
    }
}
