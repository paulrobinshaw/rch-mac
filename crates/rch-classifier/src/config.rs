//! Classifier configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Configuration for the classifier, derived from repo config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Required workspace (must match if specified).
    #[serde(default)]
    pub workspace: Option<String>,

    /// Required project (must match if specified).
    #[serde(default)]
    pub project: Option<String>,

    /// Allowed schemes (empty means any scheme is allowed).
    #[serde(default)]
    pub allowed_schemes: HashSet<String>,

    /// Allowed destination (must match if specified).
    #[serde(default)]
    pub destination: Option<String>,

    /// Allowed configurations (empty means any configuration is allowed).
    #[serde(default)]
    pub allowed_configurations: HashSet<String>,
}

/// Constraints that were matched during classification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatchedConstraints {
    /// Matched workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,

    /// Matched project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,

    /// Matched scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    /// Matched destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,

    /// Matched configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClassifierConfig::default();
        assert!(config.workspace.is_none());
        assert!(config.project.is_none());
        assert!(config.allowed_schemes.is_empty());
        assert!(config.destination.is_none());
        assert!(config.allowed_configurations.is_empty());
    }

    #[test]
    fn test_matched_constraints_default() {
        let constraints = MatchedConstraints::default();
        assert!(constraints.workspace.is_none());
        assert!(constraints.scheme.is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = ClassifierConfig {
            workspace: Some("Test.xcworkspace".to_string()),
            allowed_schemes: vec!["Scheme1".to_string(), "Scheme2".to_string()]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ClassifierConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspace, config.workspace);
        assert!(parsed.allowed_schemes.contains("Scheme1"));
    }
}
