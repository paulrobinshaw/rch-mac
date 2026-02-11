//! M4 Artifact Tests (rch-mac-y6s.7)
//!
//! Tests for artifact integrity, manifest, attestation, two-phase commit,
//! artifact indices, and schema versioning.

use std::fs;
use std::path::Path;

use rch_xcode_lane::artifact::{
    verify_artifacts, ArtifactManifest, Attestation, IntegrityError, JobIndex, RunIndex,
    VerificationError, VerificationResult, ATTESTATION_SCHEMA_ID, EXCLUDED_FILES, SCHEMA_ID,
};
use rch_xcode_lane::summary::{ArtifactProfile, FailureKind, FailureSubkind};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Compute SHA-256 of bytes and return hex string
fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Create a test artifact directory with typical job artifacts
fn create_test_artifacts(dir: &Path) {
    fs::write(dir.join("summary.json"), r#"{"status":"success"}"#).unwrap();
    fs::write(dir.join("build.log"), "=== Build Output ===\nCompiling...\n").unwrap();
    fs::write(
        dir.join("toolchain.json"),
        r#"{"xcode_version":"15.0"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("destination.json"),
        r#"{"platform":"iOS Simulator"}"#,
    )
    .unwrap();

    // Create subdirectory
    fs::create_dir(dir.join("xcresult")).unwrap();
    fs::write(
        dir.join("xcresult/TestResults.json"),
        r#"{"tests":[]}"#,
    )
    .unwrap();
}

// =============================================================================
// Manifest Tests
// =============================================================================

/// Test 1: manifest.json generation with correct entries
#[test]
fn test_manifest_generation() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();

    // Verify schema fields
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.schema_id, SCHEMA_ID);
    assert_eq!(manifest.run_id, "run-123");
    assert_eq!(manifest.job_id, "job-456");

    // Verify entries are sorted lexicographically
    let paths: Vec<_> = manifest.entries.iter().map(|e| &e.path).collect();
    let mut sorted_paths = paths.clone();
    sorted_paths.sort();
    assert_eq!(paths, sorted_paths, "Entries must be sorted by path");

    // Verify file entries have sha256 and size
    let build_log = manifest
        .entries
        .iter()
        .find(|e| e.path == "build.log")
        .unwrap();
    assert!(build_log.sha256.is_some());
    assert!(build_log.size > 0);

    // Verify directory entries have sha256=null and size=0
    let xcresult = manifest
        .entries
        .iter()
        .find(|e| e.path == "xcresult")
        .unwrap();
    assert!(xcresult.sha256.is_none());
    assert_eq!(xcresult.size, 0);

    // Verify artifact_root_sha256 is present
    assert!(!manifest.artifact_root_sha256.is_empty());
    assert!(manifest.verify_artifact_root().unwrap());

    // Verify excluded files are not in entries
    for excluded in EXCLUDED_FILES {
        assert!(
            !manifest.entries.iter().any(|e| &e.path == excluded),
            "Excluded file {} should not be in entries",
            excluded
        );
    }
}

/// Test 2: Host manifest verification - sha256 mismatch
#[test]
fn test_manifest_verification_sha256_mismatch() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();
    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Tamper with a file (flip bytes in build.log)
    let original = fs::read(artifact_dir.join("build.log")).unwrap();
    let mut tampered = original.clone();
    if let Some(byte) = tampered.get_mut(0) {
        *byte = byte.wrapping_add(1);
    }
    fs::write(artifact_dir.join("build.log"), tampered).unwrap();

    // Verify should detect mismatch
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(!result.passed);

    let (kind, subkind, errors) = result.failure_info().unwrap();
    assert_eq!(kind, FailureKind::Artifacts);
    assert_eq!(subkind, FailureSubkind::IntegrityMismatch);
    assert!(errors.iter().any(|e| e.contains("build.log")));
}

/// Test 3: Host manifest verification - size mismatch
#[test]
fn test_manifest_verification_size_mismatch() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();
    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Truncate a file
    fs::write(artifact_dir.join("build.log"), "short").unwrap();

    // Verify should detect mismatch
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(!result.passed);

    let (_, subkind, errors) = result.failure_info().unwrap();
    assert_eq!(subkind, FailureSubkind::IntegrityMismatch);
    assert!(
        errors.iter().any(|e| e.contains("build.log")),
        "Error should mention build.log"
    );
}

/// Test 4: Host manifest verification - extra files
#[test]
fn test_manifest_verification_extra_files() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();
    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Add unexpected file
    fs::write(artifact_dir.join("unexpected.txt"), "extra content").unwrap();

    // Verify should detect extra file
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(!result.passed);

    let (_, subkind, errors) = result.failure_info().unwrap();
    assert_eq!(subkind, FailureSubkind::IntegrityMismatch);
    assert!(
        errors.iter().any(|e| e.contains("unexpected.txt")),
        "Error should mention unexpected.txt"
    );
}

/// Test 5: Host manifest verification - missing files
#[test]
fn test_manifest_verification_missing_files() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();
    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Delete a listed file
    fs::remove_file(artifact_dir.join("build.log")).unwrap();

    // Verify should detect missing file
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(!result.passed);

    let (_, subkind, errors) = result.failure_info().unwrap();
    assert_eq!(subkind, FailureSubkind::IntegrityMismatch);
    assert!(
        errors.iter().any(|e| e.contains("build.log") && e.contains("missing")),
        "Error should mention missing build.log"
    );
}

/// Test 6: artifact_root_sha256 tampering detection
#[test]
fn test_manifest_root_hash_tampering() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let mut manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();

    // Tamper with root hash
    manifest.artifact_root_sha256 = "0".repeat(64);

    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Verify should detect tampering
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(!result.passed);

    assert!(result.errors.iter().any(|e| matches!(
        e,
        VerificationError::RootHashMismatch { .. }
    )));
}

// =============================================================================
// Attestation Tests
// =============================================================================

/// Test 7: attestation.json content
#[test]
fn test_attestation_content() {
    let manifest_bytes = br#"{"entries":[]}"#;
    let capabilities_bytes = br#"{"xcode":["15.0"]}"#;

    let attestation = Attestation::new(
        "run-123".to_string(),
        "job-456".to_string(),
        "key-789".to_string(),
        "source-hash-abc123".to_string(),
        "macmini-01".to_string(),
        "SHA256:fingerprint123".to_string(),
        capabilities_bytes,
        "xcodebuild".to_string(),
        "15.0".to_string(),
        manifest_bytes,
    );

    // Verify all fields present
    assert_eq!(attestation.schema_version, 1);
    assert_eq!(attestation.schema_id, ATTESTATION_SCHEMA_ID);
    assert_eq!(attestation.job_id, "job-456");
    assert_eq!(attestation.job_key, "key-789");
    assert_eq!(attestation.source_sha256, "source-hash-abc123");
    assert_eq!(attestation.worker.name, "macmini-01");
    assert_eq!(attestation.worker.fingerprint, "SHA256:fingerprint123");
    assert_eq!(attestation.backend.name, "xcodebuild");
    assert_eq!(attestation.backend.version, "15.0");

    // Verify manifest_sha256 is correct
    let expected_manifest_sha256 = compute_sha256(manifest_bytes);
    assert_eq!(attestation.manifest_sha256, expected_manifest_sha256);

    // Verify capabilities_digest is correct
    let expected_capabilities_digest = compute_sha256(capabilities_bytes);
    assert_eq!(attestation.capabilities_digest, expected_capabilities_digest);
}

/// Test 8: Attestation binding - manifest modification detected
#[test]
fn test_attestation_binding() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();

    // Create manifest
    let manifest_content = br#"{"entries":[]}"#;
    fs::write(artifact_dir.join("manifest.json"), manifest_content).unwrap();

    // Create attestation with manifest hash
    let attestation = Attestation::new(
        "run-123".to_string(),
        "job-456".to_string(),
        "key-789".to_string(),
        "source-hash".to_string(),
        "worker".to_string(),
        "fingerprint".to_string(),
        b"{}",
        "xcodebuild".to_string(),
        "15.0".to_string(),
        manifest_content,
    );
    attestation
        .write_to_file(&artifact_dir.join("attestation.json"))
        .unwrap();

    // Verify binding intact
    assert!(attestation
        .verify_manifest(&artifact_dir.join("manifest.json"))
        .unwrap());

    // Modify manifest
    fs::write(artifact_dir.join("manifest.json"), b"modified").unwrap();

    // Binding should now be broken
    assert!(!attestation
        .verify_manifest(&artifact_dir.join("manifest.json"))
        .unwrap());
}

// =============================================================================
// Two-Phase Commit Tests
// =============================================================================

/// Test 10: Partial artifacts (no job_index.json) = incomplete
#[test]
fn test_incomplete_without_job_index() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();

    // Create partial artifacts but NO job_index.json
    fs::write(artifact_dir.join("summary.json"), "{}").unwrap();
    fs::write(artifact_dir.join("build.log"), "output").unwrap();
    fs::write(artifact_dir.join("manifest.json"), "{}").unwrap();

    // Should be treated as incomplete
    assert!(!JobIndex::is_complete(artifact_dir));

    // Now add job_index.json
    let job_index = JobIndex::new(
        "run-123".to_string(),
        "job-456".to_string(),
        "key-789".to_string(),
        "build".to_string(),
    );
    job_index
        .write_to_file(&artifact_dir.join("job_index.json"))
        .unwrap();

    // Now should be complete
    assert!(JobIndex::is_complete(artifact_dir));
}

// =============================================================================
// Artifact Index Tests
// =============================================================================

/// Test 12: job_index.json pointers resolve
#[test]
fn test_job_index_pointers() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();

    // Create required artifacts
    fs::write(artifact_dir.join("job.json"), "{}").unwrap();
    fs::write(artifact_dir.join("job_state.json"), "{}").unwrap();
    fs::write(artifact_dir.join("summary.json"), "{}").unwrap();
    fs::write(artifact_dir.join("manifest.json"), "{}").unwrap();
    fs::write(artifact_dir.join("attestation.json"), "{}").unwrap();
    fs::write(artifact_dir.join("toolchain.json"), "{}").unwrap();
    fs::write(artifact_dir.join("destination.json"), "{}").unwrap();
    fs::write(artifact_dir.join("effective_config.json"), "{}").unwrap();
    fs::write(artifact_dir.join("invocation.json"), "{}").unwrap();
    fs::write(artifact_dir.join("job_key_inputs.json"), "{}").unwrap();
    fs::write(artifact_dir.join("build.log"), "output").unwrap();

    // Create some optional artifacts
    fs::write(artifact_dir.join("metrics.json"), "{}").unwrap();
    fs::write(artifact_dir.join("events.jsonl"), "{}").unwrap();

    let mut job_index = JobIndex::new(
        "run-123".to_string(),
        "job-456".to_string(),
        "key-789".to_string(),
        "build".to_string(),
    )
    .with_artifact_profile(ArtifactProfile::Rich);

    job_index.scan_optional_artifacts(artifact_dir);

    // Verify required pointers
    assert_eq!(job_index.required.job, "job.json");
    assert_eq!(job_index.required.summary, "summary.json");
    assert_eq!(job_index.required.manifest, "manifest.json");
    assert_eq!(job_index.required.attestation, "attestation.json");
    assert_eq!(job_index.required.build_log, "build.log");

    // Verify optional pointers
    let metrics = job_index
        .optional
        .iter()
        .find(|a| a.name == "metrics")
        .unwrap();
    assert!(metrics.present);

    let events = job_index
        .optional
        .iter()
        .find(|a| a.name == "events")
        .unwrap();
    assert!(events.present);

    let junit = job_index
        .optional
        .iter()
        .find(|a| a.name == "junit")
        .unwrap();
    assert!(!junit.present);

    // Verify artifact profile
    assert_eq!(job_index.artifact_profile, Some(ArtifactProfile::Rich));

    // All required pointers should resolve to actual files
    assert!(artifact_dir.join(&job_index.required.job).exists());
    assert!(artifact_dir.join(&job_index.required.summary).exists());
    assert!(artifact_dir.join(&job_index.required.manifest).exists());
}

/// Test 13: run_index.json pointers
#[test]
fn test_run_index_pointers() {
    let mut run_index = RunIndex::new("run-123".to_string());

    // Verify default pointers
    assert_eq!(run_index.run_plan, "run_plan.json");
    assert_eq!(run_index.run_state, "run_state.json");
    assert_eq!(run_index.run_summary, "run_summary.json");
    assert_eq!(run_index.source_manifest, "source_manifest.json");
    assert_eq!(run_index.worker_selection, "worker_selection.json");
    assert_eq!(run_index.capabilities, "capabilities.json");

    // Add steps
    run_index.add_step(0, "build", "job-001");
    run_index.add_step(1, "test", "job-002");

    // Verify ordered step list
    assert_eq!(run_index.steps.len(), 2);
    assert_eq!(run_index.steps[0].index, 0);
    assert_eq!(run_index.steps[0].action, "build");
    assert_eq!(run_index.steps[0].job_id, "job-001");
    assert_eq!(
        run_index.steps[0].job_index_path,
        "steps/build/job-001/job_index.json"
    );

    assert_eq!(run_index.steps[1].index, 1);
    assert_eq!(run_index.steps[1].action, "test");
}

// =============================================================================
// Schema Versioning Tests
// =============================================================================

/// Test 14: Forward compatibility - parse newer schema version (same major)
#[test]
fn test_forward_compatibility() {
    let json = r#"{
        "schema_version": 2,
        "schema_id": "rch-xcode/summary@1",
        "run_id": "run-123",
        "job_id": "job-456",
        "job_key": "key-789",
        "created_at": "2024-01-01T00:00:00Z",
        "status": "success",
        "exit_code": 0,
        "human_summary": "Build succeeded",
        "duration_ms": 1000,
        "unknown_future_field": "should be ignored"
    }"#;

    // Parse should succeed (ignoring unknown fields)
    let summary: rch_xcode_lane::summary::JobSummary =
        serde_json::from_str(json).unwrap();

    // Known fields accessible
    assert_eq!(summary.run_id, "run-123");
    assert_eq!(summary.job_id, "job-456");
    assert_eq!(summary.exit_code, 0);
}

/// Test 15: Major version mismatch detection (schema_id change)
#[test]
fn test_major_version_detection() {
    // This test verifies that code can detect schema_id mismatches
    // In a real implementation, the consumer would check schema_id

    let json_v1 = r#"{"schema_id": "rch-xcode/summary@1"}"#;
    let json_v2 = r#"{"schema_id": "rch-xcode/summary@2"}"#;

    let v1: Value = serde_json::from_str(json_v1).unwrap();
    let v2: Value = serde_json::from_str(json_v2).unwrap();

    let expected_schema_id = "rch-xcode/summary@1";

    // V1 matches expected
    assert_eq!(v1["schema_id"], expected_schema_id);

    // V2 does not match expected - consumer should reject
    assert_ne!(v2["schema_id"], expected_schema_id);

    // In real code, this would trigger a clear rejection with diagnostic:
    // "Schema mismatch: expected rch-xcode/summary@1, got rch-xcode/summary@2"
}

/// Test: Manifest entries are properly sorted across nested directories
#[test]
fn test_manifest_nested_directory_sorting() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();

    // Create nested structure
    fs::create_dir_all(artifact_dir.join("a/b")).unwrap();
    fs::create_dir_all(artifact_dir.join("z")).unwrap();
    fs::write(artifact_dir.join("z/file.txt"), "z").unwrap();
    fs::write(artifact_dir.join("a/b/file.txt"), "ab").unwrap();
    fs::write(artifact_dir.join("a/file.txt"), "a").unwrap();

    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();

    let paths: Vec<_> = manifest.entries.iter().map(|e| e.path.as_str()).collect();

    // Verify lexicographic order
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
}

/// Test: Excluded files don't prevent verification
#[test]
fn test_excluded_files_allowed_in_directory() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();
    create_test_artifacts(artifact_dir);

    // Create manifest
    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();
    manifest
        .write_to_file(&artifact_dir.join("manifest.json"))
        .unwrap();

    // Add excluded files (should be allowed)
    fs::write(artifact_dir.join("attestation.json"), "{}").unwrap();
    fs::write(artifact_dir.join("job_index.json"), "{}").unwrap();

    // Verification should still pass (excluded files are allowed)
    let result = verify_artifacts(artifact_dir).unwrap();
    assert!(result.passed);
}

/// Test: Empty artifact directory produces valid manifest
#[test]
fn test_empty_artifact_directory() {
    let dir = TempDir::new().unwrap();
    let artifact_dir = dir.path();

    let manifest =
        ArtifactManifest::from_directory(artifact_dir, "run-123", "job-456", "key-789").unwrap();

    assert!(manifest.entries.is_empty());
    assert!(!manifest.artifact_root_sha256.is_empty());
    assert!(manifest.verify_artifact_root().unwrap());
}
