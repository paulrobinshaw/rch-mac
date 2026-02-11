//! Attestation (attestation.json) per PLAN.md normative spec
//!
//! Binds the artifact set to job inputs and worker identity.
//! Signing is post-MVP (M7, bead b7s.1).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;

/// Schema version for attestation.json
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for attestation.json
pub const ATTESTATION_SCHEMA_ID: &str = "rch-xcode/attestation@1";

/// Worker identity in attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationWorkerIdentity {
    /// Worker name (e.g., "macmini-01")
    pub name: String,

    /// Stable fingerprint (e.g., SSH host key fingerprint)
    pub fingerprint: String,
}

/// Backend identity in attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationBackendIdentity {
    /// Backend name ("xcodebuild" or "mcp")
    pub name: String,

    /// Backend version (e.g., Xcode version or MCP version)
    pub version: String,
}

/// Attestation (attestation.json)
///
/// Binds the artifact set to:
/// - Job inputs (job_id, job_key, source_sha256)
/// - Worker identity (name, fingerprint, capabilities digest)
/// - Backend identity (name, version)
/// - Artifact set (manifest_sha256)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the attestation was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash of job inputs)
    pub job_key: String,

    /// SHA-256 of the source bundle
    pub source_sha256: String,

    /// Worker identity
    pub worker: AttestationWorkerIdentity,

    /// SHA-256 of capabilities.json bytes
    pub capabilities_digest: String,

    /// Backend identity
    pub backend: AttestationBackendIdentity,

    /// SHA-256 of manifest.json bytes
    pub manifest_sha256: String,
}

impl Attestation {
    /// Create a new attestation
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: String,
        job_id: String,
        job_key: String,
        source_sha256: String,
        worker_name: String,
        worker_fingerprint: String,
        capabilities_json: &[u8],
        backend_name: String,
        backend_version: String,
        manifest_json: &[u8],
    ) -> Self {
        let capabilities_digest = compute_sha256(capabilities_json);
        let manifest_sha256 = compute_sha256(manifest_json);

        Self {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            job_key,
            source_sha256,
            worker: AttestationWorkerIdentity {
                name: worker_name,
                fingerprint: worker_fingerprint,
            },
            capabilities_digest,
            backend: AttestationBackendIdentity {
                name: backend_name,
                version: backend_version,
            },
            manifest_sha256,
        }
    }

    /// Create from existing components (when digests are pre-computed)
    pub fn from_components(
        run_id: String,
        job_id: String,
        job_key: String,
        source_sha256: String,
        worker: AttestationWorkerIdentity,
        capabilities_digest: String,
        backend: AttestationBackendIdentity,
        manifest_sha256: String,
    ) -> Self {
        Self {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            job_key,
            source_sha256,
            worker,
            capabilities_digest,
            backend,
            manifest_sha256,
        }
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
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

    /// Verify the manifest_sha256 matches the given manifest file
    pub fn verify_manifest(&self, manifest_path: &Path) -> io::Result<bool> {
        let manifest_bytes = fs::read(manifest_path)?;
        let actual_hash = compute_sha256(&manifest_bytes);
        Ok(actual_hash == self.manifest_sha256)
    }

    /// Verify the capabilities_digest matches the given capabilities file
    pub fn verify_capabilities(&self, capabilities_path: &Path) -> io::Result<bool> {
        let capabilities_bytes = fs::read(capabilities_path)?;
        let actual_hash = compute_sha256(&capabilities_bytes);
        Ok(actual_hash == self.capabilities_digest)
    }
}

/// Compute SHA-256 of bytes and return hex string
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Builder for constructing attestations
pub struct AttestationBuilder {
    run_id: String,
    job_id: String,
    job_key: String,
    source_sha256: String,
    worker_name: Option<String>,
    worker_fingerprint: Option<String>,
    capabilities_digest: Option<String>,
    backend_name: Option<String>,
    backend_version: Option<String>,
    manifest_sha256: Option<String>,
}

impl AttestationBuilder {
    /// Create a new builder with required job identifiers
    pub fn new(run_id: String, job_id: String, job_key: String, source_sha256: String) -> Self {
        Self {
            run_id,
            job_id,
            job_key,
            source_sha256,
            worker_name: None,
            worker_fingerprint: None,
            capabilities_digest: None,
            backend_name: None,
            backend_version: None,
            manifest_sha256: None,
        }
    }

    /// Set worker identity
    pub fn worker(mut self, name: String, fingerprint: String) -> Self {
        self.worker_name = Some(name);
        self.worker_fingerprint = Some(fingerprint);
        self
    }

    /// Set capabilities digest from bytes
    pub fn capabilities_from_bytes(mut self, bytes: &[u8]) -> Self {
        self.capabilities_digest = Some(compute_sha256(bytes));
        self
    }

    /// Set capabilities digest directly
    pub fn capabilities_digest(mut self, digest: String) -> Self {
        self.capabilities_digest = Some(digest);
        self
    }

    /// Set backend identity
    pub fn backend(mut self, name: String, version: String) -> Self {
        self.backend_name = Some(name);
        self.backend_version = Some(version);
        self
    }

    /// Set manifest SHA-256 from bytes
    pub fn manifest_from_bytes(mut self, bytes: &[u8]) -> Self {
        self.manifest_sha256 = Some(compute_sha256(bytes));
        self
    }

    /// Set manifest SHA-256 directly
    pub fn manifest_sha256(mut self, sha256: String) -> Self {
        self.manifest_sha256 = Some(sha256);
        self
    }

    /// Build the attestation
    ///
    /// Returns None if required fields are missing.
    pub fn build(self) -> Option<Attestation> {
        Some(Attestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: self.run_id,
            job_id: self.job_id,
            job_key: self.job_key,
            source_sha256: self.source_sha256,
            worker: AttestationWorkerIdentity {
                name: self.worker_name?,
                fingerprint: self.worker_fingerprint?,
            },
            capabilities_digest: self.capabilities_digest?,
            backend: AttestationBackendIdentity {
                name: self.backend_name?,
                version: self.backend_version?,
            },
            manifest_sha256: self.manifest_sha256?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_attestation() -> Attestation {
        Attestation::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "abc123def456".to_string(),
            "macmini-01".to_string(),
            "SHA256:abcdef123456".to_string(),
            br#"{"schema_version":1,"xcode":["15.0"]}"#,
            "xcodebuild".to_string(),
            "15.0".to_string(),
            br#"{"schema_version":1,"entries":[]}"#,
        )
    }

    #[test]
    fn test_attestation_new() {
        let att = sample_attestation();

        assert_eq!(att.schema_version, ATTESTATION_SCHEMA_VERSION);
        assert_eq!(att.schema_id, ATTESTATION_SCHEMA_ID);
        assert_eq!(att.run_id, "run-123");
        assert_eq!(att.job_id, "job-456");
        assert_eq!(att.job_key, "key-789");
        assert_eq!(att.source_sha256, "abc123def456");
        assert_eq!(att.worker.name, "macmini-01");
        assert_eq!(att.worker.fingerprint, "SHA256:abcdef123456");
        assert_eq!(att.backend.name, "xcodebuild");
        assert_eq!(att.backend.version, "15.0");
        assert!(!att.capabilities_digest.is_empty());
        assert!(!att.manifest_sha256.is_empty());
    }

    #[test]
    fn test_attestation_serialization() {
        let att = sample_attestation();

        let json = att.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/attestation@1""#));
        assert!(json.contains(r#""job_id": "job-456""#));
        assert!(json.contains(r#""worker""#));
        assert!(json.contains(r#""backend""#));
        assert!(json.contains(r#""manifest_sha256""#));
    }

    #[test]
    fn test_attestation_round_trip() {
        let att = sample_attestation();

        let json = att.to_json().unwrap();
        let parsed = Attestation::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, att.run_id);
        assert_eq!(parsed.job_id, att.job_id);
        assert_eq!(parsed.job_key, att.job_key);
        assert_eq!(parsed.source_sha256, att.source_sha256);
        assert_eq!(parsed.worker.name, att.worker.name);
        assert_eq!(parsed.worker.fingerprint, att.worker.fingerprint);
        assert_eq!(parsed.capabilities_digest, att.capabilities_digest);
        assert_eq!(parsed.backend.name, att.backend.name);
        assert_eq!(parsed.backend.version, att.backend.version);
        assert_eq!(parsed.manifest_sha256, att.manifest_sha256);
    }

    #[test]
    fn test_attestation_file_io() {
        let dir = TempDir::new().unwrap();
        let att = sample_attestation();

        let path = dir.path().join("attestation.json");
        att.write_to_file(&path).unwrap();

        let loaded = Attestation::from_file(&path).unwrap();
        assert_eq!(loaded.job_id, att.job_id);
        assert_eq!(loaded.manifest_sha256, att.manifest_sha256);
    }

    #[test]
    fn test_verify_manifest() {
        let dir = TempDir::new().unwrap();
        let manifest_content = br#"{"schema_version":1,"entries":[]}"#;

        let att = Attestation::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "abc123".to_string(),
            "worker-01".to_string(),
            "fingerprint".to_string(),
            b"{}",
            "xcodebuild".to_string(),
            "15.0".to_string(),
            manifest_content,
        );

        // Write matching manifest
        let manifest_path = dir.path().join("manifest.json");
        fs::write(&manifest_path, manifest_content).unwrap();

        assert!(att.verify_manifest(&manifest_path).unwrap());

        // Modify manifest
        fs::write(&manifest_path, b"modified").unwrap();
        assert!(!att.verify_manifest(&manifest_path).unwrap());
    }

    #[test]
    fn test_verify_capabilities() {
        let dir = TempDir::new().unwrap();
        let capabilities_content = br#"{"schema_version":1,"xcode":["15.0"]}"#;

        let att = Attestation::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "abc123".to_string(),
            "worker-01".to_string(),
            "fingerprint".to_string(),
            capabilities_content,
            "xcodebuild".to_string(),
            "15.0".to_string(),
            b"{}",
        );

        // Write matching capabilities
        let capabilities_path = dir.path().join("capabilities.json");
        fs::write(&capabilities_path, capabilities_content).unwrap();

        assert!(att.verify_capabilities(&capabilities_path).unwrap());

        // Modify capabilities
        fs::write(&capabilities_path, b"modified").unwrap();
        assert!(!att.verify_capabilities(&capabilities_path).unwrap());
    }

    #[test]
    fn test_compute_sha256() {
        // Known test vector
        let hash = compute_sha256(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_attestation_builder() {
        let att = AttestationBuilder::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "source-hash".to_string(),
        )
        .worker("macmini-01".to_string(), "fingerprint-abc".to_string())
        .capabilities_from_bytes(b"{}")
        .backend("xcodebuild".to_string(), "15.0".to_string())
        .manifest_from_bytes(b"{}")
        .build()
        .unwrap();

        assert_eq!(att.run_id, "run-123");
        assert_eq!(att.worker.name, "macmini-01");
        assert_eq!(att.backend.name, "xcodebuild");
    }

    #[test]
    fn test_attestation_builder_missing_fields() {
        // Missing worker
        let result = AttestationBuilder::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "source-hash".to_string(),
        )
        .backend("xcodebuild".to_string(), "15.0".to_string())
        .manifest_from_bytes(b"{}")
        .build();

        assert!(result.is_none());
    }

    #[test]
    fn test_from_components() {
        let att = Attestation::from_components(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "source-hash".to_string(),
            AttestationWorkerIdentity {
                name: "worker-01".to_string(),
                fingerprint: "fp-123".to_string(),
            },
            "capabilities-hash".to_string(),
            AttestationBackendIdentity {
                name: "xcodebuild".to_string(),
                version: "15.0".to_string(),
            },
            "manifest-hash".to_string(),
        );

        assert_eq!(att.worker.name, "worker-01");
        assert_eq!(att.capabilities_digest, "capabilities-hash");
        assert_eq!(att.manifest_sha256, "manifest-hash");
    }
}
