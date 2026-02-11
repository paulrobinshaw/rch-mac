//! Classifier result types
//!
//! Defines the ClassifierResult structure returned by the classifier,
//! along with rejection reasons and matched constraints.

use serde::{Deserialize, Serialize};

/// Machine-readable rejection reasons
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "details")]
pub enum RejectionReason {
    /// Failed to parse the argv
    ParseError(String),

    /// Action is explicitly denied (e.g., "archive")
    DeniedAction(String),

    /// Action is not in the allowlist
    UnknownAction(String),

    /// No action was specified
    MissingAction,

    /// Flag is explicitly denied (e.g., "-exportArchive")
    DeniedFlag(String),

    /// Flag is not in the allowlist
    UnknownFlag(String),

    /// Required flag is missing
    MissingRequiredFlag(String),

    /// Workspace doesn't match config
    WorkspaceMismatch { expected: String, actual: String },

    /// Project doesn't match config
    ProjectMismatch { expected: String, actual: String },

    /// Scheme doesn't match config
    SchemeMismatch { expected: String, actual: String },

    /// Configuration is not in the allowed list
    ConfigurationNotAllowed(String),

    /// Destination doesn't match pinned destination
    DestinationMismatch { expected: String, actual: String },
}

impl RejectionReason {
    /// Convert to machine-readable string format
    /// Example: "DENIED_ACTION:archive", "SCHEME_MISMATCH:BadScheme"
    pub fn to_machine_string(&self) -> String {
        match self {
            RejectionReason::ParseError(e) => format!("PARSE_ERROR:{}", e),
            RejectionReason::DeniedAction(a) => format!("DENIED_ACTION:{}", a),
            RejectionReason::UnknownAction(a) => format!("UNKNOWN_ACTION:{}", a),
            RejectionReason::MissingAction => "MISSING_ACTION".to_string(),
            RejectionReason::DeniedFlag(f) => format!("DENIED_FLAG:{}", f),
            RejectionReason::UnknownFlag(f) => format!("UNKNOWN_FLAG:{}", f),
            RejectionReason::MissingRequiredFlag(f) => format!("MISSING_REQUIRED_FLAG:{}", f),
            RejectionReason::WorkspaceMismatch { actual, .. } => {
                format!("WORKSPACE_MISMATCH:{}", actual)
            }
            RejectionReason::ProjectMismatch { actual, .. } => {
                format!("PROJECT_MISMATCH:{}", actual)
            }
            RejectionReason::SchemeMismatch { actual, .. } => format!("SCHEME_MISMATCH:{}", actual),
            RejectionReason::ConfigurationNotAllowed(c) => {
                format!("CONFIGURATION_NOT_ALLOWED:{}", c)
            }
            RejectionReason::DestinationMismatch { actual, .. } => {
                format!("DESTINATION_MISMATCH:{}", actual)
            }
        }
    }
}

/// What config constraints were matched during classification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchedConstraints {
    /// Workspace from argv (if specified)
    pub workspace: Option<String>,
    /// Project from argv (if specified)
    pub project: Option<String>,
    /// Scheme from argv
    pub scheme: String,
    /// Destination from argv (if specified)
    pub destination: Option<String>,
    /// Configuration from argv (if specified)
    pub configuration: Option<String>,
}

/// Result of classifying an xcodebuild invocation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifierResult {
    /// Whether the invocation was accepted
    pub accepted: bool,

    /// The action (build, test) - None when rejected
    pub action: Option<String>,

    /// Sanitized argv with canonical ordering - None when rejected
    pub sanitized_argv: Option<Vec<String>>,

    /// Flags that caused rejection (may be empty)
    pub rejected_flags: Vec<String>,

    /// Machine-readable rejection reasons
    pub rejection_reasons: Vec<RejectionReason>,

    /// What config constraints were applied
    pub matched_constraints: MatchedConstraints,
}

impl ClassifierResult {
    /// Create a rejected result with the given reasons
    pub fn rejected(reasons: Vec<RejectionReason>) -> Self {
        Self {
            accepted: false,
            action: None,
            sanitized_argv: None,
            rejected_flags: vec![],
            rejection_reasons: reasons,
            matched_constraints: MatchedConstraints::default(),
        }
    }

    /// Get rejection reasons as machine-readable strings
    pub fn rejection_reason_strings(&self) -> Vec<String> {
        self.rejection_reasons
            .iter()
            .map(|r| r.to_machine_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejection_reason_machine_string() {
        assert_eq!(
            RejectionReason::DeniedAction("archive".to_string()).to_machine_string(),
            "DENIED_ACTION:archive"
        );

        assert_eq!(
            RejectionReason::SchemeMismatch {
                expected: "MyApp".to_string(),
                actual: "BadScheme".to_string()
            }
            .to_machine_string(),
            "SCHEME_MISMATCH:BadScheme"
        );
    }

    #[test]
    fn test_classifier_result_serialization() {
        let result = ClassifierResult {
            accepted: true,
            action: Some("build".to_string()),
            sanitized_argv: Some(vec!["build".to_string(), "-scheme".to_string()]),
            rejected_flags: vec![],
            rejection_reasons: vec![],
            matched_constraints: MatchedConstraints {
                workspace: Some("MyApp.xcworkspace".to_string()),
                project: None,
                scheme: "MyApp".to_string(),
                destination: None,
                configuration: None,
            },
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"accepted\":true"));
        assert!(json.contains("\"action\":\"build\""));
    }
}
