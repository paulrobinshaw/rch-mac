//! Artifact manifest (manifest.json) per PLAN.md normative spec
//!
//! Implements the artifact manifest format with JCS-based `artifact_root_sha256`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use walkdir::WalkDir;

/// Schema version for manifest.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/manifest@1";

/// Files excluded from manifest entries (per PLAN.md)
pub const EXCLUDED_FILES: &[&str] = &["manifest.json", "attestation.json", "job_index.json"];

/// Entry type in the artifact manifest
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactEntryType {
    File,
    Directory,
}

/// A single entry in the artifact manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactEntry {
    /// Relative path within the artifact directory
    pub path: String,

    /// Size in bytes (0 for directories)
    pub size: u64,

    /// SHA-256 hash of file contents (null for directories)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Type of entry
    #[serde(rename = "type")]
    pub entry_type: ArtifactEntryType,
}

/// Artifact manifest (manifest.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactManifest {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the manifest was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash of job inputs)
    pub job_key: String,

    /// All entries in the artifact directory (sorted by path)
    pub entries: Vec<ArtifactEntry>,

    /// SHA-256 of JCS(entries) to bind the artifact set
    pub artifact_root_sha256: String,
}

/// Errors for artifact manifest operations
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("JCS canonicalization error: {0}")]
    JcsError(String),

    #[error("Walk error: {0}")]
    WalkError(#[from] walkdir::Error),

    #[error("Path is not within artifact root: {0}")]
    PathNotInRoot(String),
}

impl ArtifactManifest {
    /// Compute artifact_root_sha256 from entries using JCS
    pub fn compute_artifact_root_sha256(entries: &[ArtifactEntry]) -> Result<String, ManifestError> {
        // Convert to Vec for serialization (JCS requires Sized types)
        let entries_vec: Vec<_> = entries.to_vec();
        // Serialize entries to JCS (JSON Canonicalization Scheme)
        let jcs_bytes = serde_json_canonicalizer::to_vec(&entries_vec)
            .map_err(|e| ManifestError::JcsError(e.to_string()))?;

        // Compute SHA-256
        let mut hasher = Sha256::new();
        hasher.update(&jcs_bytes);
        Ok(hex::encode(hasher.finalize()))
    }

    /// Collect entries from an artifact directory
    ///
    /// This walks the directory, computes hashes for files, and builds
    /// a sorted list of entries. Excludes manifest.json, attestation.json,
    /// and job_index.json per PLAN.md normative spec.
    pub fn collect_entries(artifact_dir: &Path) -> Result<Vec<ArtifactEntry>, ManifestError> {
        let mut entries_map: BTreeMap<String, ArtifactEntry> = BTreeMap::new();

        for entry in WalkDir::new(artifact_dir)
            .follow_links(false)
            .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        {
            let entry = entry?;
            let path = entry.path();

            // Get relative path
            let rel_path = path
                .strip_prefix(artifact_dir)
                .map_err(|_| ManifestError::PathNotInRoot(path.display().to_string()))?;

            // Skip root itself
            if rel_path.as_os_str().is_empty() {
                continue;
            }

            let rel_path_str = rel_path.to_string_lossy().to_string();

            // Skip excluded files
            if EXCLUDED_FILES.contains(&rel_path_str.as_str()) {
                continue;
            }

            let file_type = entry.file_type();

            let artifact_entry = if file_type.is_dir() {
                ArtifactEntry {
                    path: rel_path_str.clone(),
                    size: 0,
                    sha256: None,
                    entry_type: ArtifactEntryType::Directory,
                }
            } else if file_type.is_file() {
                // Read file and compute hash
                let mut file = File::open(path)?;
                let mut contents = Vec::new();
                file.read_to_end(&mut contents)?;

                let file_hash = {
                    let mut hasher = Sha256::new();
                    hasher.update(&contents);
                    hex::encode(hasher.finalize())
                };

                ArtifactEntry {
                    path: rel_path_str.clone(),
                    size: contents.len() as u64,
                    sha256: Some(file_hash),
                    entry_type: ArtifactEntryType::File,
                }
            } else {
                // Skip symlinks and other special files in artifact manifests
                continue;
            };

            entries_map.insert(rel_path_str, artifact_entry);
        }

        // Return entries sorted lexicographically by path (BTreeMap ensures this)
        Ok(entries_map.into_values().collect())
    }

    /// Create a new manifest from an artifact directory
    pub fn from_directory(
        artifact_dir: &Path,
        run_id: &str,
        job_id: &str,
        job_key: &str,
    ) -> Result<Self, ManifestError> {
        let entries = Self::collect_entries(artifact_dir)?;
        let artifact_root_sha256 = Self::compute_artifact_root_sha256(&entries)?;

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            entries,
            artifact_root_sha256,
        })
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

    /// Verify the manifest's artifact_root_sha256 is correct
    pub fn verify_artifact_root(&self) -> Result<bool, ManifestError> {
        let computed = Self::compute_artifact_root_sha256(&self.entries)?;
        Ok(computed == self.artifact_root_sha256)
    }

    /// Verify all entries against actual files
    pub fn verify_entries(&self, artifact_dir: &Path) -> Result<Vec<IntegrityError>, ManifestError> {
        let mut errors = Vec::new();

        for entry in &self.entries {
            let full_path = artifact_dir.join(&entry.path);

            if !full_path.exists() {
                errors.push(IntegrityError::MissingFile {
                    path: entry.path.clone(),
                });
                continue;
            }

            match entry.entry_type {
                ArtifactEntryType::Directory => {
                    if !full_path.is_dir() {
                        errors.push(IntegrityError::TypeMismatch {
                            path: entry.path.clone(),
                            expected: "directory".to_string(),
                            actual: "file".to_string(),
                        });
                    }
                }
                ArtifactEntryType::File => {
                    if !full_path.is_file() {
                        errors.push(IntegrityError::TypeMismatch {
                            path: entry.path.clone(),
                            expected: "file".to_string(),
                            actual: "directory".to_string(),
                        });
                        continue;
                    }

                    // Verify size and hash
                    let metadata = fs::metadata(&full_path)?;
                    if metadata.len() != entry.size {
                        errors.push(IntegrityError::SizeMismatch {
                            path: entry.path.clone(),
                            expected: entry.size,
                            actual: metadata.len(),
                        });
                    }

                    if let Some(expected_hash) = &entry.sha256 {
                        let mut file = File::open(&full_path)?;
                        let mut contents = Vec::new();
                        file.read_to_end(&mut contents)?;

                        let actual_hash = {
                            let mut hasher = Sha256::new();
                            hasher.update(&contents);
                            hex::encode(hasher.finalize())
                        };

                        if &actual_hash != expected_hash {
                            errors.push(IntegrityError::HashMismatch {
                                path: entry.path.clone(),
                                expected: expected_hash.clone(),
                                actual: actual_hash,
                            });
                        }
                    }
                }
            }
        }

        Ok(errors)
    }

    /// Check for extra files not in manifest
    pub fn find_extra_files(&self, artifact_dir: &Path) -> Result<Vec<String>, ManifestError> {
        let mut extra_files = Vec::new();
        let manifest_paths: std::collections::HashSet<_> =
            self.entries.iter().map(|e| e.path.clone()).collect();

        for entry in WalkDir::new(artifact_dir).follow_links(false) {
            let entry = entry?;
            let path = entry.path();

            let rel_path = path
                .strip_prefix(artifact_dir)
                .map_err(|_| ManifestError::PathNotInRoot(path.display().to_string()))?;

            if rel_path.as_os_str().is_empty() {
                continue;
            }

            let rel_path_str = rel_path.to_string_lossy().to_string();

            // Skip excluded files (they're allowed to exist but not be in manifest)
            if EXCLUDED_FILES.contains(&rel_path_str.as_str()) {
                continue;
            }

            if !manifest_paths.contains(&rel_path_str) {
                extra_files.push(rel_path_str);
            }
        }

        Ok(extra_files)
    }

    /// Get total size of all file entries
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// Get count of files and directories
    pub fn entry_counts(&self) -> (usize, usize) {
        let files = self
            .entries
            .iter()
            .filter(|e| e.entry_type == ArtifactEntryType::File)
            .count();
        let dirs = self
            .entries
            .iter()
            .filter(|e| e.entry_type == ArtifactEntryType::Directory)
            .count();
        (files, dirs)
    }
}

/// Integrity verification error
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntegrityError {
    MissingFile { path: String },
    TypeMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_artifact_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create some test files
        fs::write(dir.path().join("summary.json"), r#"{"status":"ok"}"#).unwrap();
        fs::write(dir.path().join("logs.txt"), "Build output here").unwrap();

        // Create a subdirectory
        fs::create_dir(dir.path().join("xcresult")).unwrap();
        fs::write(
            dir.path().join("xcresult/test_results.json"),
            r#"{"tests":[]}"#,
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_collect_entries_basic() {
        let dir = create_test_artifact_dir();
        let entries = ArtifactManifest::collect_entries(dir.path()).unwrap();

        // Should have: logs.txt, summary.json, xcresult (dir), xcresult/test_results.json
        assert_eq!(entries.len(), 4);

        // Check entries are sorted
        let paths: Vec<_> = entries.iter().map(|e| &e.path).collect();
        let mut sorted_paths = paths.clone();
        sorted_paths.sort();
        assert_eq!(paths, sorted_paths);
    }

    #[test]
    fn test_collect_entries_excludes_special_files() {
        let dir = create_test_artifact_dir();

        // Create the excluded files
        fs::write(dir.path().join("manifest.json"), "{}").unwrap();
        fs::write(dir.path().join("attestation.json"), "{}").unwrap();
        fs::write(dir.path().join("job_index.json"), "{}").unwrap();

        let entries = ArtifactManifest::collect_entries(dir.path()).unwrap();

        // Should NOT include the excluded files
        assert!(!entries.iter().any(|e| e.path == "manifest.json"));
        assert!(!entries.iter().any(|e| e.path == "attestation.json"));
        assert!(!entries.iter().any(|e| e.path == "job_index.json"));
    }

    #[test]
    fn test_directory_entry_has_null_sha256_and_zero_size() {
        let dir = create_test_artifact_dir();
        let entries = ArtifactManifest::collect_entries(dir.path()).unwrap();

        let dir_entry = entries
            .iter()
            .find(|e| e.entry_type == ArtifactEntryType::Directory)
            .unwrap();

        assert_eq!(dir_entry.sha256, None);
        assert_eq!(dir_entry.size, 0);
    }

    #[test]
    fn test_file_entry_has_sha256_and_size() {
        let dir = create_test_artifact_dir();
        let entries = ArtifactManifest::collect_entries(dir.path()).unwrap();

        let file_entry = entries.iter().find(|e| e.path == "logs.txt").unwrap();

        assert!(file_entry.sha256.is_some());
        assert!(file_entry.size > 0);
        assert_eq!(file_entry.entry_type, ArtifactEntryType::File);
    }

    #[test]
    fn test_artifact_root_sha256_deterministic() {
        let dir = create_test_artifact_dir();

        let manifest1 = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let manifest2 = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        // Same entries should produce same artifact_root_sha256
        assert_eq!(
            manifest1.artifact_root_sha256,
            manifest2.artifact_root_sha256
        );
    }

    #[test]
    fn test_manifest_serialization() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let json = manifest.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"schema_id\": \"rch-xcode/manifest@1\""));
        assert!(json.contains("\"artifact_root_sha256\""));
    }

    #[test]
    fn test_manifest_deserialization() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let json = manifest.to_json().unwrap();
        let parsed = ArtifactManifest::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, manifest.run_id);
        assert_eq!(parsed.job_id, manifest.job_id);
        assert_eq!(parsed.entries.len(), manifest.entries.len());
        assert_eq!(parsed.artifact_root_sha256, manifest.artifact_root_sha256);
    }

    #[test]
    fn test_verify_artifact_root_valid() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        assert!(manifest.verify_artifact_root().unwrap());
    }

    #[test]
    fn test_verify_entries_valid() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let errors = manifest.verify_entries(dir.path()).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_verify_entries_detects_missing_file() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        // Remove a file
        fs::remove_file(dir.path().join("logs.txt")).unwrap();

        let errors = manifest.verify_entries(dir.path()).unwrap();
        assert!(!errors.is_empty());
        assert!(matches!(&errors[0], IntegrityError::MissingFile { path } if path == "logs.txt"));
    }

    #[test]
    fn test_verify_entries_detects_hash_mismatch() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        // Modify a file
        fs::write(dir.path().join("logs.txt"), "Modified content").unwrap();

        let errors = manifest.verify_entries(dir.path()).unwrap();
        assert!(!errors.is_empty());
        // Could be size or hash mismatch depending on content
        assert!(errors.iter().any(|e| matches!(e,
            IntegrityError::HashMismatch { path, .. } | IntegrityError::SizeMismatch { path, .. }
            if path == "logs.txt"
        )));
    }

    #[test]
    fn test_find_extra_files() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        // Add an extra file
        fs::write(dir.path().join("unexpected.txt"), "extra").unwrap();

        let extra = manifest.find_extra_files(dir.path()).unwrap();
        assert!(extra.contains(&"unexpected.txt".to_string()));
    }

    #[test]
    fn test_find_extra_files_ignores_excluded() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        // Add excluded files (these should be allowed)
        fs::write(dir.path().join("manifest.json"), "{}").unwrap();
        fs::write(dir.path().join("attestation.json"), "{}").unwrap();
        fs::write(dir.path().join("job_index.json"), "{}").unwrap();

        let extra = manifest.find_extra_files(dir.path()).unwrap();
        assert!(extra.is_empty());
    }

    #[test]
    fn test_entry_counts() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let (files, dirs) = manifest.entry_counts();
        assert_eq!(files, 3); // logs.txt, summary.json, xcresult/test_results.json
        assert_eq!(dirs, 1); // xcresult
    }

    #[test]
    fn test_total_size() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        assert!(manifest.total_size() > 0);
    }

    #[test]
    fn test_write_and_read_file() {
        let dir = create_test_artifact_dir();
        let manifest = ArtifactManifest::from_directory(
            dir.path(),
            "run-123",
            "job-456",
            "key-789",
        )
        .unwrap();

        let manifest_path = dir.path().join("manifest.json");
        manifest.write_to_file(&manifest_path).unwrap();

        let loaded = ArtifactManifest::from_file(&manifest_path).unwrap();
        assert_eq!(loaded.run_id, manifest.run_id);
        assert_eq!(loaded.artifact_root_sha256, manifest.artifact_root_sha256);
    }
}
