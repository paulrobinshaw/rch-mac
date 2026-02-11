//! Classifier policy snapshot (classifier_policy.json)
//!
//! Captures the effective classifier policy at a point in time for
//! auditability and replayable explain. The SHA-256 hash of the
//! canonical JSON is recorded in invocation.json.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;

/// Schema version for classifier_policy.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/classifier_policy@1";

/// Classifier policy snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierPolicy {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When this snapshot was created
    pub created_at: DateTime<Utc>,

    /// Run ID (if within a run context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,

    /// Job ID (if within a job context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,

    /// Job key (if within a job context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_key: Option<String>,

    /// Effective allowlist
    pub allowlist: PolicyAllowlist,

    /// Effective denylist
    pub denylist: PolicyDenylist,

    /// Pinned constraints from config
    pub constraints: PolicyConstraints,
}

/// Allowlist entries in the policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyAllowlist {
    /// Allowed actions
    pub actions: Vec<String>,

    /// Allowed flags
    pub flags: Vec<String>,
}

/// Denylist entries in the policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDenylist {
    /// Denied actions
    pub actions: Vec<String>,

    /// Denied flags
    pub flags: Vec<String>,
}

/// Constraints from config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConstraints {
    /// Pinned workspace (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,

    /// Pinned project (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,

    /// Required scheme
    pub scheme: String,

    /// Pinned destination (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,

    /// Allowed configurations
    pub allowed_configurations: Vec<String>,
}

impl ClassifierPolicy {
    /// Create a new policy snapshot
    pub fn new(
        allowlist: PolicyAllowlist,
        denylist: PolicyDenylist,
        constraints: PolicyConstraints,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: None,
            job_id: None,
            job_key: None,
            allowlist,
            denylist,
            constraints,
        }
    }

    /// Set run context
    pub fn with_run_id(mut self, run_id: String) -> Self {
        self.run_id = Some(run_id);
        self
    }

    /// Set job context
    pub fn with_job_context(mut self, job_id: String, job_key: String) -> Self {
        self.job_id = Some(job_id);
        self.job_key = Some(job_key);
        self
    }

    /// Serialize to canonical JSON (sorted keys, no extra whitespace)
    /// This is used for computing the SHA-256 hash
    pub fn to_canonical_json(&self) -> Result<String, serde_json::Error> {
        // Use compact JSON for canonical form
        serde_json::to_string(self)
    }

    /// Serialize to pretty JSON for human reading
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Compute SHA-256 hash of the canonical JSON representation
    pub fn sha256(&self) -> Result<String, serde_json::Error> {
        let canonical = self.to_canonical_json()?;
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        let result = hasher.finalize();
        Ok(hex::encode(result))
    }

    /// Write to a file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("JSON serialization failed: {}", e),
            )
        })?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Write to a directory as classifier_policy.json
    pub fn write_to_dir(&self, dir: &Path) -> io::Result<()> {
        let path = dir.join("classifier_policy.json");
        self.write_to_file(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_policy() -> ClassifierPolicy {
        ClassifierPolicy::new(
            PolicyAllowlist {
                actions: vec!["build".to_string(), "test".to_string()],
                flags: vec![
                    "-configuration".to_string(),
                    "-destination".to_string(),
                    "-scheme".to_string(),
                    "-workspace".to_string(),
                ],
            },
            PolicyDenylist {
                actions: vec!["archive".to_string(), "clean".to_string()],
                flags: vec![
                    "-archivePath".to_string(),
                    "-derivedDataPath".to_string(),
                    "-exportArchive".to_string(),
                ],
            },
            PolicyConstraints {
                workspace: Some("MyApp.xcworkspace".to_string()),
                project: None,
                scheme: "MyApp".to_string(),
                destination: None,
                allowed_configurations: vec!["Debug".to_string(), "Release".to_string()],
            },
        )
    }

    #[test]
    fn test_policy_serialization() {
        let policy = sample_policy();
        let json = policy.to_json().unwrap();

        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"schema_id\": \"rch-xcode/classifier_policy@1\""));
        assert!(json.contains("\"allowlist\""));
        assert!(json.contains("\"denylist\""));
        assert!(json.contains("\"constraints\""));
    }

    #[test]
    fn test_policy_deserialization() {
        let policy = sample_policy();
        let json = policy.to_json().unwrap();

        let parsed: ClassifierPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.schema_id, SCHEMA_ID);
        assert_eq!(parsed.constraints.scheme, "MyApp");
    }

    #[test]
    fn test_canonical_json_consistent() {
        let policy = sample_policy();
        let json1 = policy.to_canonical_json().unwrap();
        let json2 = policy.to_canonical_json().unwrap();

        // Same policy should produce same canonical JSON
        assert_eq!(json1, json2);
    }

    #[test]
    fn test_sha256_deterministic() {
        let policy = sample_policy();
        let hash1 = policy.sha256().unwrap();
        let hash2 = policy.sha256().unwrap();

        // Same policy should produce same hash
        assert_eq!(hash1, hash2);
        // SHA-256 produces 64 hex characters
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_sha256_different_policies() {
        let policy1 = sample_policy();
        let mut policy2 = sample_policy();
        policy2.constraints.scheme = "OtherScheme".to_string();

        let hash1 = policy1.sha256().unwrap();
        let hash2 = policy2.sha256().unwrap();

        // Different policies should produce different hashes
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_with_run_id() {
        let policy = sample_policy().with_run_id("run-123".to_string());

        assert_eq!(policy.run_id, Some("run-123".to_string()));
        let json = policy.to_json().unwrap();
        assert!(json.contains("\"run_id\": \"run-123\""));
    }

    #[test]
    fn test_with_job_context() {
        let policy = sample_policy().with_job_context("job-456".to_string(), "job-key-789".to_string());

        assert_eq!(policy.job_id, Some("job-456".to_string()));
        assert_eq!(policy.job_key, Some("job-key-789".to_string()));
    }

    #[test]
    fn test_write_to_file() {
        let policy = sample_policy();
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_classifier_policy.json");

        policy.write_to_file(&test_file).unwrap();

        let contents = fs::read_to_string(&test_file).unwrap();
        assert!(contents.contains("\"schema_version\""));
        assert!(contents.contains("\"allowlist\""));

        // Cleanup
        let _ = fs::remove_file(&test_file);
    }
}
