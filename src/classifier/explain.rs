//! Explain command output for the classifier
//!
//! Provides structured JSON and human-readable explanations of classifier
//! decisions for diagnostic purposes.

use serde::{Deserialize, Serialize};

use super::{ClassifierResult, RejectionReason};

/// Explanation output for the classifier decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainOutput {
    /// The input command that was classified
    pub input_argv: Vec<String>,

    /// Whether the command was accepted
    pub accepted: bool,

    /// The classified action (if parseable)
    pub action: Option<String>,

    /// The sanitized argv (only if accepted)
    pub sanitized_argv: Option<Vec<String>>,

    /// Machine-readable rejection reasons
    pub rejection_reasons: Vec<String>,

    /// Matched constraints from the input
    pub matched_constraints: MatchedConstraintsOutput,

    /// The effective policy used for classification
    pub effective_policy: EffectivePolicy,

    /// Human-readable explanation
    pub explanation: String,
}

/// Matched constraints output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedConstraintsOutput {
    pub workspace: Option<String>,
    pub project: Option<String>,
    pub scheme: Option<String>,
    pub destination: Option<String>,
    pub configuration: Option<String>,
}

/// The effective policy used for classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivePolicy {
    /// Allowed actions
    pub allowed_actions: Vec<String>,

    /// Explicitly denied actions
    pub denied_actions: Vec<String>,

    /// Allowed flags
    pub allowed_flags: Vec<String>,

    /// Explicitly denied flags
    pub denied_flags: Vec<String>,

    /// Config constraints
    pub config_constraints: ConfigConstraints,
}

/// Config constraints from repo config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigConstraints {
    pub workspace: Option<String>,
    pub project: Option<String>,
    pub required_scheme: String,
    pub allowed_configurations: Vec<String>,
}

impl ExplainOutput {
    /// Create an ExplainOutput from a ClassifierResult
    pub fn from_result(
        input_argv: Vec<String>,
        result: &ClassifierResult,
        policy: EffectivePolicy,
    ) -> Self {
        let explanation = Self::generate_explanation(result, &input_argv);

        let scheme = if result.matched_constraints.scheme.is_empty() {
            None
        } else {
            Some(result.matched_constraints.scheme.clone())
        };

        Self {
            input_argv,
            accepted: result.accepted,
            action: result.action.clone(),
            sanitized_argv: result.sanitized_argv.clone(),
            rejection_reasons: result.rejection_reason_strings(),
            matched_constraints: MatchedConstraintsOutput {
                workspace: result.matched_constraints.workspace.clone(),
                project: result.matched_constraints.project.clone(),
                scheme,
                destination: result.matched_constraints.destination.clone(),
                configuration: result.matched_constraints.configuration.clone(),
            },
            effective_policy: policy,
            explanation,
        }
    }

    /// Generate human-readable explanation
    fn generate_explanation(result: &ClassifierResult, argv: &[String]) -> String {
        let mut lines = Vec::new();

        lines.push(format!("Command: {}", argv.join(" ")));
        lines.push(String::new());

        if result.accepted {
            lines.push("Decision: ACCEPTED".to_string());
            lines.push(String::new());
            lines.push(format!("Action: {}", result.action.as_ref().unwrap_or(&"?".to_string())));

            if let Some(ref sanitized) = result.sanitized_argv {
                lines.push(format!("Sanitized form: {}", sanitized.join(" ")));
            }
        } else {
            lines.push("Decision: REJECTED".to_string());
            lines.push(String::new());

            if !result.rejection_reasons.is_empty() {
                lines.push("Reasons:".to_string());
                for reason in &result.rejection_reasons {
                    lines.push(format!("  - {}", Self::format_reason(reason)));
                }
            }
        }

        lines.join("\n")
    }

    /// Format a rejection reason for human reading
    fn format_reason(reason: &RejectionReason) -> String {
        match reason {
            RejectionReason::ParseError(e) => format!("Parse error: {}", e),
            RejectionReason::DeniedAction(a) => {
                format!("Action '{}' is explicitly denied", a)
            }
            RejectionReason::UnknownAction(a) => {
                format!("Action '{}' is not in the allowlist", a)
            }
            RejectionReason::MissingAction => "No action specified".to_string(),
            RejectionReason::DeniedFlag(f) => {
                format!("Flag '{}' is explicitly denied", f)
            }
            RejectionReason::UnknownFlag(f) => {
                format!("Flag '{}' is not in the allowlist", f)
            }
            RejectionReason::MissingRequiredFlag(f) => {
                format!("Required flag '{}' is missing", f)
            }
            RejectionReason::WorkspaceMismatch { expected, actual } => {
                format!(
                    "Workspace mismatch: expected '{}', got '{}'",
                    expected, actual
                )
            }
            RejectionReason::ProjectMismatch { expected, actual } => {
                format!("Project mismatch: expected '{}', got '{}'", expected, actual)
            }
            RejectionReason::SchemeMismatch { expected, actual } => {
                format!("Scheme mismatch: expected '{}', got '{}'", expected, actual)
            }
            RejectionReason::ConfigurationNotAllowed(c) => {
                format!("Configuration '{}' is not in the allowed list", c)
            }
            RejectionReason::DestinationMismatch { expected, actual } => {
                format!(
                    "Destination mismatch: expected '{}', got '{}'",
                    expected, actual
                )
            }
        }
    }

    /// Format as JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Format as human-readable text
    pub fn to_human(&self) -> String {
        let mut output = self.explanation.clone();
        output.push_str("\n\n--- Effective Policy ---\n");

        output.push_str(&format!(
            "Allowed actions: {}\n",
            self.effective_policy.allowed_actions.join(", ")
        ));
        output.push_str(&format!(
            "Denied actions: {}\n",
            self.effective_policy.denied_actions.join(", ")
        ));
        output.push_str(&format!(
            "Allowed flags: {}\n",
            self.effective_policy.allowed_flags.join(", ")
        ));
        output.push_str(&format!(
            "Denied flags: {}\n",
            self.effective_policy.denied_flags.join(", ")
        ));

        output.push_str("\n--- Config Constraints ---\n");
        let cc = &self.effective_policy.config_constraints;
        if let Some(ref ws) = cc.workspace {
            output.push_str(&format!("Workspace: {}\n", ws));
        }
        if let Some(ref proj) = cc.project {
            output.push_str(&format!("Project: {}\n", proj));
        }
        output.push_str(&format!("Required scheme: {}\n", cc.required_scheme));
        if !cc.allowed_configurations.is_empty() {
            output.push_str(&format!(
                "Allowed configurations: {}\n",
                cc.allowed_configurations.join(", ")
            ));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Classifier, ClassifierConfig};

    fn test_policy() -> EffectivePolicy {
        EffectivePolicy {
            allowed_actions: vec!["build".to_string(), "test".to_string()],
            denied_actions: vec!["archive".to_string(), "clean".to_string()],
            allowed_flags: vec![
                "-workspace".to_string(),
                "-project".to_string(),
                "-scheme".to_string(),
            ],
            denied_flags: vec!["-exportArchive".to_string()],
            config_constraints: ConfigConstraints {
                workspace: Some("MyApp.xcworkspace".to_string()),
                project: None,
                required_scheme: "MyApp".to_string(),
                allowed_configurations: vec!["Debug".to_string(), "Release".to_string()],
            },
        }
    }

    #[test]
    fn test_explain_accepted() {
        let config = ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            project: None,
            scheme: "MyApp".to_string(),
            destination: None,
            allowed_configurations: vec![],
        };
        let classifier = Classifier::new(config);

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
        let explain = ExplainOutput::from_result(argv.clone(), &result, test_policy());

        assert!(explain.accepted);
        assert_eq!(explain.action, Some("build".to_string()));
        assert!(explain.sanitized_argv.is_some());
        assert!(explain.rejection_reasons.is_empty());
    }

    #[test]
    fn test_explain_rejected() {
        let config = ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            project: None,
            scheme: "MyApp".to_string(),
            destination: None,
            allowed_configurations: vec![],
        };
        let classifier = Classifier::new(config);

        let argv: Vec<String> = vec!["archive", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let result = classifier.classify(&argv);
        let explain = ExplainOutput::from_result(argv.clone(), &result, test_policy());

        assert!(!explain.accepted);
        assert!(!explain.rejection_reasons.is_empty());
        assert!(explain.explanation.contains("REJECTED"));
    }

    #[test]
    fn test_explain_to_json() {
        let config = ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            project: None,
            scheme: "MyApp".to_string(),
            destination: None,
            allowed_configurations: vec![],
        };
        let classifier = Classifier::new(config);

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
        let explain = ExplainOutput::from_result(argv, &result, test_policy());

        let json = explain.to_json().unwrap();
        assert!(json.contains("\"accepted\": true"));
        assert!(json.contains("\"action\": \"build\""));
    }

    #[test]
    fn test_explain_to_human() {
        let config = ClassifierConfig {
            workspace: Some("MyApp.xcworkspace".to_string()),
            project: None,
            scheme: "MyApp".to_string(),
            destination: None,
            allowed_configurations: vec![],
        };
        let classifier = Classifier::new(config);

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
        let explain = ExplainOutput::from_result(argv, &result, test_policy());

        let human = explain.to_human();
        assert!(human.contains("Decision: ACCEPTED"));
        assert!(human.contains("Effective Policy"));
        assert!(human.contains("Allowed actions"));
    }
}
