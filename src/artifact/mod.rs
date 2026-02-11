//! Artifact management for RCH Xcode Lane
//!
//! Handles artifact manifests, attestation, and integrity verification per PLAN.md normative spec.

mod attestation;
mod manifest;
mod verify;

pub use attestation::{
    compute_sha256, Attestation, AttestationBackendIdentity, AttestationBuilder,
    AttestationWorkerIdentity, ATTESTATION_SCHEMA_ID, ATTESTATION_SCHEMA_VERSION,
};
pub use manifest::{
    ArtifactEntry, ArtifactEntryType, ArtifactManifest, IntegrityError, ManifestError,
    EXCLUDED_FILES, SCHEMA_ID, SCHEMA_VERSION,
};
pub use verify::{
    verify_artifacts, verify_manifest_consistency, VerificationError, VerificationResult,
};
