//! M7 fixture tests (rch-mac-b7s.7)
//!
//! Tests using the fixture project for golden-file assertions.

mod fixtures;

use fixtures::{ClassifierCorpus, GoldenExpectations};
use rch_xcode_lane::{Bundler, Classifier, ClassifierConfig};

/// Run all classifier corpus test cases
#[test]
fn test_classifier_corpus() {
    let corpus = ClassifierCorpus::load().expect("Failed to load corpus");

    // Build classifier config from corpus
    let config = ClassifierConfig {
        workspace: corpus.config.workspace,
        project: corpus.config.project,
        scheme: corpus.config.scheme,
        destination: corpus.config.destination,
        allowed_configurations: corpus.config.allowed_configurations,
    };

    let classifier = Classifier::new(config);

    let mut passed = 0;
    let mut failed = 0;

    for tc in &corpus.test_cases {
        let result = classifier.classify(&tc.argv);

        // Check acceptance
        if result.accepted != tc.expected.accepted {
            eprintln!(
                "FAIL [{}]: expected accepted={}, got={}",
                tc.id, tc.expected.accepted, result.accepted
            );
            eprintln!("  Description: {}", tc.description);
            eprintln!("  Argv: {:?}", tc.argv);
            if !result.accepted {
                eprintln!("  Rejection reasons: {:?}", result.rejection_reasons);
            }
            failed += 1;
            continue;
        }

        // Check action if expected
        if let Some(ref expected_action) = tc.expected.action {
            if result.action.as_ref() != Some(expected_action) {
                eprintln!(
                    "FAIL [{}]: expected action={:?}, got={:?}",
                    tc.id, expected_action, result.action
                );
                failed += 1;
                continue;
            }
        }

        passed += 1;
    }

    eprintln!("\nClassifier corpus: {} passed, {} failed", passed, failed);
    assert_eq!(failed, 0, "Classifier corpus had {} failures", failed);
}

/// Verify golden expectations file is valid
#[test]
fn test_golden_expectations_valid() {
    let golden = GoldenExpectations::load().expect("Failed to load golden");

    // Verify expected artifacts
    assert!(golden.expected_artifacts.contains(&"manifest.json".to_string()));
    assert!(golden.expected_artifacts.contains(&"summary.json".to_string()));
    assert!(golden.expected_artifacts.contains(&"attestation.json".to_string()));
    assert!(golden.expected_artifacts.contains(&"build.log".to_string()));

    // Verify job_key_inputs has required fields
    assert!(golden.job_key_inputs.get("workspace").is_some());
    assert!(golden.job_key_inputs.get("scheme").is_some());
    assert!(golden.job_key_inputs.get("action").is_some());

    // Verify schema versions
    assert!(golden.schema_versions.get("manifest").is_some());
    assert!(golden.schema_versions.get("summary").is_some());
    assert!(golden.schema_versions.get("attestation").is_some());
}

/// Verify source bundle structure is complete
#[test]
fn test_source_bundle_structure() {
    let path = fixtures::source_bundle_path();

    // Workspace
    assert!(
        path.join("MyApp.xcworkspace/contents.xcworkspacedata").exists(),
        "Missing workspace file"
    );

    // Project
    assert!(
        path.join("MyApp.xcodeproj/project.pbxproj").exists(),
        "Missing project.pbxproj"
    );

    // App sources
    assert!(path.join("MyApp/AppDelegate.swift").exists());
    assert!(path.join("MyApp/ViewController.swift").exists());
    assert!(path.join("MyApp/Main.storyboard").exists());
    assert!(path.join("MyApp/Info.plist").exists());
    assert!(path.join("MyApp/Assets.xcassets/Contents.json").exists());

    // Test sources
    assert!(path.join("MyAppTests/MyAppTests.swift").exists());
}

/// Test source bundle can be hashed deterministically
#[test]
fn test_source_bundle_hashing() {
    let source_path = fixtures::source_bundle_path();

    // Create bundler for the fixture
    let bundler = Bundler::new(source_path.clone());

    // Create bundle and get hash
    let result1 = bundler.create_bundle("test-run-1").expect("Failed to create bundle");
    let hash1 = &result1.source_sha256;

    // Create again - should produce same hash
    let bundler2 = Bundler::new(source_path);
    let result2 = bundler2.create_bundle("test-run-2").expect("Failed to create bundle");
    let hash2 = &result2.source_sha256;

    assert_eq!(
        hash1, hash2,
        "Source bundle hash is not deterministic"
    );

    // Verify hash is non-empty
    assert!(!hash1.is_empty(), "Source hash should not be empty");
    assert_eq!(hash1.len(), 64, "SHA-256 hash should be 64 hex chars");
}

/// Test that excluded files don't affect source hash
#[test]
fn test_source_bundle_excludes() {
    let source_path = fixtures::source_bundle_path();

    let bundler = Bundler::new(source_path);
    let result = bundler.create_bundle("test-run").expect("Failed to create bundle");

    // Verify no .git files included
    for entry in &result.manifest.entries {
        assert!(
            !entry.path.contains(".git"),
            "Should not include .git: {}",
            entry.path
        );
        assert!(
            !entry.path.contains(".DS_Store"),
            "Should not include .DS_Store: {}",
            entry.path
        );
    }
}
