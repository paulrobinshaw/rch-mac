//! Repo configuration for classifier constraints (.rch/xcode.toml)
//!
//! Defines the configuration format and parsing for project-level
//! classifier constraints. This is layer 3 in the merge precedence:
//! built-in defaults → host config → repo config → CLI flags.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

/// Error types for config operations
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] io::Error),

    #[error("Failed to parse TOML: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

/// Verify action configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyAction {
    /// Action to perform: "build" or "test"
    pub action: String,

    /// Additional arguments for this action
    #[serde(default)]
    pub args: Vec<String>,

    /// Scheme override for this action (optional)
    pub scheme: Option<String>,

    /// Configuration override for this action (optional)
    pub configuration: Option<String>,
}

/// Bundle configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleConfig {
    /// Maximum bundle size in bytes (0 = no limit)
    /// If set, bundles exceeding this size will be rejected before upload
    #[serde(default)]
    pub max_bytes: u64,
}

/// Repository configuration from .rch/xcode.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct RepoConfig {
    /// Workspace path (e.g., "MyApp.xcworkspace")
    /// Mutually exclusive with project
    pub workspace: Option<String>,

    /// Project path (e.g., "MyApp.xcodeproj")
    /// Mutually exclusive with workspace
    pub project: Option<String>,

    /// Allowed schemes (at least one required)
    #[serde(default)]
    pub schemes: Vec<String>,

    /// Allowed destinations (constraint strings)
    /// e.g., ["platform=iOS Simulator,name=iPhone 16,OS=18.0"]
    #[serde(default)]
    pub destinations: Vec<String>,

    /// Allowed configurations (optional)
    /// If empty, any configuration is allowed
    #[serde(default)]
    pub configurations: Vec<String>,

    /// Verify actions for `rch xcode verify`
    /// Ordered array of actions to run for verification
    #[serde(default)]
    pub verify: Vec<VerifyAction>,

    /// Bundle configuration
    #[serde(default)]
    pub bundle: BundleConfig,
}


impl RepoConfig {
    /// Load and parse config from a TOML file
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        Self::from_str(&contents)
    }

    /// Parse config from a TOML string
    pub fn from_str(s: &str) -> Result<Self, ConfigError> {
        let config: RepoConfig = toml::from_str(s)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Rule: Cannot have both workspace and project
        if self.workspace.is_some() && self.project.is_some() {
            return Err(ConfigError::ValidationError(
                "Cannot specify both 'workspace' and 'project'".to_string(),
            ));
        }

        // Rule: Must have at least one scheme
        if self.schemes.is_empty() {
            return Err(ConfigError::ValidationError(
                "At least one scheme must be defined in 'schemes'".to_string(),
            ));
        }

        // Rule: Validate destination syntax
        for dest in &self.destinations {
            Self::validate_destination(dest)?;
        }

        // Rule: Validate verify actions
        for action in &self.verify {
            if action.action != "build" && action.action != "test" {
                return Err(ConfigError::ValidationError(format!(
                    "Invalid verify action '{}': must be 'build' or 'test'",
                    action.action
                )));
            }
        }

        Ok(())
    }

    /// Validate destination constraint syntax
    fn validate_destination(dest: &str) -> Result<(), ConfigError> {
        // Destination format: key=value pairs separated by commas
        // e.g., "platform=iOS Simulator,name=iPhone 16,OS=18.0"
        if dest.is_empty() {
            return Err(ConfigError::ValidationError(
                "Destination constraint cannot be empty".to_string(),
            ));
        }

        // Must contain at least one key=value pair
        let pairs: Vec<&str> = dest.split(',').collect();
        for pair in pairs {
            let kv: Vec<&str> = pair.splitn(2, '=').collect();
            if kv.len() != 2 {
                return Err(ConfigError::ValidationError(format!(
                    "Invalid destination syntax: '{}' (expected key=value format)",
                    pair.trim()
                )));
            }
            let key = kv[0].trim();
            if key.is_empty() {
                return Err(ConfigError::ValidationError(
                    "Destination key cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Convert to ClassifierConfig, using the first scheme as default
    pub fn to_classifier_config(&self) -> super::ClassifierConfig {
        super::ClassifierConfig {
            workspace: self.workspace.clone(),
            project: self.project.clone(),
            scheme: self.schemes.first().cloned().unwrap_or_default(),
            destination: self.destinations.first().cloned(),
            allowed_configurations: self.configurations.clone(),
        }
    }

    /// Convert to ClassifierConfig with a specific scheme
    pub fn to_classifier_config_for_scheme(&self, scheme: &str) -> Option<super::ClassifierConfig> {
        if !self.schemes.contains(&scheme.to_string()) {
            return None;
        }

        Some(super::ClassifierConfig {
            workspace: self.workspace.clone(),
            project: self.project.clone(),
            scheme: scheme.to_string(),
            destination: self.destinations.first().cloned(),
            allowed_configurations: self.configurations.clone(),
        })
    }

    /// Check if a scheme is allowed
    pub fn is_scheme_allowed(&self, scheme: &str) -> bool {
        self.schemes.contains(&scheme.to_string())
    }

    /// Check if a destination matches any allowed destination
    pub fn is_destination_allowed(&self, dest: &str) -> bool {
        // Empty destinations list means all destinations allowed
        if self.destinations.is_empty() {
            return true;
        }
        self.destinations.iter().any(|d| d == dest)
    }

    /// Check if a configuration is allowed
    pub fn is_configuration_allowed(&self, config: &str) -> bool {
        // Empty configurations list means all configurations allowed
        if self.configurations.is_empty() {
            return true;
        }
        self.configurations.contains(&config.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert_eq!(config.workspace, Some("MyApp.xcworkspace".to_string()));
        assert_eq!(config.schemes, vec!["MyApp"]);
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp", "MyAppTests"]
            destinations = ["platform=iOS Simulator,name=iPhone 16,OS=18.0"]
            configurations = ["Debug", "Release"]

            [[verify]]
            action = "build"

            [[verify]]
            action = "test"
            scheme = "MyAppTests"
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert_eq!(config.schemes.len(), 2);
        assert_eq!(config.destinations.len(), 1);
        assert_eq!(config.configurations.len(), 2);
        assert_eq!(config.verify.len(), 2);
        assert_eq!(config.verify[0].action, "build");
        assert_eq!(config.verify[1].action, "test");
        assert_eq!(config.verify[1].scheme, Some("MyAppTests".to_string()));
    }

    #[test]
    fn test_reject_both_workspace_and_project() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            project = "MyApp.xcodeproj"
            schemes = ["MyApp"]
        "#;

        let result = RepoConfig::from_str(toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Cannot specify both"));
    }

    #[test]
    fn test_reject_no_schemes() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
        "#;

        let result = RepoConfig::from_str(toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("At least one scheme"));
    }

    #[test]
    fn test_reject_invalid_destination_syntax() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
            destinations = ["invalid-no-equals"]
        "#;

        let result = RepoConfig::from_str(toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid destination syntax"));
    }

    #[test]
    fn test_reject_invalid_verify_action() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]

            [[verify]]
            action = "archive"
        "#;

        let result = RepoConfig::from_str(toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid verify action"));
    }

    #[test]
    fn test_to_classifier_config() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp", "MyAppTests"]
            configurations = ["Debug", "Release"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        let classifier_config = config.to_classifier_config();

        assert_eq!(classifier_config.workspace, Some("MyApp.xcworkspace".to_string()));
        assert_eq!(classifier_config.scheme, "MyApp"); // First scheme
        assert_eq!(classifier_config.allowed_configurations, vec!["Debug", "Release"]);
    }

    #[test]
    fn test_to_classifier_config_for_scheme() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp", "MyAppTests"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();

        // Valid scheme
        let cc = config.to_classifier_config_for_scheme("MyAppTests");
        assert!(cc.is_some());
        assert_eq!(cc.unwrap().scheme, "MyAppTests");

        // Invalid scheme
        let cc = config.to_classifier_config_for_scheme("UnknownScheme");
        assert!(cc.is_none());
    }

    #[test]
    fn test_is_scheme_allowed() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp", "MyAppTests"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert!(config.is_scheme_allowed("MyApp"));
        assert!(config.is_scheme_allowed("MyAppTests"));
        assert!(!config.is_scheme_allowed("Unknown"));
    }

    #[test]
    fn test_is_destination_allowed() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
            destinations = ["platform=iOS Simulator,name=iPhone 16"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert!(config.is_destination_allowed("platform=iOS Simulator,name=iPhone 16"));
        assert!(!config.is_destination_allowed("platform=macOS"));
    }

    #[test]
    fn test_empty_destinations_allows_all() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert!(config.is_destination_allowed("any-destination"));
    }

    #[test]
    fn test_project_instead_of_workspace() {
        let toml = r#"
            project = "MyApp.xcodeproj"
            schemes = ["MyApp"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert_eq!(config.project, Some("MyApp.xcodeproj".to_string()));
        assert!(config.workspace.is_none());
    }

    #[test]
    fn test_destination_with_spaces() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
            destinations = ["platform=iOS Simulator,name=iPhone 16 Pro,OS=18.0"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert!(config.is_destination_allowed("platform=iOS Simulator,name=iPhone 16 Pro,OS=18.0"));
    }

    #[test]
    fn test_bundle_config_default() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert_eq!(config.bundle.max_bytes, 0); // Default is no limit
    }

    #[test]
    fn test_bundle_config_with_max_bytes() {
        let toml = r#"
            workspace = "MyApp.xcworkspace"
            schemes = ["MyApp"]

            [bundle]
            max_bytes = 104857600
        "#;

        let config = RepoConfig::from_str(toml).unwrap();
        assert_eq!(config.bundle.max_bytes, 104857600); // 100 MB
    }
}
