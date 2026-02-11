//! DerivedData cache management
//!
//! Per PLAN.md §Caching:
//! - DerivedData modes: `off` | `per_job` | `shared`
//! - `per_job` DerivedData MUST be written under a directory derived from `job_key`
//! - `shared` caches MUST use a lock to prevent concurrent writers corrupting state
//! - Directory layout: caches/<namespace>/derived_data/<mode>/<key>/...

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::lock::{CacheLock, LockError};
use super::toolchain_key::ToolchainKey;

/// Cache result type
pub type CacheResult<T> = Result<T, CacheError>;

/// Errors from cache operations
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("lock error: {0}")]
    Lock(#[from] LockError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid cache mode: {0}")]
    InvalidMode(String),

    #[error("missing namespace")]
    MissingNamespace,
}

/// DerivedData cache mode.
///
/// Per PLAN.md §Caching:
/// - `off`: no caching, clean build every time
/// - `per_job`: DerivedData dir derived from job_key, reusable for same inputs
/// - `shared`: shared DerivedData with toolchain-keyed directories and locking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedDataMode {
    /// No caching - Xcode uses its default location, cleaned each time
    Off,
    /// Per-job caching - directory derived from job_key
    PerJob,
    /// Shared caching - toolchain-keyed directory with locking
    #[default]
    Shared,
}

impl DerivedDataMode {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "per_job" => Some(Self::PerJob),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    /// Convert to string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::PerJob => "per_job",
            Self::Shared => "shared",
        }
    }
}

/// Configuration for DerivedData caching.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Root directory for all caches
    pub cache_root: PathBuf,
    /// Cache namespace (repo-specific identifier)
    pub namespace: String,
    /// Lock timeout for shared caches
    pub lock_timeout: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_root: PathBuf::from("/var/lib/rch/caches"),
            namespace: "default".to_string(),
            lock_timeout: Duration::from_secs(30),
        }
    }
}

/// DerivedData cache manager.
///
/// Provides methods to get cache paths for different modes, with proper
/// toolchain keying and locking for shared mode.
pub struct DerivedDataCache {
    config: CacheConfig,
    /// Active lock for shared mode (held while cache is in use)
    #[allow(dead_code)]
    lock: Option<CacheLock>,
}

impl DerivedDataCache {
    /// Create a new DerivedData cache manager.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            lock: None,
        }
    }

    /// Get the DerivedData path for the given mode and parameters.
    ///
    /// For `off` mode, returns `None` - Xcode uses its default.
    /// For `per_job` mode, returns a directory derived from `job_key`.
    /// For `shared` mode, returns a toolchain-keyed directory.
    ///
    /// # Arguments
    /// * `mode` - The cache mode
    /// * `job_key` - The job key (used for per_job mode)
    /// * `toolchain` - The toolchain identity (used for shared mode)
    ///
    /// # Returns
    /// * `Ok(Some(path))` - The path to use for DerivedData
    /// * `Ok(None)` - No DerivedData path (off mode)
    pub fn get_path(
        &mut self,
        mode: DerivedDataMode,
        job_key: &str,
        toolchain: &ToolchainKey,
    ) -> CacheResult<Option<PathBuf>> {
        match mode {
            DerivedDataMode::Off => Ok(None),
            DerivedDataMode::PerJob => self.get_per_job_path(job_key),
            DerivedDataMode::Shared => self.get_shared_path(toolchain),
        }
    }

    /// Get the per-job DerivedData path.
    ///
    /// Layout: caches/<namespace>/derived_data/per_job/<job_key>/
    fn get_per_job_path(&self, job_key: &str) -> CacheResult<Option<PathBuf>> {
        let path = self.config.cache_root
            .join(&self.config.namespace)
            .join("derived_data")
            .join("per_job")
            .join(job_key);

        // Create the directory
        fs::create_dir_all(&path)?;

        Ok(Some(path))
    }

    /// Get the shared DerivedData path with locking.
    ///
    /// Layout: caches/<namespace>/derived_data/shared/<toolchain_key>/
    fn get_shared_path(&mut self, toolchain: &ToolchainKey) -> CacheResult<Option<PathBuf>> {
        let path = self.config.cache_root
            .join(&self.config.namespace)
            .join("derived_data")
            .join("shared")
            .join(toolchain.to_dir_name());

        // Acquire lock on the shared cache directory
        let lock = CacheLock::acquire(&path, self.config.lock_timeout)?;
        self.lock = Some(lock);

        Ok(Some(path))
    }

    /// Release the lock (for explicit cleanup).
    pub fn release_lock(&mut self) {
        self.lock = None;
    }

    /// Get the base cache path for a given mode (without job/toolchain specifics).
    ///
    /// Useful for GC operations.
    pub fn base_path(&self, mode: DerivedDataMode) -> PathBuf {
        self.config.cache_root
            .join(&self.config.namespace)
            .join("derived_data")
            .join(mode.as_str())
    }

    /// List all cache directories for a given mode.
    pub fn list_caches(&self, mode: DerivedDataMode) -> CacheResult<Vec<PathBuf>> {
        let base = self.base_path(mode);
        if !base.exists() {
            return Ok(Vec::new());
        }

        let entries: Vec<PathBuf> = fs::read_dir(&base)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();

        Ok(entries)
    }

    /// Get cache statistics for a given mode.
    pub fn cache_stats(&self, mode: DerivedDataMode) -> CacheResult<CacheStats> {
        let caches = self.list_caches(mode)?;
        let mut total_size: u64 = 0;

        for cache in &caches {
            total_size += Self::dir_size(cache)?;
        }

        Ok(CacheStats {
            mode,
            count: caches.len(),
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

/// Cache statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Cache mode
    pub mode: DerivedDataMode,
    /// Number of cache directories
    pub count: usize,
    /// Total size in bytes
    pub total_size_bytes: u64,
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

    fn make_test_toolchain() -> ToolchainKey {
        ToolchainKey::new("16C5032a", "15.3", "arm64")
    }

    #[test]
    fn test_derived_data_mode_parsing() {
        assert_eq!(DerivedDataMode::from_str("off"), Some(DerivedDataMode::Off));
        assert_eq!(DerivedDataMode::from_str("per_job"), Some(DerivedDataMode::PerJob));
        assert_eq!(DerivedDataMode::from_str("shared"), Some(DerivedDataMode::Shared));
        assert_eq!(DerivedDataMode::from_str("SHARED"), Some(DerivedDataMode::Shared));
        assert_eq!(DerivedDataMode::from_str("invalid"), None);
    }

    #[test]
    fn test_derived_data_mode_as_str() {
        assert_eq!(DerivedDataMode::Off.as_str(), "off");
        assert_eq!(DerivedDataMode::PerJob.as_str(), "per_job");
        assert_eq!(DerivedDataMode::Shared.as_str(), "shared");
    }

    #[test]
    fn test_derived_data_mode_default() {
        assert_eq!(DerivedDataMode::default(), DerivedDataMode::Shared);
    }

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.namespace, "default");
        assert_eq!(config.lock_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_get_path_off_mode() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config);

        let path = cache.get_path(
            DerivedDataMode::Off,
            "job-key-123",
            &make_test_toolchain(),
        ).unwrap();

        assert!(path.is_none(), "Off mode should return None");
    }

    #[test]
    fn test_get_path_per_job_mode() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config.clone());

        let path = cache.get_path(
            DerivedDataMode::PerJob,
            "job-key-abc123",
            &make_test_toolchain(),
        ).unwrap().unwrap();

        // Verify path structure
        assert!(path.ends_with("job-key-abc123"));
        assert!(path.to_string_lossy().contains("per_job"));
        assert!(path.to_string_lossy().contains(&config.namespace));

        // Directory should exist
        assert!(path.exists());
    }

    #[test]
    fn test_get_path_shared_mode() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config.clone());
        let toolchain = make_test_toolchain();

        let path = cache.get_path(
            DerivedDataMode::Shared,
            "job-key-123",
            &toolchain,
        ).unwrap().unwrap();

        // Verify path structure
        assert!(path.to_string_lossy().contains("shared"));
        assert!(path.to_string_lossy().contains(&config.namespace));
        assert!(path.to_string_lossy().contains(&toolchain.to_dir_name()));

        // Directory should exist
        assert!(path.exists());

        // Lock should be held
        assert!(cache.lock.is_some());
    }

    #[test]
    fn test_release_lock() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config);

        // Acquire shared cache (gets lock)
        cache.get_path(
            DerivedDataMode::Shared,
            "job-key-123",
            &make_test_toolchain(),
        ).unwrap();

        assert!(cache.lock.is_some());

        // Release lock
        cache.release_lock();

        assert!(cache.lock.is_none());
    }

    #[test]
    fn test_base_path() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = DerivedDataCache::new(config.clone());

        let shared_base = cache.base_path(DerivedDataMode::Shared);
        assert!(shared_base.ends_with("shared"));
        assert!(shared_base.to_string_lossy().contains(&config.namespace));

        let per_job_base = cache.base_path(DerivedDataMode::PerJob);
        assert!(per_job_base.ends_with("per_job"));
    }

    #[test]
    fn test_list_caches_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = DerivedDataCache::new(config);

        let caches = cache.list_caches(DerivedDataMode::PerJob).unwrap();
        assert!(caches.is_empty());
    }

    #[test]
    fn test_list_caches_with_entries() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config);

        // Create some per-job caches
        cache.get_path(DerivedDataMode::PerJob, "job-1", &make_test_toolchain()).unwrap();
        cache.get_path(DerivedDataMode::PerJob, "job-2", &make_test_toolchain()).unwrap();
        cache.get_path(DerivedDataMode::PerJob, "job-3", &make_test_toolchain()).unwrap();

        let caches = cache.list_caches(DerivedDataMode::PerJob).unwrap();
        assert_eq!(caches.len(), 3);
    }

    #[test]
    fn test_cache_stats() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config);

        // Create some per-job caches with content
        let path1 = cache.get_path(DerivedDataMode::PerJob, "job-1", &make_test_toolchain())
            .unwrap().unwrap();
        let path2 = cache.get_path(DerivedDataMode::PerJob, "job-2", &make_test_toolchain())
            .unwrap().unwrap();

        // Write some data
        fs::write(path1.join("test.txt"), "hello").unwrap();
        fs::write(path2.join("test.txt"), "world").unwrap();

        let stats = cache.cache_stats(DerivedDataMode::PerJob).unwrap();
        assert_eq!(stats.count, 2);
        assert!(stats.total_size_bytes >= 10); // "hello" + "world"
    }

    #[test]
    fn test_per_job_uses_job_key_not_job_id() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = DerivedDataCache::new(config);

        // The same job_key should produce the same path
        let path1 = cache.get_path(
            DerivedDataMode::PerJob,
            "sha256-abc123def456",
            &make_test_toolchain(),
        ).unwrap().unwrap();

        let path2 = cache.get_path(
            DerivedDataMode::PerJob,
            "sha256-abc123def456",
            &make_test_toolchain(),
        ).unwrap().unwrap();

        assert_eq!(path1, path2, "Same job_key should produce same path");

        // Different job_key should produce different path
        let path3 = cache.get_path(
            DerivedDataMode::PerJob,
            "sha256-different789",
            &make_test_toolchain(),
        ).unwrap().unwrap();

        assert_ne!(path1, path3, "Different job_key should produce different path");
    }

    #[test]
    fn test_shared_mode_toolchain_keyed() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        let toolchain1 = ToolchainKey::new("16C5032a", "15.3", "arm64");
        let toolchain2 = ToolchainKey::new("16C5032a", "14.0", "arm64"); // Different macOS

        // Get path for first toolchain
        {
            let mut cache = DerivedDataCache::new(config.clone());
            let path1 = cache.get_path(DerivedDataMode::Shared, "job-1", &toolchain1)
                .unwrap().unwrap();

            // Should contain toolchain key
            assert!(path1.to_string_lossy().contains("xcode_16c5032a__macos_15__arm64"));
        }

        // Get path for second toolchain
        {
            let mut cache = DerivedDataCache::new(config);
            let path2 = cache.get_path(DerivedDataMode::Shared, "job-1", &toolchain2)
                .unwrap().unwrap();

            // Should contain different toolchain key
            assert!(path2.to_string_lossy().contains("xcode_16c5032a__macos_14__arm64"));
        }
    }

    #[test]
    fn test_derived_data_mode_serialization() {
        let shared = DerivedDataMode::Shared;
        let json = serde_json::to_string(&shared).unwrap();
        assert_eq!(json, "\"shared\"");

        let parsed: DerivedDataMode = serde_json::from_str("\"per_job\"").unwrap();
        assert_eq!(parsed, DerivedDataMode::PerJob);
    }
}
