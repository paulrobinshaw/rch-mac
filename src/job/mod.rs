//! JobSpec builder and job_key computation
//!
//! Implements the deterministic JobSpec (`job.json`) per PLAN.md normative spec.
//! The job_key is computed using RFC 8785 JSON Canonicalization Scheme (JCS).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

use crate::config::EffectiveConfig;
use crate::destination::{Provisioning, ResolvedDestination};
use crate::toolchain::ToolchainIdentity;

/// Schema version for job.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier for job.json
pub const SCHEMA_ID: &str = "rch-xcode/job@1";

/// Schema identifier for job_key_inputs.json
pub const JOB_KEY_INPUTS_SCHEMA_ID: &str = "rch-xcode/job_key_inputs@1";

/// Artifact profile for controlling output verbosity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactProfile {
    /// Minimal output (default): essential artifacts only
    #[default]
    Minimal,
    /// Rich output: includes additional diagnostic artifacts
    Rich,
}

/// Action for xcodebuild (build or test)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Build action
    Build,
    /// Test action
    Test,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::Build => write!(f, "build"),
            Action::Test => write!(f, "test"),
        }
    }
}

impl std::str::FromStr for Action {
    type Err = JobError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "build" => Ok(Action::Build),
            "test" => Ok(Action::Test),
            _ => Err(JobError::InvalidAction(s.to_string())),
        }
    }
}

/// Toolchain fields for job_key_inputs
///
/// Per PLAN.md normative spec, job_key_inputs.toolchain MUST include:
/// - xcode_build (e.g., "16C5032a")
/// - developer_dir (absolute path on worker)
/// - macos_version (e.g., "15.3.1")
/// - macos_build (e.g., "24D60")
/// - arch (e.g., "arm64")
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobKeyToolchain {
    /// Xcode build identifier
    pub xcode_build: String,
    /// DEVELOPER_DIR value (absolute path on worker)
    pub developer_dir: String,
    /// macOS marketing version
    pub macos_version: String,
    /// macOS build identifier
    pub macos_build: String,
    /// CPU architecture
    pub arch: String,
}

impl From<&ToolchainIdentity> for JobKeyToolchain {
    fn from(identity: &ToolchainIdentity) -> Self {
        Self {
            xcode_build: identity.xcode_build.clone(),
            developer_dir: identity.developer_dir.clone(),
            macos_version: identity.macos_version.clone(),
            macos_build: identity.macos_build.clone(),
            arch: identity.arch.clone(),
        }
    }
}

/// Destination fields for job_key_inputs
///
/// Per PLAN.md normative spec, job_key_inputs.destination MUST include:
/// - platform (e.g., "iOS Simulator")
/// - name (device name)
/// - os_version (resolved concrete version; MUST NOT be "latest")
/// - provisioning ("existing" | "ephemeral")
///
/// For simulator destinations, also includes:
/// - sim_runtime_identifier
/// - sim_runtime_build
/// - device_type_identifier
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobKeyDestination {
    /// Platform (e.g., "iOS Simulator", "macOS")
    pub platform: String,
    /// Device name
    pub name: String,
    /// Resolved concrete OS version (MUST NOT be "latest")
    pub os_version: String,
    /// Provisioning mode
    pub provisioning: Provisioning,
    /// Simulator runtime identifier (for simulators only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_identifier: Option<String>,
    /// Simulator runtime build string (for simulators only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_build: Option<String>,
    /// Device type identifier (for simulators only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type_identifier: Option<String>,
}

impl From<&ResolvedDestination> for JobKeyDestination {
    fn from(dest: &ResolvedDestination) -> Self {
        Self {
            platform: dest.platform.clone(),
            name: dest.name.clone(),
            os_version: dest.os_version.clone(),
            provisioning: dest.provisioning,
            sim_runtime_identifier: dest.sim_runtime_identifier.clone(),
            sim_runtime_build: dest.sim_runtime_build.clone(),
            device_type_identifier: dest.device_type_identifier.clone(),
        }
    }
}

/// Job key inputs - the canonical, output-affecting inputs for cache keying
///
/// Per PLAN.md normative spec:
/// - job_key_inputs MUST include fully-resolved, output-affecting inputs
/// - job_key_inputs MUST NOT include host-only operational settings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobKeyInputs {
    /// SHA-256 digest of the canonical source bundle
    pub source_sha256: String,

    /// Canonical xcodebuild arguments (action first, flags sorted)
    /// Note: Includes action as first element per classifier canonical ordering
    pub sanitized_argv: Vec<String>,

    /// Resolved toolchain identity
    pub toolchain: JobKeyToolchain,

    /// Resolved destination identity
    pub destination: JobKeyDestination,
}

impl JobKeyInputs {
    /// Create new job key inputs
    pub fn new(
        source_sha256: String,
        sanitized_argv: Vec<String>,
        toolchain: JobKeyToolchain,
        destination: JobKeyDestination,
    ) -> Self {
        Self {
            source_sha256,
            sanitized_argv,
            toolchain,
            destination,
        }
    }

    /// Compute the job_key using RFC 8785 JSON Canonicalization Scheme (JCS)
    ///
    /// job_key = SHA-256 hex digest of JCS(job_key_inputs)
    pub fn compute_job_key(&self) -> Result<String, JobError> {
        // Serialize to JCS (RFC 8785)
        let jcs_bytes = serde_json_canonicalizer::to_vec(self)
            .map_err(|e| JobError::JcsError(e.to_string()))?;

        // Compute SHA-256
        let mut hasher = Sha256::new();
        hasher.update(&jcs_bytes);
        Ok(hex::encode(hasher.finalize()))
    }

    /// Serialize to JSON (pretty printed)
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Write to file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e))
        })?;
        fs::write(path, json)
    }
}

/// JobSpec - deterministic, fully-resolved step job description
///
/// Per PLAN.md: "job.json is transmitted to the worker. The worker uses it
/// to emit per-job artifact files."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// Parent run identifier
    pub run_id: String,

    /// Unique step job identifier
    pub job_id: String,

    /// Action: "build" or "test"
    pub action: String,

    /// Canonical key-object hashed to produce job_key
    pub job_key_inputs: JobKeyInputs,

    /// SHA-256 hex digest of JCS(job_key_inputs)
    pub job_key: String,

    /// Merged repo + host config snapshot
    pub effective_config: EffectiveConfig,

    /// Original destination constraint string (for worker to include in destination.json)
    pub original_constraint: String,

    /// When this job was created
    pub created_at: DateTime<Utc>,

    /// Artifact profile (optional, defaults to minimal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_profile: Option<ArtifactProfile>,
}

impl JobSpec {
    /// Create a new JobSpec from resolved components
    pub fn new(
        run_id: String,
        job_id: String,
        action: String,
        job_key_inputs: JobKeyInputs,
        effective_config: EffectiveConfig,
        original_constraint: String,
        artifact_profile: Option<ArtifactProfile>,
    ) -> Result<Self, JobError> {
        // Compute job_key
        let job_key = job_key_inputs.compute_job_key()?;

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            run_id,
            job_id,
            action,
            job_key_inputs,
            job_key,
            effective_config,
            original_constraint,
            created_at: Utc::now(),
            artifact_profile,
        })
    }

    /// Serialize to JSON (pretty printed)
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write to file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e))
        })?;
        fs::write(path, json)
    }

    /// Load from file
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        Self::from_json(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e)))
    }

    /// Verify that job_key matches SHA-256(JCS(job_key_inputs))
    pub fn verify_job_key(&self) -> Result<bool, JobError> {
        let computed = self.job_key_inputs.compute_job_key()?;
        Ok(computed == self.job_key)
    }

    /// Write job_key_inputs.json to the specified directory
    ///
    /// Per PLAN.md: "The host MUST emit a standalone job_key_inputs.json artifact
    /// (byte-for-byte identical to the job_key_inputs object embedded in job.json)"
    pub fn write_job_key_inputs(&self, dir: &Path) -> io::Result<()> {
        let path = dir.join("job_key_inputs.json");
        self.job_key_inputs.write_to_file(&path)
    }
}

/// JobSpec builder for constructing jobs with proper validation
pub struct JobSpecBuilder {
    run_id: String,
    job_id: Option<String>,
    action: String,
    source_sha256: Option<String>,
    sanitized_argv: Option<Vec<String>>,
    toolchain: Option<JobKeyToolchain>,
    destination: Option<JobKeyDestination>,
    effective_config: Option<EffectiveConfig>,
    original_constraint: Option<String>,
    artifact_profile: Option<ArtifactProfile>,
}

impl JobSpecBuilder {
    /// Create a new builder with required run_id and action
    pub fn new(run_id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            job_id: None,
            action: action.into(),
            source_sha256: None,
            sanitized_argv: None,
            toolchain: None,
            destination: None,
            effective_config: None,
            original_constraint: None,
            artifact_profile: None,
        }
    }

    /// Set the job_id (or generate one if not provided)
    pub fn job_id(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }

    /// Set source bundle SHA-256
    pub fn source_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.source_sha256 = Some(sha256.into());
        self
    }

    /// Set sanitized argv (from classifier)
    pub fn sanitized_argv(mut self, argv: Vec<String>) -> Self {
        self.sanitized_argv = Some(argv);
        self
    }

    /// Set toolchain identity
    pub fn toolchain(mut self, toolchain: JobKeyToolchain) -> Self {
        self.toolchain = Some(toolchain);
        self
    }

    /// Set toolchain from ToolchainIdentity
    pub fn toolchain_from(mut self, identity: &ToolchainIdentity) -> Self {
        self.toolchain = Some(JobKeyToolchain::from(identity));
        self
    }

    /// Set destination
    pub fn destination(mut self, destination: JobKeyDestination) -> Self {
        self.destination = Some(destination);
        self
    }

    /// Set destination from ResolvedDestination
    pub fn destination_from(mut self, resolved: &ResolvedDestination) -> Self {
        self.destination = Some(JobKeyDestination::from(resolved));
        self
    }

    /// Set effective config
    pub fn effective_config(mut self, config: EffectiveConfig) -> Self {
        self.effective_config = Some(config);
        self
    }

    /// Set original destination constraint string
    pub fn original_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.original_constraint = Some(constraint.into());
        self
    }

    /// Set artifact profile
    pub fn artifact_profile(mut self, profile: ArtifactProfile) -> Self {
        self.artifact_profile = Some(profile);
        self
    }

    /// Build the JobSpec
    pub fn build(self) -> Result<JobSpec, JobError> {
        // Generate job_id if not provided
        let job_id = self.job_id.unwrap_or_else(generate_job_id);

        // Validate job_id format
        validate_identifier(&job_id)?;

        // Validate run_id format
        validate_identifier(&self.run_id)?;

        // Collect required fields
        let source_sha256 = self
            .source_sha256
            .ok_or(JobError::MissingField("source_sha256"))?;

        let sanitized_argv = self
            .sanitized_argv
            .ok_or(JobError::MissingField("sanitized_argv"))?;

        let toolchain = self
            .toolchain
            .ok_or(JobError::MissingField("toolchain"))?;

        let destination = self
            .destination
            .ok_or(JobError::MissingField("destination"))?;

        let effective_config = self
            .effective_config
            .ok_or(JobError::MissingField("effective_config"))?;

        let original_constraint = self
            .original_constraint
            .ok_or(JobError::MissingField("original_constraint"))?;

        // Validate action
        if self.action != "build" && self.action != "test" {
            return Err(JobError::InvalidAction(self.action.clone()));
        }

        // Validate destination os_version is not "latest"
        if destination.os_version.to_lowercase() == "latest" {
            return Err(JobError::UnresolvedLatest);
        }

        // Build job_key_inputs
        let job_key_inputs = JobKeyInputs::new(source_sha256, sanitized_argv, toolchain, destination);

        // Create JobSpec
        JobSpec::new(
            self.run_id,
            job_id,
            self.action,
            job_key_inputs,
            effective_config,
            original_constraint,
            self.artifact_profile,
        )
    }
}

/// Generate a new job_id using ULID (sortable, filesystem-safe)
pub fn generate_job_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

/// Generate a new run_id using ULID (sortable, filesystem-safe)
pub fn generate_run_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

/// Generate a new request_id using ULID
pub fn generate_request_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

/// Validate identifier format per PLAN.md normative spec
///
/// Identifiers MUST be filesystem-safe: ^[A-Za-z0-9][A-Za-z0-9_-]{9,63}$
/// ULID format (26 chars, alphanumeric) satisfies this.
pub fn validate_identifier(id: &str) -> Result<(), JobError> {
    // Check length: 10-64 characters
    if id.len() < 10 || id.len() > 64 {
        return Err(JobError::InvalidIdentifier(format!(
            "identifier must be 10-64 characters, got {}",
            id.len()
        )));
    }

    // Check pattern: starts with alphanumeric, followed by alphanumeric, underscore, or hyphen
    let mut chars = id.chars();

    // First character must be alphanumeric
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => {
            return Err(JobError::InvalidIdentifier(
                "identifier must start with alphanumeric character".to_string(),
            ))
        }
    }

    // Rest must be alphanumeric, underscore, or hyphen
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
            return Err(JobError::InvalidIdentifier(format!(
                "identifier contains invalid character: {:?}",
                c
            )));
        }
    }

    // Check for forbidden patterns
    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return Err(JobError::InvalidIdentifier(
            "identifier contains forbidden pattern".to_string(),
        ));
    }

    Ok(())
}

/// Errors for JobSpec operations
#[derive(Debug, Error)]
pub enum JobSpecError {
    #[error("JCS canonicalization error: {0}")]
    JcsError(String),

    #[error("missing required field: {0}")]
    MissingField(&'static str),

    #[error("invalid action: {0} (must be 'build' or 'test')")]
    InvalidAction(String),

    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),

    #[error("destination os_version is 'latest' - must be resolved to concrete version")]
    UnresolvedLatest,

    #[error("job_key verification failed")]
    JobKeyMismatch,
}

/// Type alias for JobSpec errors (for backward compatibility)
pub type JobError = JobSpecError;

/// Type alias for JobKeyDestination (for backward compatibility)
pub type DestinationIdentity = JobKeyDestination;

/// Check if identifier is valid (returns bool)
///
/// Wrapper around `validate_identifier` that returns bool instead of Result.
pub fn is_valid_identifier(id: &str) -> bool {
    validate_identifier(id).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigOrigin, ConfigSource};
    use serde_json::json;

    fn make_test_toolchain() -> JobKeyToolchain {
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3.1".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        }
    }

    fn make_test_destination() -> JobKeyDestination {
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: Some(
                "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
            ),
            sim_runtime_build: Some("22C150".to_string()),
            device_type_identifier: Some(
                "com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string(),
            ),
        }
    }

    fn make_test_effective_config() -> EffectiveConfig {
        EffectiveConfig {
            schema_version: 1,
            schema_id: "rch-xcode/effective_config@1".to_string(),
            created_at: Utc::now(),
            run_id: None,
            job_id: None,
            job_key: None,
            config: json!({
                "overall_seconds": 1800,
                "bundle": { "mode": "worktree" }
            }),
            sources: vec![ConfigSource {
                origin: ConfigOrigin::Builtin,
                path: None,
                digest: None,
            }],
            redactions: vec![],
        }
    }

    fn make_test_job_key_inputs() -> JobKeyInputs {
        JobKeyInputs::new(
            "abc123def456789012345678901234567890123456789012345678901234".to_string(),
            vec![
                "build".to_string(),
                "-scheme".to_string(),
                "MyApp".to_string(),
                "-workspace".to_string(),
                "MyApp.xcworkspace".to_string(),
            ],
            make_test_toolchain(),
            make_test_destination(),
        )
    }

    #[test]
    fn test_job_key_computation_deterministic() {
        let inputs1 = make_test_job_key_inputs();
        let inputs2 = make_test_job_key_inputs();

        let key1 = inputs1.compute_job_key().unwrap();
        let key2 = inputs2.compute_job_key().unwrap();

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 64); // SHA-256 hex is 64 chars
    }

    #[test]
    fn test_job_key_changes_with_inputs() {
        let inputs1 = make_test_job_key_inputs();

        let mut inputs2 = make_test_job_key_inputs();
        inputs2.source_sha256 = "different_hash".to_string();

        let key1 = inputs1.compute_job_key().unwrap();
        let key2 = inputs2.compute_job_key().unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_job_key_changes_with_toolchain() {
        let inputs1 = make_test_job_key_inputs();

        let mut inputs2 = make_test_job_key_inputs();
        inputs2.toolchain.xcode_build = "15F31d".to_string();

        let key1 = inputs1.compute_job_key().unwrap();
        let key2 = inputs2.compute_job_key().unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_job_key_changes_with_destination() {
        let inputs1 = make_test_job_key_inputs();

        let mut inputs2 = make_test_job_key_inputs();
        inputs2.destination.os_version = "17.0".to_string();

        let key1 = inputs1.compute_job_key().unwrap();
        let key2 = inputs2.compute_job_key().unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_job_key_changes_with_argv() {
        let inputs1 = make_test_job_key_inputs();

        let mut inputs2 = make_test_job_key_inputs();
        inputs2.sanitized_argv.push("-configuration".to_string());
        inputs2.sanitized_argv.push("Debug".to_string());

        let key1 = inputs1.compute_job_key().unwrap();
        let key2 = inputs2.compute_job_key().unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_job_spec_builder_basic() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec![
                "build".to_string(),
                "-scheme".to_string(),
                "MyApp".to_string(),
            ])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator,name=iPhone 16,OS=18.2")
            .build()
            .unwrap();

        assert_eq!(spec.schema_version, SCHEMA_VERSION);
        assert_eq!(spec.schema_id, SCHEMA_ID);
        assert_eq!(spec.action, "build");
        assert!(!spec.job_key.is_empty());
    }

    #[test]
    fn test_job_spec_verify_job_key() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "test")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec![
                "test".to_string(),
                "-scheme".to_string(),
                "MyAppTests".to_string(),
            ])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator,name=iPhone 16,OS=18.2")
            .build()
            .unwrap();

        assert!(spec.verify_job_key().unwrap());
    }

    #[test]
    fn test_job_spec_builder_generates_job_id() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec!["build".to_string()])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator,name=iPhone 16,OS=18.2")
            .build()
            .unwrap();

        assert!(!spec.job_id.is_empty());
        // ULID is 26 characters
        assert_eq!(spec.job_id.len(), 26);
    }

    #[test]
    fn test_job_spec_builder_rejects_invalid_action() {
        let result = JobSpecBuilder::new("run_01hv3q8zk1234567890", "archive")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123")
            .sanitized_argv(vec!["archive".to_string()])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator")
            .build();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), JobError::InvalidAction(_)));
    }

    #[test]
    fn test_job_spec_builder_rejects_unresolved_latest() {
        let mut dest = make_test_destination();
        dest.os_version = "latest".to_string();

        let result = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec!["build".to_string()])
            .toolchain(make_test_toolchain())
            .destination(dest)
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator,OS=latest")
            .build();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), JobError::UnresolvedLatest));
    }

    #[test]
    fn test_job_spec_builder_rejects_missing_fields() {
        let result = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            // Missing source_sha256
            .sanitized_argv(vec!["build".to_string()])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator")
            .build();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            JobError::MissingField("source_sha256")
        ));
    }

    #[test]
    fn test_generate_job_id_valid() {
        let id = generate_job_id();

        // ULID is 26 characters
        assert_eq!(id.len(), 26);

        // Should be all lowercase alphanumeric
        assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));

        // Should pass validation
        assert!(validate_identifier(&id).is_ok());
    }

    #[test]
    fn test_generate_run_id_valid() {
        let id = generate_run_id();
        assert_eq!(id.len(), 26);
        assert!(validate_identifier(&id).is_ok());
    }

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("01hv3q8zk1234567890").is_ok());
        assert!(validate_identifier("run_01hv3q8zk1234567890").is_ok());
        assert!(validate_identifier("job-id-with-dashes").is_ok());
        assert!(validate_identifier("MixedCase123").is_ok());
    }

    #[test]
    fn test_validate_identifier_invalid_length() {
        assert!(validate_identifier("short").is_err()); // Too short
        assert!(validate_identifier(&"a".repeat(65)).is_err()); // Too long
    }

    #[test]
    fn test_validate_identifier_invalid_chars() {
        assert!(validate_identifier("has spaces here").is_err());
        assert!(validate_identifier("has/slashes").is_err());
        assert!(validate_identifier("has\\backslash").is_err());
        assert!(validate_identifier("has..dots").is_err());
    }

    #[test]
    fn test_validate_identifier_invalid_start() {
        assert!(validate_identifier("_starts_with_underscore").is_err());
        assert!(validate_identifier("-starts_with_dash").is_err());
    }

    #[test]
    fn test_job_spec_serialization() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec![
                "build".to_string(),
                "-scheme".to_string(),
                "MyApp".to_string(),
            ])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator,name=iPhone 16,OS=18.2")
            .build()
            .unwrap();

        let json = spec.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"schema_id\": \"rch-xcode/job@1\""));
        assert!(json.contains("\"job_key\""));
        assert!(json.contains("\"job_key_inputs\""));
    }

    #[test]
    fn test_job_spec_deserialization() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec!["build".to_string()])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator")
            .build()
            .unwrap();

        let json = spec.to_json().unwrap();
        let parsed = JobSpec::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, spec.run_id);
        assert_eq!(parsed.job_id, spec.job_id);
        assert_eq!(parsed.job_key, spec.job_key);
        assert_eq!(parsed.action, spec.action);
    }

    #[test]
    fn test_job_key_inputs_serialization() {
        let inputs = make_test_job_key_inputs();
        let json = inputs.to_json().unwrap();

        assert!(json.contains("\"source_sha256\""));
        assert!(json.contains("\"sanitized_argv\""));
        assert!(json.contains("\"toolchain\""));
        assert!(json.contains("\"destination\""));
    }

    #[test]
    fn test_artifact_profile_serialization() {
        let spec = JobSpecBuilder::new("run_01hv3q8zk1234567890", "build")
            .job_id("job_01hv3q8zk1234567890")
            .source_sha256("abc123def456789012345678901234567890123456789012345678901234")
            .sanitized_argv(vec!["build".to_string()])
            .toolchain(make_test_toolchain())
            .destination(make_test_destination())
            .effective_config(make_test_effective_config())
            .original_constraint("platform=iOS Simulator")
            .artifact_profile(ArtifactProfile::Rich)
            .build()
            .unwrap();

        let json = spec.to_json().unwrap();
        assert!(json.contains("\"artifact_profile\": \"rich\""));
    }

    #[test]
    fn test_job_key_toolchain_from_identity() {
        let identity = ToolchainIdentity {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3.1".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        };

        let toolchain = JobKeyToolchain::from(&identity);

        assert_eq!(toolchain.xcode_build, identity.xcode_build);
        assert_eq!(toolchain.developer_dir, identity.developer_dir);
        assert_eq!(toolchain.macos_version, identity.macos_version);
        assert_eq!(toolchain.macos_build, identity.macos_build);
        assert_eq!(toolchain.arch, identity.arch);
    }

    #[test]
    fn test_job_key_destination_from_resolved() {
        let resolved = ResolvedDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            original_constraint: "platform=iOS Simulator,name=iPhone 16,OS=latest".to_string(),
            sim_runtime_identifier: Some(
                "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
            ),
            sim_runtime_build: Some("22C150".to_string()),
            device_type_identifier: Some(
                "com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string(),
            ),
            udid: None,
        };

        let destination = JobKeyDestination::from(&resolved);

        assert_eq!(destination.platform, resolved.platform);
        assert_eq!(destination.name, resolved.name);
        assert_eq!(destination.os_version, resolved.os_version);
        assert_eq!(destination.provisioning, resolved.provisioning);
        assert_eq!(
            destination.sim_runtime_identifier,
            resolved.sim_runtime_identifier
        );
    }
}
