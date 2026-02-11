//! Deny-by-default classifier for xcodebuild invocations.
//!
//! The classifier parses xcodebuild argv and validates it against an allowlist,
//! producing a structured accept/reject decision.

mod config;
mod parser;
mod result;

pub use config::{ClassifierConfig, MatchedConstraints};
pub use parser::parse_argv;
pub use result::{ClassifierResult, RejectionReason};

use std::collections::BTreeMap;

/// Allowed xcodebuild actions.
const ALLOWED_ACTIONS: &[&str] = &["build", "test"];

/// Explicitly denied actions.
const DENIED_ACTIONS: &[&str] = &[
    "archive",
    "exportArchive",
    "exportNotarizedApp",
    "notarize",
    "altool",
    "staple",
];

/// Explicitly denied flags (worker controls these).
const DENIED_FLAGS: &[&str] = &["-resultBundlePath", "-derivedDataPath"];

/// Allowed flags with values (flag name -> true if requires value).
const ALLOWED_FLAGS_WITH_VALUES: &[(&str, bool)] = &[
    ("-workspace", true),
    ("-project", true),
    ("-scheme", true),
    ("-destination", true),
    ("-configuration", true),
    ("-sdk", true),
    ("-arch", true),
    ("-target", true),
];

/// Allowed boolean flags (no value).
const ALLOWED_BOOLEAN_FLAGS: &[&str] = &[
    "-quiet",
    "-verbose",
    "-enableCodeCoverage",
    "-showBuildSettings",
    "-showdestinations",
];

/// Classify an xcodebuild invocation.
///
/// Takes the argv (arguments after `xcodebuild`) and a repo config,
/// and produces a ClassifierResult indicating acceptance or rejection.
pub fn classify(argv: &[String], config: &ClassifierConfig) -> ClassifierResult {
    let mut rejected_flags: Vec<String> = Vec::new();
    let mut rejection_reasons: Vec<RejectionReason> = Vec::new();

    // Parse argv into action and flags
    let parsed = match parse_argv(argv) {
        Ok(p) => p,
        Err(e) => {
            rejection_reasons.push(RejectionReason::ParseError(e));
            return ClassifierResult::rejected(rejected_flags, rejection_reasons);
        }
    };

    // Check action
    let action = match &parsed.action {
        Some(a) => {
            // Check if action is denied
            if DENIED_ACTIONS.contains(&a.as_str()) {
                rejection_reasons.push(RejectionReason::DeniedAction(a.clone()));
                return ClassifierResult::rejected(rejected_flags, rejection_reasons);
            }
            // Check if action is allowed
            if !ALLOWED_ACTIONS.contains(&a.as_str()) {
                rejection_reasons.push(RejectionReason::UnknownAction(a.clone()));
                return ClassifierResult::rejected(rejected_flags, rejection_reasons);
            }
            a.clone()
        }
        None => {
            // Default action is "build" if no action specified
            "build".to_string()
        }
    };

    // Check each flag
    let mut matched_constraints = MatchedConstraints::default();
    let mut sanitized_flags: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (flag, value) in &parsed.flags {
        // Check denied flags
        if DENIED_FLAGS.contains(&flag.as_str()) {
            rejected_flags.push(flag.clone());
            rejection_reasons.push(RejectionReason::DeniedFlag(flag.clone()));
            continue;
        }

        // Check if flag is in allowed list
        let allowed_with_value = ALLOWED_FLAGS_WITH_VALUES
            .iter()
            .find(|(f, _)| *f == flag.as_str());
        let is_boolean = ALLOWED_BOOLEAN_FLAGS.contains(&flag.as_str());

        if allowed_with_value.is_none() && !is_boolean {
            rejected_flags.push(flag.clone());
            rejection_reasons.push(RejectionReason::UnknownFlag(flag.clone()));
            continue;
        }

        // Validate flag value against config constraints
        match flag.as_str() {
            "-workspace" => {
                if let Some(v) = value {
                    if let Some(ref allowed) = config.workspace {
                        if v != allowed {
                            rejection_reasons.push(RejectionReason::WorkspaceMismatch {
                                got: v.clone(),
                                expected: allowed.clone(),
                            });
                            continue;
                        }
                    }
                    matched_constraints.workspace = Some(v.clone());
                    sanitized_flags.insert(flag.clone(), Some(v.clone()));
                }
            }
            "-project" => {
                if let Some(v) = value {
                    if let Some(ref allowed) = config.project {
                        if v != allowed {
                            rejection_reasons.push(RejectionReason::ProjectMismatch {
                                got: v.clone(),
                                expected: allowed.clone(),
                            });
                            continue;
                        }
                    }
                    matched_constraints.project = Some(v.clone());
                    sanitized_flags.insert(flag.clone(), Some(v.clone()));
                }
            }
            "-scheme" => {
                if let Some(v) = value {
                    if !config.allowed_schemes.is_empty()
                        && !config.allowed_schemes.contains(v)
                    {
                        rejection_reasons.push(RejectionReason::SchemeMismatch(v.clone()));
                        continue;
                    }
                    matched_constraints.scheme = Some(v.clone());
                    sanitized_flags.insert(flag.clone(), Some(v.clone()));
                }
            }
            "-destination" => {
                if let Some(v) = value {
                    if let Some(ref allowed) = config.destination {
                        if v != allowed {
                            rejection_reasons.push(RejectionReason::DestinationMismatch {
                                got: v.clone(),
                                expected: allowed.clone(),
                            });
                            continue;
                        }
                    }
                    matched_constraints.destination = Some(v.clone());
                    sanitized_flags.insert(flag.clone(), Some(v.clone()));
                }
            }
            "-configuration" => {
                if let Some(v) = value {
                    if !config.allowed_configurations.is_empty()
                        && !config.allowed_configurations.contains(v)
                    {
                        rejection_reasons.push(RejectionReason::ConfigurationMismatch(v.clone()));
                        continue;
                    }
                    matched_constraints.configuration = Some(v.clone());
                    sanitized_flags.insert(flag.clone(), Some(v.clone()));
                }
            }
            _ => {
                // Other allowed flags - just pass through
                sanitized_flags.insert(flag.clone(), value.clone());
            }
        }
    }

    // If we have rejection reasons, reject
    if !rejection_reasons.is_empty() {
        return ClassifierResult::rejected(rejected_flags, rejection_reasons);
    }

    // Scheme is required by config if allowed_schemes is non-empty
    if !config.allowed_schemes.is_empty() && matched_constraints.scheme.is_none() {
        rejection_reasons.push(RejectionReason::MissingRequiredFlag("-scheme".to_string()));
        return ClassifierResult::rejected(rejected_flags, rejection_reasons);
    }

    // Build sanitized_argv in canonical order: action first, then flags sorted lexicographically
    let mut sanitized_argv: Vec<String> = vec![action.clone()];
    for (flag, value) in &sanitized_flags {
        sanitized_argv.push(flag.clone());
        if let Some(v) = value {
            sanitized_argv.push(v.clone());
        }
    }

    ClassifierResult::accepted(action, sanitized_argv, matched_constraints)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_accept_simple_build() {
        let config = ClassifierConfig {
            workspace: Some("Foo.xcworkspace".to_string()),
            allowed_schemes: vec!["Bar".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let argv = to_argv("-workspace Foo.xcworkspace -scheme Bar build");
        let result = classify(&argv, &config);
        assert!(result.accepted);
        assert_eq!(result.action, Some("build".to_string()));
        assert!(result.rejection_reasons.is_empty());
    }

    #[test]
    fn test_reject_archive_action() {
        let config = ClassifierConfig::default();
        let argv = to_argv("archive -workspace Foo.xcworkspace -scheme Bar");
        let result = classify(&argv, &config);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::DeniedAction(_))));
    }

    #[test]
    fn test_reject_result_bundle_path() {
        let config = ClassifierConfig::default();
        let argv = to_argv("build -resultBundlePath /tmp/out -workspace Foo.xcworkspace -scheme Bar");
        let result = classify(&argv, &config);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::DeniedFlag(_))));
    }

    #[test]
    fn test_reject_unknown_scheme() {
        let config = ClassifierConfig {
            allowed_schemes: vec!["AllowedScheme".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let argv = to_argv("build -scheme Unknown");
        let result = classify(&argv, &config);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::SchemeMismatch(_))));
    }

    #[test]
    fn test_reject_unknown_flag() {
        let config = ClassifierConfig::default();
        let argv = to_argv("build -unknownFlag value");
        let result = classify(&argv, &config);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::UnknownFlag(_))));
    }

    #[test]
    fn test_sanitized_argv_canonical_order() {
        let config = ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            allowed_schemes: vec!["MyApp".to_string()].into_iter().collect(),
            allowed_configurations: vec!["Debug".to_string()].into_iter().collect(),
            ..Default::default()
        };
        // Provide flags in non-sorted order
        let argv = to_argv("-scheme MyApp -workspace MyApp.xcworkspace -configuration Debug build");
        let result = classify(&argv, &config);
        assert!(result.accepted);
        // Sanitized argv should be: build, then flags sorted lexicographically
        let sanitized = result.sanitized_argv.unwrap();
        assert_eq!(sanitized[0], "build");
        // Flags should be sorted: -configuration, -scheme, -workspace
        assert_eq!(sanitized[1], "-configuration");
        assert_eq!(sanitized[2], "Debug");
        assert_eq!(sanitized[3], "-scheme");
        assert_eq!(sanitized[4], "MyApp");
        assert_eq!(sanitized[5], "-workspace");
        assert_eq!(sanitized[6], "MyApp.xcworkspace");
    }

    #[test]
    fn test_default_action_is_build() {
        let config = ClassifierConfig::default();
        let argv = to_argv("-scheme Bar");
        let result = classify(&argv, &config);
        assert!(result.accepted);
        assert_eq!(result.action, Some("build".to_string()));
    }

    #[test]
    fn test_accept_test_action() {
        let config = ClassifierConfig::default();
        let argv = to_argv("test -scheme Bar");
        let result = classify(&argv, &config);
        assert!(result.accepted);
        assert_eq!(result.action, Some("test".to_string()));
    }
}
