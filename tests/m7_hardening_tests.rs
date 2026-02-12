//! M7 tests: hardening and conformance (rch-mac-b7s.10)
//!
//! Tests for M7-specific hardening features:
//! - Attestation signing (Ed25519)
//! - Attestation key pinning
//! - Ephemeral simulator provisioning (requires macOS with simctl)
//! - JSON Schema validation
//! - Golden-file assertions

use rch_xcode_lane::artifact::{
    compute_key_fingerprint, decode_signing_key, decode_verifying_key, encode_signing_key,
    encode_verifying_key, generate_keypair, Attestation, AttestationBackendIdentity,
    AttestationVerification, AttestationWorkerIdentity, SignedAttestation, SigningError,
    SIGNATURE_ALGORITHM,
};
use rch_xcode_lane::artifact::signing::VerificationResult;

// =============================================================================
// Test 1: Attestation signing (Ed25519)
// =============================================================================

#[test]
fn test_attestation_signing_valid() {
    let keypair = generate_keypair();
    let attestation = sample_attestation();

    // Sign the attestation
    let signed = SignedAttestation::sign(attestation.clone(), &keypair)
        .expect("Signing should succeed");

    // Verify with correct key
    let verifying_key = keypair.verifying_key();
    let verified = signed.verify(&verifying_key).expect("Verification should not error");
    assert!(verified, "Valid signature should verify");

    // Check fields are populated
    assert_eq!(signed.signature_algorithm, SIGNATURE_ALGORITHM);
    assert!(!signed.signature.is_empty());
    assert_eq!(signed.pubkey_fingerprint.len(), 64); // SHA-256 hex
}

#[test]
fn test_attestation_tampered_detected() {
    let keypair = generate_keypair();
    let attestation = sample_attestation();

    // Sign the attestation
    let mut signed = SignedAttestation::sign(attestation, &keypair)
        .expect("Signing should succeed");

    // Tamper with the attestation
    signed.attestation.job_id = "tampered-job-id".to_string();

    // Verify should fail (returns false, not error)
    let verifying_key = keypair.verifying_key();
    let verified = signed.verify(&verifying_key).expect("Verification should not error");
    assert!(!verified, "Tampered attestation should fail verification");
}

#[test]
fn test_attestation_wrong_key_rejected() {
    let keypair1 = generate_keypair();
    let keypair2 = generate_keypair();
    let attestation = sample_attestation();

    // Sign with key1
    let signed = SignedAttestation::sign(attestation, &keypair1)
        .expect("Signing should succeed");

    // Verify with key2 - should fail
    let wrong_key = keypair2.verifying_key();
    let verified = signed.verify(&wrong_key).expect("Verification should not error");
    assert!(!verified, "Wrong key should fail verification");
}

// =============================================================================
// Test 2: Attestation key pinning
// =============================================================================

#[test]
fn test_key_pinning_correct_fingerprint() {
    let keypair = generate_keypair();
    let verifying_key = keypair.verifying_key();
    let fingerprint = compute_key_fingerprint(&verifying_key);
    let attestation = sample_attestation();

    let signed = SignedAttestation::sign(attestation, &keypair)
        .expect("Signing should succeed");

    // Verify with correct pinned fingerprint
    let result = signed.verify_with_pinning(&verifying_key, Some(&fingerprint));
    assert!(result.is_ok());
    assert!(result.unwrap(), "Should pass with correct fingerprint");
}

#[test]
fn test_key_pinning_wrong_fingerprint() {
    let keypair = generate_keypair();
    let verifying_key = keypair.verifying_key();
    let attestation = sample_attestation();

    let signed = SignedAttestation::sign(attestation, &keypair)
        .expect("Signing should succeed");

    // Verify with wrong pinned fingerprint
    let result = signed.verify_with_pinning(&verifying_key, Some("wrong-fingerprint"));

    match result {
        Err(SigningError::FingerprintMismatch { expected, actual }) => {
            assert_eq!(expected, "wrong-fingerprint");
            assert_eq!(actual.len(), 64); // SHA-256 hex
        }
        _ => panic!("Expected FingerprintMismatch error, got {:?}", result),
    }
}

#[test]
fn test_key_pinning_no_pinning() {
    let keypair = generate_keypair();
    let verifying_key = keypair.verifying_key();
    let attestation = sample_attestation();

    let signed = SignedAttestation::sign(attestation, &keypair)
        .expect("Signing should succeed");

    // Verify without pinning - should succeed
    let result = signed.verify_with_pinning(&verifying_key, None);
    assert!(result.is_ok());
    assert!(result.unwrap(), "Should pass without pinning");
}

// =============================================================================
// Test 3: Attestation verification record
// =============================================================================

#[test]
fn test_verification_record_pass() {
    let record = AttestationVerification::pass(
        "run-123".to_string(),
        "job-456".to_string(),
        "fingerprint-abc".to_string(),
        Some("pinned-fp".to_string()),
    );

    assert_eq!(record.verification_result, VerificationResult::Pass);
    assert!(record.error_message.is_none());
    assert_eq!(record.pinned_fingerprint, Some("pinned-fp".to_string()));

    // Serialize to JSON
    let json = record.to_json().expect("Serialization should work");
    assert!(json.contains("\"verification_result\": \"pass\""));
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
    assert_eq!(record.error_message, Some("signature verification failed".to_string()));
    assert!(record.pinned_fingerprint.is_none());

    // Serialize to JSON
    let json = record.to_json().expect("Serialization should work");
    assert!(json.contains("\"verification_result\": \"fail\""));
    assert!(json.contains("signature verification failed"));
}

// =============================================================================
// Test 4: Key encoding/decoding round-trip
// =============================================================================

#[test]
fn test_key_encoding_roundtrip() {
    let keypair = generate_keypair();

    // Signing key round-trip
    let encoded = encode_signing_key(&keypair);
    let decoded = decode_signing_key(&encoded).expect("Decoding should work");
    assert_eq!(keypair.to_bytes(), decoded.to_bytes());

    // Verifying key round-trip
    let verifying = keypair.verifying_key();
    let encoded = encode_verifying_key(&verifying);
    let decoded = decode_verifying_key(&encoded).expect("Decoding should work");
    assert_eq!(verifying.as_bytes(), decoded.as_bytes());
}

#[test]
fn test_key_fingerprint_deterministic() {
    let keypair = generate_keypair();
    let verifying_key = keypair.verifying_key();

    let fp1 = compute_key_fingerprint(&verifying_key);
    let fp2 = compute_key_fingerprint(&verifying_key);

    assert_eq!(fp1, fp2, "Fingerprint should be deterministic");
    assert_eq!(fp1.len(), 64, "SHA-256 should produce 64 hex chars");
}

// =============================================================================
// Test 5: Ephemeral simulator (mock-based, actual simctl requires macOS)
// =============================================================================

#[test]
fn test_ephemeral_naming_convention() {
    use rch_worker::EPHEMERAL_PREFIX;

    let job_id = "job-12345-abc";
    let name = format!("{}{}", EPHEMERAL_PREFIX, job_id);

    assert!(name.starts_with("rch-ephemeral-"));
    assert!(name.ends_with(job_id));
    assert_eq!(name, "rch-ephemeral-job-12345-abc");
}

// =============================================================================
// Test 6: JSON Schema validation
// =============================================================================

#[test]
fn test_attestation_schema_fields() {
    let attestation = sample_attestation();
    let json = serde_json::to_value(&attestation).expect("Serialization should work");

    // Required fields per schema
    assert!(json.get("schema_version").is_some());
    assert!(json.get("schema_id").is_some());
    assert!(json.get("created_at").is_some());
    assert!(json.get("run_id").is_some());
    assert!(json.get("job_id").is_some());
    assert!(json.get("job_key").is_some());
    assert!(json.get("source_sha256").is_some());
    assert!(json.get("manifest_sha256").is_some());
    assert!(json.get("worker").is_some());
    assert!(json.get("backend").is_some());
}

#[test]
fn test_signed_attestation_schema_fields() {
    let keypair = generate_keypair();
    let attestation = sample_attestation();
    let signed = SignedAttestation::sign(attestation, &keypair)
        .expect("Signing should succeed");

    let json = serde_json::to_value(&signed).expect("Serialization should work");

    // Required fields for signed attestation
    assert!(json.get("attestation").is_some());
    assert!(json.get("signature").is_some());
    assert!(json.get("signature_algorithm").is_some());
    assert!(json.get("pubkey_fingerprint").is_some());
}

#[test]
fn test_verification_schema_fields() {
    let record = AttestationVerification::pass(
        "run-123".to_string(),
        "job-456".to_string(),
        "fp-abc".to_string(),
        None,
    );

    let json = serde_json::to_value(&record).expect("Serialization should work");

    // Required fields per schema
    assert!(json.get("schema_version").is_some());
    assert!(json.get("schema_id").is_some());
    assert!(json.get("created_at").is_some());
    assert!(json.get("run_id").is_some());
    assert!(json.get("job_id").is_some());
    assert!(json.get("verification_result").is_some());
    assert!(json.get("pubkey_fingerprint").is_some());
    assert!(json.get("signature_algorithm").is_some());
}

// =============================================================================
// Test 7: Conformance runner integration
// =============================================================================

#[test]
fn test_conformance_runner_all_categories() {
    use rch_xcode_lane::ConformanceRunner;

    let runner = ConformanceRunner::new(false);
    let report = runner.run_all();

    assert!(report.category_count > 0, "Should have categories");
    assert!(report.test_count > 0, "Should have tests");

    // All tests should pass
    assert!(report.passed, "Conformance should pass: {} failed tests", report.failed_count);
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn test_conformance_runner_specific_category() {
    use rch_xcode_lane::ConformanceRunner;

    let runner = ConformanceRunner::new(false);
    let report = runner.run_categories(&["jobspec"]);

    assert_eq!(report.category_count, 1);
    assert!(report.test_count > 0);
}

// =============================================================================
// Helper functions
// =============================================================================

fn sample_attestation() -> Attestation {
    Attestation::from_components(
        "run-123".to_string(),
        "job-456".to_string(),
        "key-abc123def456789012345678901234567890123456789012345678901234".to_string(),
        "source-sha256-hash".to_string(),
        AttestationWorkerIdentity {
            name: "worker-01".to_string(),
            fingerprint: "fp-123".to_string(),
        },
        "capabilities-hash".to_string(),
        AttestationBackendIdentity {
            name: "xcodebuild".to_string(),
            version: "15.0".to_string(),
        },
        "manifest-sha256-hash".to_string(),
    )
}
