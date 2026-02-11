//! Attestation generation (attestation.json)
//!
//! Per bead y6s.3: Worker generates unsigned attestation per job binding:
//! - Job identity (job_id, job_key, source_sha256)
//! - Worker identity (name, stable fingerprint)
//! - Capabilities digest (SHA-256 of capabilities.json)
//! - Backend identity (xcodebuild/MCP, version)
//! - Manifest binding (manifest_sha256)
//!
//! Signing is post-MVP (M7, bead b7s.1).

use std::fs;
use std::io;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Schema version for attestation.json
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for attestation.json
pub const ATTESTATION_SCHEMA_ID: &str = "rch-xcode/attestation@1";

/// Worker identity in attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerIdentity {
    /// Worker name (e.g., "macmini-01")
    pub name: String,

    /// Stable fingerprint (e.g., SSH host key fingerprint)
    pub fingerprint: String,
}

/// Backend identity in attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendIdentity {
    /// Backend name ("xcodebuild" or "mcp")
    pub name: String,

    /// Backend version (e.g., "16.2" for Xcode)
    pub version: String,
}

/// Attestation document (attestation.json)
///
/// Binds the artifact set to job inputs and worker identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the attestation was created
    pub created_at: String,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash of job inputs)
    pub job_key: String,

    /// SHA-256 of source bundle
    pub source_sha256: String,

    /// Worker identity
    pub worker: WorkerIdentity,

    /// SHA-256 of capabilities.json content
    pub capabilities_digest: String,

    /// Backend identity
    pub backend: BackendIdentity,

    /// SHA-256 of manifest.json content
    pub manifest_sha256: String,
}

impl Attestation {
    /// Create a new attestation
    pub fn new(
        run_id: &str,
        job_id: &str,
        job_key: &str,
        source_sha256: &str,
        worker: WorkerIdentity,
        capabilities_digest: &str,
        backend: BackendIdentity,
        manifest_sha256: &str,
    ) -> Self {
        Self {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: Utc::now().to_rfc3339(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            source_sha256: source_sha256.to_string(),
            worker,
            capabilities_digest: capabilities_digest.to_string(),
            backend,
            manifest_sha256: manifest_sha256.to_string(),
        }
    }

    /// Write attestation.json to a directory with atomic write-then-rename
    pub fn write_to_file(&self, artifact_dir: &Path) -> Result<(), io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let final_path = artifact_dir.join("attestation.json");
        let temp_path = artifact_dir.join(".attestation.json.tmp");

        fs::write(&temp_path, &json)?;
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }
}

/// Builder for creating attestations
pub struct AttestationBuilder {
    run_id: String,
    job_id: String,
    job_key: String,
    source_sha256: String,
    worker: Option<WorkerIdentity>,
    capabilities_digest: Option<String>,
    backend: Option<BackendIdentity>,
    manifest_sha256: Option<String>,
}

impl AttestationBuilder {
    /// Create a new builder with required job fields
    pub fn new(run_id: &str, job_id: &str, job_key: &str, source_sha256: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            source_sha256: source_sha256.to_string(),
            worker: None,
            capabilities_digest: None,
            backend: None,
            manifest_sha256: None,
        }
    }

    /// Set worker identity
    pub fn worker(mut self, name: &str, fingerprint: &str) -> Self {
        self.worker = Some(WorkerIdentity {
            name: name.to_string(),
            fingerprint: fingerprint.to_string(),
        });
        self
    }

    /// Set capabilities digest from raw bytes
    pub fn capabilities_from_bytes(mut self, capabilities_bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(capabilities_bytes);
        self.capabilities_digest = Some(hex::encode(hasher.finalize()));
        self
    }

    /// Set capabilities digest directly
    pub fn capabilities_digest(mut self, digest: &str) -> Self {
        self.capabilities_digest = Some(digest.to_string());
        self
    }

    /// Set backend identity
    pub fn backend(mut self, name: &str, version: &str) -> Self {
        self.backend = Some(BackendIdentity {
            name: name.to_string(),
            version: version.to_string(),
        });
        self
    }

    /// Set manifest SHA-256 directly
    pub fn manifest_sha256(mut self, sha256: &str) -> Self {
        self.manifest_sha256 = Some(sha256.to_string());
        self
    }

    /// Build the attestation
    ///
    /// Returns an error if any required field is missing.
    pub fn build(self) -> Result<Attestation, io::Error> {
        let worker = self.worker.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "worker identity required")
        })?;

        let capabilities_digest = self.capabilities_digest.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "capabilities_digest required")
        })?;

        let backend = self.backend.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "backend identity required")
        })?;

        let manifest_sha256 = self.manifest_sha256.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "manifest_sha256 required")
        })?;

        Ok(Attestation::new(
            &self.run_id,
            &self.job_id,
            &self.job_key,
            &self.source_sha256,
            worker,
            &capabilities_digest,
            backend,
            &manifest_sha256,
        ))
    }
}

/// Compute SHA-256 of any byte slice (utility function)
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Generate attestation.json for a job
pub fn generate_attestation(
    artifact_dir: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
    source_sha256: &str,
    worker_name: &str,
    worker_fingerprint: &str,
    capabilities_json: &[u8],
    backend_name: &str,
    backend_version: &str,
    manifest_sha256: &str,
) -> Result<Attestation, io::Error> {
    let attestation = AttestationBuilder::new(run_id, job_id, job_key, source_sha256)
        .worker(worker_name, worker_fingerprint)
        .capabilities_from_bytes(capabilities_json)
        .backend(backend_name, backend_version)
        .manifest_sha256(manifest_sha256)
        .build()?;

    attestation.write_to_file(artifact_dir)?;
    Ok(attestation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_attestation_new() {
        let attestation = Attestation::new(
            "run-001",
            "job-001",
            "abc123",
            "source456",
            WorkerIdentity {
                name: "macmini-01".to_string(),
                fingerprint: "SHA256:abcdef".to_string(),
            },
            "caps789",
            BackendIdentity {
                name: "xcodebuild".to_string(),
                version: "16.2".to_string(),
            },
            "manifest012",
        );

        assert_eq!(attestation.schema_version, ATTESTATION_SCHEMA_VERSION);
        assert_eq!(attestation.schema_id, ATTESTATION_SCHEMA_ID);
        assert_eq!(attestation.run_id, "run-001");
        assert_eq!(attestation.job_id, "job-001");
        assert_eq!(attestation.job_key, "abc123");
        assert_eq!(attestation.source_sha256, "source456");
        assert_eq!(attestation.worker.name, "macmini-01");
        assert_eq!(attestation.worker.fingerprint, "SHA256:abcdef");
        assert_eq!(attestation.capabilities_digest, "caps789");
        assert_eq!(attestation.backend.name, "xcodebuild");
        assert_eq!(attestation.backend.version, "16.2");
        assert_eq!(attestation.manifest_sha256, "manifest012");
    }

    #[test]
    fn test_write_to_file() {
        let temp_dir = TempDir::new().unwrap();

        let attestation = Attestation::new(
            "run-001",
            "job-001",
            "abc123",
            "source456",
            WorkerIdentity {
                name: "macmini-01".to_string(),
                fingerprint: "SHA256:abcdef".to_string(),
            },
            "caps789",
            BackendIdentity {
                name: "xcodebuild".to_string(),
                version: "16.2".to_string(),
            },
            "manifest012",
        );

        attestation.write_to_file(temp_dir.path()).unwrap();

        let path = temp_dir.path().join("attestation.json");
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["schema_version"], ATTESTATION_SCHEMA_VERSION);
        assert_eq!(parsed["schema_id"], ATTESTATION_SCHEMA_ID);
        assert_eq!(parsed["worker"]["name"], "macmini-01");
        assert_eq!(parsed["backend"]["name"], "xcodebuild");
    }

    #[test]
    fn test_builder() {
        let attestation = AttestationBuilder::new("run-001", "job-001", "abc123", "source456")
            .worker("macmini-01", "SHA256:abcdef")
            .capabilities_digest("caps789")
            .backend("xcodebuild", "16.2")
            .manifest_sha256("manifest012")
            .build()
            .unwrap();

        assert_eq!(attestation.worker.name, "macmini-01");
        assert_eq!(attestation.backend.name, "xcodebuild");
    }

    #[test]
    fn test_builder_missing_worker() {
        let result = AttestationBuilder::new("run-001", "job-001", "abc123", "source456")
            .capabilities_digest("caps789")
            .backend("xcodebuild", "16.2")
            .manifest_sha256("manifest012")
            .build();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("worker identity"));
    }

    #[test]
    fn test_builder_missing_backend() {
        let result = AttestationBuilder::new("run-001", "job-001", "abc123", "source456")
            .worker("macmini-01", "SHA256:abcdef")
            .capabilities_digest("caps789")
            .manifest_sha256("manifest012")
            .build();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("backend identity"));
    }

    #[test]
    fn test_capabilities_from_bytes() {
        let capabilities = r#"{"max_upload_bytes": 104857600}"#.as_bytes();

        let attestation = AttestationBuilder::new("run-001", "job-001", "abc123", "source456")
            .worker("macmini-01", "SHA256:abcdef")
            .capabilities_from_bytes(capabilities)
            .backend("xcodebuild", "16.2")
            .manifest_sha256("manifest012")
            .build()
            .unwrap();

        // Verify it's a 64-char hex string (SHA-256)
        assert_eq!(attestation.capabilities_digest.len(), 64);
        // Verify it's deterministic
        let expected = compute_sha256(capabilities);
        assert_eq!(attestation.capabilities_digest, expected);
    }

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);

        // Known SHA-256 of "hello world"
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_generate_attestation() {
        let temp_dir = TempDir::new().unwrap();

        let capabilities = r#"{"max_upload_bytes": 104857600}"#.as_bytes();

        let attestation = generate_attestation(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
            "source456",
            "macmini-01",
            "SHA256:abcdef",
            capabilities,
            "xcodebuild",
            "16.2",
            "manifest012",
        ).unwrap();

        // File should exist
        assert!(temp_dir.path().join("attestation.json").exists());

        // Values should be correct
        assert_eq!(attestation.run_id, "run-001");
        assert_eq!(attestation.job_id, "job-001");
        assert_eq!(attestation.backend.name, "xcodebuild");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let attestation = Attestation::new(
            "run-001",
            "job-001",
            "abc123",
            "source456",
            WorkerIdentity {
                name: "macmini-01".to_string(),
                fingerprint: "SHA256:abcdef".to_string(),
            },
            "caps789",
            BackendIdentity {
                name: "xcodebuild".to_string(),
                version: "16.2".to_string(),
            },
            "manifest012",
        );

        let json = serde_json::to_string(&attestation).unwrap();
        let parsed: Attestation = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.run_id, attestation.run_id);
        assert_eq!(parsed.job_id, attestation.job_id);
        assert_eq!(parsed.worker.name, attestation.worker.name);
        assert_eq!(parsed.backend.name, attestation.backend.name);
    }
}
