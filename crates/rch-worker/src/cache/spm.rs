//! SPM (Swift Package Manager) cache management
//!
//! Per PLAN.md §Caching:
//! - SPM cache modes: `off` | `shared`
//! - `off`: re-resolve every time
//! - `shared`: keyed by resolved Package.resolved + toolchain identity
//! - Directory layout: caches/<namespace>/spm/<toolchain_key>/...

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

use super::lock::CacheLock;
use super::toolchain_key::ToolchainKey;
use super::{CacheConfig, CacheError, CacheResult};

/// SPM cache mode.
///
/// Per PLAN.md §Caching:
/// - `off`: re-resolve every time (no caching)
/// - `shared`: keyed by Package.resolved hash + toolchain identity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpmCacheMode {
    /// No caching - re-resolve dependencies every time
    Off,
    /// Shared caching - keyed by Package.resolved + toolchain
    #[default]
    Shared,
}

impl SpmCacheMode {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    /// Convert to string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shared => "shared",
        }
    }
}

/// SPM cache key components.
///
/// The cache key combines Package.resolved content hash with toolchain identity.
#[derive(Debug, Clone)]
pub struct SpmCacheKey {
    /// SHA256 hash of Package.resolved content
    pub resolved_hash: String,
    /// Toolchain identity
    pub toolchain: ToolchainKey,
}

impl SpmCacheKey {
    /// Create a new SPM cache key from Package.resolved content and toolchain.
    pub fn new(package_resolved_content: &str, toolchain: ToolchainKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(package_resolved_content.as_bytes());
        let resolved_hash = hex::encode(hasher.finalize());

        Self {
            resolved_hash,
            toolchain,
        }
    }

    /// Create from an existing hash (when Package.resolved was already hashed).
    pub fn from_hash(resolved_hash: &str, toolchain: ToolchainKey) -> Self {
        Self {
            resolved_hash: resolved_hash.to_string(),
            toolchain,
        }
    }

    /// Convert to a filesystem-safe directory name.
    ///
    /// Format: `<toolchain_dir>/<resolved_hash_prefix>`
    ///
    /// Uses first 16 chars of hash for brevity while maintaining uniqueness.
    pub fn to_dir_name(&self) -> String {
        let hash_prefix = if self.resolved_hash.len() >= 16 {
            &self.resolved_hash[..16]
        } else {
            &self.resolved_hash
        };
        format!("{}/{}", self.toolchain.to_dir_name(), hash_prefix)
    }
}

/// SPM cache manager.
///
/// Provides methods to get cache paths for SPM dependencies.
pub struct SpmCache {
    config: CacheConfig,
    /// Active lock for shared mode (held while cache is in use)
    #[allow(dead_code)]
    lock: Option<CacheLock>,
}

impl SpmCache {
    /// Create a new SPM cache manager.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            lock: None,
        }
    }

    /// Get the SPM cache path for the given mode and key.
    ///
    /// For `off` mode, returns `None` - SPM resolves fresh.
    /// For `shared` mode, returns a toolchain+resolved-keyed directory.
    ///
    /// # Arguments
    /// * `mode` - The cache mode
    /// * `key` - The cache key (Package.resolved hash + toolchain)
    ///
    /// # Returns
    /// * `Ok(Some(path))` - The path to use for SPM cache
    /// * `Ok(None)` - No SPM cache path (off mode)
    pub fn get_path(
        &mut self,
        mode: SpmCacheMode,
        key: &SpmCacheKey,
    ) -> CacheResult<Option<PathBuf>> {
        match mode {
            SpmCacheMode::Off => Ok(None),
            SpmCacheMode::Shared => self.get_shared_path(key),
        }
    }

    /// Get the shared SPM cache path with locking.
    ///
    /// Layout: caches/<namespace>/spm/<toolchain_key>/<resolved_hash>/
    fn get_shared_path(&mut self, key: &SpmCacheKey) -> CacheResult<Option<PathBuf>> {
        let path = self.config.cache_root
            .join(&self.config.namespace)
            .join("spm")
            .join(key.to_dir_name());

        // Acquire lock on the shared cache directory
        let lock = CacheLock::acquire(&path, self.config.lock_timeout)?;
        self.lock = Some(lock);

        Ok(Some(path))
    }

    /// Release the lock (for explicit cleanup).
    pub fn release_lock(&mut self) {
        self.lock = None;
    }

    /// Get the base cache path for SPM caches.
    ///
    /// Useful for GC operations.
    pub fn base_path(&self) -> PathBuf {
        self.config.cache_root
            .join(&self.config.namespace)
            .join("spm")
    }

    /// List all SPM cache directories.
    pub fn list_caches(&self) -> CacheResult<Vec<PathBuf>> {
        let base = self.base_path();
        if !base.exists() {
            return Ok(Vec::new());
        }

        // SPM caches are nested: spm/<toolchain>/<hash>/
        let mut caches = Vec::new();
        for toolchain_entry in fs::read_dir(&base)? {
            let toolchain_entry = toolchain_entry?;
            if !toolchain_entry.file_type()?.is_dir() {
                continue;
            }

            for hash_entry in fs::read_dir(toolchain_entry.path())? {
                let hash_entry = hash_entry?;
                if hash_entry.file_type()?.is_dir() {
                    caches.push(hash_entry.path());
                }
            }
        }

        Ok(caches)
    }

    /// Get SPM cache statistics.
    pub fn cache_stats(&self) -> CacheResult<SpmCacheStats> {
        let caches = self.list_caches()?;
        let mut total_size: u64 = 0;

        for cache in &caches {
            total_size += Self::dir_size(cache)?;
        }

        Ok(SpmCacheStats {
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

/// SPM cache statistics.
#[derive(Debug, Clone)]
pub struct SpmCacheStats {
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

    fn make_test_key() -> SpmCacheKey {
        SpmCacheKey::new(
            r#"{"pins":[{"identity":"swift-argument-parser","version":"1.2.3"}]}"#,
            make_test_toolchain(),
        )
    }

    #[test]
    fn test_spm_cache_mode_parsing() {
        assert_eq!(SpmCacheMode::from_str("off"), Some(SpmCacheMode::Off));
        assert_eq!(SpmCacheMode::from_str("shared"), Some(SpmCacheMode::Shared));
        assert_eq!(SpmCacheMode::from_str("SHARED"), Some(SpmCacheMode::Shared));
        assert_eq!(SpmCacheMode::from_str("per_job"), None);
        assert_eq!(SpmCacheMode::from_str("invalid"), None);
    }

    #[test]
    fn test_spm_cache_mode_as_str() {
        assert_eq!(SpmCacheMode::Off.as_str(), "off");
        assert_eq!(SpmCacheMode::Shared.as_str(), "shared");
    }

    #[test]
    fn test_spm_cache_mode_default() {
        assert_eq!(SpmCacheMode::default(), SpmCacheMode::Shared);
    }

    #[test]
    fn test_spm_cache_key_new() {
        let toolchain = make_test_toolchain();
        let key = SpmCacheKey::new(
            r#"{"pins":[{"identity":"swift-argument-parser","version":"1.2.3"}]}"#,
            toolchain.clone(),
        );

        // Hash should be deterministic
        assert_eq!(key.resolved_hash.len(), 64); // SHA256 hex
        assert_eq!(key.toolchain, toolchain);
    }

    #[test]
    fn test_spm_cache_key_deterministic() {
        let content = r#"{"pins":[{"identity":"swift-argument-parser","version":"1.2.3"}]}"#;
        let toolchain = make_test_toolchain();

        let key1 = SpmCacheKey::new(content, toolchain.clone());
        let key2 = SpmCacheKey::new(content, toolchain);

        assert_eq!(key1.resolved_hash, key2.resolved_hash);
    }

    #[test]
    fn test_spm_cache_key_different_content() {
        let toolchain = make_test_toolchain();

        let key1 = SpmCacheKey::new(
            r#"{"pins":[{"identity":"package-a","version":"1.0.0"}]}"#,
            toolchain.clone(),
        );
        let key2 = SpmCacheKey::new(
            r#"{"pins":[{"identity":"package-b","version":"2.0.0"}]}"#,
            toolchain,
        );

        assert_ne!(key1.resolved_hash, key2.resolved_hash);
    }

    #[test]
    fn test_spm_cache_key_from_hash() {
        let toolchain = make_test_toolchain();
        let hash = "abc123def456789012345678901234567890123456789012345678901234567";

        let key = SpmCacheKey::from_hash(hash, toolchain.clone());

        assert_eq!(key.resolved_hash, hash);
        assert_eq!(key.toolchain, toolchain);
    }

    #[test]
    fn test_spm_cache_key_to_dir_name() {
        let key = make_test_key();
        let dir_name = key.to_dir_name();

        // Should contain toolchain info and hash prefix
        assert!(dir_name.contains("xcode_16c5032a__macos_15__arm64"));
        assert_eq!(dir_name.len(), "xcode_16c5032a__macos_15__arm64".len() + 1 + 16); // toolchain/hash
    }

    #[test]
    fn test_get_path_off_mode() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config);

        let path = cache.get_path(SpmCacheMode::Off, &make_test_key()).unwrap();

        assert!(path.is_none(), "Off mode should return None");
    }

    #[test]
    fn test_get_path_shared_mode() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config.clone());
        let key = make_test_key();

        let path = cache.get_path(SpmCacheMode::Shared, &key).unwrap().unwrap();

        // Verify path structure
        assert!(path.to_string_lossy().contains("spm"));
        assert!(path.to_string_lossy().contains(&config.namespace));
        assert!(path.to_string_lossy().contains("xcode_16c5032a__macos_15__arm64"));

        // Directory should exist
        assert!(path.exists());

        // Lock should be held
        assert!(cache.lock.is_some());
    }

    #[test]
    fn test_release_lock() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config);

        // Acquire shared cache (gets lock)
        cache.get_path(SpmCacheMode::Shared, &make_test_key()).unwrap();
        assert!(cache.lock.is_some());

        // Release lock
        cache.release_lock();
        assert!(cache.lock.is_none());
    }

    #[test]
    fn test_base_path() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = SpmCache::new(config.clone());

        let base = cache.base_path();
        assert!(base.ends_with("spm"));
        assert!(base.to_string_lossy().contains(&config.namespace));
    }

    #[test]
    fn test_list_caches_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let cache = SpmCache::new(config);

        let caches = cache.list_caches().unwrap();
        assert!(caches.is_empty());
    }

    #[test]
    fn test_list_caches_with_entries() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config);

        // Create some caches with different Package.resolved content
        let toolchain = make_test_toolchain();
        let key1 = SpmCacheKey::new(r#"{"pins":[{"identity":"pkg-a"}]}"#, toolchain.clone());
        let key2 = SpmCacheKey::new(r#"{"pins":[{"identity":"pkg-b"}]}"#, toolchain.clone());
        let key3 = SpmCacheKey::new(r#"{"pins":[{"identity":"pkg-c"}]}"#, toolchain);

        cache.get_path(SpmCacheMode::Shared, &key1).unwrap();
        cache.release_lock();
        cache.get_path(SpmCacheMode::Shared, &key2).unwrap();
        cache.release_lock();
        cache.get_path(SpmCacheMode::Shared, &key3).unwrap();
        cache.release_lock();

        let caches = cache.list_caches().unwrap();
        assert_eq!(caches.len(), 3);
    }

    #[test]
    fn test_cache_stats() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config);

        // Create some caches with content
        let toolchain = make_test_toolchain();
        let key1 = SpmCacheKey::new(r#"{"pins":[{"identity":"pkg-1"}]}"#, toolchain.clone());
        let key2 = SpmCacheKey::new(r#"{"pins":[{"identity":"pkg-2"}]}"#, toolchain);

        let path1 = cache.get_path(SpmCacheMode::Shared, &key1).unwrap().unwrap();
        cache.release_lock();
        let path2 = cache.get_path(SpmCacheMode::Shared, &key2).unwrap().unwrap();
        cache.release_lock();

        // Write some data
        fs::write(path1.join("checkouts"), "package data 1").unwrap();
        fs::write(path2.join("checkouts"), "package data 2").unwrap();

        let stats = cache.cache_stats().unwrap();
        assert_eq!(stats.count, 2);
        assert!(stats.total_size_bytes >= 28); // "package data 1" + "package data 2"
    }

    #[test]
    fn test_same_resolved_same_cache() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);
        let mut cache = SpmCache::new(config);

        let content = r#"{"pins":[{"identity":"same-pkg","version":"1.0.0"}]}"#;
        let toolchain = make_test_toolchain();

        // Get path twice with same content
        let key1 = SpmCacheKey::new(content, toolchain.clone());
        let path1 = cache.get_path(SpmCacheMode::Shared, &key1).unwrap().unwrap();
        cache.release_lock();

        let key2 = SpmCacheKey::new(content, toolchain);
        let path2 = cache.get_path(SpmCacheMode::Shared, &key2).unwrap().unwrap();
        cache.release_lock();

        assert_eq!(path1, path2, "Same Package.resolved content should produce same path");
    }

    #[test]
    fn test_different_toolchain_different_cache() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_test_config(&temp_dir);

        let content = r#"{"pins":[{"identity":"pkg"}]}"#;
        let toolchain1 = ToolchainKey::new("16C5032a", "15.3", "arm64");
        let toolchain2 = ToolchainKey::new("16C5032a", "14.0", "arm64"); // Different macOS

        // Get path with first toolchain
        {
            let mut cache = SpmCache::new(config.clone());
            let key = SpmCacheKey::new(content, toolchain1);
            let path = cache.get_path(SpmCacheMode::Shared, &key).unwrap().unwrap();
            assert!(path.to_string_lossy().contains("macos_15"));
        }

        // Get path with second toolchain
        {
            let mut cache = SpmCache::new(config);
            let key = SpmCacheKey::new(content, toolchain2);
            let path = cache.get_path(SpmCacheMode::Shared, &key).unwrap().unwrap();
            assert!(path.to_string_lossy().contains("macos_14"));
        }
    }

    #[test]
    fn test_spm_cache_mode_serialization() {
        let shared = SpmCacheMode::Shared;
        let json = serde_json::to_string(&shared).unwrap();
        assert_eq!(json, "\"shared\"");

        let parsed: SpmCacheMode = serde_json::from_str("\"off\"").unwrap();
        assert_eq!(parsed, SpmCacheMode::Off);
    }
}
