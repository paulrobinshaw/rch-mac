//! Result cache for job outputs
//!
//! Per PLAN.md §Caching:
//! - Optional result cache on worker keyed by job_key
//! - If present and complete, submit may be satisfied by materializing from cache
//! - Must rewrite run_id, job_id, created_at, job_key in all artifacts
//! - Must record cached_from_job_id in summary.json
//! - Must emit correct attestation.json for new job_id
//! - Profile-aware: cached artifact_profile must be >= requested profile

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::executor::ArtifactProfile;
use super::lock::CacheLock;
use super::{CacheConfig, CacheError, CacheResult};

/// Metadata for a cached result entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultCacheEntry {
    /// The job_key this result is keyed by
    pub job_key: String,
    /// The original job_id when this result was created
    pub original_job_id: String,
    /// The original run_id when this result was created
    pub original_run_id: String,
    /// Artifact profile this cache entry satisfies
    pub artifact_profile: ArtifactProfile,
    /// When this cache entry was created
    pub created_at: String,
    /// When this cache entry expires (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl ResultCacheEntry {
    /// Metadata file name in the cache directory.
    pub const METADATA_FILENAME: &'static str = ".rch_cache_meta.json";

    /// Create a new cache entry metadata.
    pub fn new(
        job_key: &str,
        original_job_id: &str,
        original_run_id: &str,
        artifact_profile: ArtifactProfile,
    ) -> Self {
        Self {
            job_key: job_key.to_string(),
            original_job_id: original_job_id.to_string(),
            original_run_id: original_run_id.to_string(),
            artifact_profile,
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: None,
        }
    }

    /// Create with an expiry time.
    pub fn with_expiry(mut self, expires_at: &str) -> Self {
        self.expires_at = Some(expires_at.to_string());
        self
    }

    /// Check if this entry satisfies the requested profile.
    ///
    /// A cached entry satisfies a request if its profile is >= requested.
    /// Rich satisfies both Rich and Minimal. Minimal only satisfies Minimal.
    pub fn satisfies_profile(&self, requested: ArtifactProfile) -> bool {
        match (self.artifact_profile, requested) {
            // Rich satisfies everything
            (ArtifactProfile::Rich, _) => true,
            // Minimal only satisfies Minimal
            (ArtifactProfile::Minimal, ArtifactProfile::Minimal) => true,
            (ArtifactProfile::Minimal, ArtifactProfile::Rich) => false,
        }
    }

    /// Check if this entry has expired.
    pub fn is_expired(&self) -> bool {
        if let Some(ref expires_at) = self.expires_at {
            if let Ok(expires) = chrono::DateTime::parse_from_rfc3339(expires_at) {
                return chrono::Utc::now() > expires;
            }
        }
        false
    }
}

/// Result cache manager.
///
/// Provides methods to store and retrieve cached job results.
pub struct ResultCache {
    config: CacheConfig,
    /// Active lock (held while cache is in use)
    #[allow(dead_code)]
    lock: Option<CacheLock>,
}

impl ResultCache {
    /// Create a new result cache manager.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            lock: None,
        }
    }

    /// Get the cache path for a job_key if a valid cached result exists.
    ///
    /// Returns the cache directory path and metadata if:
    /// - A cache entry exists for this job_key
    /// - The cached artifact_profile satisfies the requested profile
    /// - The entry has not expired
    ///
    /// # Arguments
    /// * `job_key` - The job key to look up
    /// * `requested_profile` - The artifact profile being requested
    ///
    /// # Returns
    /// * `Ok(Some((path, entry)))` - Valid cached result found
    /// * `Ok(None)` - No valid cached result
    pub fn get_cached(
        &mut self,
        job_key: &str,
        requested_profile: ArtifactProfile,
    ) -> CacheResult<Option<(PathBuf, ResultCacheEntry)>> {
        let cache_path = self.cache_path_for_key(job_key);

        if !cache_path.exists() {
            return Ok(None);
        }

        // Read metadata
        let meta_path = cache_path.join(ResultCacheEntry::METADATA_FILENAME);
        if !meta_path.exists() {
            return Ok(None);
        }

        let meta_content = fs::read_to_string(&meta_path)?;
        let entry: ResultCacheEntry = serde_json::from_str(&meta_content)
            .map_err(|e| CacheError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid cache metadata: {}", e),
            )))?;

        // Check if expired
        if entry.is_expired() {
            return Ok(None);
        }

        // Check if profile is sufficient
        if !entry.satisfies_profile(requested_profile) {
            return Ok(None);
        }

        // Acquire lock before returning
        let lock = CacheLock::acquire(&cache_path, self.config.lock_timeout)?;
        self.lock = Some(lock);

        Ok(Some((cache_path, entry)))
    }

    /// Store artifacts in the result cache.
    ///
    /// Creates a cache entry for the given job_key. The caller should copy
    /// artifacts to the returned path.
    ///
    /// # Arguments
    /// * `job_key` - The job key to cache under
    /// * `job_id` - The original job_id
    /// * `run_id` - The original run_id
    /// * `artifact_profile` - The artifact profile being cached
    ///
    /// # Returns
    /// * `Ok(path)` - The cache directory to store artifacts in
    pub fn store(
        &mut self,
        job_key: &str,
        job_id: &str,
        run_id: &str,
        artifact_profile: ArtifactProfile,
    ) -> CacheResult<PathBuf> {
        let cache_path = self.cache_path_for_key(job_key);

        // Create directory
        fs::create_dir_all(&cache_path)?;

        // Acquire lock
        let lock = CacheLock::acquire(&cache_path, self.config.lock_timeout)?;
        self.lock = Some(lock);

        // Write metadata
        let entry = ResultCacheEntry::new(job_key, job_id, run_id, artifact_profile);
        let meta_path = cache_path.join(ResultCacheEntry::METADATA_FILENAME);
        let meta_content = serde_json::to_string_pretty(&entry)
            .map_err(|e| CacheError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to serialize metadata: {}", e),
            )))?;
        fs::write(&meta_path, meta_content)?;

        Ok(cache_path)
    }

    /// Remove a cached result.
    pub fn remove(&mut self, job_key: &str) -> CacheResult<bool> {
        let cache_path = self.cache_path_for_key(job_key);

        if !cache_path.exists() {
            return Ok(false);
        }

        // Acquire lock first
        let lock = CacheLock::acquire(&cache_path, self.config.lock_timeout)?;

        // Remove the directory
        fs::remove_dir_all(&cache_path)?;

        // Release lock (it will be dropped automatically, but the file is gone)
        drop(lock);

        Ok(true)
    }

    /// Release the lock (for explicit cleanup).
    pub fn release_lock(&mut self) {
        self.lock = None;
    }

    /// Get the cache path for a job_key.
    ///
    /// Layout: caches/<namespace>/results/<job_key>/
    fn cache_path_for_key(&self, job_key: &str) -> PathBuf {
        self.config.cache_root
            .join(&self.config.namespace)
            .join("results")
            .join(job_key)
    }

    /// Get the base cache path for result caches.
    pub fn base_path(&self) -> PathBuf {
        self.config.cache_root
            .join(&self.config.namespace)
            .join("results")
    }

    /// List all cached results.
    pub fn list_caches(&self) -> CacheResult<Vec<(PathBuf, Option<ResultCacheEntry>)>> {
        let base = self.base_path();
        if !base.exists() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let path = entry.path();
            let meta_path = path.join(ResultCacheEntry::METADATA_FILENAME);

            let metadata = if meta_path.exists() {
                fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            } else {
                None
            };

            results.push((path, metadata));
        }

        Ok(results)
    }

    /// Get cache statistics.
    pub fn cache_stats(&self) -> CacheResult<ResultCacheStats> {
        let caches = self.list_caches()?;
        let mut total_size: u64 = 0;
        let mut expired_count = 0;

        for (path, metadata) in &caches {
            total_size += Self::dir_size(path)?;
            if let Some(meta) = metadata {
                if meta.is_expired() {
                    expired_count += 1;
                }
            }
        }

        Ok(ResultCacheStats {
            count: caches.len(),
            expired_count,
            total_size_bytes: total_size,
        })
    }

    /// Calculate the size of a directory recursively.
    fn dir_size(path: &Path) -> CacheResult<u64> {
        let mut size = 0;
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    size += Self::dir_size(&path)?;
                } else {
                    size += entry.metadata()?.len();
                }
            }
        }
        Ok(size)
    }
}

/// Result cache statistics.
#[derive(Debug, Clone)]
pub struct ResultCacheStats {
    /// Number of cache entries
    pub count: usize,
    /// Number of expired entries
    pub expired_count: usize,
    /// Total size in bytes
    pub total_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    fn make_test_config(temp_dir: &TempDir) -> CacheConfig {
        CacheConfig {
            cache_root: temp_dir.path().join("caches"),
            namespace: "test-repo".to_string(),
            lock_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn test_cache_entry_new() {
        let entry = ResultCacheEntry::new(
            "job-key-123",
            "job-id-456",
            "run-id-789",
            ArtifactProfile::Rich,
        );

        assert_eq!(entry.job_key, "job-key-123");
        assert_eq!(entry.original_job_id, "job-id-456");
        assert_eq!(entry.original_run_id, "run-id-789");
        assert_eq!(entry.artifact_profile, ArtifactProfile::Rich);
        assert!(entry.expires_at.is_none());
    }

    #[test]
    fn test_cache_entry_with_expiry() {
        let entry = ResultCacheEntry::new(
            "key",
            "job",
            "run",
            ArtifactProfile::Minimal,
        ).with_expiry("2030-01-01T00:00:00Z");

        assert_eq!(entry.expires_at, Some("2030-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_cache_entry_satisfies_profile_rich() {
        let entry = ResultCacheEntry::new("k", "j", "r", ArtifactProfile::Rich);

        assert!(entry.satisfies_profile(ArtifactProfile::Rich));
        assert!(entry.satisfies_profile(ArtifactProfile::Minimal));
    }

    #[test]
    fn test_cache_entry_satisfies_profile_minimal() {
        let entry = ResultCacheEntry::new("k", "j", "r", ArtifactProfile::Minimal);

        assert!(entry.satisfies_profile(ArtifactProfile::Minimal));
        assert!(!entry.satisfies_profile(ArtifactProfile::Rich));
    }

    #[test]
    fn test_cache_entry_is_expired_none() {
        let entry = ResultCacheEntry::new("k", "j", "r", ArtifactProfile::Rich);
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_cache_entry_is_expired_future() {
        let entry = ResultCacheEntry::new("k", "j", "r", ArtifactProfile::Rich)
            .with_expiry("2050-01-01T00:00:00Z");
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_cache_entry_is_expired_past() {
        let entry = ResultCacheEntry::new("k", "j", "r", ArtifactProfile::Rich)
            .with_expiry("2020-01-01T00:00:00Z");
        assert!(entry.is_expired());
    }

    #[test]
    fn test_get_cached_not_exists() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = ResultCache::new(config);

        let result = cache.get_cached("nonexistent-key", ArtifactProfile::Minimal).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_store_and_get_cached() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store a result
        {
            let mut cache = ResultCache::new(config.clone());
            let path = cache.store(
                "job-key-abc",
                "original-job-id",
                "original-run-id",
                ArtifactProfile::Rich,
            ).unwrap();

            // Write some artifacts
            fs::write(path.join("summary.json"), "{}").unwrap();
        }

        // Retrieve it
        {
            let mut cache = ResultCache::new(config);
            let result = cache.get_cached("job-key-abc", ArtifactProfile::Rich).unwrap();

            assert!(result.is_some());
            let (path, entry) = result.unwrap();
            assert!(path.exists());
            assert_eq!(entry.original_job_id, "original-job-id");
            assert_eq!(entry.original_run_id, "original-run-id");
            assert_eq!(entry.artifact_profile, ArtifactProfile::Rich);
        }
    }

    #[test]
    fn test_get_cached_profile_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store with Minimal profile
        {
            let mut cache = ResultCache::new(config.clone());
            cache.store(
                "job-key-minimal",
                "job-id",
                "run-id",
                ArtifactProfile::Minimal,
            ).unwrap();
        }

        // Try to get with Rich profile - should fail
        {
            let mut cache = ResultCache::new(config.clone());
            let result = cache.get_cached("job-key-minimal", ArtifactProfile::Rich).unwrap();
            assert!(result.is_none(), "Minimal cache should not satisfy Rich request");
        }

        // Try to get with Minimal profile - should succeed
        {
            let mut cache = ResultCache::new(config);
            let result = cache.get_cached("job-key-minimal", ArtifactProfile::Minimal).unwrap();
            assert!(result.is_some());
        }
    }

    #[test]
    fn test_get_cached_rich_satisfies_minimal() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store with Rich profile
        {
            let mut cache = ResultCache::new(config.clone());
            cache.store(
                "job-key-rich",
                "job-id",
                "run-id",
                ArtifactProfile::Rich,
            ).unwrap();
        }

        // Get with Minimal profile - should succeed
        {
            let mut cache = ResultCache::new(config);
            let result = cache.get_cached("job-key-rich", ArtifactProfile::Minimal).unwrap();
            assert!(result.is_some(), "Rich cache should satisfy Minimal request");
        }
    }

    #[test]
    fn test_remove() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store a result
        {
            let mut cache = ResultCache::new(config.clone());
            cache.store("key-to-remove", "job", "run", ArtifactProfile::Rich).unwrap();
        }

        // Remove it
        {
            let mut cache = ResultCache::new(config.clone());
            let removed = cache.remove("key-to-remove").unwrap();
            assert!(removed);
        }

        // Verify it's gone
        {
            let mut cache = ResultCache::new(config);
            let result = cache.get_cached("key-to-remove", ArtifactProfile::Rich).unwrap();
            assert!(result.is_none());
        }
    }

    #[test]
    fn test_remove_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = ResultCache::new(config);

        let removed = cache.remove("nonexistent").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_list_caches_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = ResultCache::new(config);

        let caches = cache.list_caches().unwrap();
        assert!(caches.is_empty());
    }

    #[test]
    fn test_list_caches_with_entries() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store some results
        {
            let mut cache = ResultCache::new(config.clone());
            cache.store("key-1", "job-1", "run-1", ArtifactProfile::Rich).unwrap();
            cache.release_lock();
            cache.store("key-2", "job-2", "run-2", ArtifactProfile::Minimal).unwrap();
            cache.release_lock();
        }

        // List them
        let cache = ResultCache::new(config);
        let caches = cache.list_caches().unwrap();
        assert_eq!(caches.len(), 2);

        // All should have valid metadata
        for (_, meta) in &caches {
            assert!(meta.is_some());
        }
    }

    #[test]
    fn test_cache_stats() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Store results with content
        {
            let mut cache = ResultCache::new(config.clone());
            let path1 = cache.store("stats-key-1", "j1", "r1", ArtifactProfile::Rich).unwrap();
            fs::write(path1.join("data.txt"), "some data").unwrap();
            cache.release_lock();

            let path2 = cache.store("stats-key-2", "j2", "r2", ArtifactProfile::Minimal).unwrap();
            fs::write(path2.join("data.txt"), "more data").unwrap();
        }

        let cache = ResultCache::new(config);
        let stats = cache.cache_stats().unwrap();
        assert_eq!(stats.count, 2);
        assert_eq!(stats.expired_count, 0);
        assert!(stats.total_size_bytes > 0);
    }

    #[test]
    fn test_cache_entry_serialization() {
        let entry = ResultCacheEntry::new(
            "key",
            "job",
            "run",
            ArtifactProfile::Rich,
        );

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ResultCacheEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.job_key, entry.job_key);
        assert_eq!(parsed.original_job_id, entry.original_job_id);
        assert_eq!(parsed.artifact_profile, entry.artifact_profile);
    }

    #[test]
    fn test_base_path() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = ResultCache::new(config.clone());

        let base = cache.base_path();
        assert!(base.ends_with("results"));
        assert!(base.to_string_lossy().contains(&config.namespace));
    }

    #[test]
    fn test_expired_cache_not_returned() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Manually create an expired entry
        let cache_path = config.cache_root
            .join(&config.namespace)
            .join("results")
            .join("expired-key");
        fs::create_dir_all(&cache_path).unwrap();

        let entry = ResultCacheEntry::new("expired-key", "job", "run", ArtifactProfile::Rich)
            .with_expiry("2020-01-01T00:00:00Z"); // Past date
        let meta_path = cache_path.join(ResultCacheEntry::METADATA_FILENAME);
        fs::write(&meta_path, serde_json::to_string(&entry).unwrap()).unwrap();

        // Try to get - should not return expired entry
        let mut cache = ResultCache::new(config);
        let result = cache.get_cached("expired-key", ArtifactProfile::Rich).unwrap();
        assert!(result.is_none(), "Expired cache should not be returned");
    }
}
