//! Classifier result types.

use crate::config::MatchedConstraints;
use serde::{Deserialize, Serialize};

/// Machine-readable rejection reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "detail")]
pub enum RejectionReason {
    /// Parse error in argv.
    #[serde(rename = "PARSE_ERROR")]
    ParseError(String),

    /// Action is explicitly denied.
    #[serde(rename = "DENIED_ACTION")]
    DeniedAction(String),

    /// Action is not in allowed set.
    #[serde(rename = "UNKNOWN_ACTION")]
    UnknownAction(String),

    /// Flag is explicitly denied.
    #[serde(rename = "DENIED_FLAG")]
    DeniedFlag(String),

    /// Flag is not in allowed set.
    #[serde(rename = "UNKNOWN_FLAG")]
    UnknownFlag(String),

    /// Workspace doesn't match config.
    #[serde(rename = "WORKSPACE_MISMATCH")]
    WorkspaceMismatch { got: String, expected: String },

    /// Project doesn't match config.
    #[serde(rename = "PROJECT_MISMATCH")]
    ProjectMismatch { got: String, expected: String },

    /// Scheme not in allowed set.
    #[serde(rename = "SCHEME_MISMATCH")]
    SchemeMismatch(String),

    /// Destination doesn't match config.
    #[serde(rename = "DESTINATION_MISMATCH")]
    DestinationMismatch { got: String, expected: String },

    /// Configuration not in allowed set.
    #[serde(rename = "CONFIGURATION_MISMATCH")]
    ConfigurationMismatch(String),

    /// Required flag is missing.
    #[serde(rename = "MISSING_REQUIRED_FLAG")]
    MissingRequiredFlag(String),
}

impl RejectionReason {
    /// Get a machine-readable string representation.
    pub fn to_code(&self) -> String {
        match self {
            RejectionReason::ParseError(e) => format!("PARSE_ERROR:{}", e),
            RejectionReason::DeniedAction(a) => format!("DENIED_ACTION:{}", a),
            RejectionReason::UnknownAction(a) => format!("UNKNOWN_ACTION:{}", a),
            RejectionReason::DeniedFlag(f) => format!("DENIED_FLAG:{}", f),
            RejectionReason::UnknownFlag(f) => format!("UNKNOWN_FLAG:{}", f),
            RejectionReason::WorkspaceMismatch { got, expected } => {
                format!("WORKSPACE_MISMATCH:{}!={}", got, expected)
            }
            RejectionReason::ProjectMismatch { got, expected } => {
                format!("PROJECT_MISMATCH:{}!={}", got, expected)
            }
            RejectionReason::SchemeMismatch(s) => format!("SCHEME_MISMATCH:{}", s),
            RejectionReason::DestinationMismatch { got, expected } => {
                format!("DESTINATION_MISMATCH:{}!={}", got, expected)
            }
            RejectionReason::ConfigurationMismatch(c) => format!("CONFIGURATION_MISMATCH:{}", c),
            RejectionReason::MissingRequiredFlag(f) => format!("MISSING_REQUIRED_FLAG:{}", f),
        }
    }
}

/// Result of classifying an xcodebuild invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierResult {
    /// Whether the invocation was accepted.
    pub accepted: bool,

    /// The accepted action ("build" or "test"). None when rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    /// Sanitized argv in canonical order. None when rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sanitized_argv: Option<Vec<String>>,

    /// Flags that caused rejection (may be empty).
    #[serde(default)]
    pub rejected_flags: Vec<String>,

    /// Machine-readable rejection reasons.
    #[serde(default)]
    pub rejection_reasons: Vec<RejectionReason>,

    /// Constraints that were matched.
    pub matched_constraints: MatchedConstraints,
}

impl ClassifierResult {
    /// Create an accepted result.
    pub fn accepted(
        action: String,
        sanitized_argv: Vec<String>,
        matched_constraints: MatchedConstraints,
    ) -> Self {
        Self {
            accepted: true,
            action: Some(action),
            sanitized_argv: Some(sanitized_argv),
            rejected_flags: Vec::new(),
            rejection_reasons: Vec::new(),
            matched_constraints,
        }
    }

    /// Create a rejected result.
    pub fn rejected(rejected_flags: Vec<String>, rejection_reasons: Vec<RejectionReason>) -> Self {
        Self {
            accepted: false,
            action: None,
            sanitized_argv: None,
            rejected_flags,
            rejection_reasons,
            matched_constraints: MatchedConstraints::default(),
        }
    }

    /// Get rejection reasons as machine-readable strings.
    pub fn rejection_reason_codes(&self) -> Vec<String> {
        self.rejection_reasons.iter().map(|r| r.to_code()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accepted_result() {
        let result = ClassifierResult::accepted(
            "build".to_string(),
            vec!["build".to_string(), "-scheme".to_string(), "Foo".to_string()],
            MatchedConstraints {
                scheme: Some("Foo".to_string()),
                ..Default::default()
            },
        );
        assert!(result.accepted);
        assert_eq!(result.action, Some("build".to_string()));
        assert!(result.rejection_reasons.is_empty());
    }

    #[test]
    fn test_rejected_result() {
        let result = ClassifierResult::rejected(
            vec!["-badFlag".to_string()],
            vec![RejectionReason::UnknownFlag("-badFlag".to_string())],
        );
        assert!(!result.accepted);
        assert!(result.action.is_none());
        assert!(result.sanitized_argv.is_none());
        assert_eq!(result.rejected_flags, vec!["-badFlag".to_string()]);
    }

    #[test]
    fn test_rejection_reason_codes() {
        let result = ClassifierResult::rejected(
            vec!["-foo".to_string()],
            vec![
                RejectionReason::UnknownFlag("-foo".to_string()),
                RejectionReason::SchemeMismatch("BadScheme".to_string()),
            ],
        );
        let codes = result.rejection_reason_codes();
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0], "UNKNOWN_FLAG:-foo");
        assert_eq!(codes[1], "SCHEME_MISMATCH:BadScheme");
    }

    #[test]
    fn test_serialization() {
        let result = ClassifierResult::accepted(
            "test".to_string(),
            vec!["test".to_string(), "-scheme".to_string(), "Tests".to_string()],
            MatchedConstraints {
                scheme: Some("Tests".to_string()),
                ..Default::default()
            },
        );
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"accepted\": true"));
        assert!(json.contains("\"action\": \"test\""));
    }

    #[test]
    fn test_rejection_serialization() {
        let result = ClassifierResult::rejected(
            vec![],
            vec![RejectionReason::DeniedAction("archive".to_string())],
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("DENIED_ACTION"));
    }
}
