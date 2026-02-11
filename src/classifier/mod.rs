//! Classifier module - Safety gate for xcodebuild commands
//!
//! The classifier is a deny-by-default gate that accepts/rejects xcodebuild
//! invocations based on an allowlist. It ensures only safe, supported commands
//! are routed to remote workers.

mod parser;
mod result;

pub use parser::parse_xcodebuild_argv;
pub use result::{ClassifierResult, MatchedConstraints, RejectionReason};

use std::collections::HashSet;

/// Configuration for the classifier, typically from `.rch/xcode.toml`
#[derive(Debug, Clone, Default)]
pub struct ClassifierConfig {
    /// Workspace path (e.g., "MyApp.xcworkspace")
    pub workspace: Option<String>,
    /// Project path (e.g., "MyApp.xcodeproj") - mutually exclusive with workspace
    pub project: Option<String>,
    /// Required scheme name
    pub scheme: String,
    /// Pinned/resolved destination string
    pub destination: Option<String>,
    /// Allowed configurations (if empty, any is allowed)
    pub allowed_configurations: Vec<String>,
}

/// The classifier - determines if an xcodebuild invocation is safe to execute remotely
pub struct Classifier {
    config: ClassifierConfig,
    allowed_actions: HashSet<String>,
    allowed_flags: HashSet<String>,
    denied_actions: HashSet<String>,
    denied_flags: HashSet<String>,
}

impl Classifier {
    /// Create a new classifier with the given configuration
    pub fn new(config: ClassifierConfig) -> Self {
        let mut allowed_actions = HashSet::new();
        allowed_actions.insert("build".to_string());
        allowed_actions.insert("test".to_string());

        let mut allowed_flags = HashSet::new();
        // Minimal safe subset per PLAN.md
        allowed_flags.insert("-workspace".to_string());
        allowed_flags.insert("-project".to_string());
        allowed_flags.insert("-scheme".to_string());
        allowed_flags.insert("-destination".to_string());
        allowed_flags.insert("-configuration".to_string());
        allowed_flags.insert("-sdk".to_string());
        allowed_flags.insert("-quiet".to_string());

        let mut denied_actions = HashSet::new();
        denied_actions.insert("archive".to_string());
        denied_actions.insert("clean".to_string()); // Potentially destructive

        let mut denied_flags = HashSet::new();
        // Explicitly denied per PLAN.md
        denied_flags.insert("-exportArchive".to_string());
        denied_flags.insert("-exportNotarizedApp".to_string());
        denied_flags.insert("-resultBundlePath".to_string()); // Worker controls output paths
        denied_flags.insert("-derivedDataPath".to_string()); // Worker controls per cache mode
        denied_flags.insert("-archivePath".to_string());
        denied_flags.insert("-exportPath".to_string());
        denied_flags.insert("-exportOptionsPlist".to_string());

        Self {
            config,
            allowed_actions,
            allowed_flags,
            denied_actions,
            denied_flags,
        }
    }

    /// Classify an xcodebuild invocation
    ///
    /// This is a pure function: (argv, config) -> ClassifierResult
    pub fn classify(&self, argv: &[String]) -> ClassifierResult {
        let parsed = match parse_xcodebuild_argv(argv) {
            Ok(p) => p,
            Err(e) => {
                return ClassifierResult::rejected(vec![RejectionReason::ParseError(e)]);
            }
        };

        let mut rejection_reasons = Vec::new();
        let mut rejected_flags = Vec::new();

        // Check action
        let action = match &parsed.action {
            Some(action) => {
                if self.denied_actions.contains(action) {
                    rejection_reasons.push(RejectionReason::DeniedAction(action.clone()));
                    None
                } else if !self.allowed_actions.contains(action) {
                    rejection_reasons.push(RejectionReason::UnknownAction(action.clone()));
                    None
                } else {
                    Some(action.clone())
                }
            }
            None => {
                rejection_reasons.push(RejectionReason::MissingAction);
                None
            }
        };

        // Check flags
        for (flag, value) in &parsed.flags {
            // Check if explicitly denied
            if self.denied_flags.contains(flag) {
                rejection_reasons.push(RejectionReason::DeniedFlag(flag.clone()));
                rejected_flags.push(flag.clone());
                continue;
            }

            // Check if in allowlist
            if !self.allowed_flags.contains(flag) {
                rejection_reasons.push(RejectionReason::UnknownFlag(flag.clone()));
                rejected_flags.push(flag.clone());
                continue;
            }

            // Validate flag values against config constraints
            if let Some(value) = value {
                match flag.as_str() {
                    "-workspace" => {
                        if let Some(expected) = &self.config.workspace {
                            if value != expected {
                                rejection_reasons.push(RejectionReason::WorkspaceMismatch {
                                    expected: expected.clone(),
                                    actual: value.clone(),
                                });
                                rejected_flags.push(flag.clone());
                            }
                        }
                    }
                    "-project" => {
                        if let Some(expected) = &self.config.project {
                            if value != expected {
                                rejection_reasons.push(RejectionReason::ProjectMismatch {
                                    expected: expected.clone(),
                                    actual: value.clone(),
                                });
                                rejected_flags.push(flag.clone());
                            }
                        }
                    }
                    "-scheme" => {
                        if value != &self.config.scheme {
                            rejection_reasons.push(RejectionReason::SchemeMismatch {
                                expected: self.config.scheme.clone(),
                                actual: value.clone(),
                            });
                            rejected_flags.push(flag.clone());
                        }
                    }
                    "-configuration" => {
                        if !self.config.allowed_configurations.is_empty()
                            && !self.config.allowed_configurations.contains(value)
                        {
                            rejection_reasons.push(RejectionReason::ConfigurationNotAllowed(
                                value.clone(),
                            ));
                            rejected_flags.push(flag.clone());
                        }
                    }
                    _ => {}
                }
            }
        }

        // Check required constraints
        if self.config.workspace.is_some() && !parsed.flags.contains_key("-workspace") {
            rejection_reasons.push(RejectionReason::MissingRequiredFlag("-workspace".to_string()));
        }
        if self.config.project.is_some() && !parsed.flags.contains_key("-project") {
            rejection_reasons.push(RejectionReason::MissingRequiredFlag("-project".to_string()));
        }
        if !parsed.flags.contains_key("-scheme") {
            rejection_reasons.push(RejectionReason::MissingRequiredFlag("-scheme".to_string()));
        }

        if !rejection_reasons.is_empty() {
            return ClassifierResult {
                accepted: false,
                action: None,
                sanitized_argv: None,
                rejected_flags,
                rejection_reasons,
                matched_constraints: self.extract_matched_constraints(&parsed),
            };
        }

        // Build sanitized argv with canonical ordering
        let sanitized_argv = self.build_sanitized_argv(action.as_ref().unwrap(), &parsed);

        ClassifierResult {
            accepted: true,
            action,
            sanitized_argv: Some(sanitized_argv),
            rejected_flags: vec![],
            rejection_reasons: vec![],
            matched_constraints: self.extract_matched_constraints(&parsed),
        }
    }

    /// Build sanitized argv with canonical ordering:
    /// action first, then flags sorted lexicographically by flag name
    fn build_sanitized_argv(
        &self,
        action: &str,
        parsed: &parser::ParsedXcodebuildArgs,
    ) -> Vec<String> {
        let mut result = vec![action.to_string()];

        // Collect and sort flags
        let mut sorted_flags: Vec<_> = parsed.flags.iter().collect();
        sorted_flags.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (flag, value) in sorted_flags {
            result.push(flag.clone());
            if let Some(v) = value {
                result.push(v.clone());
            }
        }

        result
    }

    /// Extract matched constraints from parsed args
    fn extract_matched_constraints(
        &self,
        parsed: &parser::ParsedXcodebuildArgs,
    ) -> MatchedConstraints {
        MatchedConstraints {
            workspace: parsed.flags.get("-workspace").and_then(|v| v.clone()),
            project: parsed.flags.get("-project").and_then(|v| v.clone()),
            scheme: parsed
                .flags
                .get("-scheme")
                .and_then(|v| v.clone())
                .unwrap_or_default(),
            destination: parsed.flags.get("-destination").and_then(|v| v.clone()),
            configuration: parsed.flags.get("-configuration").and_then(|v| v.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ClassifierConfig {
        ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            project: None,
            scheme: "MyApp".to_string(),
            destination: None,
            allowed_configurations: vec!["Debug".to_string(), "Release".to_string()],
        }
    }

    #[test]
    fn test_accept_valid_build() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "build",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "MyApp",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(result.accepted);
        assert_eq!(result.action, Some("build".to_string()));
    }

    #[test]
    fn test_accept_valid_test() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "test",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "MyApp",
            "-destination",
            "platform=iOS Simulator,name=iPhone 16",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(result.accepted);
        assert_eq!(result.action, Some("test".to_string()));
    }

    #[test]
    fn test_reject_archive_action() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "archive",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "MyApp",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::DeniedAction(_))));
    }

    #[test]
    fn test_reject_unknown_flag() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "build",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "MyApp",
            "-unknownFlag",
            "value",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::UnknownFlag(_))));
    }

    #[test]
    fn test_reject_scheme_mismatch() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "build",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "WrongScheme",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(!result.accepted);
        assert!(result
            .rejection_reasons
            .iter()
            .any(|r| matches!(r, RejectionReason::SchemeMismatch { .. })));
    }

    #[test]
    fn test_sanitized_argv_ordering() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "build",
            "-scheme",
            "MyApp",
            "-workspace",
            "MyApp.xcworkspace",
            "-configuration",
            "Debug",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        assert!(result.accepted);

        let sanitized = result.sanitized_argv.unwrap();
        // Action first, then flags sorted alphabetically
        assert_eq!(sanitized[0], "build");
        assert_eq!(sanitized[1], "-configuration");
        assert_eq!(sanitized[2], "Debug");
        assert_eq!(sanitized[3], "-scheme");
        assert_eq!(sanitized[4], "MyApp");
        assert_eq!(sanitized[5], "-workspace");
        assert_eq!(sanitized[6], "MyApp.xcworkspace");
    }
}
