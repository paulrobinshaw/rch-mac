//! Invocation record for accepted xcodebuild commands
//!
//! When the classifier accepts an invocation, we emit a structured JSON record
//! containing all relevant metadata for auditing, debugging, and worker coordination.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

/// Structured record of an accepted xcodebuild invocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    /// The original argv as received (before sanitization)
    pub original_argv: Vec<String>,

    /// Canonicalized argv with deterministic ordering:
    /// action first, then flags sorted lexicographically by flag name
    pub sanitized_argv: Vec<String>,

    /// The accepted action (only "build" or "test" per spec)
    pub accepted_action: String,

    /// Flags that were rejected (for dry-run/explain mode)
    /// Empty for fully accepted invocations
    pub rejected_flags: Vec<String>,

    /// SHA-256 hash of the classifier policy used
    /// This enables reproducibility audits
    pub classifier_policy_sha256: String,
}

impl Invocation {
    /// Create a new Invocation record
    pub fn new(
        original_argv: Vec<String>,
        sanitized_argv: Vec<String>,
        accepted_action: String,
        rejected_flags: Vec<String>,
        classifier_policy_sha256: String,
    ) -> Self {
        Self {
            original_argv,
            sanitized_argv,
            accepted_action,
            rejected_flags,
            classifier_policy_sha256,
        }
    }

    /// Serialize to JSON string with pretty formatting
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize to compact JSON string (no whitespace)
    pub fn to_json_compact(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Write invocation.json to the specified directory
    pub fn write_to_dir(&self, dir: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON serialization failed: {}", e))
        })?;

        let path = dir.join("invocation.json");
        fs::write(&path, json)?;
        Ok(())
    }

    /// Write invocation.json to a specific file path
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON serialization failed: {}", e))
        })?;

        fs::write(path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_invocation() -> Invocation {
        Invocation::new(
            vec![
                "build".to_string(),
                "-scheme".to_string(),
                "MyApp".to_string(),
                "-workspace".to_string(),
                "MyApp.xcworkspace".to_string(),
            ],
            vec![
                "build".to_string(),
                "-scheme".to_string(),
                "MyApp".to_string(),
                "-workspace".to_string(),
                "MyApp.xcworkspace".to_string(),
            ],
            "build".to_string(),
            vec![],
            "abc123def456".to_string(),
        )
    }

    #[test]
    fn test_invocation_serialization() {
        let inv = sample_invocation();
        let json = inv.to_json().unwrap();

        assert!(json.contains("\"original_argv\""));
        assert!(json.contains("\"sanitized_argv\""));
        assert!(json.contains("\"accepted_action\""));
        assert!(json.contains("\"rejected_flags\""));
        assert!(json.contains("\"classifier_policy_sha256\""));
        assert!(json.contains("\"build\""));
        assert!(json.contains("\"MyApp\""));
    }

    #[test]
    fn test_invocation_deserialization() {
        let inv = sample_invocation();
        let json = inv.to_json().unwrap();

        let parsed: Invocation = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.original_argv, inv.original_argv);
        assert_eq!(parsed.sanitized_argv, inv.sanitized_argv);
        assert_eq!(parsed.accepted_action, inv.accepted_action);
        assert_eq!(parsed.rejected_flags, inv.rejected_flags);
        assert_eq!(parsed.classifier_policy_sha256, inv.classifier_policy_sha256);
    }

    #[test]
    fn test_invocation_compact_json() {
        let inv = sample_invocation();
        let compact = inv.to_json_compact().unwrap();
        let pretty = inv.to_json().unwrap();

        // Compact should be shorter (no whitespace)
        assert!(compact.len() < pretty.len());
        // But both should parse to same data
        let parsed_compact: Invocation = serde_json::from_str(&compact).unwrap();
        let parsed_pretty: Invocation = serde_json::from_str(&pretty).unwrap();
        assert_eq!(parsed_compact.accepted_action, parsed_pretty.accepted_action);
    }

    #[test]
    fn test_invocation_with_rejected_flags() {
        let inv = Invocation::new(
            vec!["build".to_string(), "-unknownFlag".to_string()],
            vec!["build".to_string()],
            "build".to_string(),
            vec!["-unknownFlag".to_string()],
            "policy_hash_here".to_string(),
        );

        let json = inv.to_json().unwrap();
        assert!(json.contains("\"-unknownFlag\""));
    }

    #[test]
    fn test_write_to_file() {
        let inv = sample_invocation();
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_invocation.json");

        inv.write_to_file(&test_file).unwrap();

        let contents = fs::read_to_string(&test_file).unwrap();
        assert!(contents.contains("\"accepted_action\""));
        assert!(contents.contains("\"build\""));

        // Clean up
        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_write_to_dir() {
        let inv = sample_invocation();
        let temp_dir = std::env::temp_dir();

        inv.write_to_dir(&temp_dir).unwrap();

        let expected_path = temp_dir.join("invocation.json");
        let contents = fs::read_to_string(&expected_path).unwrap();
        assert!(contents.contains("\"accepted_action\""));

        // Clean up
        let _ = fs::remove_file(&expected_path);
    }
}
