//! Host-side artifact manifest verification per PLAN.md normative spec
//!
//! After fetching job artifacts, the host MUST verify:
//! 1. artifact_root_sha256 matches recomputed value from entries
//! 2. sha256 and size for every file entry matches actual files
//! 3. No extra files exist beyond manifest entries + excluded triple
//!
//! If any check fails: failure_kind=ARTIFACTS, failure_subkind=INTEGRITY_MISMATCH

use std::path::Path;

use crate::artifact::{ArtifactManifest, IntegrityError, ManifestError, EXCLUDED_FILES};
use crate::summary::{FailureKind, FailureSubkind};

/// Result of host-side artifact verification
#[derive(Debug)]
pub struct VerificationResult {
    /// Whether verification passed
    pub passed: bool,

    /// Errors found during verification (empty if passed)
    pub errors: Vec<VerificationError>,

    /// Human-readable summary
    pub summary: String,
}

/// Types of verification errors
#[derive(Debug, Clone)]
pub enum VerificationError {
    /// artifact_root_sha256 mismatch
    RootHashMismatch {
        expected: String,
        actual: String,
    },

    /// File integrity error (missing, size mismatch, hash mismatch, type mismatch)
    EntryError(IntegrityError),

    /// Extra file found that's not in manifest
    ExtraFile { path: String },
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerificationError::RootHashMismatch { expected, actual } => {
                write!(
                    f,
                    "artifact_root_sha256 mismatch: expected {}, got {}",
                    &expected[..12],
                    &actual[..12]
                )
            }
            VerificationError::EntryError(err) => match err {
                IntegrityError::MissingFile { path } => {
                    write!(f, "{}: missing", path)
                }
                IntegrityError::TypeMismatch {
                    path,
                    expected,
                    actual,
                } => {
                    write!(f, "{}: expected {}, got {}", path, expected, actual)
                }
                IntegrityError::SizeMismatch {
                    path,
                    expected,
                    actual,
                } => {
                    write!(f, "{}: size mismatch ({} vs {})", path, expected, actual)
                }
                IntegrityError::HashMismatch {
                    path,
                    expected,
                    actual,
                } => {
                    write!(
                        f,
                        "{}: hash mismatch ({}... vs {}...)",
                        path,
                        &expected[..12],
                        &actual[..12]
                    )
                }
            },
            VerificationError::ExtraFile { path } => {
                write!(f, "{}: unexpected file", path)
            }
        }
    }
}

impl VerificationResult {
    /// Create a passing result
    pub fn pass() -> Self {
        Self {
            passed: true,
            errors: Vec::new(),
            summary: "Artifact verification passed".to_string(),
        }
    }

    /// Create a failing result
    pub fn fail(errors: Vec<VerificationError>) -> Self {
        let error_count = errors.len();
        let summary = if error_count == 1 {
            format!("Artifact verification failed: {}", errors[0])
        } else {
            format!(
                "Artifact verification failed: {} errors (first: {})",
                error_count, errors[0]
            )
        };

        Self {
            passed: false,
            errors,
            summary,
        }
    }

    /// Get failure info for summary.json
    pub fn failure_info(&self) -> Option<(FailureKind, FailureSubkind, Vec<String>)> {
        if self.passed {
            None
        } else {
            let error_strings: Vec<String> = self.errors.iter().map(|e| e.to_string()).collect();
            Some((
                FailureKind::Artifacts,
                FailureSubkind::IntegrityMismatch,
                error_strings,
            ))
        }
    }
}

/// Verify artifacts against their manifest
///
/// This performs the full host-side verification per PLAN.md:
/// 1. Load and verify manifest.json exists
/// 2. Recompute artifact_root_sha256 and verify match
/// 3. Verify sha256 and size for every entry
/// 4. Check for extra files beyond manifest entries + excluded triple
///
/// Returns a VerificationResult indicating pass/fail with error details.
pub fn verify_artifacts(artifact_dir: &Path) -> Result<VerificationResult, ManifestError> {
    let manifest_path = artifact_dir.join("manifest.json");

    // Load manifest
    let manifest = ArtifactManifest::from_file(&manifest_path).map_err(|e| {
        ManifestError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Failed to load manifest.json: {}", e),
        ))
    })?;

    let mut errors = Vec::new();

    // Step 1: Verify artifact_root_sha256
    let computed_root = ArtifactManifest::compute_artifact_root_sha256(&manifest.entries)?;
    if computed_root != manifest.artifact_root_sha256 {
        errors.push(VerificationError::RootHashMismatch {
            expected: manifest.artifact_root_sha256.clone(),
            actual: computed_root,
        });
    }

    // Step 2: Verify all entries (sha256, size, type)
    let entry_errors = manifest.verify_entries(artifact_dir)?;
    for err in entry_errors {
        errors.push(VerificationError::EntryError(err));
    }

    // Step 3: Check for extra files
    let extra_files = manifest.find_extra_files(artifact_dir)?;
    for path in extra_files {
        // Double-check it's not an excluded file
        if !EXCLUDED_FILES.contains(&path.as_str()) {
            errors.push(VerificationError::ExtraFile { path });
        }
    }

    if errors.is_empty() {
        Ok(VerificationResult::pass())
    } else {
        Ok(VerificationResult::fail(errors))
    }
}

/// Verify a manifest's internal consistency (without checking files)
///
/// This only verifies that artifact_root_sha256 is correct for the entries.
/// Useful for quick validation before fetching actual artifacts.
pub fn verify_manifest_consistency(manifest: &ArtifactManifest) -> Result<bool, ManifestError> {
    manifest.verify_artifact_root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::ArtifactManifest;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_artifact_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create test files
        fs::write(dir.path().join("summary.json"), r#"{"status":"ok"}"#).unwrap();
        fs::write(dir.path().join("logs.txt"), "Build output here").unwrap();

        // Create subdirectory
        fs::create_dir(dir.path().join("xcresult")).unwrap();
        fs::write(
            dir.path().join("xcresult/test_results.json"),
            r#"{"tests":[]}"#,
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_verify_valid_artifacts() {
        let dir = create_test_artifact_dir();

        // Create valid manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Verify should pass
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(result.passed);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_verify_detects_modified_file() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Modify a file
        fs::write(dir.path().join("logs.txt"), "Modified content!").unwrap();

        // Verify should fail
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(!result.passed);
        assert!(result.errors.iter().any(|e| matches!(e,
            VerificationError::EntryError(IntegrityError::HashMismatch { path, .. })
            | VerificationError::EntryError(IntegrityError::SizeMismatch { path, .. })
            if path == "logs.txt"
        )));
    }

    #[test]
    fn test_verify_detects_missing_file() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Remove a file
        fs::remove_file(dir.path().join("logs.txt")).unwrap();

        // Verify should fail
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(!result.passed);
        assert!(result.errors.iter().any(|e| matches!(e,
            VerificationError::EntryError(IntegrityError::MissingFile { path })
            if path == "logs.txt"
        )));
    }

    #[test]
    fn test_verify_detects_extra_file() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Add an extra file
        fs::write(dir.path().join("unexpected.txt"), "extra").unwrap();

        // Verify should fail
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(!result.passed);
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, VerificationError::ExtraFile { path } if path == "unexpected.txt")));
    }

    #[test]
    fn test_verify_allows_excluded_files() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Add excluded files (these should be allowed)
        fs::write(dir.path().join("attestation.json"), "{}").unwrap();
        fs::write(dir.path().join("job_index.json"), "{}").unwrap();

        // Verify should pass (excluded files are allowed)
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(result.passed);
    }

    #[test]
    fn test_verify_detects_tampered_root_hash() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let mut manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();

        // Tamper with the root hash
        manifest.artifact_root_sha256 = "0".repeat(64);

        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Verify should fail
        let result = verify_artifacts(dir.path()).unwrap();
        assert!(!result.passed);
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, VerificationError::RootHashMismatch { .. })));
    }

    #[test]
    fn test_failure_info() {
        let dir = create_test_artifact_dir();

        // Create manifest
        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();
        manifest
            .write_to_file(&dir.path().join("manifest.json"))
            .unwrap();

        // Remove a file to cause failure
        fs::remove_file(dir.path().join("logs.txt")).unwrap();

        let result = verify_artifacts(dir.path()).unwrap();
        assert!(!result.passed);

        let (kind, subkind, errors) = result.failure_info().unwrap();
        assert_eq!(kind, FailureKind::Artifacts);
        assert_eq!(subkind, FailureSubkind::IntegrityMismatch);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("logs.txt"));
    }

    #[test]
    fn test_verification_error_display() {
        let err = VerificationError::ExtraFile {
            path: "test.txt".to_string(),
        };
        assert_eq!(err.to_string(), "test.txt: unexpected file");

        let err = VerificationError::EntryError(IntegrityError::MissingFile {
            path: "missing.txt".to_string(),
        });
        assert_eq!(err.to_string(), "missing.txt: missing");
    }

    #[test]
    fn test_verify_manifest_consistency() {
        let dir = create_test_artifact_dir();

        let manifest =
            ArtifactManifest::from_directory(dir.path(), "run-123", "job-456", "key-789").unwrap();

        // Valid manifest should pass
        assert!(verify_manifest_consistency(&manifest).unwrap());

        // Tampered manifest should fail
        let mut tampered = manifest.clone();
        tampered.artifact_root_sha256 = "0".repeat(64);
        assert!(!verify_manifest_consistency(&tampered).unwrap());
    }
}
