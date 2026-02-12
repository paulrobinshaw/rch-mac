//! Ed25519 signing and verification for attestations (M7, bead b7s.1)
//!
//! Per PLAN.md:
//! - Worker signs attestation.json with Ed25519 key
//! - Host verifies signature and records attestation_verification.json
//! - If verification fails, failure_kind=ATTESTATION, exit_code=93
//! - Host may pin attestation_pubkey_fingerprint in worker inventory

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

use super::Attestation;

/// Schema version for attestation_verification.json
pub const VERIFICATION_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for attestation_verification.json
pub const VERIFICATION_SCHEMA_ID: &str = "rch-xcode/attestation_verification@1";

/// Signature algorithm identifier
pub const SIGNATURE_ALGORITHM: &str = "Ed25519";

/// Errors from signing/verification operations
#[derive(Debug, Error)]
pub enum SigningError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    #[error("invalid key: {0}")]
    InvalidKey(String),

    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
}

/// Result type for signing operations
pub type SigningResult<T> = Result<T, SigningError>;

/// Signed attestation wrapping an attestation with its signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedAttestation {
    /// The attestation content
    pub attestation: Attestation,

    /// Base64-encoded Ed25519 signature over canonical attestation JSON
    pub signature: String,

    /// Signature algorithm identifier (always "Ed25519")
    pub signature_algorithm: String,

    /// SHA-256 fingerprint of the signing public key (hex-encoded)
    pub pubkey_fingerprint: String,
}

impl SignedAttestation {
    /// Create a signed attestation from an attestation and signing key
    pub fn sign(attestation: Attestation, signing_key: &SigningKey) -> SigningResult<Self> {
        // Compute canonical JSON for signing
        let canonical = serde_json::to_string(&attestation)?;

        // Sign the canonical JSON bytes
        let signature = signing_key.sign(canonical.as_bytes());

        // Get the verifying (public) key
        let verifying_key = signing_key.verifying_key();

        // Compute fingerprint of public key
        let pubkey_fingerprint = compute_key_fingerprint(&verifying_key);

        Ok(Self {
            attestation,
            signature: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                signature.to_bytes(),
            ),
            signature_algorithm: SIGNATURE_ALGORITHM.to_string(),
            pubkey_fingerprint,
        })
    }

    /// Verify the signature against a verifying key
    pub fn verify(&self, verifying_key: &VerifyingKey) -> SigningResult<bool> {
        // Decode signature
        let sig_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &self.signature,
        )?;

        let signature = Signature::from_slice(&sig_bytes)
            .map_err(|e| SigningError::InvalidSignature(e.to_string()))?;

        // Compute canonical JSON
        let canonical = serde_json::to_string(&self.attestation)?;

        // Verify signature
        match verifying_key.verify(canonical.as_bytes(), &signature) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Verify the signature and check fingerprint against pinned value
    pub fn verify_with_pinning(
        &self,
        verifying_key: &VerifyingKey,
        pinned_fingerprint: Option<&str>,
    ) -> SigningResult<bool> {
        // First check fingerprint if pinned
        if let Some(pinned) = pinned_fingerprint {
            let actual = compute_key_fingerprint(verifying_key);
            if actual != pinned {
                return Err(SigningError::FingerprintMismatch {
                    expected: pinned.to_string(),
                    actual,
                });
            }
        }

        // Then verify signature
        self.verify(verifying_key)
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
}

/// Verification result for attestation_verification.json
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationResult {
    Pass,
    Fail,
}

/// Attestation verification record (attestation_verification.json)
///
/// Emitted by the host after verifying a signed attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationVerification {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the verification was performed
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Verification result
    pub verification_result: VerificationResult,

    /// SHA-256 fingerprint of the signing public key
    pub pubkey_fingerprint: String,

    /// Pinned fingerprint from worker inventory (if configured)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_fingerprint: Option<String>,

    /// Signature algorithm used
    pub signature_algorithm: String,

    /// Error message if verification failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl AttestationVerification {
    /// Create a passing verification record
    pub fn pass(
        run_id: String,
        job_id: String,
        pubkey_fingerprint: String,
        pinned_fingerprint: Option<String>,
    ) -> Self {
        Self {
            schema_version: VERIFICATION_SCHEMA_VERSION,
            schema_id: VERIFICATION_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            verification_result: VerificationResult::Pass,
            pubkey_fingerprint,
            pinned_fingerprint,
            signature_algorithm: SIGNATURE_ALGORITHM.to_string(),
            error_message: None,
        }
    }

    /// Create a failing verification record
    pub fn fail(
        run_id: String,
        job_id: String,
        pubkey_fingerprint: String,
        pinned_fingerprint: Option<String>,
        error: String,
    ) -> Self {
        Self {
            schema_version: VERIFICATION_SCHEMA_VERSION,
            schema_id: VERIFICATION_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            verification_result: VerificationResult::Fail,
            pubkey_fingerprint,
            pinned_fingerprint,
            signature_algorithm: SIGNATURE_ALGORITHM.to_string(),
            error_message: Some(error),
        }
    }

    /// Serialize to JSON
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

/// Compute SHA-256 fingerprint of an Ed25519 public key
pub fn compute_key_fingerprint(key: &VerifyingKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a new Ed25519 keypair
pub fn generate_keypair() -> SigningKey {
    SigningKey::generate(&mut rand::thread_rng())
}

/// Encode a signing key to base64 for storage
pub fn encode_signing_key(key: &SigningKey) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key.to_bytes())
}

/// Decode a signing key from base64
pub fn decode_signing_key(encoded: &str) -> SigningResult<SigningKey> {
    let bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)?;
    let bytes_array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SigningError::InvalidKey("key must be 32 bytes".to_string()))?;
    Ok(SigningKey::from_bytes(&bytes_array))
}

/// Encode a verifying key to base64 for storage
pub fn encode_verifying_key(key: &VerifyingKey) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key.as_bytes())
}

/// Decode a verifying key from base64
pub fn decode_verifying_key(encoded: &str) -> SigningResult<VerifyingKey> {
    let bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)?;
    let bytes_array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SigningError::InvalidKey("key must be 32 bytes".to_string()))?;
    VerifyingKey::from_bytes(&bytes_array)
        .map_err(|e| SigningError::InvalidKey(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{AttestationBackendIdentity, AttestationWorkerIdentity};

    fn sample_attestation() -> Attestation {
        Attestation::from_components(
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
        )
    }

    #[test]
    fn test_sign_and_verify() {
        let keypair = generate_keypair();
        let attestation = sample_attestation();

        // Sign
        let signed = SignedAttestation::sign(attestation.clone(), &keypair).unwrap();

        // Verify
        let verifying_key = keypair.verifying_key();
        assert!(signed.verify(&verifying_key).unwrap());

        // Check fields
        assert_eq!(signed.signature_algorithm, SIGNATURE_ALGORITHM);
        assert!(!signed.signature.is_empty());
        assert!(!signed.pubkey_fingerprint.is_empty());
    }

    #[test]
    fn test_verify_with_wrong_key() {
        let keypair1 = generate_keypair();
        let keypair2 = generate_keypair();
        let attestation = sample_attestation();

        // Sign with key1
        let signed = SignedAttestation::sign(attestation, &keypair1).unwrap();

        // Verify with key2 - should fail
        let wrong_key = keypair2.verifying_key();
        assert!(!signed.verify(&wrong_key).unwrap());
    }

    #[test]
    fn test_verify_with_pinning() {
        let keypair = generate_keypair();
        let attestation = sample_attestation();
        let verifying_key = keypair.verifying_key();
        let fingerprint = compute_key_fingerprint(&verifying_key);

        // Sign
        let signed = SignedAttestation::sign(attestation, &keypair).unwrap();

        // Verify with correct pinned fingerprint
        assert!(signed
            .verify_with_pinning(&verifying_key, Some(&fingerprint))
            .unwrap());

        // Verify with wrong pinned fingerprint
        let result = signed.verify_with_pinning(&verifying_key, Some("wrong-fingerprint"));
        assert!(matches!(result, Err(SigningError::FingerprintMismatch { .. })));
    }

    #[test]
    fn test_signed_attestation_round_trip() {
        let keypair = generate_keypair();
        let attestation = sample_attestation();

        let signed = SignedAttestation::sign(attestation, &keypair).unwrap();
        let json = signed.to_json().unwrap();
        let parsed = SignedAttestation::from_json(&json).unwrap();

        assert_eq!(parsed.attestation.job_id, signed.attestation.job_id);
        assert_eq!(parsed.signature, signed.signature);
        assert_eq!(parsed.pubkey_fingerprint, signed.pubkey_fingerprint);
    }

    #[test]
    fn test_verification_record_pass() {
        let record = AttestationVerification::pass(
            "run-123".to_string(),
            "job-456".to_string(),
            "fingerprint-abc".to_string(),
            Some("pinned-fingerprint".to_string()),
        );

        assert_eq!(record.verification_result, VerificationResult::Pass);
        assert_eq!(record.schema_version, VERIFICATION_SCHEMA_VERSION);
        assert_eq!(record.schema_id, VERIFICATION_SCHEMA_ID);
        assert!(record.error_message.is_none());
    }

    #[test]
    fn test_verification_record_fail() {
        let record = AttestationVerification::fail(
            "run-123".to_string(),
            "job-456".to_string(),
            "fingerprint-abc".to_string(),
            None,
            "signature verification failed".to_string(),
        );

        assert_eq!(record.verification_result, VerificationResult::Fail);
        assert!(record.error_message.is_some());
    }

    #[test]
    fn test_key_encoding() {
        let keypair = generate_keypair();

        // Encode and decode signing key
        let encoded = encode_signing_key(&keypair);
        let decoded = decode_signing_key(&encoded).unwrap();
        assert_eq!(keypair.to_bytes(), decoded.to_bytes());

        // Encode and decode verifying key
        let verifying = keypair.verifying_key();
        let encoded = encode_verifying_key(&verifying);
        let decoded = decode_verifying_key(&encoded).unwrap();
        assert_eq!(verifying.as_bytes(), decoded.as_bytes());
    }

    #[test]
    fn test_key_fingerprint() {
        let keypair = generate_keypair();
        let verifying_key = keypair.verifying_key();

        let fp1 = compute_key_fingerprint(&verifying_key);
        let fp2 = compute_key_fingerprint(&verifying_key);

        // Fingerprint should be deterministic
        assert_eq!(fp1, fp2);

        // Fingerprint should be 64 hex chars (SHA-256)
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn test_file_io() {
        let dir = tempfile::TempDir::new().unwrap();
        let keypair = generate_keypair();
        let attestation = sample_attestation();

        let signed = SignedAttestation::sign(attestation, &keypair).unwrap();

        // Write and read signed attestation
        let path = dir.path().join("signed_attestation.json");
        signed.write_to_file(&path).unwrap();
        let loaded = SignedAttestation::from_file(&path).unwrap();
        assert_eq!(loaded.signature, signed.signature);

        // Write and read verification record
        let record = AttestationVerification::pass(
            "run-123".to_string(),
            "job-456".to_string(),
            "fp".to_string(),
            None,
        );
        let verify_path = dir.path().join("attestation_verification.json");
        record.write_to_file(&verify_path).unwrap();
    }
}
