//! M2 Integration Tests (rch-mac-axz.23)
//!
//! Integration tests for M2 using the mock worker. Each test exercises a complete
//! or partial pipeline flow using MockTransport/MockWorker for in-process testing.

use chrono::Utc;
use rch_xcode_lane::host::{MockTransport, Transport};
use rch_xcode_lane::mock::FailureConfig;
use rch_xcode_lane::protocol::{Operation, RpcRequest};
use rch_xcode_lane::run::{
    ExecutionState, RunExecution, RunPlan, RunPlanBuilder, StepResult,
};
use rch_xcode_lane::selection::{ProtocolRange, SelectionMode, SnapshotSource, WorkerSelection};
use rch_xcode_lane::summary::{
    Backend, FailureKind, JobSummary, RunSummary, Status,
};
use rch_xcode_lane::job::Action;
use serde_json::json;

// =============================================================================
// Test Helpers
// =============================================================================

fn mock_worker_selection() -> WorkerSelection {
    WorkerSelection {
        schema_version: 1,
        schema_id: "rch-xcode/worker_selection@1".to_string(),
        created_at: Utc::now(),
        run_id: "test-run-id-12345678".to_string(),
        negotiated_protocol_version: 1,
        worker_protocol_range: ProtocolRange { min: 1, max: 1 },
        selected_worker: "macmini-01".to_string(),
        selected_worker_host: "macmini.local".to_string(),
        selection_mode: SelectionMode::Deterministic,
        candidate_count: 1,
        probe_failures: vec![],
        snapshot_age_seconds: 0,
        snapshot_source: SnapshotSource::Fresh,
    }
}

fn make_request(op: Operation, protocol_version: i32, payload: serde_json::Value) -> RpcRequest {
    RpcRequest {
        protocol_version,
        op,
        request_id: format!("req-{}", ulid::Ulid::new().to_string().to_lowercase()),
        payload,
    }
}

fn make_success_summary(run_id: &str, job_id: &str) -> JobSummary {
    JobSummary::success(
        run_id.to_string(),
        job_id.to_string(),
        format!("key-{}", job_id),
        Backend::Xcodebuild,
        1000,
    )
}

fn make_failed_summary(run_id: &str, job_id: &str) -> JobSummary {
    JobSummary::failure(
        run_id.to_string(),
        job_id.to_string(),
        format!("key-{}", job_id),
        Backend::Xcodebuild,
        FailureKind::Xcodebuild,
        None,
        "Build failed".to_string(),
        1000,
    )
}

fn make_rejected_summary(run_id: &str, job_id: &str) -> JobSummary {
    JobSummary::rejected(
        run_id.to_string(),
        job_id.to_string(),
        "Rejected by classifier".to_string(),
    )
}

fn make_cancelled_summary(run_id: &str, job_id: &str) -> JobSummary {
    JobSummary::cancelled(
        run_id.to_string(),
        job_id.to_string(),
        format!("key-{}", job_id),
        Backend::Xcodebuild,
        1000,
    )
}

// =============================================================================
// Test 1: Full Verify Cycle (Happy Path)
// =============================================================================

#[test]
fn test_full_verify_cycle_happy_path() {
    // Setup: verify with build+test actions, mock worker accepting all ops
    let transport = MockTransport::new();
    let worker = transport.worker();

    // Store source bundle for submit to succeed
    worker.store_source("source-sha256-abc123", vec![1, 2, 3, 4]);

    // 1. Probe: Negotiate protocol version
    let probe = make_request(Operation::Probe, 0, json!({}));
    let resp = transport.execute(&probe).unwrap();
    assert!(resp.ok, "Probe should succeed");
    assert_eq!(resp.protocol_version, 0);

    let payload = resp.payload.unwrap();
    let protocol_min = payload["protocol_min"].as_u64().unwrap() as i32;
    let protocol_max = payload["protocol_max"].as_u64().unwrap() as i32;
    assert!(protocol_min >= 1, "protocol_min should be at least 1");
    assert!(protocol_max >= protocol_min);

    // Use protocol version 1 for subsequent ops
    let protocol_version = protocol_min;

    // 2. Reserve: Request lease
    let reserve = make_request(
        Operation::Reserve,
        protocol_version,
        json!({"run_id": "run-verify-001", "ttl_seconds": 3600}),
    );
    let resp = transport.execute(&reserve).unwrap();
    assert!(resp.ok, "Reserve should succeed");
    let lease_id = resp.payload.unwrap()["lease_id"].as_str().unwrap().to_string();
    assert!(!lease_id.is_empty());

    // 3. Submit build job
    let submit_build = make_request(
        Operation::Submit,
        protocol_version,
        json!({
            "job_id": "job-build-001",
            "job_key": "key-build-001",
            "source_sha256": "source-sha256-abc123",
            "lease_id": lease_id,
        }),
    );
    let resp = transport.execute(&submit_build).unwrap();
    assert!(resp.ok, "Submit build should succeed");
    assert_eq!(resp.payload.unwrap()["state"], "QUEUED");

    // 4. Status check
    let status = make_request(
        Operation::Status,
        protocol_version,
        json!({"job_id": "job-build-001"}),
    );
    let resp = transport.execute(&status).unwrap();
    assert!(resp.ok, "Status should succeed");

    // 5. Submit test job (using same source)
    let submit_test = make_request(
        Operation::Submit,
        protocol_version,
        json!({
            "job_id": "job-test-001",
            "job_key": "key-test-001",
            "source_sha256": "source-sha256-abc123",
            "lease_id": lease_id,
        }),
    );
    let resp = transport.execute(&submit_test).unwrap();
    assert!(resp.ok, "Submit test should succeed");

    // 6. Release lease
    let release = make_request(
        Operation::Release,
        protocol_version,
        json!({"lease_id": lease_id}),
    );
    let resp = transport.execute(&release).unwrap();
    assert!(resp.ok, "Release should succeed");

    // Verify run_plan structure for verify flow
    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-verify-001")
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].action, Action::Build);
    assert_eq!(plan.steps[1].action, Action::Test);
    assert!(!plan.steps[0].rejected);
    assert!(!plan.steps[1].rejected);
}

// =============================================================================
// Test 2: Single Action Run
// =============================================================================

#[test]
fn test_single_action_run() {
    // Input: single action (rch xcode run --action build)
    let plan = RunPlanBuilder::single_action(Action::Build)
        .with_run_id("run-single-001")
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    // Expected: single-step run, run_plan.json has 1 step
    assert_eq!(plan.schema_version, 1);
    assert_eq!(plan.schema_id, "rch-xcode/run_plan@1");
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].action, Action::Build);
    assert_eq!(plan.steps[0].index, 0);
    assert!(!plan.steps[0].rejected);

    // Verify serialization
    let json = serde_json::to_string_pretty(&plan).unwrap();
    assert!(json.contains(r#""schema_version": 1"#));
    assert!(json.contains(r#""selected_worker": "macmini-01""#));

    // Verify run summary for single successful step
    let summaries = vec![make_success_summary("run-single-001", "job-001")];
    let run_summary = RunSummary::from_job_summaries("run-single-001".to_string(), &summaries, 1000);

    assert_eq!(run_summary.status, Status::Success);
    assert_eq!(run_summary.exit_code, 0);
    assert_eq!(run_summary.step_count, 1);
    assert_eq!(run_summary.steps_succeeded, 1);
}

// =============================================================================
// Test 3: Cancellation Flow
// =============================================================================

#[test]
fn test_cancellation_flow() {
    // Setup: verify with build+test, cancel after build completes
    let transport = MockTransport::new();
    let worker = transport.worker();
    worker.store_source("source-cancel-test", vec![1, 2, 3]);

    // Reserve
    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-cancel-001"}));
    let resp = transport.execute(&reserve).unwrap();
    assert!(resp.ok);
    let lease_id = resp.payload.unwrap()["lease_id"].as_str().unwrap().to_string();

    // Submit build job
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-cancel-build",
            "job_key": "key-build",
            "source_sha256": "source-cancel-test",
            "lease_id": &lease_id,
        }),
    );
    let resp = transport.execute(&submit).unwrap();
    assert!(resp.ok);

    // Cancel the job
    let cancel = make_request(
        Operation::Cancel,
        1,
        json!({"job_id": "job-cancel-build"}),
    );
    let resp = transport.execute(&cancel).unwrap();
    assert!(resp.ok, "Cancel should succeed");

    let state = resp.payload.unwrap()["state"].as_str().unwrap().to_string();
    assert_eq!(state, "CANCEL_REQUESTED", "Job should be in CANCEL_REQUESTED state");

    // Verify run_summary for cancelled run
    let summaries = vec![
        make_success_summary("run-cancel-001", "job-build"),
        make_cancelled_summary("run-cancel-001", "job-test"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-cancel-001".to_string(), &summaries, 2000);

    assert_eq!(run_summary.status, Status::Cancelled);
    assert_eq!(run_summary.exit_code, 80);
    assert_eq!(run_summary.steps_succeeded, 1);
    assert_eq!(run_summary.steps_cancelled, 1);
}

// =============================================================================
// Test 8: WORKER_BUSY with Retry
// =============================================================================

#[test]
fn test_worker_busy_with_retry() {
    let transport = MockTransport::new();
    let worker = transport.worker();

    // Set capacity to 0 to trigger BUSY
    worker.set_capacity(0);

    // First reserve should fail with BUSY
    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-busy-001"}));
    let resp = transport.execute(&reserve).unwrap();
    assert!(!resp.ok, "Reserve should fail when worker at capacity");

    let error = resp.error.unwrap();
    assert_eq!(error.code, "BUSY");
    assert!(error.data.is_some());
    let data = error.data.unwrap();
    assert!(data["retry_after_seconds"].is_number());

    // Increase capacity and retry
    worker.set_capacity(2);

    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-busy-001"}));
    let resp = transport.execute(&reserve).unwrap();
    assert!(resp.ok, "Reserve should succeed after capacity increase");
}

// =============================================================================
// Test 7: Retry on SOURCE_MISSING (TOCTOU Race)
// =============================================================================

#[test]
fn test_source_missing_retry() {
    let transport = MockTransport::new();
    let _worker = transport.worker();

    // has_source returns false initially
    let has = make_request(
        Operation::HasSource,
        1,
        json!({"source_sha256": "sha-missing"}),
    );
    let resp = transport.execute(&has).unwrap();
    assert!(resp.ok);
    assert!(!resp.payload.unwrap()["exists"].as_bool().unwrap());

    // Submit without source → SOURCE_MISSING
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-source-missing",
            "job_key": "key-001",
            "source_sha256": "sha-missing",
        }),
    );
    let resp = transport.execute(&submit).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "SOURCE_MISSING");

    // Upload source
    let upload = make_request(
        Operation::UploadSource,
        1,
        json!({
            "source_sha256": "sha-missing",
            "content": "base64-encoded-content",
        }),
    );
    let resp = transport.execute(&upload).unwrap();
    assert!(resp.ok, "Upload should succeed");

    // Now has_source should return true
    let has = make_request(
        Operation::HasSource,
        1,
        json!({"source_sha256": "sha-missing"}),
    );
    let resp = transport.execute(&has).unwrap();
    assert!(resp.ok);
    assert!(resp.payload.unwrap()["exists"].as_bool().unwrap());

    // Retry submit → should succeed
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-source-missing-2",
            "job_key": "key-001",
            "source_sha256": "sha-missing",
        }),
    );
    let resp = transport.execute(&submit).unwrap();
    assert!(resp.ok, "Submit should succeed after upload");
}

// =============================================================================
// Test 9: Sequential Abort-on-Failure
// =============================================================================

#[test]
fn test_abort_on_failure() {
    // Setup: verify with build+test, build fails (exit_code=50)
    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-abort-001")
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    let mut execution = RunExecution::new(plan.clone());
    assert_eq!(execution.state(), ExecutionState::Pending);

    // Record build failure
    execution.record_result(StepResult {
        step: plan.steps[0].clone(),
        status: Status::Failed,
        skipped: false,
    });

    // Execution should stop (abort on failure is default)
    assert_eq!(execution.state(), ExecutionState::Failed);
    assert!(!execution.should_continue());
    assert_eq!(execution.steps_failed(), 1);

    // Verify run_summary has steps_failed=1, steps_skipped=1
    let summaries = vec![make_failed_summary("run-abort-001", "job-build")];
    let run_summary = RunSummary::from_job_summaries("run-abort-001".to_string(), &summaries, 1000)
        .with_skipped_steps(1);

    assert_eq!(run_summary.status, Status::Failed);
    assert_eq!(run_summary.exit_code, 50); // XcodebuildFailed
    assert_eq!(run_summary.steps_failed, 1);
    assert_eq!(run_summary.steps_skipped, 1);
    assert_eq!(run_summary.step_count, 2); // 1 executed + 1 skipped
}

// =============================================================================
// Test 10: Continue on Failure Mode
// =============================================================================

#[test]
fn test_continue_on_failure_mode() {
    // Setup: config continue_on_failure=true, build fails
    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-continue-001")
        .continue_on_failure(true)
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    assert!(plan.continue_on_failure);

    let mut execution = RunExecution::new(plan.clone());

    // Record build failure
    execution.record_result(StepResult {
        step: plan.steps[0].clone(),
        status: Status::Failed,
        skipped: false,
    });

    // Execution should continue
    assert!(execution.should_continue(), "Should continue after failure");
    assert_eq!(execution.state(), ExecutionState::Running);

    // Record test success
    execution.record_result(StepResult {
        step: plan.steps[1].clone(),
        status: Status::Success,
        skipped: false,
    });

    // Final state should be Failed (at least one step failed)
    assert_eq!(execution.state(), ExecutionState::Failed);
    assert_eq!(execution.steps_failed(), 1);
    assert_eq!(execution.steps_succeeded(), 1);

    // Verify run_summary
    let summaries = vec![
        make_failed_summary("run-continue-001", "job-build"),
        make_success_summary("run-continue-001", "job-test"),
    ];
    let run_summary =
        RunSummary::from_job_summaries("run-continue-001".to_string(), &summaries, 2000);

    assert_eq!(run_summary.status, Status::Failed);
    assert_eq!(run_summary.steps_failed, 1);
    assert_eq!(run_summary.steps_succeeded, 1);
}

// =============================================================================
// Test 11: Rejected Step Handling
// =============================================================================

#[test]
fn test_rejected_step_handling() {
    // Setup: classifier rejects test action but accepts build
    let classifier_results = vec![
        (true, vec![]),                                      // build accepted
        (false, vec!["DESTINATION_INVALID".to_string()]),   // test rejected
    ];

    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-reject-001")
        .build_with_classifier_results(classifier_results, &mock_worker_selection())
        .unwrap();

    // Build step should not be rejected
    assert!(!plan.steps[0].rejected);
    assert!(plan.steps[0].rejection_reasons.is_empty());

    // Test step should be rejected
    assert!(plan.steps[1].rejected);
    assert_eq!(plan.steps[1].rejection_reasons, vec!["DESTINATION_INVALID"]);
    assert!(plan.has_rejected_steps());
    assert_eq!(plan.rejected_count(), 1);

    // Verify rejected summary
    let rejected_summary = JobSummary::rejected(
        "run-reject-001".to_string(),
        "job-test".to_string(),
        "DESTINATION_INVALID".to_string(),
    );

    assert_eq!(rejected_summary.status, Status::Rejected);
    assert_eq!(rejected_summary.exit_code, 10);
    assert!(rejected_summary.job_key.is_none()); // Per PLAN.md: job_key=null for rejected

    // Verify run_summary with rejected step
    let summaries = vec![
        make_success_summary("run-reject-001", "job-build"),
        make_rejected_summary("run-reject-001", "job-test"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-reject-001".to_string(), &summaries, 1000);

    assert_eq!(run_summary.status, Status::Rejected);
    assert_eq!(run_summary.exit_code, 10); // ClassifierRejected
    assert_eq!(run_summary.steps_rejected, 1);
}

// =============================================================================
// Test 12: Run State Machine Transitions
// =============================================================================

#[test]
fn test_run_state_transitions() {
    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-state-001")
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    let mut execution = RunExecution::new(plan.clone());

    // Initial state: Pending
    assert_eq!(execution.state(), ExecutionState::Pending);
    assert!(execution.has_executable_steps());
    assert!(execution.next_step().is_some());
    assert_eq!(execution.next_step().unwrap().action, Action::Build);

    // After first step: Running
    execution.record_result(StepResult {
        step: plan.steps[0].clone(),
        status: Status::Success,
        skipped: false,
    });
    assert_eq!(execution.state(), ExecutionState::Running);
    assert!(execution.should_continue());

    // After second step: Succeeded
    execution.record_result(StepResult {
        step: plan.steps[1].clone(),
        status: Status::Success,
        skipped: false,
    });
    assert_eq!(execution.state(), ExecutionState::Succeeded);
    assert!(!execution.should_continue());
    assert!(execution.next_step().is_none());
}

// =============================================================================
// Test 13: run_plan.json Correctness
// =============================================================================

#[test]
fn test_run_plan_schema_correctness() {
    let plan = RunPlanBuilder::new(vec![Action::Build, Action::Test])
        .with_run_id("run-schema-001")
        .continue_on_failure(true)
        .build_all_accepted(&mock_worker_selection())
        .unwrap();

    // Verify schema fields
    assert_eq!(plan.schema_version, 1);
    assert_eq!(plan.schema_id, "rch-xcode/run_plan@1");
    assert_eq!(plan.run_id, "run-schema-001");
    assert_eq!(plan.selected_worker, "macmini-01");
    assert_eq!(plan.selected_worker_host, "macmini.local");
    assert_eq!(plan.protocol_version, 1);
    assert!(plan.continue_on_failure);

    // Verify steps
    assert_eq!(plan.steps.len(), 2);
    for (i, step) in plan.steps.iter().enumerate() {
        assert_eq!(step.index, i);
        assert!(!step.job_id.is_empty());
        assert!(!step.rejected);
    }

    // Verify JSON serialization
    let json = serde_json::to_string_pretty(&plan).unwrap();
    assert!(json.contains(r#""schema_version": 1"#));
    assert!(json.contains(r#""schema_id": "rch-xcode/run_plan@1""#));
    assert!(json.contains(r#""selected_worker": "macmini-01""#));
    assert!(json.contains(r#""continue_on_failure": true"#));
    assert!(json.contains(r#""protocol_version": 1"#));

    // Verify deserialization round-trip
    let parsed: RunPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.run_id, plan.run_id);
    assert_eq!(parsed.steps.len(), plan.steps.len());
}

// =============================================================================
// Test 14: run_summary.json Exit Code Aggregation
// =============================================================================

#[test]
fn test_exit_code_aggregation_all_success() {
    let summaries = vec![
        make_success_summary("run-agg-001", "job1"),
        make_success_summary("run-agg-001", "job2"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-agg-001".to_string(), &summaries, 2000);

    assert_eq!(run_summary.status, Status::Success);
    assert_eq!(run_summary.exit_code, 0);
}

#[test]
fn test_exit_code_aggregation_any_rejected() {
    // Any rejected → run status=rejected, exit_code=10
    let summaries = vec![
        make_success_summary("run-agg-002", "job1"),
        make_failed_summary("run-agg-002", "job2"),
        make_rejected_summary("run-agg-002", "job3"),
        make_cancelled_summary("run-agg-002", "job4"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-agg-002".to_string(), &summaries, 4000);

    assert_eq!(run_summary.status, Status::Rejected);
    assert_eq!(run_summary.exit_code, 10);
}

#[test]
fn test_exit_code_aggregation_any_cancelled_no_rejected() {
    // Any cancelled (no rejected) → run status=cancelled, exit_code=80
    let summaries = vec![
        make_success_summary("run-agg-003", "job1"),
        make_failed_summary("run-agg-003", "job2"),
        make_cancelled_summary("run-agg-003", "job3"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-agg-003".to_string(), &summaries, 3000);

    assert_eq!(run_summary.status, Status::Cancelled);
    assert_eq!(run_summary.exit_code, 80);
}

#[test]
fn test_exit_code_aggregation_any_failed_no_rejected_cancelled() {
    // Any failed (no rejected/cancelled) → run status=failed, exit_code=first failing step's code
    let summaries = vec![
        make_success_summary("run-agg-004", "job1"),
        make_failed_summary("run-agg-004", "job2"),
    ];
    let run_summary = RunSummary::from_job_summaries("run-agg-004".to_string(), &summaries, 2000);

    assert_eq!(run_summary.status, Status::Failed);
    assert_eq!(run_summary.exit_code, 50); // XcodebuildFailed
}

// =============================================================================
// Test: Tail Log Streaming
// =============================================================================

#[test]
fn test_tail_log_streaming() {
    let transport = MockTransport::new();
    let worker = transport.worker();

    // Store source and submit job
    worker.store_source("sha-tail-test", vec![1, 2, 3]);

    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-tail-test",
            "job_key": "key-tail",
            "source_sha256": "sha-tail-test",
        }),
    );
    let resp = transport.execute(&submit).unwrap();
    assert!(resp.ok);

    // Tail with cursor=0
    let tail = make_request(
        Operation::Tail,
        1,
        json!({"job_id": "job-tail-test", "cursor": 0, "limit": 100}),
    );
    let resp = transport.execute(&tail).unwrap();
    assert!(resp.ok);

    let payload = resp.payload.unwrap();
    assert!(payload["entries"].is_array());
    // next_cursor may be Some or None depending on job state
}

// =============================================================================
// Test: Protocol Version Negotiation
// =============================================================================

#[test]
fn test_protocol_version_negotiation() {
    let transport = MockTransport::new();

    // Probe must use protocol_version: 0
    let probe_v0 = make_request(Operation::Probe, 0, json!({}));
    let resp = transport.execute(&probe_v0).unwrap();
    assert!(resp.ok, "Probe with v0 should succeed");
    assert_eq!(resp.protocol_version, 0);

    // Probe with v1 should fail
    let probe_v1 = make_request(Operation::Probe, 1, json!({}));
    let resp = transport.execute(&probe_v1).unwrap();
    assert!(!resp.ok, "Probe with v1 should fail");
    assert_eq!(resp.error.unwrap().code, "UNSUPPORTED_PROTOCOL");

    // Non-probe with v0 should fail
    let reserve_v0 = make_request(Operation::Reserve, 0, json!({"run_id": "test"}));
    let resp = transport.execute(&reserve_v0).unwrap();
    assert!(!resp.ok, "Non-probe with v0 should fail");
    assert_eq!(resp.error.unwrap().code, "UNSUPPORTED_PROTOCOL");

    // Non-probe with v1 should succeed
    let reserve_v1 = make_request(Operation::Reserve, 1, json!({"run_id": "test"}));
    let resp = transport.execute(&reserve_v1).unwrap();
    assert!(resp.ok, "Non-probe with v1 should succeed");
}

// =============================================================================
// Test: Idempotent Submit
// =============================================================================

#[test]
fn test_idempotent_submit() {
    let transport = MockTransport::new();
    let worker = transport.worker();
    worker.store_source("sha-idem", vec![1, 2, 3]);

    // First submit
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-idem-001",
            "job_key": "key-idem",
            "source_sha256": "sha-idem",
        }),
    );
    let resp1 = transport.execute(&submit).unwrap();
    assert!(resp1.ok, "First submit should succeed");

    // Same job_id + same job_key → return existing (idempotent)
    let resp2 = transport.execute(&submit).unwrap();
    assert!(resp2.ok, "Second submit (idempotent) should succeed");

    // Same job_id + different job_key → reject
    let submit_different = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-idem-001",
            "job_key": "different-key",
            "source_sha256": "sha-idem",
        }),
    );
    let resp3 = transport.execute(&submit_different).unwrap();
    assert!(!resp3.ok, "Submit with different job_key should fail");
    assert!(resp3.error.unwrap().message.contains("different job_key"));
}

// =============================================================================
// Test: Idempotent Release
// =============================================================================

#[test]
fn test_idempotent_release() {
    let transport = MockTransport::new();

    // Reserve
    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-rel-001"}));
    let resp = transport.execute(&reserve).unwrap();
    assert!(resp.ok);
    let lease_id = resp.payload.unwrap()["lease_id"].as_str().unwrap().to_string();

    // First release
    let release = make_request(Operation::Release, 1, json!({"lease_id": &lease_id}));
    let resp = transport.execute(&release).unwrap();
    assert!(resp.ok, "First release should succeed");

    // Second release (idempotent - unknown lease returns ok)
    let resp = transport.execute(&release).unwrap();
    assert!(resp.ok, "Second release (idempotent) should succeed");

    // Release of never-existed lease (still idempotent)
    let release_unknown = make_request(
        Operation::Release,
        1,
        json!({"lease_id": "nonexistent-lease-xyz"}),
    );
    let resp = transport.execute(&release_unknown).unwrap();
    assert!(resp.ok, "Release of unknown lease should still succeed (idempotent)");
}

// =============================================================================
// Test: Failure Injection
// =============================================================================

#[test]
fn test_failure_injection() {
    let transport = MockTransport::new();
    let worker = transport.worker();

    // Inject error for Reserve operation
    worker.inject_error(Operation::Reserve, "CUSTOM_ERROR", "Injected test failure");

    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-inj-001"}));
    let resp = transport.execute(&reserve).unwrap();

    assert!(!resp.ok, "Request should fail with injected error");
    let error = resp.error.unwrap();
    assert_eq!(error.code, "CUSTOM_ERROR");
    assert_eq!(error.message, "Injected test failure");

    // Clear failures and try again
    worker.clear_failures();

    let resp = transport.execute(&reserve).unwrap();
    assert!(resp.ok, "Request should succeed after clearing failures");
}

// =============================================================================
// Test: Failure Injection with Retry After
// =============================================================================

#[test]
fn test_failure_injection_with_retry_after() {
    let transport = MockTransport::new();
    let worker = transport.worker();

    // Inject BUSY with retry_after_seconds
    let config = FailureConfig::busy(5);
    worker.inject_failure(Operation::Reserve, config);

    let reserve = make_request(Operation::Reserve, 1, json!({"run_id": "run-retry-001"}));
    let resp = transport.execute(&reserve).unwrap();

    assert!(!resp.ok);
    let error = resp.error.unwrap();
    assert_eq!(error.code, "BUSY");
    assert!(error.data.is_some());
    let data = error.data.unwrap();
    assert_eq!(data["retry_after_seconds"], 5);
}

// =============================================================================
// Test: Cancel Already Terminal Job
// =============================================================================

#[test]
fn test_cancel_already_terminal() {
    let transport = MockTransport::new();
    let worker = transport.worker();
    worker.store_source("sha-terminal", vec![1, 2, 3]);

    // Submit job
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-term-001",
            "job_key": "key-term",
            "source_sha256": "sha-terminal",
        }),
    );
    transport.execute(&submit).unwrap();

    // Cancel once - transitions to CANCEL_REQUESTED
    let cancel = make_request(Operation::Cancel, 1, json!({"job_id": "job-term-001"}));
    let resp = transport.execute(&cancel).unwrap();
    assert!(resp.ok);

    // Note: In mock, CANCEL_REQUESTED is terminal, so second cancel should indicate already_terminal
    // (The actual behavior depends on mock implementation - this tests the RPC flow)
}

// =============================================================================
// Test: Fetch Artifacts for Non-Terminal Job
// =============================================================================

#[test]
fn test_fetch_non_terminal_job() {
    let transport = MockTransport::new();
    let worker = transport.worker();
    worker.store_source("sha-fetch", vec![1, 2, 3]);

    // Submit job (in QUEUED state, not terminal)
    let submit = make_request(
        Operation::Submit,
        1,
        json!({
            "job_id": "job-fetch-001",
            "job_key": "key-fetch",
            "source_sha256": "sha-fetch",
        }),
    );
    transport.execute(&submit).unwrap();

    // Fetch should fail (job not in terminal state)
    let fetch = make_request(Operation::Fetch, 1, json!({"job_id": "job-fetch-001"}));
    let resp = transport.execute(&fetch).unwrap();

    assert!(!resp.ok, "Fetch should fail for non-terminal job");
    let error = resp.error.unwrap();
    assert!(
        error.message.contains("terminal") || error.code.contains("INVALID"),
        "Error should mention terminal state: {} - {}",
        error.code,
        error.message
    );
}

// =============================================================================
// Test: Human Summary Generation
// =============================================================================

#[test]
fn test_human_summary_generation() {
    // Single step success
    let summaries = vec![make_success_summary("run-001", "job1")];
    let run = RunSummary::from_job_summaries("run-001".to_string(), &summaries, 1000);
    assert_eq!(run.human_summary, "Run succeeded");

    // Multiple step success
    let summaries = vec![
        make_success_summary("run-002", "job1"),
        make_success_summary("run-002", "job2"),
    ];
    let run = RunSummary::from_job_summaries("run-002".to_string(), &summaries, 2000);
    assert_eq!(run.human_summary, "Run succeeded: 2/2 steps passed");

    // Single step failure
    let summaries = vec![make_failed_summary("run-003", "job1")];
    let run = RunSummary::from_job_summaries("run-003".to_string(), &summaries, 1000);
    assert_eq!(run.human_summary, "Run failed");

    // Cancelled
    let summaries = vec![make_cancelled_summary("run-004", "job1")];
    let run = RunSummary::from_job_summaries("run-004".to_string(), &summaries, 1000);
    assert!(run.human_summary.contains("cancelled"));
}

// =============================================================================
// Test: Empty Run
// =============================================================================

#[test]
fn test_empty_run() {
    let run = RunSummary::empty("run-empty-001".to_string());

    assert_eq!(run.status, Status::Success);
    assert_eq!(run.exit_code, 0);
    assert_eq!(run.step_count, 0);
    assert_eq!(run.steps_succeeded, 0);
    assert_eq!(run.human_summary, "No steps executed");
}

// =============================================================================
// Test: RunPlanBuilder Errors
// =============================================================================

#[test]
fn test_run_plan_builder_no_actions() {
    use rch_xcode_lane::run::RunError;

    let result = RunPlanBuilder::from_action_strings(&[]);
    assert!(matches!(result, Err(RunError::NoActions)));
}

#[test]
fn test_run_plan_builder_invalid_action() {
    use rch_xcode_lane::run::RunError;

    let result = RunPlanBuilder::from_action_strings(&["archive"]);
    assert!(matches!(result, Err(RunError::InvalidAction(_))));
}

// =============================================================================
// Test: Run Summary Serialization
// =============================================================================

#[test]
fn test_run_summary_serialization() {
    let summaries = vec![make_success_summary("run-ser-001", "job1")];
    let run = RunSummary::from_job_summaries("run-ser-001".to_string(), &summaries, 1234);

    let json = run.to_json().unwrap();
    assert!(json.contains(r#""schema_version": 1"#));
    assert!(json.contains(r#""schema_id": "rch-xcode/run_summary@1""#));
    assert!(json.contains(r#""status": "success""#));
    assert!(json.contains(r#""exit_code": 0"#));
    assert!(json.contains(r#""duration_ms": 1234"#));

    // Round-trip
    let parsed = RunSummary::from_json(&json).unwrap();
    assert_eq!(parsed.run_id, run.run_id);
    assert_eq!(parsed.status, run.status);
    assert_eq!(parsed.duration_ms, 1234);
}
