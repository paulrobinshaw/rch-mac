//! Two-phase commit protocol for artifact writing per PLAN.md normative spec
//!
//! Implements the artifact commit protocol (bead y6s.5):
//! 1. Write all job artifacts to final paths
//! 2. Write manifest.json enumerating the final set
//! 3. Write attestation.json binding manifest
//! 4. Write job_index.json LAST (commit marker)
//!
//! Consumers treat job_index.json existence as proof of artifact set completeness.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Schema version for job_index.json (mirrors main crate)
pub const JOB_INDEX_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for job_index.json
pub const JOB_INDEX_SCHEMA_ID: &str = "rch-xcode/job_index@1";

/// Schema version for manifest.json
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for manifest.json
pub const MANIFEST_SCHEMA_ID: &str = "rch-xcode/manifest@1";

/// Schema version for attestation.json
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for attestation.json
pub const ATTESTATION_SCHEMA_ID: &str = "rch-xcode/attestation@1";

/// Errors from artifact commit operations
#[derive(Debug, Error)]
pub enum ArtifactCommitError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("JCS canonicalization error: {0}")]
    JcsError(String),

    #[error("Disk space too low: {available_mb}MB available, {required_mb}MB required")]
    DiskSpaceLow { available_mb: u64, required_mb: u64 },
}

/// Result type for artifact commit operations
pub type ArtifactCommitResult<T> = Result<T, ArtifactCommitError>;

/// Files excluded from manifest entries
pub const EXCLUDED_FILES: &[&str] = &["manifest.json", "attestation.json", "job_index.json"];

/// Minimum disk space required for commit (100MB per PLAN.md)
pub const MIN_DISK_SPACE_MB: u64 = 100;

/// Entry in the artifact manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    /// Relative path within artifact directory
    pub path: String,
    /// Size in bytes (0 for directories)
    pub size: u64,
    /// SHA-256 of file contents (null for directories)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Entry type
    #[serde(rename = "type")]
    pub entry_type: String,
}

/// Worker identity for attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerIdentity {
    /// Worker name
    pub name: String,
    /// Stable fingerprint (e.g., SSH host key fingerprint)
    pub fingerprint: String,
}

/// Backend identity for attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendIdentity {
    /// Backend name
    pub name: String,
    /// Backend version
    pub version: String,
}

/// Configuration for artifact commit
pub struct ArtifactCommitConfig {
    /// Worker identity
    pub worker: WorkerIdentity,
    /// Capabilities JSON bytes (for computing digest)
    pub capabilities_json: Vec<u8>,
    /// Backend identity
    pub backend: BackendIdentity,
}

/// Artifact committer - handles the two-phase commit protocol
pub struct ArtifactCommitter {
    config: ArtifactCommitConfig,
}

impl ArtifactCommitter {
    /// Create a new artifact committer
    pub fn new(config: ArtifactCommitConfig) -> Self {
        Self { config }
    }

    /// Check available disk space
    pub fn check_disk_space(&self, path: &Path) -> ArtifactCommitResult<()> {
        // Use statfs to check available space
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("df")
                .arg("-m")
                .arg(path)
                .output()
                .map_err(io::Error::from)?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Parse df output - second to last column in last line is available
                if let Some(last_line) = stdout.lines().last() {
                    let parts: Vec<&str> = last_line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        if let Ok(available_mb) = parts[3].parse::<u64>() {
                            if available_mb < MIN_DISK_SPACE_MB {
                                return Err(ArtifactCommitError::DiskSpaceLow {
                                    available_mb,
                                    required_mb: MIN_DISK_SPACE_MB,
                                });
                            }
                        }
                    }
                }
            }
        }

        // On other platforms, skip the check for now
        Ok(())
    }

    /// Commit artifacts for a completed job
    ///
    /// This performs the two-phase commit:
    /// 1. All job artifacts are already written
    /// 2. Write manifest.json
    /// 3. Write attestation.json
    /// 4. Write job_index.json (commit marker)
    pub fn commit(
        &self,
        artifact_dir: &Path,
        run_id: &str,
        job_id: &str,
        job_key: &str,
        source_sha256: &str,
        action: &str,
    ) -> ArtifactCommitResult<()> {
        // Check disk space before committing
        self.check_disk_space(artifact_dir)?;

        // Phase 2: Write manifest.json
        let manifest = self.build_manifest(artifact_dir, run_id, job_id, job_key)?;
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        self.atomic_write(artifact_dir, "manifest.json", manifest_json.as_bytes())?;

        // Phase 3: Write attestation.json
        let attestation = self.build_attestation(
            run_id,
            job_id,
            job_key,
            source_sha256,
            manifest_json.as_bytes(),
        )?;
        let attestation_json = serde_json::to_string_pretty(&attestation)?;
        self.atomic_write(artifact_dir, "attestation.json", attestation_json.as_bytes())?;

        // Phase 4: Write job_index.json (COMMIT MARKER)
        let job_index = self.build_job_index(artifact_dir, run_id, job_id, job_key, action)?;
        let job_index_json = serde_json::to_string_pretty(&job_index)?;
        self.atomic_write(artifact_dir, "job_index.json", job_index_json.as_bytes())?;

        Ok(())
    }

    /// Build manifest.json content
    fn build_manifest(
        &self,
        artifact_dir: &Path,
        run_id: &str,
        job_id: &str,
        job_key: &str,
    ) -> ArtifactCommitResult<serde_json::Value> {
        let entries = self.collect_entries(artifact_dir)?;
        let artifact_root_sha256 = self.compute_root_hash(&entries)?;

        Ok(serde_json::json!({
            "schema_version": MANIFEST_SCHEMA_VERSION,
            "schema_id": MANIFEST_SCHEMA_ID,
            "created_at": Utc::now().to_rfc3339(),
            "run_id": run_id,
            "job_id": job_id,
            "job_key": job_key,
            "entries": entries,
            "artifact_root_sha256": artifact_root_sha256,
        }))
    }

    /// Build attestation.json content
    fn build_attestation(
        &self,
        run_id: &str,
        job_id: &str,
        job_key: &str,
        source_sha256: &str,
        manifest_bytes: &[u8],
    ) -> ArtifactCommitResult<serde_json::Value> {
        let capabilities_digest = compute_sha256(&self.config.capabilities_json);
        let manifest_sha256 = compute_sha256(manifest_bytes);

        Ok(serde_json::json!({
            "schema_version": ATTESTATION_SCHEMA_VERSION,
            "schema_id": ATTESTATION_SCHEMA_ID,
            "created_at": Utc::now().to_rfc3339(),
            "run_id": run_id,
            "job_id": job_id,
            "job_key": job_key,
            "source_sha256": source_sha256,
            "worker": {
                "name": self.config.worker.name,
                "fingerprint": self.config.worker.fingerprint,
            },
            "capabilities_digest": capabilities_digest,
            "backend": {
                "name": self.config.backend.name,
                "version": self.config.backend.version,
            },
            "manifest_sha256": manifest_sha256,
        }))
    }

    /// Build job_index.json content
    fn build_job_index(
        &self,
        artifact_dir: &Path,
        run_id: &str,
        job_id: &str,
        job_key: &str,
        action: &str,
    ) -> ArtifactCommitResult<serde_json::Value> {
        // Check for optional artifacts
        let optional: Vec<serde_json::Value> = [
            ("metrics", "metrics.json"),
            ("executor_env", "executor_env.json"),
            ("classifier_policy", "classifier_policy.json"),
            ("events", "events.jsonl"),
            ("test_summary", "test_summary.json"),
            ("build_summary", "build_summary.json"),
            ("junit", "junit.xml"),
            ("xcresult", "result.xcresult"),
        ]
        .iter()
        .map(|(name, filename)| {
            let present = artifact_dir.join(filename).exists();
            serde_json::json!({
                "name": name,
                "path": filename,
                "present": present,
            })
        })
        .collect();

        Ok(serde_json::json!({
            "schema_version": JOB_INDEX_SCHEMA_VERSION,
            "schema_id": JOB_INDEX_SCHEMA_ID,
            "created_at": Utc::now().to_rfc3339(),
            "run_id": run_id,
            "job_id": job_id,
            "job_key": job_key,
            "action": action,
            "required": {
                "job": "job.json",
                "job_state": "job_state.json",
                "summary": "summary.json",
                "manifest": "manifest.json",
                "attestation": "attestation.json",
                "toolchain": "toolchain.json",
                "destination": "destination.json",
                "effective_config": "effective_config.json",
                "invocation": "invocation.json",
                "job_key_inputs": "job_key_inputs.json",
                "build_log": "build.log",
            },
            "optional": optional,
        }))
    }

    /// Collect manifest entries from artifact directory
    fn collect_entries(&self, artifact_dir: &Path) -> ArtifactCommitResult<Vec<ManifestEntry>> {
        let mut entries = Vec::new();

        self.walk_dir(artifact_dir, artifact_dir, &mut entries)?;

        // Sort by path
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(entries)
    }

    /// Walk directory recursively
    fn walk_dir(
        &self,
        root: &Path,
        current: &Path,
        entries: &mut Vec<ManifestEntry>,
    ) -> ArtifactCommitResult<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            let rel_path = path
                .strip_prefix(root)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

            let rel_path_str = rel_path.to_string_lossy().to_string();

            // Skip excluded files
            if EXCLUDED_FILES.contains(&rel_path_str.as_str()) {
                continue;
            }

            if file_type.is_dir() {
                entries.push(ManifestEntry {
                    path: rel_path_str.clone(),
                    size: 0,
                    sha256: None,
                    entry_type: "directory".to_string(),
                });
                self.walk_dir(root, &path, entries)?;
            } else if file_type.is_file() {
                let content = fs::read(&path)?;
                let hash = compute_sha256(&content);
                entries.push(ManifestEntry {
                    path: rel_path_str,
                    size: content.len() as u64,
                    sha256: Some(hash),
                    entry_type: "file".to_string(),
                });
            }
            // Skip symlinks and other file types
        }

        Ok(())
    }

    /// Compute artifact_root_sha256 using JCS
    fn compute_root_hash(&self, entries: &[ManifestEntry]) -> ArtifactCommitResult<String> {
        // Convert to Vec for Sized requirement
        let entries_vec: Vec<_> = entries.to_vec();
        let jcs_bytes = serde_json_canonicalizer::to_vec(&entries_vec)
            .map_err(|e| ArtifactCommitError::JcsError(e.to_string()))?;

        Ok(compute_sha256(&jcs_bytes))
    }

    /// Atomic write using write-then-rename
    fn atomic_write(&self, dir: &Path, filename: &str, content: &[u8]) -> ArtifactCommitResult<()> {
        let final_path = dir.join(filename);
        let temp_path = dir.join(format!(".{}.tmp", filename));

        fs::write(&temp_path, content)?;
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }
}

/// Compute SHA-256 of bytes and return hex string
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Check if a job directory is complete (has job_index.json)
pub fn is_job_complete(job_dir: &Path) -> bool {
    job_dir.join("job_index.json").exists()
}

/// Find orphaned job directories
///
/// Orphan = has artifacts but no job_index.json and not actively running.
/// Returns list of orphaned job directory paths.
pub fn find_orphaned_jobs(jobs_root: &Path, active_job_ids: &[String]) -> io::Result<Vec<PathBuf>> {
    let mut orphans = Vec::new();

    if !jobs_root.exists() {
        return Ok(orphans);
    }

    for entry in fs::read_dir(jobs_root)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let job_id = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Skip if actively running
        if active_job_ids.contains(&job_id) {
            continue;
        }

        // Check if artifacts directory exists but job_index.json is missing
        let artifacts_dir = path.join("artifacts");
        if artifacts_dir.exists() && !artifacts_dir.join("job_index.json").exists() {
            orphans.push(path);
        }
    }

    Ok(orphans)
}

/// Clean up orphaned job directories
///
/// Logs each cleanup at WARN level.
pub fn cleanup_orphaned_jobs(orphans: &[PathBuf]) -> io::Result<u64> {
    let mut total_size = 0u64;

    for path in orphans {
        let size = dir_size(path)?;
        total_size += size;

        // Log at WARN level (in real implementation would use tracing)
        eprintln!(
            "WARN: Cleaning up orphaned job directory: {} ({} bytes)",
            path.display(),
            size
        );

        fs::remove_dir_all(path)?;
    }

    Ok(total_size)
}

/// Calculate directory size recursively
fn dir_size(path: &Path) -> io::Result<u64> {
    let mut size = 0u64;

    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            size += fs::metadata(&path)?.len();
        } else if path.is_dir() {
            size += dir_size(&path)?;
        }
    }

    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> ArtifactCommitConfig {
        ArtifactCommitConfig {
            worker: WorkerIdentity {
                name: "test-worker".to_string(),
                fingerprint: "SHA256:abc123".to_string(),
            },
            capabilities_json: br#"{"xcode":["15.0"]}"#.to_vec(),
            backend: BackendIdentity {
                name: "xcodebuild".to_string(),
                version: "15.0".to_string(),
            },
        }
    }

    #[test]
    fn test_compute_sha256() {
        let hash = compute_sha256(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_is_job_complete() {
        let dir = TempDir::new().unwrap();

        assert!(!is_job_complete(dir.path()));

        fs::write(dir.path().join("job_index.json"), "{}").unwrap();

        assert!(is_job_complete(dir.path()));
    }

    #[test]
    fn test_find_orphaned_jobs() {
        let dir = TempDir::new().unwrap();
        let jobs_root = dir.path();

        // Create a complete job
        let complete_job = jobs_root.join("job-complete");
        let complete_artifacts = complete_job.join("artifacts");
        fs::create_dir_all(&complete_artifacts).unwrap();
        fs::write(complete_artifacts.join("job_index.json"), "{}").unwrap();

        // Create an orphaned job
        let orphan_job = jobs_root.join("job-orphan");
        let orphan_artifacts = orphan_job.join("artifacts");
        fs::create_dir_all(&orphan_artifacts).unwrap();
        fs::write(orphan_artifacts.join("summary.json"), "{}").unwrap();
        // No job_index.json

        // Create an active job (in active_job_ids)
        let active_job = jobs_root.join("job-active");
        let active_artifacts = active_job.join("artifacts");
        fs::create_dir_all(&active_artifacts).unwrap();
        // No job_index.json, but active

        let active_ids = vec!["job-active".to_string()];
        let orphans = find_orphaned_jobs(jobs_root, &active_ids).unwrap();

        assert_eq!(orphans.len(), 1);
        assert!(orphans[0].ends_with("job-orphan"));
    }

    #[test]
    fn test_atomic_write() {
        let dir = TempDir::new().unwrap();
        let committer = ArtifactCommitter::new(test_config());

        committer
            .atomic_write(dir.path(), "test.json", b"{}")
            .unwrap();

        assert!(dir.path().join("test.json").exists());
        assert!(!dir.path().join(".test.json.tmp").exists());

        let content = fs::read_to_string(dir.path().join("test.json")).unwrap();
        assert_eq!(content, "{}");
    }

    #[test]
    fn test_commit_creates_all_files() {
        let dir = TempDir::new().unwrap();
        let artifact_dir = dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        // Create some existing artifacts
        fs::write(artifact_dir.join("summary.json"), r#"{"status":"ok"}"#).unwrap();
        fs::write(artifact_dir.join("build.log"), "Build output").unwrap();

        let committer = ArtifactCommitter::new(test_config());
        committer
            .commit(
                &artifact_dir,
                "run-123",
                "job-456",
                "key-789",
                "source-hash",
                "build",
            )
            .unwrap();

        // Check all commit files exist
        assert!(artifact_dir.join("manifest.json").exists());
        assert!(artifact_dir.join("attestation.json").exists());
        assert!(artifact_dir.join("job_index.json").exists());

        // Verify manifest content
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(artifact_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["schema_id"], MANIFEST_SCHEMA_ID);
        assert!(manifest["entries"].as_array().unwrap().len() >= 2);
        assert!(manifest["artifact_root_sha256"].is_string());

        // Verify attestation content
        let attestation: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(artifact_dir.join("attestation.json")).unwrap())
                .unwrap();
        assert_eq!(attestation["schema_version"], 1);
        assert_eq!(attestation["worker"]["name"], "test-worker");
        assert!(attestation["manifest_sha256"].is_string());

        // Verify job_index content
        let job_index: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(artifact_dir.join("job_index.json")).unwrap())
                .unwrap();
        assert_eq!(job_index["schema_version"], 1);
        assert_eq!(job_index["job_id"], "job-456");
        assert!(job_index["required"].is_object());
        assert!(job_index["optional"].is_array());
    }

    #[test]
    fn test_collect_entries_excludes_commit_files() {
        let dir = TempDir::new().unwrap();
        let artifact_dir = dir.path();

        // Create files including ones that should be excluded
        fs::write(artifact_dir.join("summary.json"), "{}").unwrap();
        fs::write(artifact_dir.join("manifest.json"), "{}").unwrap();
        fs::write(artifact_dir.join("attestation.json"), "{}").unwrap();
        fs::write(artifact_dir.join("job_index.json"), "{}").unwrap();

        let committer = ArtifactCommitter::new(test_config());
        let entries = committer.collect_entries(artifact_dir).unwrap();

        // Should only include summary.json, not the excluded files
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "summary.json");
    }

    #[test]
    fn test_entries_sorted_by_path() {
        let dir = TempDir::new().unwrap();
        let artifact_dir = dir.path();

        fs::write(artifact_dir.join("z.json"), "{}").unwrap();
        fs::write(artifact_dir.join("a.json"), "{}").unwrap();
        fs::write(artifact_dir.join("m.json"), "{}").unwrap();

        let committer = ArtifactCommitter::new(test_config());
        let entries = committer.collect_entries(artifact_dir).unwrap();

        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.json", "m.json", "z.json"]);
    }

    #[test]
    fn test_directory_entries() {
        let dir = TempDir::new().unwrap();
        let artifact_dir = dir.path();

        fs::create_dir(artifact_dir.join("subdir")).unwrap();
        fs::write(artifact_dir.join("subdir/file.txt"), "content").unwrap();

        let committer = ArtifactCommitter::new(test_config());
        let entries = committer.collect_entries(artifact_dir).unwrap();

        let dir_entry = entries.iter().find(|e| e.path == "subdir").unwrap();
        assert_eq!(dir_entry.entry_type, "directory");
        assert_eq!(dir_entry.size, 0);
        assert!(dir_entry.sha256.is_none());

        let file_entry = entries.iter().find(|e| e.path == "subdir/file.txt").unwrap();
        assert_eq!(file_entry.entry_type, "file");
        assert!(file_entry.size > 0);
        assert!(file_entry.sha256.is_some());
    }

    #[test]
    fn test_cleanup_orphaned_jobs() {
        let dir = TempDir::new().unwrap();

        // Create an orphan
        let orphan = dir.path().join("orphan");
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("file.txt"), "some content").unwrap();

        let orphans = vec![orphan.clone()];
        let size = cleanup_orphaned_jobs(&orphans).unwrap();

        assert!(size > 0);
        assert!(!orphan.exists());
    }
}
