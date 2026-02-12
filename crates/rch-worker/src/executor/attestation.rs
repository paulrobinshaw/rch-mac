//! Attestation generation and signing (attestation.json)
//!
//! Per bead y6s.3: Worker generates attestation per job binding:
//! - Job identity (job_id, job_key, source_sha256)
//! - Worker identity (name, stable fingerprint)
//! - Capabilities digest (SHA-256 of capabilities.json)
//! - Backend identity (xcodebuild/MCP, version)
//! - Manifest binding (manifest_sha256)
//!
//! Per bead b7s.1: Worker signs attestation with Ed25519 key.
//! - signature: Base64-encoded Ed25519 signature of JCS(attestation)
//! - pubkey_fingerprint: SHA-256 fingerprint of public key

use std::fs;
use std::io;
use std::path::Path;

use base64::Engine;
use chrono::Utc;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
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

/// Internal struct for signing (excludes signature fields)
#[derive(Serialize)]
struct SignableAttestation<'a> {
    schema_version: u32,
    schema_id: &'a str,
    created_at: &'a str,
    run_id: &'a str,
    job_id: &'a str,
    job_key: &'a str,
    source_sha256: &'a str,
    worker: &'a WorkerIdentity,
    capabilities_digest: &'a str,
    backend: &'a BackendIdentity,
    manifest_sha256: &'a str,
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

    /// Ed25519 signature of the attestation (Base64-encoded)
    /// Present only when signing is enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// SHA-256 fingerprint of the signing public key
    /// Present only when signing is enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey_fingerprint: Option<String>,
}

/// Attestation signing key pair
pub struct AttestationKeyPair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl AttestationKeyPair {
    /// Generate a new random key pair
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let verifying_key = signing_key.verifying_key();
        Self { signing_key, verifying_key }
    }

    /// Load from a private key file (32 bytes raw or 64 bytes hex)
    pub fn from_private_key_bytes(bytes: &[u8]) -> Result<Self, io::Error> {
        let key_bytes: [u8; 32] = if bytes.len() == 64 {
            // Hex-encoded
            let decoded = hex::decode(bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            decoded.try_into()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid key length"))?
        } else if bytes.len() == 32 {
            // Raw bytes
            bytes.try_into()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid key length"))?
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("expected 32 or 64 bytes, got {}", bytes.len())));
        };

        let signing_key = SigningKey::from_bytes(&key_bytes);
        let verifying_key = signing_key.verifying_key();
        Ok(Self { signing_key, verifying_key })
    }

    /// Load from a private key file
    pub fn from_file(path: &Path) -> Result<Self, io::Error> {
        let content = fs::read(path)?;
        Self::from_private_key_bytes(&content)
    }

    /// Get the public key fingerprint (SHA-256 of public key bytes)
    pub fn pubkey_fingerprint(&self) -> String {
        let pubkey_bytes = self.verifying_key.as_bytes();
        let mut hasher = Sha256::new();
        hasher.update(pubkey_bytes);
        format!("SHA256:{}", hex::encode(hasher.finalize()))
    }

    /// Get the verifying key
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Export private key as hex string
    pub fn private_key_hex(&self) -> String {
        hex::encode(self.signing_key.to_bytes())
    }

    /// Export public key as hex string
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.verifying_key.as_bytes())
    }
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
            signature: None,
            pubkey_fingerprint: None,
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

    /// Get the signable content (JCS-canonicalized attestation without signature fields)
    fn signable_content(&self) -> Result<Vec<u8>, io::Error> {
        // Create a copy without signature fields for signing
        let signable = SignableAttestation {
            schema_version: self.schema_version,
            schema_id: &self.schema_id,
            created_at: &self.created_at,
            run_id: &self.run_id,
            job_id: &self.job_id,
            job_key: &self.job_key,
            source_sha256: &self.source_sha256,
            worker: &self.worker,
            capabilities_digest: &self.capabilities_digest,
            backend: &self.backend,
            manifest_sha256: &self.manifest_sha256,
        };

        let json = serde_json::to_value(&signable)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        serde_json_canonicalizer::to_vec(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Sign the attestation with the given key pair
    ///
    /// Signs the JCS-canonicalized attestation content (excluding signature fields)
    /// and stores the signature and public key fingerprint.
    pub fn sign(&mut self, key_pair: &AttestationKeyPair) -> Result<(), io::Error> {
        let content = self.signable_content()?;
        let signature: Signature = key_pair.signing_key.sign(&content);

        self.signature = Some(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
        self.pubkey_fingerprint = Some(key_pair.pubkey_fingerprint());

        Ok(())
    }

    /// Verify the attestation signature with the given verifying key
    ///
    /// Returns Ok(true) if signature is valid, Ok(false) if invalid or missing.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<bool, io::Error> {
        let signature_b64 = match &self.signature {
            Some(s) => s,
            None => return Ok(false),
        };

        let signature_bytes = base64::engine::general_purpose::STANDARD
            .decode(signature_b64)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let content = self.signable_content()?;

        Ok(verifying_key.verify(&content, &signature).is_ok())
    }

    /// Check if this attestation is signed
    pub fn is_signed(&self) -> bool {
        self.signature.is_some() && self.pubkey_fingerprint.is_some()
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

        Ok(Attestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: Utc::now().to_rfc3339(),
            run_id: self.run_id,
            job_id: self.job_id,
            job_key: self.job_key,
            source_sha256: self.source_sha256,
            worker,
            capabilities_digest,
            backend,
            manifest_sha256,
            signature: None,
            pubkey_fingerprint: None,
        })
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

    #[test]
    fn test_keypair_generate() {
        let kp = AttestationKeyPair::generate();

        // Fingerprint should be SHA256:hex format
        let fp = kp.pubkey_fingerprint();
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fp.len(), 7 + 64); // "SHA256:" + 64 hex chars

        // Private key should be 64 hex chars (32 bytes)
        assert_eq!(kp.private_key_hex().len(), 64);

        // Public key should be 64 hex chars (32 bytes)
        assert_eq!(kp.public_key_hex().len(), 64);
    }

    #[test]
    fn test_keypair_from_bytes_raw() {
        let kp1 = AttestationKeyPair::generate();
        let private_bytes = hex::decode(kp1.private_key_hex()).unwrap();

        let kp2 = AttestationKeyPair::from_private_key_bytes(&private_bytes).unwrap();

        // Same private key should produce same public key
        assert_eq!(kp1.public_key_hex(), kp2.public_key_hex());
        assert_eq!(kp1.pubkey_fingerprint(), kp2.pubkey_fingerprint());
    }

    #[test]
    fn test_keypair_from_bytes_hex() {
        let kp1 = AttestationKeyPair::generate();
        let private_hex = kp1.private_key_hex();

        let kp2 = AttestationKeyPair::from_private_key_bytes(private_hex.as_bytes()).unwrap();

        // Same private key should produce same public key
        assert_eq!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_keypair_from_file() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("signing.key");

        let kp1 = AttestationKeyPair::generate();
        fs::write(&key_path, kp1.private_key_hex()).unwrap();

        let kp2 = AttestationKeyPair::from_file(&key_path).unwrap();

        assert_eq!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_sign_and_verify() {
        let mut attestation = Attestation::new(
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

        assert!(!attestation.is_signed());

        let kp = AttestationKeyPair::generate();
        attestation.sign(&kp).unwrap();

        assert!(attestation.is_signed());
        assert!(attestation.signature.is_some());
        assert_eq!(attestation.pubkey_fingerprint.as_ref().unwrap(), &kp.pubkey_fingerprint());

        // Verify with correct key
        assert!(attestation.verify(kp.verifying_key()).unwrap());

        // Verify with wrong key should fail
        let wrong_kp = AttestationKeyPair::generate();
        assert!(!attestation.verify(wrong_kp.verifying_key()).unwrap());
    }

    #[test]
    fn test_verify_unsigned() {
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

        let kp = AttestationKeyPair::generate();

        // Unsigned attestation should return false (not error)
        assert!(!attestation.verify(kp.verifying_key()).unwrap());
    }

    #[test]
    fn test_signature_detects_tampering() {
        let mut attestation = Attestation::new(
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

        let kp = AttestationKeyPair::generate();
        attestation.sign(&kp).unwrap();

        // Tamper with the attestation
        attestation.run_id = "run-002-tampered".to_string();

        // Verification should fail
        assert!(!attestation.verify(kp.verifying_key()).unwrap());
    }

    #[test]
    fn test_signed_attestation_serialization() {
        let mut attestation = Attestation::new(
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

        let kp = AttestationKeyPair::generate();
        attestation.sign(&kp).unwrap();

        // Serialize and deserialize
        let json = serde_json::to_string(&attestation).unwrap();
        let parsed: Attestation = serde_json::from_str(&json).unwrap();

        // Signature should survive roundtrip
        assert!(parsed.is_signed());
        assert!(parsed.verify(kp.verifying_key()).unwrap());
    }

    #[test]
    fn test_signature_is_deterministic() {
        // Create two attestations with same content but different times
        // by using builder which sets created_at on build
        let attestation1 = Attestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            schema_id: ATTESTATION_SCHEMA_ID.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            run_id: "run-001".to_string(),
            job_id: "job-001".to_string(),
            job_key: "abc123".to_string(),
            source_sha256: "source456".to_string(),
            worker: WorkerIdentity {
                name: "macmini-01".to_string(),
                fingerprint: "SHA256:abcdef".to_string(),
            },
            capabilities_digest: "caps789".to_string(),
            backend: BackendIdentity {
                name: "xcodebuild".to_string(),
                version: "16.2".to_string(),
            },
            manifest_sha256: "manifest012".to_string(),
            signature: None,
            pubkey_fingerprint: None,
        };

        let attestation2 = attestation1.clone();

        let kp = AttestationKeyPair::generate();

        let mut a1 = attestation1;
        let mut a2 = attestation2;

        a1.sign(&kp).unwrap();
        a2.sign(&kp).unwrap();

        // Same content with same key should produce same signature
        assert_eq!(a1.signature, a2.signature);
    }
}
