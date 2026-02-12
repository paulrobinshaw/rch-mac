//! Cache garbage collection and eviction
//!
//! Per PLAN.md §Caching:
//! - Worker-side cache eviction and garbage collection
//! - Eviction strategies: size-based (keep under N GB) or age-based (delete items unused for N days)
//! - Eviction MUST NOT delete caches currently locked/in-use
//! - Bundle GC MUST NOT remove bundles referenced by RUNNING jobs

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::{CacheConfig, CacheResult};
use super::lock::CacheLock;
use super::result::ResultCacheEntry;

/// Eviction policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionPolicy {
    /// Maximum total cache size in bytes (0 = unlimited)
    #[serde(default)]
    pub max_size_bytes: u64,
    /// Maximum age for unused items (0 = unlimited)
    #[serde(default)]
    pub max_age_days: u32,
    /// Whether to dry-run (log but don't delete)
    #[serde(default)]
    pub dry_run: bool,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self {
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10 GB default
            max_age_days: 7,                          // 7 days default
            dry_run: false,
        }
    }
}

impl EvictionPolicy {
    /// Create a size-based eviction policy.
    pub fn size_based(max_size_bytes: u64) -> Self {
        Self {
            max_size_bytes,
            max_age_days: 0,
            dry_run: false,
        }
    }

    /// Create an age-based eviction policy.
    pub fn age_based(max_age_days: u32) -> Self {
        Self {
            max_size_bytes: 0,
            max_age_days,
            dry_run: false,
        }
    }

    /// Set dry-run mode.
    pub fn with_dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// Result of a garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcResult {
    /// Number of entries scanned
    pub scanned: usize,
    /// Number of entries deleted
    pub deleted: usize,
    /// Bytes reclaimed
    pub bytes_reclaimed: u64,
    /// Entries skipped (locked or protected)
    pub skipped: usize,
    /// Errors encountered (non-fatal)
    pub errors: Vec<String>,
}

/// Cache entry info for GC decisions.
#[derive(Debug, Clone)]
struct CacheEntryInfo {
    path: PathBuf,
    size_bytes: u64,
    last_accessed: SystemTime,
}

/// Cache garbage collector.
pub struct CacheGc {
    config: CacheConfig,
    policy: EvictionPolicy,
    /// Set of job_ids that are currently running (protected from GC)
    protected_jobs: HashSet<String>,
}

impl CacheGc {
    /// Create a new cache garbage collector.
    pub fn new(config: CacheConfig, policy: EvictionPolicy) -> Self {
        Self {
            config,
            policy,
            protected_jobs: HashSet::new(),
        }
    }

    /// Add a job_id to the protected set (won't be evicted).
    pub fn protect_job(&mut self, job_id: &str) {
        self.protected_jobs.insert(job_id.to_string());
    }

    /// Remove a job_id from the protected set.
    pub fn unprotect_job(&mut self, job_id: &str) {
        self.protected_jobs.remove(job_id);
    }

    /// Set the protected jobs set.
    pub fn set_protected_jobs(&mut self, jobs: HashSet<String>) {
        self.protected_jobs = jobs;
    }

    /// Run garbage collection on all cache types.
    pub fn run(&self) -> CacheResult<GcResult> {
        let mut result = GcResult::default();

        // GC DerivedData caches
        let dd_result = self.gc_derived_data()?;
        result.merge(&dd_result);

        // GC SPM caches
        let spm_result = self.gc_spm()?;
        result.merge(&spm_result);

        // GC Result caches
        let result_cache_result = self.gc_results()?;
        result.merge(&result_cache_result);

        Ok(result)
    }

    /// GC DerivedData caches.
    pub fn gc_derived_data(&self) -> CacheResult<GcResult> {
        let base = self.config.cache_root
            .join(&self.config.namespace)
            .join("derived_data");

        self.gc_directory(&base, |_path, _meta| {
            // DerivedData entries don't have job_id protection
            // They're protected only by locks
            true
        })
    }

    /// GC SPM caches.
    pub fn gc_spm(&self) -> CacheResult<GcResult> {
        let base = self.config.cache_root
            .join(&self.config.namespace)
            .join("spm");

        self.gc_directory(&base, |_path, _meta| {
            // SPM caches don't have job_id protection
            true
        })
    }

    /// GC Result caches.
    pub fn gc_results(&self) -> CacheResult<GcResult> {
        let base = self.config.cache_root
            .join(&self.config.namespace)
            .join("results");

        self.gc_directory(&base, |path, _meta| {
            // Check if this result cache is protected by a running job
            let meta_path = path.join(ResultCacheEntry::METADATA_FILENAME);
            if meta_path.exists() {
                if let Ok(content) = fs::read_to_string(&meta_path) {
                    if let Ok(entry) = serde_json::from_str::<ResultCacheEntry>(&content) {
                        // Protect if original job is still running
                        if self.protected_jobs.contains(&entry.original_job_id) {
                            return false;
                        }
                    }
                }
            }
            true
        })
    }

    /// GC a directory of cache entries.
    fn gc_directory<F>(&self, base: &Path, can_delete: F) -> CacheResult<GcResult>
    where
        F: Fn(&Path, &fs::Metadata) -> bool,
    {
        let mut result = GcResult::default();

        if !base.exists() {
            return Ok(result);
        }

        // Collect all cache entries
        let mut entries = Vec::new();
        self.collect_cache_entries(base, &mut entries)?;
        result.scanned = entries.len();

        // Sort by last accessed (oldest first) for age-based and size-based eviction
        entries.sort_by_key(|e| e.last_accessed);

        // Calculate current total size
        let mut current_size: u64 = entries.iter().map(|e| e.size_bytes).sum();
        let now = SystemTime::now();

        for entry in &entries {
            let should_delete = self.should_evict(&entry, current_size, now);

            if !should_delete {
                continue;
            }

            // Check if entry is locked
            if self.is_locked(&entry.path) {
                result.skipped += 1;
                continue;
            }

            // Check if entry can be deleted (protection callback)
            let meta = match fs::metadata(&entry.path) {
                Ok(m) => m,
                Err(_) => {
                    result.errors.push(format!("Failed to stat: {}", entry.path.display()));
                    continue;
                }
            };

            if !can_delete(&entry.path, &meta) {
                result.skipped += 1;
                continue;
            }

            // Delete the entry
            if self.policy.dry_run {
                eprintln!("[gc] DRY-RUN: Would delete {} ({} bytes)",
                    entry.path.display(), entry.size_bytes);
            } else {
                if let Err(e) = fs::remove_dir_all(&entry.path) {
                    result.errors.push(format!("Failed to delete {}: {}", entry.path.display(), e));
                    continue;
                }
                eprintln!("[gc] Deleted {} ({} bytes)", entry.path.display(), entry.size_bytes);
            }

            result.deleted += 1;
            result.bytes_reclaimed += entry.size_bytes;
            current_size = current_size.saturating_sub(entry.size_bytes);
        }

        Ok(result)
    }

    /// Collect all cache entries recursively.
    fn collect_cache_entries(&self, dir: &Path, entries: &mut Vec<CacheEntryInfo>) -> CacheResult<()> {
        if !dir.exists() || !dir.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            // Check if this is a cache entry (has lock file or metadata)
            let is_cache_entry = path.join(".rch_cache.lock").exists()
                || path.join(".rch_cache_meta.json").exists();

            if is_cache_entry {
                let size = self.dir_size(&path).unwrap_or(0);
                let last_accessed = self.get_last_accessed(&path);

                entries.push(CacheEntryInfo {
                    path,
                    size_bytes: size,
                    last_accessed,
                });
            } else {
                // Recurse into subdirectory
                self.collect_cache_entries(&path, entries)?;
            }
        }

        Ok(())
    }

    /// Check if a cache entry should be evicted.
    fn should_evict(&self, entry: &CacheEntryInfo, current_total_size: u64, now: SystemTime) -> bool {
        // Age-based eviction
        if self.policy.max_age_days > 0 {
            let max_age = Duration::from_secs(self.policy.max_age_days as u64 * 24 * 60 * 60);
            if let Ok(age) = now.duration_since(entry.last_accessed) {
                if age > max_age {
                    return true;
                }
            }
        }

        // Size-based eviction (only if over limit)
        if self.policy.max_size_bytes > 0 && current_total_size > self.policy.max_size_bytes {
            return true;
        }

        false
    }

    /// Check if a cache directory is currently locked.
    fn is_locked(&self, path: &Path) -> bool {
        // Try to acquire the lock with a very short timeout
        // If we can't, it's locked by someone else
        match CacheLock::acquire(path, Duration::from_millis(1)) {
            Ok(_lock) => {
                // We got the lock, so it wasn't locked - drop it immediately
                false
            }
            Err(_) => {
                // Couldn't acquire lock - either locked or timeout
                true
            }
        }
    }

    /// Get the last accessed time for a cache directory.
    fn get_last_accessed(&self, path: &Path) -> SystemTime {
        // Use the most recent mtime of any file in the directory
        let mut latest = SystemTime::UNIX_EPOCH;

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        if mtime > latest {
                            latest = mtime;
                        }
                    }
                }
            }
        }

        // Fall back to directory mtime if no files
        if latest == SystemTime::UNIX_EPOCH {
            if let Ok(meta) = fs::metadata(path) {
                if let Ok(mtime) = meta.modified() {
                    latest = mtime;
                }
            }
        }

        latest
    }

    /// Calculate directory size recursively.
    fn dir_size(&self, path: &Path) -> CacheResult<u64> {
        let mut size = 0;
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    size += self.dir_size(&path)?;
                } else {
                    size += entry.metadata()?.len();
                }
            }
        }
        Ok(size)
    }
}

impl GcResult {
    /// Merge another GcResult into this one.
    fn merge(&mut self, other: &GcResult) {
        self.scanned += other.scanned;
        self.deleted += other.deleted;
        self.bytes_reclaimed += other.bytes_reclaimed;
        self.skipped += other.skipped;
        self.errors.extend(other.errors.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_test_config(temp_dir: &TempDir) -> CacheConfig {
        CacheConfig {
            cache_root: temp_dir.path().join("caches"),
            namespace: "test-repo".to_string(),
            lock_timeout: Duration::from_secs(5),
        }
    }

    fn create_cache_entry(path: &Path, content_size: usize) {
        fs::create_dir_all(path).unwrap();
        // Create lock file marker
        fs::write(path.join(".rch_cache.lock"), "").unwrap();
        // Create some content
        fs::write(path.join("data.bin"), vec![0u8; content_size]).unwrap();
    }

    fn create_result_cache_entry(path: &Path, job_id: &str) {
        use crate::executor::ArtifactProfile;

        fs::create_dir_all(path).unwrap();
        let entry = ResultCacheEntry::new("key", job_id, "run", ArtifactProfile::Rich);
        fs::write(
            path.join(ResultCacheEntry::METADATA_FILENAME),
            serde_json::to_string(&entry).unwrap(),
        ).unwrap();
        fs::write(path.join("data.bin"), "content").unwrap();
    }

    #[test]
    fn test_eviction_policy_default() {
        let policy = EvictionPolicy::default();
        assert_eq!(policy.max_size_bytes, 10 * 1024 * 1024 * 1024);
        assert_eq!(policy.max_age_days, 7);
        assert!(!policy.dry_run);
    }

    #[test]
    fn test_eviction_policy_size_based() {
        let policy = EvictionPolicy::size_based(5 * 1024 * 1024 * 1024);
        assert_eq!(policy.max_size_bytes, 5 * 1024 * 1024 * 1024);
        assert_eq!(policy.max_age_days, 0);
    }

    #[test]
    fn test_eviction_policy_age_based() {
        let policy = EvictionPolicy::age_based(30);
        assert_eq!(policy.max_size_bytes, 0);
        assert_eq!(policy.max_age_days, 30);
    }

    #[test]
    fn test_gc_result_merge() {
        let mut result1 = GcResult {
            scanned: 10,
            deleted: 3,
            bytes_reclaimed: 1000,
            skipped: 2,
            errors: vec!["error1".to_string()],
        };

        let result2 = GcResult {
            scanned: 5,
            deleted: 2,
            bytes_reclaimed: 500,
            skipped: 1,
            errors: vec!["error2".to_string()],
        };

        result1.merge(&result2);

        assert_eq!(result1.scanned, 15);
        assert_eq!(result1.deleted, 5);
        assert_eq!(result1.bytes_reclaimed, 1500);
        assert_eq!(result1.skipped, 3);
        assert_eq!(result1.errors.len(), 2);
    }

    #[test]
    fn test_gc_empty_cache() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let policy = EvictionPolicy::default();
        let gc = CacheGc::new(config, policy);

        let result = gc.run().unwrap();
        assert_eq!(result.scanned, 0);
        assert_eq!(result.deleted, 0);
    }

    #[test]
    fn test_gc_size_based_eviction() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Create cache entries
        let dd_base = config.cache_root
            .join(&config.namespace)
            .join("derived_data")
            .join("per_job");

        create_cache_entry(&dd_base.join("job-1"), 1000);
        create_cache_entry(&dd_base.join("job-2"), 1000);
        create_cache_entry(&dd_base.join("job-3"), 1000);

        // Set max size to 2000 bytes (should evict 1 entry)
        let policy = EvictionPolicy::size_based(2000);
        let gc = CacheGc::new(config, policy);

        let result = gc.run().unwrap();
        assert_eq!(result.scanned, 3);
        assert!(result.deleted >= 1, "Should delete at least 1 entry");
    }

    #[test]
    fn test_gc_dry_run() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Create cache entry
        let dd_base = config.cache_root
            .join(&config.namespace)
            .join("derived_data")
            .join("per_job");
        create_cache_entry(&dd_base.join("job-1"), 1000);

        // Set very small max size but with dry_run
        let policy = EvictionPolicy::size_based(100).with_dry_run();
        let gc = CacheGc::new(config.clone(), policy);

        let result = gc.run().unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.deleted, 1); // Counted as deleted but not actually removed

        // Verify entry still exists
        assert!(dd_base.join("job-1").exists(), "Entry should still exist in dry-run");
    }

    #[test]
    fn test_gc_protects_running_jobs() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        // Create result cache entries
        let results_base = config.cache_root
            .join(&config.namespace)
            .join("results");
        create_result_cache_entry(&results_base.join("key-1"), "running-job-id");
        create_result_cache_entry(&results_base.join("key-2"), "completed-job-id");

        // Small size limit to trigger eviction
        let policy = EvictionPolicy::size_based(10);
        let mut gc = CacheGc::new(config.clone(), policy);

        // Protect the running job
        gc.protect_job("running-job-id");

        let result = gc.run().unwrap();

        // The protected job should be skipped
        assert!(result.skipped >= 1 || results_base.join("key-1").exists(),
            "Protected job should not be deleted");
    }

    #[test]
    fn test_gc_protect_unprotect() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let policy = EvictionPolicy::default();
        let mut gc = CacheGc::new(config, policy);

        gc.protect_job("job-1");
        gc.protect_job("job-2");
        assert!(gc.protected_jobs.contains("job-1"));
        assert!(gc.protected_jobs.contains("job-2"));

        gc.unprotect_job("job-1");
        assert!(!gc.protected_jobs.contains("job-1"));
        assert!(gc.protected_jobs.contains("job-2"));
    }

    #[test]
    fn test_gc_set_protected_jobs() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let policy = EvictionPolicy::default();
        let mut gc = CacheGc::new(config, policy);

        let jobs: HashSet<String> = vec!["job-a".to_string(), "job-b".to_string()]
            .into_iter()
            .collect();
        gc.set_protected_jobs(jobs.clone());

        assert_eq!(gc.protected_jobs, jobs);
    }

    #[test]
    fn test_gc_derived_data_only() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        let dd_base = config.cache_root
            .join(&config.namespace)
            .join("derived_data")
            .join("shared");
        create_cache_entry(&dd_base.join("toolchain-1"), 500);

        let policy = EvictionPolicy::size_based(100);
        let gc = CacheGc::new(config, policy);

        let result = gc.gc_derived_data().unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.deleted, 1);
    }

    #[test]
    fn test_gc_spm_only() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        let spm_base = config.cache_root
            .join(&config.namespace)
            .join("spm")
            .join("toolchain-1");
        create_cache_entry(&spm_base.join("hash-1"), 500);

        let policy = EvictionPolicy::size_based(100);
        let gc = CacheGc::new(config, policy);

        let result = gc.gc_spm().unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.deleted, 1);
    }

    #[test]
    fn test_gc_results_only() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        let results_base = config.cache_root
            .join(&config.namespace)
            .join("results");
        create_result_cache_entry(&results_base.join("key-1"), "old-job");

        let policy = EvictionPolicy::size_based(10);
        let gc = CacheGc::new(config, policy);

        let result = gc.gc_results().unwrap();
        assert_eq!(result.scanned, 1);
        assert_eq!(result.deleted, 1);
    }
}
