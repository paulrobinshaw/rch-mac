//! Artifact management for RCH Xcode Lane
//!
//! Handles artifact manifests and integrity verification per PLAN.md normative spec.

mod manifest;

pub use manifest::{
    ArtifactEntry, ArtifactEntryType, ArtifactManifest, IntegrityError, ManifestError,
    EXCLUDED_FILES, SCHEMA_ID, SCHEMA_VERSION,
};
