//! Classifier module - Safety gate for xcodebuild commands
//!
//! The classifier is a deny-by-default gate that accepts/rejects xcodebuild
//! invocations based on an allowlist. It ensures only safe, supported commands
//! are routed to remote workers.

mod config;
mod explain;
mod invocation;
mod parser;
mod policy;
mod result;

pub use config::{BundleConfig, ConfigError, RepoConfig, VerifyAction};
pub use explain::{ConfigConstraints, EffectivePolicy, ExplainOutput, MatchedConstraintsOutput};
pub use invocation::Invocation;
pub use parser::parse_xcodebuild_argv;
pub use policy::{ClassifierPolicy, PolicyAllowlist, PolicyConstraints, PolicyDenylist};
pub use result::{ClassifierResult, MatchedConstraints, RejectionReason};

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

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
        sorted_flags.sort_by_key(|(a, _)| *a);

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

    /// Compute SHA-256-like hash of the classifier policy
    ///
    /// This includes all allowlist/denylist entries and config constraints
    /// to enable reproducibility audits. Uses DefaultHasher for simplicity
    /// (actual SHA-256 would require a crypto dependency).
    pub fn policy_hash(&self) -> String {
        let mut hasher = DefaultHasher::new();

        // Hash config
        self.config.workspace.hash(&mut hasher);
        self.config.project.hash(&mut hasher);
        self.config.scheme.hash(&mut hasher);
        self.config.destination.hash(&mut hasher);
        for config in &self.config.allowed_configurations {
            config.hash(&mut hasher);
        }

        // Hash allowlists (sorted for determinism)
        let mut actions: Vec<_> = self.allowed_actions.iter().collect();
        actions.sort();
        for action in actions {
            action.hash(&mut hasher);
        }

        let mut flags: Vec<_> = self.allowed_flags.iter().collect();
        flags.sort();
        for flag in flags {
            flag.hash(&mut hasher);
        }

        // Hash denylists (sorted for determinism)
        let mut denied: Vec<_> = self.denied_actions.iter().collect();
        denied.sort();
        for d in denied {
            d.hash(&mut hasher);
        }

        let mut denied_f: Vec<_> = self.denied_flags.iter().collect();
        denied_f.sort();
        for d in denied_f {
            d.hash(&mut hasher);
        }

        format!("{:016x}", hasher.finish())
    }

    /// Create an Invocation record from a successful classification
    ///
    /// Returns None if the classification was rejected
    pub fn create_invocation(
        &self,
        original_argv: &[String],
        result: &ClassifierResult,
    ) -> Option<Invocation> {
        if !result.accepted {
            return None;
        }

        Some(Invocation::new(
            original_argv.to_vec(),
            result.sanitized_argv.clone().unwrap_or_default(),
            result.action.clone().unwrap_or_default(),
            result.rejected_flags.clone(),
            self.policy_sha256(),
        ))
    }

    /// Generate the effective policy for explain output
    pub fn effective_policy(&self) -> EffectivePolicy {
        let mut allowed_actions: Vec<_> = self.allowed_actions.iter().cloned().collect();
        allowed_actions.sort();

        let mut denied_actions: Vec<_> = self.denied_actions.iter().cloned().collect();
        denied_actions.sort();

        let mut allowed_flags: Vec<_> = self.allowed_flags.iter().cloned().collect();
        allowed_flags.sort();

        let mut denied_flags: Vec<_> = self.denied_flags.iter().cloned().collect();
        denied_flags.sort();

        EffectivePolicy {
            allowed_actions,
            denied_actions,
            allowed_flags,
            denied_flags,
            config_constraints: ConfigConstraints {
                workspace: self.config.workspace.clone(),
                project: self.config.project.clone(),
                required_scheme: self.config.scheme.clone(),
                allowed_configurations: self.config.allowed_configurations.clone(),
            },
        }
    }

    /// Run classification and produce an explanation
    pub fn explain(&self, argv: &[String]) -> ExplainOutput {
        let result = self.classify(argv);
        ExplainOutput::from_result(argv.to_vec(), &result, self.effective_policy())
    }

    /// Create a classifier policy snapshot
    pub fn create_policy_snapshot(&self) -> ClassifierPolicy {
        let mut allowed_actions: Vec<_> = self.allowed_actions.iter().cloned().collect();
        allowed_actions.sort();

        let mut denied_actions: Vec<_> = self.denied_actions.iter().cloned().collect();
        denied_actions.sort();

        let mut allowed_flags: Vec<_> = self.allowed_flags.iter().cloned().collect();
        allowed_flags.sort();

        let mut denied_flags: Vec<_> = self.denied_flags.iter().cloned().collect();
        denied_flags.sort();

        ClassifierPolicy::new(
            PolicyAllowlist {
                actions: allowed_actions,
                flags: allowed_flags,
            },
            PolicyDenylist {
                actions: denied_actions,
                flags: denied_flags,
            },
            PolicyConstraints {
                workspace: self.config.workspace.clone(),
                project: self.config.project.clone(),
                scheme: self.config.scheme.clone(),
                destination: self.config.destination.clone(),
                allowed_configurations: self.config.allowed_configurations.clone(),
            },
        )
    }

    /// Get the SHA-256 hash of the classifier policy
    pub fn policy_sha256(&self) -> String {
        self.create_policy_snapshot()
            .sha256()
            .unwrap_or_else(|_| "error-computing-hash".to_string())
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

    #[test]
    fn test_policy_hash_deterministic() {
        let classifier1 = Classifier::new(test_config());
        let classifier2 = Classifier::new(test_config());

        // Same config should produce same hash
        assert_eq!(classifier1.policy_hash(), classifier2.policy_hash());
    }

    #[test]
    fn test_policy_hash_differs_with_config() {
        let classifier1 = Classifier::new(test_config());

        let mut config2 = test_config();
        config2.scheme = "DifferentScheme".to_string();
        let classifier2 = Classifier::new(config2);

        // Different config should produce different hash
        assert_ne!(classifier1.policy_hash(), classifier2.policy_hash());
    }

    #[test]
    fn test_create_invocation_accepted() {
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
        let invocation = classifier.create_invocation(&argv, &result);

        assert!(invocation.is_some());
        let inv = invocation.unwrap();
        assert_eq!(inv.original_argv, argv);
        assert_eq!(inv.accepted_action, "build");
        assert!(inv.rejected_flags.is_empty());
        assert!(!inv.classifier_policy_sha256.is_empty());
    }

    #[test]
    fn test_create_invocation_rejected() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec!["archive", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let result = classifier.classify(&argv);
        let invocation = classifier.create_invocation(&argv, &result);

        // Rejected classifications don't produce an invocation
        assert!(invocation.is_none());
    }

    #[test]
    fn test_invocation_to_json() {
        let classifier = Classifier::new(test_config());
        let argv: Vec<String> = vec![
            "test",
            "-workspace",
            "MyApp.xcworkspace",
            "-scheme",
            "MyApp",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let result = classifier.classify(&argv);
        let invocation = classifier.create_invocation(&argv, &result).unwrap();

        let json = invocation.to_json().unwrap();
        assert!(json.contains("\"original_argv\""));
        assert!(json.contains("\"sanitized_argv\""));
        assert!(json.contains("\"accepted_action\""));
        assert!(json.contains("\"test\""));
        assert!(json.contains("\"classifier_policy_sha256\""));
    }
}
