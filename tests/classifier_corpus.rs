//! Classifier correctness corpus tests
//!
//! Comprehensive test suite using a fixture corpus approach.
//! Each test case is a tuple of (input_argv, repo_config, expected_result).

use rch_xcode_lane::{Classifier, ClassifierConfig, RepoConfig};

// Helper to create argv from string slice
fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

// Standard test config
fn test_config() -> ClassifierConfig {
    ClassifierConfig {
        workspace: Some("MyApp.xcworkspace".to_string()),
        project: None,
        scheme: "MyApp".to_string(),
        destination: Some("platform=iOS Simulator,name=iPhone 16,OS=18.0".to_string()),
        allowed_configurations: vec!["Debug".to_string(), "Release".to_string()],
    }
}

// Config with no destination pinned
fn config_no_destination() -> ClassifierConfig {
    ClassifierConfig {
        workspace: Some("MyApp.xcworkspace".to_string()),
        project: None,
        scheme: "MyApp".to_string(),
        destination: None,
        allowed_configurations: vec![],
    }
}

// Config using project instead of workspace
fn config_with_project() -> ClassifierConfig {
    ClassifierConfig {
        workspace: None,
        project: Some("MyApp.xcodeproj".to_string()),
        scheme: "MyApp".to_string(),
        destination: None,
        allowed_configurations: vec![],
    }
}

// =============================================================================
// Category 1: Accepted invocations
// =============================================================================

#[test]
fn test_accepted_build_workspace_scheme() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(result.accepted, "Expected accepted, got: {:?}", result.rejection_reasons);
    assert_eq!(result.action, Some("build".to_string()));
}

#[test]
fn test_accepted_test_workspace_scheme_destination() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "test",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-destination",
        "platform=iOS Simulator,name=iPhone 16,OS=18.0",
    ]));
    assert!(result.accepted);
    assert_eq!(result.action, Some("test".to_string()));
}

#[test]
fn test_accepted_with_configuration_debug() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-configuration",
        "Debug",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_accepted_with_configuration_release() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-configuration",
        "Release",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_accepted_with_quiet_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-quiet",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_accepted_with_sdk_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-sdk",
        "iphonesimulator",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_accepted_test_with_destination_no_pin() {
    let classifier = Classifier::new(config_no_destination());
    let result = classifier.classify(&argv(&[
        "test",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-destination",
        "platform=iOS Simulator,name=iPhone 16 Pro",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_accepted_with_project() {
    let classifier = Classifier::new(config_with_project());
    let result = classifier.classify(&argv(&[
        "build",
        "-project",
        "MyApp.xcodeproj",
        "-scheme",
        "MyApp",
    ]));
    assert!(result.accepted);
}

// =============================================================================
// Category 2: Rejected invocations - denied actions
// =============================================================================

#[test]
fn test_rejected_archive_action() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "archive",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_ACTION:archive")));
}

#[test]
fn test_rejected_clean_action() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&["clean", "-scheme", "MyApp"]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_ACTION:clean")));
}

#[test]
fn test_rejected_analyze_action() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "analyze",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("UNKNOWN_ACTION:analyze")));
}

// =============================================================================
// Category 2: Rejected invocations - denied flags
// =============================================================================

#[test]
fn test_rejected_export_archive_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-exportArchive",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-exportArchive")));
}

#[test]
fn test_rejected_export_notarized_app_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-exportNotarizedApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-exportNotarizedApp")));
}

#[test]
fn test_rejected_result_bundle_path_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "test",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-resultBundlePath",
        "/tmp/results.xcresult",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-resultBundlePath")));
}

#[test]
fn test_rejected_derived_data_path_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-derivedDataPath",
        "/tmp/dd",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-derivedDataPath")));
}

#[test]
fn test_rejected_archive_path_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-archivePath",
        "/tmp/app.xcarchive",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-archivePath")));
}

#[test]
fn test_rejected_export_path_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-exportPath",
        "/tmp/export",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-exportPath")));
}

#[test]
fn test_rejected_export_options_plist_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-exportOptionsPlist",
        "/tmp/options.plist",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("DENIED_FLAG:-exportOptionsPlist")));
}

// =============================================================================
// Category 2: Rejected invocations - unknown flags
// =============================================================================

#[test]
fn test_rejected_unknown_flag() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-enableAddressSanitizer",
        "YES",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("UNKNOWN_FLAG:-enableAddressSanitizer")));
}

#[test]
fn test_rejected_unknown_flag_allow_provisioning_updates() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-allowProvisioningUpdates",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("UNKNOWN_FLAG:-allowProvisioningUpdates")));
}

// =============================================================================
// Category 2: Rejected invocations - mismatches
// =============================================================================

#[test]
fn test_rejected_scheme_mismatch() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "WrongScheme",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("SCHEME_MISMATCH:WrongScheme")));
}

#[test]
fn test_rejected_workspace_mismatch() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "Other.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("WORKSPACE_MISMATCH:Other.xcworkspace")));
}

#[test]
fn test_rejected_missing_scheme() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("MISSING_REQUIRED_FLAG:-scheme")));
}

#[test]
fn test_rejected_missing_workspace_when_required() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&["build", "-scheme", "MyApp"]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("MISSING_REQUIRED_FLAG:-workspace")));
}

#[test]
fn test_rejected_configuration_not_allowed() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-configuration",
        "Staging",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("CONFIGURATION_NOT_ALLOWED:Staging")));
}

// =============================================================================
// Category 3: Edge cases
// =============================================================================

#[test]
fn test_rejected_empty_argv() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("MISSING_ACTION")));
}

#[test]
fn test_rejected_no_action_keyword() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    // The parser treats -workspace as first arg, which is a flag not an action
    // So it will have MISSING_ACTION
    assert!(reasons.iter().any(|r| r.contains("MISSING_ACTION")));
}

#[test]
fn test_rejected_case_sensitivity_scheme() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-Scheme",  // Wrong case
        "MyApp",
    ]));
    assert!(!result.accepted);
    // -Scheme is unknown flag, and -scheme is missing
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("UNKNOWN_FLAG:-Scheme")));
}

#[test]
fn test_accepted_destination_with_spaces() {
    let classifier = Classifier::new(config_no_destination());
    let result = classifier.classify(&argv(&[
        "test",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-destination",
        "platform=iOS Simulator,name=iPhone 16 Pro Max",
    ]));
    assert!(result.accepted);
}

#[test]
fn test_xcodebuild_prefix_stripped() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "xcodebuild",
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));
    assert!(result.accepted);
    assert_eq!(result.action, Some("build".to_string()));
}

// =============================================================================
// Category 4: Constraint validation
// =============================================================================

#[test]
fn test_project_mismatch() {
    let classifier = Classifier::new(config_with_project());
    let result = classifier.classify(&argv(&[
        "build",
        "-project",
        "Other.xcodeproj",
        "-scheme",
        "MyApp",
    ]));
    assert!(!result.accepted);
    let reasons = result.rejection_reason_strings();
    assert!(reasons.iter().any(|r| r.contains("PROJECT_MISMATCH:Other.xcodeproj")));
}

// =============================================================================
// Category 5: Sanitized argv determinism
// =============================================================================

#[test]
fn test_sanitized_argv_ordering_deterministic() {
    let classifier = Classifier::new(test_config());

    // Same flags in different order
    let result1 = classifier.classify(&argv(&[
        "build",
        "-scheme",
        "MyApp",
        "-workspace",
        "MyApp.xcworkspace",
    ]));

    let result2 = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]));

    assert!(result1.accepted);
    assert!(result2.accepted);
    assert_eq!(result1.sanitized_argv, result2.sanitized_argv);
}

#[test]
fn test_sanitized_argv_ordering_with_configuration() {
    let classifier = Classifier::new(test_config());

    let result1 = classifier.classify(&argv(&[
        "build",
        "-configuration",
        "Debug",
        "-scheme",
        "MyApp",
        "-workspace",
        "MyApp.xcworkspace",
    ]));

    let result2 = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-configuration",
        "Debug",
    ]));

    let result3 = classifier.classify(&argv(&[
        "build",
        "-scheme",
        "MyApp",
        "-configuration",
        "Debug",
        "-workspace",
        "MyApp.xcworkspace",
    ]));

    assert!(result1.accepted && result2.accepted && result3.accepted);
    assert_eq!(result1.sanitized_argv, result2.sanitized_argv);
    assert_eq!(result2.sanitized_argv, result3.sanitized_argv);

    // Verify ordering: action first, then flags alphabetically
    let sanitized = result1.sanitized_argv.as_ref().unwrap();
    assert_eq!(sanitized[0], "build");
    assert_eq!(sanitized[1], "-configuration");
    assert_eq!(sanitized[3], "-scheme");
    assert_eq!(sanitized[5], "-workspace");
}

// =============================================================================
// Category 6: invocation.json output validation
// =============================================================================

#[test]
fn test_invocation_created_for_accepted() {
    let classifier = Classifier::new(test_config());
    let argv = argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]);
    let result = classifier.classify(&argv);
    assert!(result.accepted);

    let invocation = classifier.create_invocation(&argv, &result);
    assert!(invocation.is_some());

    let inv = invocation.unwrap();
    assert_eq!(inv.original_argv, argv);
    assert_eq!(inv.accepted_action, "build");
    assert!(inv.rejected_flags.is_empty());
    assert!(!inv.classifier_policy_sha256.is_empty());
}

#[test]
fn test_invocation_not_created_for_rejected() {
    let classifier = Classifier::new(test_config());
    let argv = argv(&["archive", "-scheme", "MyApp"]);
    let result = classifier.classify(&argv);
    assert!(!result.accepted);

    let invocation = classifier.create_invocation(&argv, &result);
    assert!(invocation.is_none());
}

#[test]
fn test_invocation_json_serialization() {
    let classifier = Classifier::new(test_config());
    let argv = argv(&[
        "test",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]);
    let result = classifier.classify(&argv);
    let inv = classifier.create_invocation(&argv, &result).unwrap();

    let json = inv.to_json().unwrap();
    assert!(json.contains("\"original_argv\""));
    assert!(json.contains("\"sanitized_argv\""));
    assert!(json.contains("\"accepted_action\""));
    assert!(json.contains("\"test\""));
    assert!(json.contains("\"classifier_policy_sha256\""));
}

// =============================================================================
// Category 7: Policy snapshot stability
// =============================================================================

#[test]
fn test_policy_hash_same_config() {
    let config = test_config();
    let classifier1 = Classifier::new(config.clone());
    let classifier2 = Classifier::new(config);

    assert_eq!(classifier1.policy_hash(), classifier2.policy_hash());
}

#[test]
fn test_policy_hash_different_scheme() {
    let config1 = test_config();
    let mut config2 = test_config();
    config2.scheme = "OtherScheme".to_string();

    let classifier1 = Classifier::new(config1);
    let classifier2 = Classifier::new(config2);

    assert_ne!(classifier1.policy_hash(), classifier2.policy_hash());
}

#[test]
fn test_policy_hash_different_workspace() {
    let config1 = test_config();
    let mut config2 = test_config();
    config2.workspace = Some("Other.xcworkspace".to_string());

    let classifier1 = Classifier::new(config1);
    let classifier2 = Classifier::new(config2);

    assert_ne!(classifier1.policy_hash(), classifier2.policy_hash());
}

#[test]
fn test_policy_hash_different_configurations() {
    let config1 = test_config();
    let mut config2 = test_config();
    config2.allowed_configurations = vec!["Debug".to_string()];

    let classifier1 = Classifier::new(config1);
    let classifier2 = Classifier::new(config2);

    assert_ne!(classifier1.policy_hash(), classifier2.policy_hash());
}

// =============================================================================
// Additional edge cases for completeness
// =============================================================================

#[test]
fn test_multiple_rejection_reasons() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "archive",  // Denied action
        "-workspace",
        "Other.xcworkspace",  // Mismatch
        "-scheme",
        "WrongScheme",  // Mismatch
        "-derivedDataPath",
        "/tmp/dd",  // Denied flag
    ]));
    assert!(!result.accepted);
    // Should have multiple rejection reasons
    assert!(result.rejection_reasons.len() >= 3);
}

#[test]
fn test_rejected_flags_tracked() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-derivedDataPath",
        "/tmp/dd",
        "-unknownFlag",
        "value",
    ]));
    assert!(!result.accepted);
    assert!(result.rejected_flags.contains(&"-derivedDataPath".to_string()));
    assert!(result.rejected_flags.contains(&"-unknownFlag".to_string()));
}

#[test]
fn test_matched_constraints_populated() {
    let classifier = Classifier::new(test_config());
    let result = classifier.classify(&argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
        "-configuration",
        "Debug",
    ]));
    assert!(result.accepted);
    assert_eq!(
        result.matched_constraints.workspace,
        Some("MyApp.xcworkspace".to_string())
    );
    assert_eq!(result.matched_constraints.scheme, "MyApp");
    assert_eq!(
        result.matched_constraints.configuration,
        Some("Debug".to_string())
    );
}

#[test]
fn test_explain_output_for_accepted() {
    let classifier = Classifier::new(test_config());
    let argv = argv(&[
        "build",
        "-workspace",
        "MyApp.xcworkspace",
        "-scheme",
        "MyApp",
    ]);
    let explain = classifier.explain(&argv);
    assert!(explain.accepted);
    assert!(explain.explanation.contains("ACCEPTED"));
}

#[test]
fn test_explain_output_for_rejected() {
    let classifier = Classifier::new(test_config());
    let argv = argv(&["archive", "-scheme", "MyApp"]);
    let explain = classifier.explain(&argv);
    assert!(!explain.accepted);
    assert!(explain.explanation.contains("REJECTED"));
}

// =============================================================================
// RepoConfig tests
// =============================================================================

#[test]
fn test_repo_config_to_classifier_config() {
    let toml = r#"
        workspace = "MyApp.xcworkspace"
        schemes = ["MyApp", "MyAppTests"]
        configurations = ["Debug", "Release"]
    "#;

    let repo_config = RepoConfig::from_str(toml).unwrap();
    let classifier_config = repo_config.to_classifier_config();

    // Uses first scheme as default
    assert_eq!(classifier_config.scheme, "MyApp");
    assert_eq!(
        classifier_config.workspace,
        Some("MyApp.xcworkspace".to_string())
    );
}

#[test]
fn test_repo_config_scheme_check() {
    let toml = r#"
        workspace = "MyApp.xcworkspace"
        schemes = ["MyApp", "MyAppTests"]
    "#;

    let repo_config = RepoConfig::from_str(toml).unwrap();
    assert!(repo_config.is_scheme_allowed("MyApp"));
    assert!(repo_config.is_scheme_allowed("MyAppTests"));
    assert!(!repo_config.is_scheme_allowed("Unknown"));
}
