//! Artifact management for RCH Xcode Lane
//!
//! Handles artifact manifests and integrity verification per PLAN.md normative spec.

mod manifest;
mod verify;

pub use manifest::{
    ArtifactEntry, ArtifactEntryType, ArtifactManifest, IntegrityError, ManifestError,
    EXCLUDED_FILES, SCHEMA_ID, SCHEMA_VERSION,
};
pub use verify::{verify_artifacts, verify_manifest_consistency, VerificationError, VerificationResult};
