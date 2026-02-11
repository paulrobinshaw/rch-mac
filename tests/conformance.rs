//! Conformance Test Suite (rch-mac-1nt)
//!
//! Validates core determinism and reproducibility requirements per PLAN.md:
//! - JobSpec determinism: identical inputs produce identical job_key
//! - Source bundle reproducibility: identical repo state → identical source_sha256
//! - Cache correctness: result cache hits produce artifacts identical to fresh runs
//!
//! These tests complement other test files:
//! - classifier_corpus.rs: Classifier correctness
//! - m4_artifact_tests.rs: Artifact schema compliance
//! - protocol_negotiation.rs: Protocol round-trip
//! - job_lifecycle.rs: State machine transitions

use rch_xcode_lane::job::{
    JobKeyDestination, JobKeyInputs, JobKeyToolchain,
};
use rch_xcode_lane::destination::Provisioning;
use rch_xcode_lane::bundle::Bundler;
use std::fs;
use tempfile::TempDir;

// =============================================================================
// JobSpec Determinism Tests
// =============================================================================

/// Test: Identical JobKeyInputs produce identical job_key (determinism)
#[test]
fn test_job_key_determinism() {
    let inputs1 = JobKeyInputs::new(
        "abc123def456789012345678901234567890123456789012345678901234".to_string(),
        vec!["build".to_string(), "-scheme".to_string(), "MyApp".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: Some("com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string()),
            sim_runtime_build: Some("22C150".to_string()),
            device_type_identifier: Some("com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string()),
        },
    );

    let inputs2 = inputs1.clone();

    let key1 = inputs1.compute_job_key().unwrap();
    let key2 = inputs2.compute_job_key().unwrap();

    assert_eq!(key1, key2, "Identical JobKeyInputs must produce identical job_key");
    assert_eq!(key1.len(), 64, "job_key should be a 64-char hex SHA-256");
}

/// Test: job_key is stable across multiple computations
#[test]
fn test_job_key_stability() {
    let inputs = JobKeyInputs::new(
        "source123".to_string(),
        vec!["test".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    // Compute multiple times
    let key1 = inputs.compute_job_key().unwrap();
    let key2 = inputs.compute_job_key().unwrap();
    let key3 = inputs.compute_job_key().unwrap();

    assert_eq!(key1, key2);
    assert_eq!(key2, key3);
}

/// Test: Different inputs produce different job_key
#[test]
fn test_job_key_sensitivity_to_source() {
    let inputs1 = JobKeyInputs::new(
        "source_a".to_string(),
        vec!["build".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    let mut inputs2 = inputs1.clone();
    inputs2.source_sha256 = "source_b".to_string();

    let key1 = inputs1.compute_job_key().unwrap();
    let key2 = inputs2.compute_job_key().unwrap();

    assert_ne!(key1, key2, "Different source_sha256 must produce different job_key");
}

/// Test: Different argv produces different job_key
#[test]
fn test_job_key_sensitivity_to_argv() {
    let base = JobKeyInputs::new(
        "source123".to_string(),
        vec!["build".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    let mut modified = base.clone();
    modified.sanitized_argv = vec!["test".to_string()];

    let key1 = base.compute_job_key().unwrap();
    let key2 = modified.compute_job_key().unwrap();

    assert_ne!(key1, key2, "Different argv must produce different job_key");
}

/// Test: Different toolchain produces different job_key
#[test]
fn test_job_key_sensitivity_to_toolchain() {
    let base = JobKeyInputs::new(
        "source123".to_string(),
        vec!["build".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    let mut modified = base.clone();
    modified.toolchain.xcode_build = "17A5023a".to_string(); // Different Xcode version

    let key1 = base.compute_job_key().unwrap();
    let key2 = modified.compute_job_key().unwrap();

    assert_ne!(key1, key2, "Different toolchain must produce different job_key");
}

/// Test: Different destination produces different job_key
#[test]
fn test_job_key_sensitivity_to_destination() {
    let base = JobKeyInputs::new(
        "source123".to_string(),
        vec!["build".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    let mut modified = base.clone();
    modified.destination.name = "iPhone 15".to_string(); // Different device

    let key1 = base.compute_job_key().unwrap();
    let key2 = modified.compute_job_key().unwrap();

    assert_ne!(key1, key2, "Different destination must produce different job_key");
}

/// Test: JCS produces canonical ordering (field order doesn't matter)
#[test]
fn test_job_key_jcs_canonicalization() {
    // Create two inputs with same values but construct differently
    // JCS should produce the same output regardless of construction order
    let inputs1 = JobKeyInputs {
        source_sha256: "source123".to_string(),
        sanitized_argv: vec!["build".to_string()],
        toolchain: JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        destination: JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    };

    // Parse from JSON (which might have different field order)
    let json = serde_json::to_string(&inputs1).unwrap();
    let inputs2: JobKeyInputs = serde_json::from_str(&json).unwrap();

    let key1 = inputs1.compute_job_key().unwrap();
    let key2 = inputs2.compute_job_key().unwrap();

    assert_eq!(key1, key2, "JCS must produce canonical output regardless of field order");
}

// =============================================================================
// Source Bundle Reproducibility Tests
// =============================================================================

/// Test: Identical directory content produces identical source_sha256
#[test]
fn test_source_bundle_reproducibility() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    // Create identical content in both directories
    let content = r#"
        import Foundation

        func hello() {
            print("Hello, World!")
        }
    "#;

    fs::write(dir1.path().join("main.swift"), content).unwrap();
    fs::write(dir2.path().join("main.swift"), content).unwrap();

    let bundler1 = Bundler::new(dir1.path().to_path_buf());
    let bundler2 = Bundler::new(dir2.path().to_path_buf());

    let result1 = bundler1.create_bundle("run-001").unwrap();
    let result2 = bundler2.create_bundle("run-001").unwrap();

    assert_eq!(
        result1.source_sha256,
        result2.source_sha256,
        "Identical directory content must produce identical source_sha256"
    );
}

/// Test: Different content produces different source_sha256
#[test]
fn test_source_bundle_sensitivity_to_content() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    fs::write(dir1.path().join("main.swift"), "let x = 1").unwrap();
    fs::write(dir2.path().join("main.swift"), "let x = 2").unwrap();

    let bundler1 = Bundler::new(dir1.path().to_path_buf());
    let bundler2 = Bundler::new(dir2.path().to_path_buf());

    let result1 = bundler1.create_bundle("run-001").unwrap();
    let result2 = bundler2.create_bundle("run-001").unwrap();

    assert_ne!(
        result1.source_sha256,
        result2.source_sha256,
        "Different content must produce different source_sha256"
    );
}

/// Test: Different file names produce different source_sha256
#[test]
fn test_source_bundle_sensitivity_to_filenames() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    let content = "let x = 1";

    fs::write(dir1.path().join("a.swift"), content).unwrap();
    fs::write(dir2.path().join("b.swift"), content).unwrap();

    let bundler1 = Bundler::new(dir1.path().to_path_buf());
    let bundler2 = Bundler::new(dir2.path().to_path_buf());

    let result1 = bundler1.create_bundle("run-001").unwrap();
    let result2 = bundler2.create_bundle("run-001").unwrap();

    assert_ne!(
        result1.source_sha256,
        result2.source_sha256,
        "Different file names must produce different source_sha256"
    );
}

/// Test: Bundle is stable across multiple computations
#[test]
fn test_source_bundle_stability() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("main.swift"), "let x = 1").unwrap();

    let bundler = Bundler::new(dir.path().to_path_buf());

    let result1 = bundler.create_bundle("run-001").unwrap();
    let result2 = bundler.create_bundle("run-001").unwrap();
    let result3 = bundler.create_bundle("run-001").unwrap();

    assert_eq!(result1.source_sha256, result2.source_sha256);
    assert_eq!(result2.source_sha256, result3.source_sha256);
}

/// Test: Empty directories produce consistent (possibly empty) bundle
#[test]
fn test_source_bundle_empty_directory() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    let bundler1 = Bundler::new(dir1.path().to_path_buf());
    let bundler2 = Bundler::new(dir2.path().to_path_buf());

    let result1 = bundler1.create_bundle("run-001").unwrap();
    let result2 = bundler2.create_bundle("run-001").unwrap();

    assert_eq!(
        result1.source_sha256,
        result2.source_sha256,
        "Empty directories must produce identical source_sha256"
    );
}

/// Test: Bundle excludes .git directory
#[test]
fn test_source_bundle_excludes_git() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    fs::write(dir1.path().join("main.swift"), "let x = 1").unwrap();
    fs::write(dir2.path().join("main.swift"), "let x = 1").unwrap();

    // Add .git to only one directory
    fs::create_dir(dir1.path().join(".git")).unwrap();
    fs::write(dir1.path().join(".git/config"), "git config").unwrap();

    let bundler1 = Bundler::new(dir1.path().to_path_buf());
    let bundler2 = Bundler::new(dir2.path().to_path_buf());

    let result1 = bundler1.create_bundle("run-001").unwrap();
    let result2 = bundler2.create_bundle("run-001").unwrap();

    assert_eq!(
        result1.source_sha256,
        result2.source_sha256,
        ".git directory should be excluded from bundle"
    );
}

// =============================================================================
// Schema Validation Tests
// =============================================================================

/// Test: JobKeyInputs serializes to valid JSON
#[test]
fn test_job_key_inputs_json_valid() {
    let inputs = JobKeyInputs::new(
        "source123".to_string(),
        vec!["build".to_string(), "-scheme".to_string(), "MyApp".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
        },
    );

    let json = serde_json::to_string(&inputs);
    assert!(json.is_ok(), "JobKeyInputs must serialize to valid JSON");

    let parsed: serde_json::Value = serde_json::from_str(&json.unwrap()).unwrap();
    assert!(parsed.get("source_sha256").is_some());
    assert!(parsed.get("sanitized_argv").is_some());
    assert!(parsed.get("toolchain").is_some());
    assert!(parsed.get("destination").is_some());
}

/// Test: JobKeyInputs round-trips through JSON
#[test]
fn test_job_key_inputs_json_roundtrip() {
    let original = JobKeyInputs::new(
        "source123".to_string(),
        vec!["test".to_string()],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: Some("runtime-id".to_string()),
            sim_runtime_build: Some("build-123".to_string()),
            device_type_identifier: Some("device-type".to_string()),
        },
    );

    let json = serde_json::to_string(&original).unwrap();
    let parsed: JobKeyInputs = serde_json::from_str(&json).unwrap();

    assert_eq!(original.source_sha256, parsed.source_sha256);
    assert_eq!(original.sanitized_argv, parsed.sanitized_argv);
    assert_eq!(original.toolchain, parsed.toolchain);
    assert_eq!(original.destination, parsed.destination);
}

/// Test: Provisioning enum serializes correctly
#[test]
fn test_provisioning_serialization() {
    let existing = Provisioning::Existing;
    let ephemeral = Provisioning::Ephemeral;

    let json_existing = serde_json::to_string(&existing).unwrap();
    let json_ephemeral = serde_json::to_string(&ephemeral).unwrap();

    assert_eq!(json_existing, "\"existing\"");
    assert_eq!(json_ephemeral, "\"ephemeral\"");

    let parsed_existing: Provisioning = serde_json::from_str(&json_existing).unwrap();
    let parsed_ephemeral: Provisioning = serde_json::from_str(&json_ephemeral).unwrap();

    assert_eq!(parsed_existing, Provisioning::Existing);
    assert_eq!(parsed_ephemeral, Provisioning::Ephemeral);
}
