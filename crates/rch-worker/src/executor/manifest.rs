//! Artifact manifest generation (manifest.json)
//!
//! Per bead y6s.1: Worker generates manifest.json per job listing all artifacts
//! with their SHA-256 hashes and sizes, plus an artifact_root_sha256 computed
//! via JCS canonicalization.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

/// Schema version for manifest.json
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for manifest.json
pub const MANIFEST_SCHEMA_ID: &str = "rch-xcode/manifest@1";

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
    pub created_at: String,

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

impl ArtifactManifest {
    /// Compute artifact_root_sha256 from entries using JCS
    pub fn compute_artifact_root_sha256(entries: &[ArtifactEntry]) -> Result<String, io::Error> {
        // Convert to Vec for serialization
        let entries_vec: Vec<_> = entries.to_vec();

        // Serialize entries to JCS (JSON Canonicalization Scheme)
        let jcs_bytes = serde_json_canonicalizer::to_vec(&entries_vec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

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
    pub fn collect_entries(artifact_dir: &Path) -> Result<Vec<ArtifactEntry>, io::Error> {
        let mut entries_map: BTreeMap<String, ArtifactEntry> = BTreeMap::new();

        for entry in WalkDir::new(artifact_dir)
            .follow_links(false)
            .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        {
            let entry = entry.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            let path = entry.path();

            // Get relative path
            let rel_path = path
                .strip_prefix(artifact_dir)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
                    format!("path not in root: {}", path.display())))?;

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
                // Skip symlinks and other special files
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
    ) -> Result<Self, io::Error> {
        let entries = Self::collect_entries(artifact_dir)?;
        let artifact_root_sha256 = Self::compute_artifact_root_sha256(&entries)?;

        Ok(Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            schema_id: MANIFEST_SCHEMA_ID.to_string(),
            created_at: Utc::now().to_rfc3339(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            entries,
            artifact_root_sha256,
        })
    }

    /// Write manifest.json to a directory with atomic write-then-rename
    pub fn write_to_file(&self, artifact_dir: &Path) -> Result<(), io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let final_path = artifact_dir.join("manifest.json");
        let temp_path = artifact_dir.join(".manifest.json.tmp");

        fs::write(&temp_path, &json)?;
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }

    /// Compute SHA-256 of the manifest contents (for attestation binding)
    pub fn compute_sha256(&self) -> Result<String, io::Error> {
        let json = serde_json::to_string(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        Ok(hex::encode(hasher.finalize()))
    }
}

/// Generate manifest.json for a job's artifacts
pub fn generate_manifest(
    artifact_dir: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
) -> Result<ArtifactManifest, io::Error> {
    let manifest = ArtifactManifest::from_directory(artifact_dir, run_id, job_id, job_key)?;
    manifest.write_to_file(artifact_dir)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_collect_entries_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let entries = ArtifactManifest::collect_entries(temp_dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_collect_entries_with_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create some test files
        fs::write(temp_dir.path().join("summary.json"), "{}").unwrap();
        fs::write(temp_dir.path().join("build.log"), "build output").unwrap();

        let entries = ArtifactManifest::collect_entries(temp_dir.path()).unwrap();

        assert_eq!(entries.len(), 2);
        // Entries should be sorted by path
        assert_eq!(entries[0].path, "build.log");
        assert_eq!(entries[1].path, "summary.json");

        // All should be files
        assert_eq!(entries[0].entry_type, ArtifactEntryType::File);
        assert_eq!(entries[1].entry_type, ArtifactEntryType::File);

        // Should have SHA-256 hashes
        assert!(entries[0].sha256.is_some());
        assert!(entries[1].sha256.is_some());
    }

    #[test]
    fn test_collect_entries_excludes_manifest() {
        let temp_dir = TempDir::new().unwrap();

        // Create files including excluded ones
        fs::write(temp_dir.path().join("summary.json"), "{}").unwrap();
        fs::write(temp_dir.path().join("manifest.json"), "{}").unwrap();
        fs::write(temp_dir.path().join("attestation.json"), "{}").unwrap();
        fs::write(temp_dir.path().join("job_index.json"), "{}").unwrap();

        let entries = ArtifactManifest::collect_entries(temp_dir.path()).unwrap();

        // Only summary.json should be included
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "summary.json");
    }

    #[test]
    fn test_collect_entries_with_subdirs() {
        let temp_dir = TempDir::new().unwrap();

        // Create a subdirectory
        let subdir = temp_dir.path().join("result.xcresult");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("Info.plist"), "<?xml>").unwrap();

        let entries = ArtifactManifest::collect_entries(temp_dir.path()).unwrap();

        // Should have the directory and the file
        assert_eq!(entries.len(), 2);

        // Find directory entry
        let dir_entry = entries.iter().find(|e| e.path == "result.xcresult").unwrap();
        assert_eq!(dir_entry.entry_type, ArtifactEntryType::Directory);
        assert_eq!(dir_entry.size, 0);
        assert!(dir_entry.sha256.is_none());

        // Find file entry
        let file_entry = entries.iter().find(|e| e.path.ends_with("Info.plist")).unwrap();
        assert_eq!(file_entry.entry_type, ArtifactEntryType::File);
        assert!(file_entry.sha256.is_some());
    }

    #[test]
    fn test_artifact_root_sha256_deterministic() {
        let entries = vec![
            ArtifactEntry {
                path: "a.txt".to_string(),
                size: 5,
                sha256: Some("abc".to_string()),
                entry_type: ArtifactEntryType::File,
            },
            ArtifactEntry {
                path: "b.txt".to_string(),
                size: 10,
                sha256: Some("def".to_string()),
                entry_type: ArtifactEntryType::File,
            },
        ];

        let hash1 = ArtifactManifest::compute_artifact_root_sha256(&entries).unwrap();
        let hash2 = ArtifactManifest::compute_artifact_root_sha256(&entries).unwrap();

        assert_eq!(hash1, hash2, "artifact_root_sha256 should be deterministic");
        assert_eq!(hash1.len(), 64, "Should be a 64-char hex SHA-256");
    }

    #[test]
    fn test_from_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create some test files
        fs::write(temp_dir.path().join("summary.json"), r#"{"status":"success"}"#).unwrap();
        fs::write(temp_dir.path().join("build.log"), "Build succeeded").unwrap();

        let manifest = ArtifactManifest::from_directory(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
        ).unwrap();

        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.schema_id, MANIFEST_SCHEMA_ID);
        assert_eq!(manifest.run_id, "run-001");
        assert_eq!(manifest.job_id, "job-001");
        assert_eq!(manifest.job_key, "abc123");
        assert_eq!(manifest.entries.len(), 2);
        assert!(!manifest.artifact_root_sha256.is_empty());
    }

    #[test]
    fn test_write_to_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a test file
        fs::write(temp_dir.path().join("summary.json"), "{}").unwrap();

        let manifest = ArtifactManifest::from_directory(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
        ).unwrap();

        manifest.write_to_file(temp_dir.path()).unwrap();

        // Verify file was written
        let path = temp_dir.path().join("manifest.json");
        assert!(path.exists());

        // Verify content
        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["schema_version"], MANIFEST_SCHEMA_VERSION);
        assert_eq!(parsed["schema_id"], MANIFEST_SCHEMA_ID);
    }

    #[test]
    fn test_generate_manifest() {
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        fs::write(temp_dir.path().join("summary.json"), r#"{"status":"success"}"#).unwrap();
        fs::write(temp_dir.path().join("toolchain.json"), r#"{"xcode":"15.0"}"#).unwrap();

        let manifest = generate_manifest(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
        ).unwrap();

        // File should exist
        assert!(temp_dir.path().join("manifest.json").exists());

        // Manifest should have correct entries
        assert_eq!(manifest.entries.len(), 2);
    }

    #[test]
    fn test_compute_sha256() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();

        let manifest = ArtifactManifest::from_directory(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
        ).unwrap();

        let sha1 = manifest.compute_sha256().unwrap();
        let sha2 = manifest.compute_sha256().unwrap();

        assert_eq!(sha1, sha2, "SHA-256 should be deterministic");
        assert_eq!(sha1.len(), 64, "Should be a 64-char hex SHA-256");
    }
}
