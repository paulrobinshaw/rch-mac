//! M6 Integration Tests: Worker Selection
//!
//! Tests for rch-mac-0bw.5: M6 tests - worker selection portion
//!
//! Per PLAN.md:
//! - Deterministic mode: stable ordering by priority then name
//! - worker_selection.json contains all normative fields

use chrono::Utc;

use rch_xcode_lane::selection::{
    ProbeFailure, ProtocolRange, SelectionMode, WorkerSelection, SnapshotSource,
    SCHEMA_ID, SCHEMA_VERSION,
};

// === Worker Selection Artifact Tests ===

#[test]
fn test_worker_selection_all_fields_present() {
    let selection = WorkerSelection {
        schema_version: SCHEMA_VERSION,
        schema_id: SCHEMA_ID.to_string(),
        created_at: Utc::now(),
        run_id: "run-test-12345678".to_string(),
        negotiated_protocol_version: 1,
        worker_protocol_range: ProtocolRange { min: 1, max: 1 },
        selected_worker: "macmini-01".to_string(),
        selected_worker_host: "macmini.local".to_string(),
        selection_mode: SelectionMode::Deterministic,
        candidate_count: 3,
        probe_failures: vec![
            ProbeFailure {
                worker: "macmini-02".to_string(),
                probe_error: "SSH connection refused".to_string(),
                probe_duration_ms: 5000,
            },
        ],
        snapshot_age_seconds: 60,
        snapshot_source: SnapshotSource::Fresh,
        adaptive_metrics: None,
    };

    // Serialize to JSON and verify all fields are present
    let json = serde_json::to_value(&selection).unwrap();

    // Required normative fields
    assert!(json.get("schema_version").is_some());
    assert!(json.get("schema_id").is_some());
    assert!(json.get("created_at").is_some());
    assert!(json.get("run_id").is_some());
    assert!(json.get("negotiated_protocol_version").is_some());
    assert!(json.get("worker_protocol_range").is_some());
    assert!(json.get("selected_worker").is_some());
    assert!(json.get("selected_worker_host").is_some());
    assert!(json.get("selection_mode").is_some());
    assert!(json.get("candidate_count").is_some());
    assert!(json.get("probe_failures").is_some());
    assert!(json.get("snapshot_age_seconds").is_some());
    assert!(json.get("snapshot_source").is_some());

    // Field types
    assert!(json["schema_version"].is_i64());
    assert!(json["schema_id"].is_string());
    assert!(json["run_id"].is_string());
    assert!(json["negotiated_protocol_version"].is_i64());
    assert!(json["selected_worker"].is_string());
    assert!(json["candidate_count"].is_i64());
    assert!(json["probe_failures"].is_array());
}

#[test]
fn test_worker_selection_deterministic_mode_serialization() {
    let selection = WorkerSelection {
        schema_version: SCHEMA_VERSION,
        schema_id: SCHEMA_ID.to_string(),
        created_at: Utc::now(),
        run_id: "run-001".to_string(),
        negotiated_protocol_version: 1,
        worker_protocol_range: ProtocolRange { min: 1, max: 1 },
        selected_worker: "worker-a".to_string(),
        selected_worker_host: "worker-a.local".to_string(),
        selection_mode: SelectionMode::Deterministic,
        candidate_count: 1,
        probe_failures: vec![],
        snapshot_age_seconds: 0,
        snapshot_source: SnapshotSource::Fresh,
        adaptive_metrics: None,
    };

    let json = serde_json::to_string(&selection).unwrap();

    // Deterministic mode serializes as "deterministic"
    assert!(json.contains(r#""selection_mode":"deterministic""#));
}

#[test]
fn test_worker_selection_adaptive_mode_serialization() {
    let selection = WorkerSelection {
        schema_version: SCHEMA_VERSION,
        schema_id: SCHEMA_ID.to_string(),
        created_at: Utc::now(),
        run_id: "run-001".to_string(),
        negotiated_protocol_version: 1,
        worker_protocol_range: ProtocolRange { min: 1, max: 1 },
        selected_worker: "worker-b".to_string(),
        selected_worker_host: "worker-b.local".to_string(),
        selection_mode: SelectionMode::Adaptive,
        candidate_count: 2,
        probe_failures: vec![],
        snapshot_age_seconds: 120,
        snapshot_source: SnapshotSource::Cached,
        adaptive_metrics: None, // Adaptive metrics are optional even in adaptive mode
    };

    let json = serde_json::to_string(&selection).unwrap();

    assert!(json.contains(r#""selection_mode":"adaptive""#));
    assert!(json.contains(r#""snapshot_source":"cached""#));
    // adaptive_metrics is optional (skip_serializing_if = "Option::is_none")
    // When None, it won't appear in the JSON
}

#[test]
fn test_probe_failure_structure() {
    let failure = ProbeFailure {
        worker: "unreachable-worker".to_string(),
        probe_error: "Connection timed out".to_string(),
        probe_duration_ms: 30000,
    };

    let json = serde_json::to_value(&failure).unwrap();

    assert_eq!(json["worker"], "unreachable-worker");
    assert_eq!(json["probe_error"], "Connection timed out");
    assert_eq!(json["probe_duration_ms"], 30000);
}

#[test]
fn test_worker_selection_deserialization() {
    let json = r#"{
        "schema_version": 1,
        "schema_id": "rch-xcode/worker_selection@1",
        "created_at": "2026-01-01T00:00:00Z",
        "run_id": "run-test",
        "negotiated_protocol_version": 1,
        "worker_protocol_range": {"min": 1, "max": 2},
        "selected_worker": "mac-01",
        "selected_worker_host": "mac-01.local",
        "selection_mode": "deterministic",
        "candidate_count": 5,
        "probe_failures": [],
        "snapshot_age_seconds": 0,
        "snapshot_source": "fresh"
    }"#;

    let selection: WorkerSelection = serde_json::from_str(json).unwrap();

    assert_eq!(selection.schema_version, 1);
    assert_eq!(selection.selected_worker, "mac-01");
    assert_eq!(selection.candidate_count, 5);
    assert_eq!(selection.selection_mode, SelectionMode::Deterministic);
    assert_eq!(selection.worker_protocol_range.min, 1);
    assert_eq!(selection.worker_protocol_range.max, 2);
}

// === Protocol Range Tests ===

#[test]
fn test_protocol_range_serialization() {
    let range = ProtocolRange { min: 1, max: 3 };

    let json = serde_json::to_value(&range).unwrap();

    assert_eq!(json["min"], 1);
    assert_eq!(json["max"], 3);
}
