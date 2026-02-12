//! Artifact management for RCH Xcode Lane
//!
//! Handles artifact manifests, attestation, indices, integrity verification,
//! and schema versioning per PLAN.md normative spec.
//!
//! ## Schema versioning
//!
//! All artifacts include `schema_version`, `schema_id`, and `created_at` fields.
//! Schema IDs use the format `rch-xcode/<artifact-type>@<major-version>`.
//!
//! Compatibility rules:
//! - Adding optional fields: backward-compatible (no version bump needed)
//! - Removing/changing fields: requires version bump
//!
//! Unknown version handling:
//! - Same major version: parse known fields, ignore unknown (forward-compatible)
//! - Different major version: reject with diagnostic

mod attestation;
mod gc;
mod indices;
mod manifest;
pub mod schema;
pub mod signing;
mod verify;

pub use attestation::{
    compute_sha256, Attestation, AttestationBackendIdentity, AttestationBuilder,
    AttestationWorkerIdentity, ATTESTATION_SCHEMA_ID, ATTESTATION_SCHEMA_VERSION,
};
pub use indices::{
    ArtifactPointer, JobIndex, RequiredJobArtifacts, RunIndex, StepPointer,
    JOB_INDEX_SCHEMA_ID, JOB_INDEX_SCHEMA_VERSION, RUN_INDEX_SCHEMA_ID, RUN_INDEX_SCHEMA_VERSION,
};
pub use manifest::{
    ArtifactEntry, ArtifactEntryType, ArtifactManifest, IntegrityError, ManifestError,
    EXCLUDED_FILES, SCHEMA_ID, SCHEMA_VERSION,
};
pub use verify::{
    verify_artifacts, verify_manifest_consistency, VerificationError, VerificationResult,
};
pub use schema::{
    extract_and_validate_header, is_forward_compatible, load_artifact, load_artifact_from_file,
    validate_schema_compatibility, ArtifactHeader, JobScopedHeader, LoadError, RunScopedHeader,
    SchemaError, SchemaId,
};
pub use gc::{ArtifactGc, ArtifactGcResult, ArtifactStats, RetentionPolicy};
pub use signing::{
    compute_key_fingerprint, decode_signing_key, decode_verifying_key, encode_signing_key,
    encode_verifying_key, generate_keypair, AttestationVerification, SignedAttestation,
    SigningError, SigningResult, VerificationResult as SignatureVerificationResult,
    SIGNATURE_ALGORITHM, VERIFICATION_SCHEMA_ID, VERIFICATION_SCHEMA_VERSION,
};
