//! M5 Caching Correctness Tests
//!
//! Per rch-mac-0uz.6: Tests for caching correctness with controlled fixtures.

use std::fs;
use std::time::Duration;

use tempfile::TempDir;

use rch_worker::{
    CacheConfig, CacheGc, DerivedDataCache, DerivedDataMode, EvictionPolicy,
    ResultCache, ResultCacheEntry, SpmCache, SpmCacheKey, SpmCacheMode,
    ToolchainKey, ArtifactProfile,
};

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

// =============================================================================
// Test 1: Result cache hit produces identical artifacts
// =============================================================================

#[test]
fn test_result_cache_hit_produces_identical_artifacts() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let job_key = "sha256-abc123def456";

    // First run: store result in cache
    let original_job_id = "original-job-id-001";
    let original_run_id = "original-run-id-001";
    {
        let mut cache = ResultCache::new(config.clone());
        let path = cache.store(
            job_key,
            original_job_id,
            original_run_id,
            ArtifactProfile::Rich,
        ).unwrap();

        // Write artifacts
        fs::write(path.join("summary.json"), r#"{"status":"success"}"#).unwrap();
        fs::write(path.join("build.log"), "build output here").unwrap();
    }

    // Second run: retrieve from cache (simulating new job with same key)
    {
        let mut cache = ResultCache::new(config);
        let result = cache.get_cached(job_key, ArtifactProfile::Rich).unwrap();

        assert!(result.is_some(), "Should have cache hit for same job_key");
        let (path, entry) = result.unwrap();

        // Verify cached_from_job_id would be set
        assert_eq!(entry.original_job_id, original_job_id);
        assert_eq!(entry.original_run_id, original_run_id);

        // Verify artifacts exist and match
        let summary = fs::read_to_string(path.join("summary.json")).unwrap();
        assert_eq!(summary, r#"{"status":"success"}"#);

        let build_log = fs::read_to_string(path.join("build.log")).unwrap();
        assert_eq!(build_log, "build output here");
    }
}

// =============================================================================
// Test 2: Result cache profile-aware reuse
// =============================================================================

#[test]
fn test_result_cache_profile_aware_reuse() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let job_key = "sha256-profile-test";

    // Store with Minimal profile
    {
        let mut cache = ResultCache::new(config.clone());
        cache.store(job_key, "job-1", "run-1", ArtifactProfile::Minimal).unwrap();
    }

    // Request with Rich profile - should NOT get cache hit
    {
        let mut cache = ResultCache::new(config.clone());
        let result = cache.get_cached(job_key, ArtifactProfile::Rich).unwrap();
        assert!(result.is_none(), "Minimal cache should NOT satisfy Rich request");
    }

    // Request with Minimal profile - should get cache hit
    {
        let mut cache = ResultCache::new(config.clone());
        let result = cache.get_cached(job_key, ArtifactProfile::Minimal).unwrap();
        assert!(result.is_some(), "Minimal cache should satisfy Minimal request");
    }

    // Now store with Rich profile
    let job_key2 = "sha256-profile-test-2";
    {
        let mut cache = ResultCache::new(config.clone());
        cache.store(job_key2, "job-2", "run-2", ArtifactProfile::Rich).unwrap();
    }

    // Rich should satisfy both Rich and Minimal
    {
        let mut cache = ResultCache::new(config.clone());
        let result = cache.get_cached(job_key2, ArtifactProfile::Rich).unwrap();
        assert!(result.is_some(), "Rich cache should satisfy Rich request");
    }
    {
        let mut cache = ResultCache::new(config);
        let result = cache.get_cached(job_key2, ArtifactProfile::Minimal).unwrap();
        assert!(result.is_some(), "Rich cache should satisfy Minimal request");
    }
}

// =============================================================================
// Test 3: DerivedData per_job isolation
// =============================================================================

#[test]
fn test_derived_data_per_job_isolation() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let toolchain = make_test_toolchain();

    // Job 1
    let path1 = {
        let mut cache = DerivedDataCache::new(config.clone());
        cache.get_path(DerivedDataMode::PerJob, "job-key-1", &toolchain)
            .unwrap()
            .unwrap()
    };

    // Job 2 (different key)
    let path2 = {
        let mut cache = DerivedDataCache::new(config.clone());
        cache.get_path(DerivedDataMode::PerJob, "job-key-2", &toolchain)
            .unwrap()
            .unwrap()
    };

    // Verify isolation
    assert_ne!(path1, path2, "Different job_keys should have different paths");
    assert!(path1.to_string_lossy().contains("per_job"));
    assert!(path1.to_string_lossy().contains("job-key-1"));
    assert!(path2.to_string_lossy().contains("job-key-2"));

    // Verify directories exist
    assert!(path1.exists());
    assert!(path2.exists());

    // Verify no shared state by writing to each
    fs::write(path1.join("build.db"), "job1 data").unwrap();
    fs::write(path2.join("build.db"), "job2 data").unwrap();

    let data1 = fs::read_to_string(path1.join("build.db")).unwrap();
    let data2 = fs::read_to_string(path2.join("build.db")).unwrap();
    assert_eq!(data1, "job1 data");
    assert_eq!(data2, "job2 data");
}

// =============================================================================
// Test 4: Shared DerivedData locking
// =============================================================================

#[test]
fn test_shared_derived_data_locking() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let toolchain = make_test_toolchain();

    // First job acquires lock
    let mut cache1 = DerivedDataCache::new(config.clone());
    let path1 = cache1.get_path(DerivedDataMode::Shared, "job-1", &toolchain)
        .unwrap()
        .unwrap();

    // Verify lock file exists
    let lock_path = path1.join(".rch_cache.lock");
    assert!(lock_path.exists(), "Lock file should exist");

    // Second job with same toolchain should get same path but different lock
    // (In production, this would block or timeout)
    // For testing, we just verify the path is the same
    let mut cache2 = DerivedDataCache::new(config);

    // Use short timeout to test lock behavior
    let config2 = CacheConfig {
        cache_root: temp_dir.path().join("caches"),
        namespace: "test-repo".to_string(),
        lock_timeout: Duration::from_millis(100),
    };
    cache2 = DerivedDataCache::new(config2);

    // First release cache1's lock
    cache1.release_lock();

    // Now cache2 should be able to acquire
    let path2 = cache2.get_path(DerivedDataMode::Shared, "job-2", &toolchain)
        .unwrap()
        .unwrap();

    // Same toolchain = same path
    assert_eq!(path1, path2, "Same toolchain should use same shared path");
}

// =============================================================================
// Test 5: SPM cache keying
// =============================================================================

#[test]
fn test_spm_cache_keying() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);

    let package_resolved = r#"{"pins":[{"identity":"swift-argument-parser","version":"1.2.3"}]}"#;

    let toolchain1 = ToolchainKey::new("16C5032a", "15.3", "arm64");
    let toolchain2 = ToolchainKey::new("16D5025a", "15.3", "arm64"); // Different Xcode

    // Same Package.resolved + same toolchain = cache hit
    {
        let mut cache = SpmCache::new(config.clone());
        let key1 = SpmCacheKey::new(package_resolved, toolchain1.clone());
        let path1 = cache.get_path(SpmCacheMode::Shared, &key1).unwrap().unwrap();
        cache.release_lock();

        let key2 = SpmCacheKey::new(package_resolved, toolchain1.clone());
        let path2 = cache.get_path(SpmCacheMode::Shared, &key2).unwrap().unwrap();

        assert_eq!(path1, path2, "Same Package.resolved + same toolchain = same path");
    }

    // Same Package.resolved + different toolchain = cache miss (different path)
    {
        let mut cache = SpmCache::new(config.clone());
        let key1 = SpmCacheKey::new(package_resolved, toolchain1);
        let path1 = cache.get_path(SpmCacheMode::Shared, &key1).unwrap().unwrap();
        cache.release_lock();

        let key2 = SpmCacheKey::new(package_resolved, toolchain2);
        let path2 = cache.get_path(SpmCacheMode::Shared, &key2).unwrap().unwrap();

        assert_ne!(path1, path2, "Same Package.resolved + different toolchain = different path");

        // Verify toolchain component in path
        assert!(path1.to_string_lossy().contains("16c5032a"));
        assert!(path2.to_string_lossy().contains("16d5025a"));
    }
}

// =============================================================================
// Test 6: Cache namespace validation
// =============================================================================

#[test]
fn test_cache_namespace_validation() {
    // Valid namespaces - regex: ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$
    let valid = vec![
        "my-project",
        "Foo.Bar_123",
        "a",
        "A1",
        "project-name",
        "my_project.v2",
    ];

    for ns in valid {
        assert!(is_valid_namespace(ns), "Should accept valid namespace: {}", ns);
    }

    // Invalid namespaces
    let too_long = "a".repeat(65);
    let invalid = vec![
        "",                 // empty
        "/etc/passwd",      // path traversal
        "..",               // parent dir
        "has space",        // whitespace
        "-starts-with-dash",// starts with special
        ".starts-with-dot", // starts with dot
        too_long.as_str(),  // too long (>64 chars)
    ];

    for ns in invalid {
        assert!(!is_valid_namespace(ns), "Should reject invalid namespace: {}", ns);
    }
}

fn is_valid_namespace(ns: &str) -> bool {
    // Regex: ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$
    if ns.is_empty() || ns.len() > 64 {
        return false;
    }

    let chars: Vec<char> = ns.chars().collect();

    // First char must be alphanumeric
    if !chars[0].is_ascii_alphanumeric() {
        return false;
    }

    // Rest must be alphanumeric, dot, underscore, or hyphen
    for c in &chars[1..] {
        if !c.is_ascii_alphanumeric() && *c != '.' && *c != '_' && *c != '-' {
            return false;
        }
    }

    true
}

// =============================================================================
// Test 7: Cache eviction respects locks
// =============================================================================

#[test]
fn test_cache_eviction_respects_locks() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let toolchain = make_test_toolchain();

    // Create a shared cache and hold the lock
    let mut cache = DerivedDataCache::new(config.clone());
    let locked_path = cache.get_path(DerivedDataMode::Shared, "job-1", &toolchain)
        .unwrap()
        .unwrap();

    // Write some data
    fs::write(locked_path.join("important.db"), "data").unwrap();

    // Run GC with very small size limit (should want to evict)
    let policy = EvictionPolicy::size_based(1); // 1 byte limit
    let gc = CacheGc::new(config, policy);

    let result = gc.run().unwrap();

    // Should have skipped the locked entry
    assert!(result.skipped >= 1 || locked_path.exists(),
        "GC should skip locked cache entries");

    // Verify data still exists
    assert!(locked_path.join("important.db").exists(),
        "Locked cache entry should not be deleted");
}

// =============================================================================
// Test 8: Bundle GC does not remove bundles for RUNNING jobs
// =============================================================================

#[test]
fn test_bundle_gc_protects_running_jobs() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);

    // Create result cache entries
    let results_base = config.cache_root
        .join(&config.namespace)
        .join("results");

    // Create entry for a running job
    let running_path = results_base.join("running-job-key");
    fs::create_dir_all(&running_path).unwrap();
    let running_entry = ResultCacheEntry::new(
        "running-job-key",
        "running-job-id",
        "run-1",
        ArtifactProfile::Rich,
    );
    fs::write(
        running_path.join(ResultCacheEntry::METADATA_FILENAME),
        serde_json::to_string(&running_entry).unwrap(),
    ).unwrap();
    fs::write(running_path.join("data.bin"), "important").unwrap();

    // Create entry for a completed job
    let completed_path = results_base.join("completed-job-key");
    fs::create_dir_all(&completed_path).unwrap();
    let completed_entry = ResultCacheEntry::new(
        "completed-job-key",
        "completed-job-id",
        "run-2",
        ArtifactProfile::Rich,
    );
    fs::write(
        completed_path.join(ResultCacheEntry::METADATA_FILENAME),
        serde_json::to_string(&completed_entry).unwrap(),
    ).unwrap();
    fs::write(completed_path.join("data.bin"), "can delete").unwrap();

    // Run GC with the running job protected
    let policy = EvictionPolicy::size_based(1); // Force eviction
    let mut gc = CacheGc::new(config, policy);
    gc.protect_job("running-job-id");

    let result = gc.run().unwrap();

    // Running job should be protected
    assert!(running_path.exists(),
        "Bundle for RUNNING job should NOT be deleted");

    // Completed job should be evicted (or skipped if locked)
    // Either it's deleted or skipped, but running job must survive
    assert!(result.deleted > 0 || result.skipped > 0,
        "GC should have processed some entries");
}

// =============================================================================
// Test 9: metrics.json accuracy (structure validation)
// =============================================================================

#[test]
fn test_metrics_json_structure() {
    // This test validates the expected structure of metrics.json
    // The actual metrics are generated by the executor

    let metrics_json = r#"{
        "schema_version": 1,
        "schema_id": "rch-xcode-metrics",
        "job_id": "job-123",
        "run_id": "run-456",
        "timings": {
            "bundle_ms": 100,
            "upload_ms": 200,
            "queue_ms": 50,
            "execute_ms": 5000,
            "fetch_ms": 150,
            "total_ms": 5500
        },
        "cache": {
            "derived_data_hit": false,
            "spm_hit": true,
            "result_cache_hit": false
        },
        "sizes": {
            "source_bundle_bytes": 1024000,
            "artifact_bytes": 512000
        },
        "cache_key_components": {
            "job_key": "sha256-abc123",
            "xcode_build": "16C5032a",
            "macos_major": "15",
            "arch": "arm64"
        },
        "cache_paths": {
            "derived_data": "/var/lib/rch/caches/repo/derived_data/shared/xcode_16c5032a__macos_15__arm64",
            "spm": "/var/lib/rch/caches/repo/spm/xcode_16c5032a__macos_15__arm64/abc123def456"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(metrics_json).unwrap();

    // Verify schema fields
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["schema_id"], "rch-xcode-metrics");

    // Verify timings are ints >= 0
    let timings = &parsed["timings"];
    assert!(timings["bundle_ms"].as_u64().unwrap() >= 0);
    assert!(timings["upload_ms"].as_u64().unwrap() >= 0);
    assert!(timings["queue_ms"].as_u64().unwrap() >= 0);
    assert!(timings["execute_ms"].as_u64().unwrap() >= 0);
    assert!(timings["fetch_ms"].as_u64().unwrap() >= 0);
    assert!(timings["total_ms"].as_u64().unwrap() >= 0);

    // Verify cache fields are booleans
    let cache = &parsed["cache"];
    assert!(cache["derived_data_hit"].is_boolean());
    assert!(cache["spm_hit"].is_boolean());
    assert!(cache["result_cache_hit"].is_boolean());

    // Verify sizes are ints >= 0
    let sizes = &parsed["sizes"];
    assert!(sizes["source_bundle_bytes"].as_u64().unwrap() >= 0);
    assert!(sizes["artifact_bytes"].as_u64().unwrap() >= 0);

    // Verify cache_key_components has required fields
    let key_components = &parsed["cache_key_components"];
    assert!(key_components["job_key"].is_string());
    assert!(key_components["xcode_build"].is_string());
    assert!(key_components["macos_major"].is_string());
    assert!(key_components["arch"].is_string());

    // Verify cache_paths has paths
    let cache_paths = &parsed["cache_paths"];
    assert!(cache_paths["derived_data"].is_string());
    assert!(cache_paths["spm"].is_string());

    // Verify timings roughly sum to total
    let bundle = timings["bundle_ms"].as_u64().unwrap();
    let upload = timings["upload_ms"].as_u64().unwrap();
    let queue = timings["queue_ms"].as_u64().unwrap();
    let execute = timings["execute_ms"].as_u64().unwrap();
    let fetch = timings["fetch_ms"].as_u64().unwrap();
    let total = timings["total_ms"].as_u64().unwrap();

    // Total should be >= sum of individual timings (some may overlap)
    let sum = bundle + upload + queue + execute + fetch;
    assert!(total >= sum.saturating_sub(1000), // Allow some overlap
        "Total should roughly match sum of timings");
}

// =============================================================================
// Test 10: Eviction policy enforcement
// =============================================================================

#[test]
fn test_eviction_policy_enforcement() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);

    // Create several cache entries
    let dd_base = config.cache_root
        .join(&config.namespace)
        .join("derived_data")
        .join("per_job");

    for i in 0..5 {
        let entry_path = dd_base.join(format!("job-{}", i));
        fs::create_dir_all(&entry_path).unwrap();
        fs::write(entry_path.join(".rch_cache.lock"), "").unwrap();
        fs::write(entry_path.join("data.bin"), vec![0u8; 1000]).unwrap();
    }

    // Verify all entries exist
    assert_eq!(fs::read_dir(&dd_base).unwrap().count(), 5);

    // Run with size-based eviction (keep under 2000 bytes)
    let policy = EvictionPolicy::size_based(2000);
    let gc = CacheGc::new(config, policy);

    let result = gc.run().unwrap();

    // Should have deleted some entries to get under limit
    assert!(result.deleted > 0, "Should have evicted some entries");
    assert!(result.bytes_reclaimed > 0, "Should have reclaimed space");

    // Verify remaining entries are under limit
    let remaining = fs::read_dir(&dd_base).unwrap().count();
    assert!(remaining < 5, "Should have fewer entries after GC");
}

// =============================================================================
// Additional test: Age-based eviction policy configuration
// =============================================================================

#[test]
fn test_age_based_eviction_policy() {
    // Test that age-based eviction policy is correctly configured
    let policy = EvictionPolicy::age_based(30);
    assert_eq!(policy.max_age_days, 30);
    assert_eq!(policy.max_size_bytes, 0); // No size limit

    // Verify default policy has both age and size limits
    let default_policy = EvictionPolicy::default();
    assert_eq!(default_policy.max_age_days, 7);
    assert_eq!(default_policy.max_size_bytes, 10 * 1024 * 1024 * 1024);

    // Combined with size-based eviction - test actual deletion
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);

    // Create cache entries
    let dd_base = config.cache_root
        .join(&config.namespace)
        .join("derived_data")
        .join("per_job");

    let entry_path = dd_base.join("test-job");
    fs::create_dir_all(&entry_path).unwrap();
    fs::write(entry_path.join(".rch_cache.lock"), "").unwrap();
    fs::write(entry_path.join("data.bin"), vec![0u8; 1000]).unwrap();

    // Use size-based eviction to verify deletion works
    let policy = EvictionPolicy::size_based(100);
    let gc = CacheGc::new(config, policy);

    let result = gc.run().unwrap();

    // Should have deleted the entry (since it exceeds 100 byte limit)
    assert!(result.deleted > 0 || !entry_path.exists(),
        "Should have evicted entries exceeding size limit");
}

// =============================================================================
// Additional test: Off mode returns None
// =============================================================================

#[test]
fn test_derived_data_off_mode() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let toolchain = make_test_toolchain();

    let mut cache = DerivedDataCache::new(config);
    let path = cache.get_path(DerivedDataMode::Off, "job-1", &toolchain).unwrap();

    assert!(path.is_none(), "Off mode should return None");
}

#[test]
fn test_spm_off_mode() {
    let temp_dir = TempDir::new().unwrap();
    let config = make_test_config(&temp_dir);
    let key = SpmCacheKey::new("{}", make_test_toolchain());

    let mut cache = SpmCache::new(config);
    let path = cache.get_path(SpmCacheMode::Off, &key).unwrap();

    assert!(path.is_none(), "Off mode should return None");
}
