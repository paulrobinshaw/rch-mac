//! Content-addressed source bundle store.
//!
//! Stores source bundles indexed by their SHA-256 digest for deduplication
//! and fast retrieval. Uses a two-level fan-out directory structure:
//! `<store_root>/<sha256[0:2]>/<sha256>/bundle.tar`
//!
//! Features:
//! - Atomic writes via write-to-temp-then-rename
//! - Concurrent duplicate uploads handled safely (second writer discards)
//! - Content verification (content_sha256 of upload, source_sha256 after decompression)
//! - GC-safe operations (bundles referenced by running jobs are protected)

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;

/// Errors from source store operations.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("content SHA-256 mismatch: expected {expected}, got {actual}")]
    ContentHashMismatch { expected: String, actual: String },

    #[error("source SHA-256 mismatch: expected {expected}, got {actual}")]
    SourceHashMismatch { expected: String, actual: String },

    #[error("unsupported compression: {0}")]
    UnsupportedCompression(String),

    #[error("store not initialized")]
    NotInitialized,

    #[error("disk full: {available_bytes} bytes available")]
    DiskFull { available_bytes: u64 },
}

/// Metadata about a stored source bundle.
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// SHA-256 digest of the canonical source bundle.
    pub source_sha256: String,
    /// Size of the stored bundle in bytes.
    pub size: u64,
    /// When the bundle was stored.
    pub stored_at: SystemTime,
}

/// Content-addressed source store.
///
/// Thread-safe store for source bundles, indexed by SHA-256 digest.
#[derive(Debug)]
pub struct SourceStore {
    /// Root directory for the store.
    store_root: PathBuf,
    /// Set of source_sha256 values referenced by running jobs (GC protection).
    running_refs: Arc<RwLock<HashSet<String>>>,
    /// Orphan temp file cleanup threshold.
    orphan_threshold: Duration,
}

impl SourceStore {
    /// Create a new source store at the given root directory.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(store_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let store_root = store_root.as_ref().to_path_buf();

        // Create store root with appropriate permissions
        fs::create_dir_all(&store_root)?;

        // Verify the store is writable
        let test_file = store_root.join(".store_test");
        File::create(&test_file)?;
        fs::remove_file(&test_file)?;

        Ok(Self {
            store_root,
            running_refs: Arc::new(RwLock::new(HashSet::new())),
            orphan_threshold: Duration::from_secs(3600), // 1 hour default
        })
    }

    /// Get the path for a source bundle given its SHA-256 digest.
    ///
    /// Uses two-level fan-out: `<store_root>/<sha256[0:2]>/<sha256>/bundle.tar`
    fn bundle_path(&self, source_sha256: &str) -> PathBuf {
        let prefix = &source_sha256[..2.min(source_sha256.len())];
        self.store_root
            .join(prefix)
            .join(source_sha256)
            .join("bundle.tar")
    }

    /// Get the directory containing a bundle.
    fn bundle_dir(&self, source_sha256: &str) -> PathBuf {
        let prefix = &source_sha256[..2.min(source_sha256.len())];
        self.store_root.join(prefix).join(source_sha256)
    }

    /// Get the temp directory for atomic writes.
    fn temp_dir(&self) -> PathBuf {
        self.store_root.join(".tmp")
    }

    /// Check if a source bundle exists in the store.
    pub fn has_source(&self, source_sha256: &str) -> bool {
        self.bundle_path(source_sha256).exists()
    }

    /// Get metadata for a stored source bundle.
    pub fn get_metadata(&self, source_sha256: &str) -> Option<SourceMetadata> {
        let path = self.bundle_path(source_sha256);
        let metadata = fs::metadata(&path).ok()?;

        Some(SourceMetadata {
            source_sha256: source_sha256.to_string(),
            size: metadata.len(),
            stored_at: metadata.modified().ok()?,
        })
    }

    /// Store a source bundle from a reader.
    ///
    /// # Arguments
    /// * `source_sha256` - Expected SHA-256 of the decompressed bundle
    /// * `content_sha256` - Expected SHA-256 of the raw input bytes
    /// * `compression` - Compression format ("none" or "zstd")
    /// * `reader` - Source of the bundle data
    ///
    /// Returns the number of bytes written on success.
    pub fn store<R: Read>(
        &self,
        source_sha256: &str,
        content_sha256: &str,
        compression: &str,
        reader: R,
    ) -> Result<u64, StoreError> {
        // If bundle already exists, return success (idempotent)
        if self.has_source(source_sha256) {
            if let Some(meta) = self.get_metadata(source_sha256) {
                return Ok(meta.size);
            }
        }

        // Create temp directory if needed
        let temp_dir = self.temp_dir();
        fs::create_dir_all(&temp_dir)?;

        // Generate unique temp file name
        let temp_name = format!(
            ".tmp.{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let temp_path = temp_dir.join(&temp_name);

        // Write to temp file while computing content hash
        let bytes_written = match self.write_and_verify(
            &temp_path,
            source_sha256,
            content_sha256,
            compression,
            reader,
        ) {
            Ok(bytes) => bytes,
            Err(e) => {
                // Clean up temp file on error
                let _ = fs::remove_file(&temp_path);
                return Err(e);
            }
        };

        // Create final directory structure
        let bundle_dir = self.bundle_dir(source_sha256);
        if let Err(e) = fs::create_dir_all(&bundle_dir) {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreError::Io(e));
        }

        // Atomic rename to final location
        let final_path = self.bundle_path(source_sha256);
        if let Err(e) = fs::rename(&temp_path, &final_path) {
            // If rename failed because file already exists (concurrent upload),
            // that's okay - just clean up our temp file and return success
            if final_path.exists() {
                let _ = fs::remove_file(&temp_path);
                if let Some(meta) = self.get_metadata(source_sha256) {
                    return Ok(meta.size);
                }
            }
            let _ = fs::remove_file(&temp_path);
            return Err(StoreError::Io(e));
        }

        Ok(bytes_written)
    }

    /// Write data to temp file while verifying content hash.
    fn write_and_verify<R: Read>(
        &self,
        temp_path: &Path,
        source_sha256: &str,
        content_sha256: &str,
        compression: &str,
        mut reader: R,
    ) -> Result<u64, StoreError> {
        let mut temp_file = File::create(temp_path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024]; // 64KB buffer
        let mut total_bytes = 0u64;

        // For compressed data, we need to decompress before storing
        if compression == "zstd" {
            // TODO: Implement zstd decompression
            // For now, return an error for unsupported compression
            return Err(StoreError::UnsupportedCompression(compression.to_string()));
        }

        // Read, hash, and write
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            temp_file.write_all(&buffer[..n])?;
            total_bytes += n as u64;
        }

        temp_file.flush()?;

        // Verify content hash
        let computed_content_hash = hex::encode(hasher.finalize());
        if computed_content_hash != content_sha256 {
            return Err(StoreError::ContentHashMismatch {
                expected: content_sha256.to_string(),
                actual: computed_content_hash,
            });
        }

        // For uncompressed data, content_sha256 should equal source_sha256
        // (they're the same bytes)
        if compression == "none" && content_sha256 != source_sha256 {
            return Err(StoreError::SourceHashMismatch {
                expected: source_sha256.to_string(),
                actual: content_sha256.to_string(),
            });
        }

        Ok(total_bytes)
    }

    /// Open a stored bundle for reading.
    pub fn open(&self, source_sha256: &str) -> Result<BufReader<File>, StoreError> {
        let path = self.bundle_path(source_sha256);
        let file = File::open(path)?;
        Ok(BufReader::new(file))
    }

    /// Mark a source as referenced by a running job (GC protection).
    pub fn add_running_ref(&self, source_sha256: &str) {
        if let Ok(mut refs) = self.running_refs.write() {
            refs.insert(source_sha256.to_string());
        }
    }

    /// Remove a running job reference.
    pub fn remove_running_ref(&self, source_sha256: &str) {
        if let Ok(mut refs) = self.running_refs.write() {
            refs.remove(source_sha256);
        }
    }

    /// Check if a source is referenced by running jobs.
    pub fn is_running_ref(&self, source_sha256: &str) -> bool {
        self.running_refs
            .read()
            .map(|refs| refs.contains(source_sha256))
            .unwrap_or(false)
    }

    /// Clean up orphaned temp files older than the threshold.
    pub fn cleanup_orphaned_temps(&self) -> Result<usize, StoreError> {
        let temp_dir = self.temp_dir();
        if !temp_dir.exists() {
            return Ok(0);
        }

        let mut cleaned = 0;
        let now = Instant::now();

        for entry in fs::read_dir(&temp_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Only clean up files matching our temp pattern
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(".tmp.") {
                    // Check age based on modification time
                    if let Ok(metadata) = fs::metadata(&path) {
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(age) = modified.elapsed() {
                                if age > self.orphan_threshold {
                                    if fs::remove_file(&path).is_ok() {
                                        cleaned += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(cleaned)
    }

    /// Run garbage collection on the store.
    ///
    /// # Arguments
    /// * `max_age` - Remove bundles older than this duration
    /// * `max_size` - Target maximum store size in bytes (0 = unlimited)
    ///
    /// Returns the number of bundles removed.
    pub fn gc(&self, max_age: Option<Duration>, max_size: Option<u64>) -> Result<usize, StoreError> {
        let mut removed = 0;
        let mut entries = Vec::new();

        // Collect all bundles with metadata
        for prefix_entry in fs::read_dir(&self.store_root)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            // Skip non-directories and special directories
            if !prefix_path.is_dir() {
                continue;
            }
            if let Some(name) = prefix_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            for bundle_entry in fs::read_dir(&prefix_path)? {
                let bundle_entry = bundle_entry?;
                let bundle_path = bundle_entry.path();

                if !bundle_path.is_dir() {
                    continue;
                }

                if let Some(source_sha256) = bundle_path.file_name().and_then(|n| n.to_str()) {
                    // Skip if referenced by running job
                    if self.is_running_ref(source_sha256) {
                        continue;
                    }

                    let tar_path = bundle_path.join("bundle.tar");
                    if let Ok(metadata) = fs::metadata(&tar_path) {
                        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        entries.push((
                            source_sha256.to_string(),
                            bundle_path.clone(),
                            metadata.len(),
                            modified,
                        ));
                    }
                }
            }
        }

        // Sort by modification time (oldest first)
        entries.sort_by(|a, b| a.3.cmp(&b.3));

        // Apply age-based removal
        if let Some(max_age) = max_age {
            let cutoff = SystemTime::now() - max_age;
            for (sha256, path, _, modified) in &entries {
                if *modified < cutoff && !self.is_running_ref(sha256) {
                    if fs::remove_dir_all(path).is_ok() {
                        removed += 1;
                    }
                }
            }
        }

        // Apply size-based removal (remove oldest until under limit)
        if let Some(max_size) = max_size {
            if max_size > 0 {
                let mut total_size: u64 = entries.iter().map(|(_, _, size, _)| size).sum();

                for (sha256, path, size, _) in &entries {
                    if total_size <= max_size {
                        break;
                    }
                    if !self.is_running_ref(sha256) {
                        if fs::remove_dir_all(path).is_ok() {
                            total_size -= size;
                            removed += 1;
                        }
                    }
                }
            }
        }

        // Clean up empty prefix directories
        for prefix_entry in fs::read_dir(&self.store_root)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            if prefix_path.is_dir() {
                if let Some(name) = prefix_path.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') {
                        // Try to remove if empty (will fail silently if not empty)
                        let _ = fs::remove_dir(&prefix_path);
                    }
                }
            }
        }

        Ok(removed)
    }

    /// Get the total size of all stored bundles.
    pub fn total_size(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;

        for prefix_entry in fs::read_dir(&self.store_root)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            if !prefix_path.is_dir() {
                continue;
            }
            if let Some(name) = prefix_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            for bundle_entry in fs::read_dir(&prefix_path)? {
                let bundle_entry = bundle_entry?;
                let tar_path = bundle_entry.path().join("bundle.tar");
                if let Ok(metadata) = fs::metadata(&tar_path) {
                    total += metadata.len();
                }
            }
        }

        Ok(total)
    }

    /// Get the number of stored bundles.
    pub fn bundle_count(&self) -> Result<usize, StoreError> {
        let mut count = 0;

        for prefix_entry in fs::read_dir(&self.store_root)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            if !prefix_path.is_dir() {
                continue;
            }
            if let Some(name) = prefix_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            for bundle_entry in fs::read_dir(&prefix_path)? {
                let bundle_entry = bundle_entry?;
                if bundle_entry.path().join("bundle.tar").exists() {
                    count += 1;
                }
            }
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn create_test_bundle(content: &[u8]) -> (String, Cursor<Vec<u8>>) {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let sha256 = hex::encode(hasher.finalize());
        (sha256, Cursor::new(content.to_vec()))
    }

    #[test]
    fn test_store_and_retrieve() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let content = b"test bundle content";
        let (sha256, reader) = create_test_bundle(content);

        // Store the bundle
        let bytes = store
            .store(&sha256, &sha256, "none", reader)
            .unwrap();
        assert_eq!(bytes, content.len() as u64);

        // Verify it exists
        assert!(store.has_source(&sha256));

        // Read it back
        let mut stored_content = Vec::new();
        store.open(&sha256).unwrap().read_to_end(&mut stored_content).unwrap();
        assert_eq!(stored_content, content);
    }

    #[test]
    fn test_idempotent_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let content = b"test bundle content";
        let (sha256, reader1) = create_test_bundle(content);
        let (_, reader2) = create_test_bundle(content);

        // Store twice
        store.store(&sha256, &sha256, "none", reader1).unwrap();
        store.store(&sha256, &sha256, "none", reader2).unwrap();

        // Should still only have one bundle
        assert_eq!(store.bundle_count().unwrap(), 1);
    }

    #[test]
    fn test_content_hash_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let content = b"test bundle content";
        let (sha256, reader) = create_test_bundle(content);

        // Try to store with wrong content hash
        let result = store.store(&sha256, "wronghash", "none", reader);
        assert!(matches!(result, Err(StoreError::ContentHashMismatch { .. })));
    }

    #[test]
    fn test_bundle_path_structure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let sha256 = "abc123def456";
        let path = store.bundle_path(sha256);

        // Should have two-level fan-out
        let components: Vec<_> = path.components().collect();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("ab")); // First two chars
        assert!(path_str.contains(sha256)); // Full hash
        assert!(path_str.ends_with("bundle.tar"));
    }

    #[test]
    fn test_running_refs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let sha256 = "abc123";

        assert!(!store.is_running_ref(sha256));

        store.add_running_ref(sha256);
        assert!(store.is_running_ref(sha256));

        store.remove_running_ref(sha256);
        assert!(!store.is_running_ref(sha256));
    }

    #[test]
    fn test_gc_age_based() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let content = b"test bundle";
        let (sha256, reader) = create_test_bundle(content);

        store.store(&sha256, &sha256, "none", reader).unwrap();

        // GC with 0 max_age should remove everything not referenced
        let removed = store.gc(Some(Duration::ZERO), None).unwrap();
        assert_eq!(removed, 1);
        assert!(!store.has_source(&sha256));
    }

    #[test]
    fn test_gc_protects_running_refs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = SourceStore::new(temp_dir.path()).unwrap();

        let content = b"test bundle";
        let (sha256, reader) = create_test_bundle(content);

        store.store(&sha256, &sha256, "none", reader).unwrap();
        store.add_running_ref(&sha256);

        // GC should not remove referenced bundles
        let removed = store.gc(Some(Duration::ZERO), None).unwrap();
        assert_eq!(removed, 0);
        assert!(store.has_source(&sha256));
    }
}
