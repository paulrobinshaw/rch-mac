//! Source bundling and canonicalization
//!
//! Implements deterministic source bundling per PLAN.md normative rules.
//! Creates canonical tar archives with sorted paths, normalized timestamps,
//! and computes source_sha256 for reproducibility.

mod exclude;
mod manifest;

pub use exclude::{ExcludeRules, ExcludeError};
pub use manifest::{SourceManifest, ManifestEntry, EntryType};

use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tar::{Builder, Header};
use walkdir::WalkDir;

/// Schema version for source_manifest.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/source_manifest@1";

/// Bundle mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleMode {
    /// Include tracked and untracked files (minus excludes)
    Worktree,
    /// Only git-index tracked files
    GitIndex,
}

impl Default for BundleMode {
    fn default() -> Self {
        BundleMode::Worktree
    }
}

/// Errors for bundling operations
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("Symlink escapes repo root: {path}")]
    SymlinkEscapesRoot { path: PathBuf },

    #[error("Exclude rules error: {0}")]
    ExcludeError(#[from] ExcludeError),

    #[error("Walk error: {0}")]
    WalkError(#[from] walkdir::Error),

    #[error("Path is not within repo root: {0}")]
    PathNotInRepo(PathBuf),
}

/// Source bundler for creating deterministic tar archives
pub struct Bundler {
    /// Root directory to bundle
    root: PathBuf,
    /// Exclusion rules
    exclude: ExcludeRules,
    /// Bundle mode
    mode: BundleMode,
    /// Whether to dereference symlinks within root
    dereference_symlinks: bool,
}

impl Bundler {
    /// Create a new bundler for the given root directory
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            exclude: ExcludeRules::default(),
            mode: BundleMode::default(),
            dereference_symlinks: false,
        }
    }

    /// Set bundle mode
    pub fn with_mode(mut self, mode: BundleMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set symlink dereferencing behavior
    pub fn with_dereference_symlinks(mut self, dereference: bool) -> Self {
        self.dereference_symlinks = dereference;
        self
    }

    /// Load .rchignore file if it exists
    pub fn with_ignore_file(mut self, path: &Path) -> Result<Self, BundleError> {
        if path.exists() {
            self.exclude = self.exclude.with_ignore_file(path)?;
        }
        Ok(self)
    }

    /// Add custom exclude patterns
    pub fn with_excludes(mut self, patterns: &[&str]) -> Result<Self, BundleError> {
        self.exclude = self.exclude.with_patterns(patterns)?;
        Ok(self)
    }

    /// Collect all files to include in the bundle
    fn collect_entries(&self) -> Result<BTreeMap<PathBuf, EntryInfo>, BundleError> {
        let mut entries = BTreeMap::new();

        for entry in WalkDir::new(&self.root)
            .follow_links(false)
            .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        {
            let entry = entry?;
            let path = entry.path();

            // Get relative path
            let rel_path = path
                .strip_prefix(&self.root)
                .map_err(|_| BundleError::PathNotInRepo(path.to_path_buf()))?;

            // Skip root itself
            if rel_path.as_os_str().is_empty() {
                continue;
            }

            // Check exclusions
            if self.exclude.is_excluded(rel_path) {
                continue;
            }

            let metadata = entry.metadata()?;
            let file_type = entry.file_type();

            let entry_info = if file_type.is_symlink() {
                let target = fs::read_link(path)?;

                // Check if symlink escapes root
                let resolved = if target.is_absolute() {
                    target.clone()
                } else {
                    path.parent().unwrap_or(path).join(&target)
                };

                let canonical = resolved.canonicalize().unwrap_or(resolved);
                if !canonical.starts_with(&self.root) {
                    return Err(BundleError::SymlinkEscapesRoot {
                        path: path.to_path_buf(),
                    });
                }

                if self.dereference_symlinks {
                    // Dereference: read the target file
                    let target_metadata = fs::metadata(&canonical)?;
                    EntryInfo {
                        entry_type: if target_metadata.is_dir() {
                            EntryType::Directory
                        } else {
                            EntryType::File
                        },
                        size: target_metadata.len(),
                        symlink_target: None,
                    }
                } else {
                    // Preserve symlink
                    EntryInfo {
                        entry_type: EntryType::Symlink,
                        size: 0,
                        symlink_target: Some(target),
                    }
                }
            } else if file_type.is_dir() {
                EntryInfo {
                    entry_type: EntryType::Directory,
                    size: 0,
                    symlink_target: None,
                }
            } else {
                EntryInfo {
                    entry_type: EntryType::File,
                    size: metadata.len(),
                    symlink_target: None,
                }
            };

            entries.insert(rel_path.to_path_buf(), entry_info);
        }

        Ok(entries)
    }

    /// Create a canonical tar archive and return the bytes and manifest
    pub fn create_bundle(&self, run_id: &str) -> Result<BundleResult, BundleError> {
        let entries = self.collect_entries()?;

        let mut tar_buffer = Vec::new();
        let mut manifest_entries = Vec::new();

        {
            let mut builder = Builder::new(&mut tar_buffer);

            // Process entries in sorted order (BTreeMap ensures this)
            for (rel_path, info) in &entries {
                let full_path = self.root.join(rel_path);

                match info.entry_type {
                    EntryType::File => {
                        let mut file = File::open(&full_path)?;
                        let mut contents = Vec::new();
                        file.read_to_end(&mut contents)?;

                        // Compute file hash
                        let file_hash = {
                            let mut hasher = Sha256::new();
                            hasher.update(&contents);
                            hex::encode(hasher.finalize())
                        };

                        // Create canonical header
                        let mut header = Header::new_gnu();
                        header.set_path(rel_path)?;
                        header.set_size(contents.len() as u64);
                        header.set_mtime(0); // Epoch
                        header.set_uid(0);
                        header.set_gid(0);
                        // Preserve executable bit, normalize others
                        let mode = if is_executable(&full_path) {
                            0o755
                        } else {
                            0o644
                        };
                        header.set_mode(mode);
                        header.set_cksum();

                        builder.append(&header, contents.as_slice())?;

                        manifest_entries.push(ManifestEntry {
                            path: rel_path.to_string_lossy().to_string(),
                            size: contents.len() as u64,
                            sha256: file_hash,
                            entry_type: EntryType::File,
                            symlink_target: None,
                        });
                    }
                    EntryType::Directory => {
                        let mut header = Header::new_gnu();
                        header.set_path(format!("{}/", rel_path.display()))?;
                        header.set_size(0);
                        header.set_mtime(0);
                        header.set_uid(0);
                        header.set_gid(0);
                        header.set_mode(0o755);
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_cksum();

                        builder.append(&header, &[] as &[u8])?;

                        manifest_entries.push(ManifestEntry {
                            path: rel_path.to_string_lossy().to_string(),
                            size: 0,
                            sha256: String::new(),
                            entry_type: EntryType::Directory,
                            symlink_target: None,
                        });
                    }
                    EntryType::Symlink => {
                        let target = info.symlink_target.as_ref().unwrap();
                        let mut header = Header::new_gnu();
                        header.set_path(rel_path)?;
                        header.set_size(0);
                        header.set_mtime(0);
                        header.set_uid(0);
                        header.set_gid(0);
                        header.set_mode(0o777);
                        header.set_entry_type(tar::EntryType::Symlink);
                        header.set_link_name(target)?;
                        header.set_cksum();

                        builder.append(&header, &[] as &[u8])?;

                        manifest_entries.push(ManifestEntry {
                            path: rel_path.to_string_lossy().to_string(),
                            size: 0,
                            sha256: String::new(),
                            entry_type: EntryType::Symlink,
                            symlink_target: Some(target.to_string_lossy().to_string()),
                        });
                    }
                }
            }

            builder.finish()?;
        }

        // Compute source_sha256
        let source_sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&tar_buffer);
            hex::encode(hasher.finalize())
        };

        // Create manifest
        let manifest = SourceManifest {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: run_id.to_string(),
            source_sha256: source_sha256.clone(),
            entries: manifest_entries,
        };

        Ok(BundleResult {
            tar_bytes: tar_buffer,
            source_sha256,
            manifest,
        })
    }
}

/// Result of creating a bundle
pub struct BundleResult {
    /// The canonical tar bytes (uncompressed)
    pub tar_bytes: Vec<u8>,
    /// SHA-256 of the tar bytes
    pub source_sha256: String,
    /// Source manifest
    pub manifest: SourceManifest,
}

impl BundleResult {
    /// Write tar to a file
    pub fn write_tar(&self, path: &Path) -> io::Result<()> {
        fs::write(path, &self.tar_bytes)
    }

    /// Write manifest to a file
    pub fn write_manifest(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }
}

/// Information about a collected entry
struct EntryInfo {
    entry_type: EntryType,
    #[allow(dead_code)] // Reserved for future size-limit checks
    size: u64,
    symlink_target: Option<PathBuf>,
}

/// Check if a file is executable
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            return metadata.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create some test files
        fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        // Create a subdirectory
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/file3.txt"), "content3").unwrap();

        // Create .rch directory
        fs::create_dir(dir.path().join(".rch")).unwrap();
        fs::write(dir.path().join(".rch/xcode.toml"), "test config").unwrap();

        dir
    }

    #[test]
    fn test_bundler_basic() {
        let dir = create_test_dir();
        let bundler = Bundler::new(dir.path().to_path_buf());

        let result = bundler.create_bundle("test-run-123").unwrap();

        assert!(!result.tar_bytes.is_empty());
        assert_eq!(result.source_sha256.len(), 64); // SHA-256 hex
        assert!(!result.manifest.entries.is_empty());
    }

    #[test]
    fn test_bundler_excludes_git() {
        let dir = create_test_dir();

        // Create .git directory
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/config"), "git config").unwrap();

        let bundler = Bundler::new(dir.path().to_path_buf());
        let result = bundler.create_bundle("test-run").unwrap();

        // .git should be excluded
        assert!(!result
            .manifest
            .entries
            .iter()
            .any(|e| e.path.starts_with(".git")));
    }

    #[test]
    fn test_bundler_deterministic() {
        let dir = create_test_dir();

        let bundler1 = Bundler::new(dir.path().to_path_buf());
        let result1 = bundler1.create_bundle("run-1").unwrap();

        let bundler2 = Bundler::new(dir.path().to_path_buf());
        let result2 = bundler2.create_bundle("run-1").unwrap();

        // Same source content should produce same hash
        assert_eq!(result1.source_sha256, result2.source_sha256);
    }

    #[test]
    fn test_manifest_entries() {
        let dir = create_test_dir();
        let bundler = Bundler::new(dir.path().to_path_buf());

        let result = bundler.create_bundle("test-run").unwrap();

        // Check that files are in manifest
        assert!(result.manifest.entries.iter().any(|e| e.path == "file1.txt"));
        assert!(result
            .manifest
            .entries
            .iter()
            .any(|e| e.path == "subdir/file3.txt"));

        // Files should have sha256 hashes
        for entry in &result.manifest.entries {
            if entry.entry_type == EntryType::File {
                assert!(!entry.sha256.is_empty());
            }
        }
    }

    #[test]
    fn test_sorted_entries() {
        let dir = create_test_dir();

        // Create files with various names to test sorting
        fs::write(dir.path().join("z_file.txt"), "z").unwrap();
        fs::write(dir.path().join("a_file.txt"), "a").unwrap();
        fs::write(dir.path().join("m_file.txt"), "m").unwrap();

        let bundler = Bundler::new(dir.path().to_path_buf());
        let result = bundler.create_bundle("test-run").unwrap();

        // Entries should be sorted
        let paths: Vec<_> = result.manifest.entries.iter().map(|e| &e.path).collect();
        let mut sorted_paths = paths.clone();
        sorted_paths.sort();
        assert_eq!(paths, sorted_paths);
    }
}
