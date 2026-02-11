//! Metrics collection (metrics.json)
//!
//! Per bead 0uz.4: Emit metrics.json per job containing timing, cache, and size information.

use std::fs;
use std::io;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Schema version for metrics.json
pub const METRICS_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for metrics.json
pub const METRICS_SCHEMA_ID: &str = "rch-xcode/metrics@1";

/// Job metrics (metrics.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMetrics {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the metrics were recorded
    pub created_at: String,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash of job inputs)
    pub job_key: String,

    /// Timing metrics
    pub timings: Timings,

    /// Cache hit information
    pub cache: CacheInfo,

    /// Size metrics
    pub sizes: SizeMetrics,

    /// Cache key components used for keying
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key_components: Option<CacheKeyComponents>,
}

/// Timing metrics in milliseconds
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timings {
    /// Time to create source bundle (host side)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_ms: Option<u64>,

    /// Time to upload source bundle to worker
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_ms: Option<u64>,

    /// Time spent in worker queue before execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_ms: Option<u64>,

    /// Time to execute the job (xcodebuild/MCP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execute_ms: Option<u64>,

    /// Time to fetch artifacts from worker
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_ms: Option<u64>,

    /// Total time from start to finish
    pub total_ms: u64,
}

/// Cache hit information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheInfo {
    /// Whether DerivedData cache was hit
    pub derived_data_hit: bool,

    /// Whether SPM cache was hit
    pub spm_hit: bool,

    /// Whether result cache was hit (skipped execution)
    pub result_cache_hit: bool,

    /// Paths used for caching (if any)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_paths: Vec<CachePath>,
}

/// A cache path with its role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachePath {
    /// Type of cache: "derived_data", "spm", "result"
    pub cache_type: String,

    /// Resolved path
    pub path: String,
}

/// Size metrics in bytes
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SizeMetrics {
    /// Size of source bundle
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_bundle_bytes: Option<u64>,

    /// Total size of artifacts produced
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_bytes: Option<u64>,

    /// Size of xcresult bundle (if present)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcresult_bytes: Option<u64>,
}

/// Components used for cache key computation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheKeyComponents {
    /// Job key (SHA-256 of inputs)
    pub job_key: String,

    /// Xcode build identifier
    pub xcode_build: String,

    /// macOS version
    pub macos_version: String,

    /// macOS build identifier
    pub macos_build: String,

    /// CPU architecture
    pub arch: String,
}

impl JobMetrics {
    /// Create a new metrics instance
    pub fn new(run_id: &str, job_id: &str, job_key: &str) -> Self {
        Self {
            schema_version: METRICS_SCHEMA_VERSION,
            schema_id: METRICS_SCHEMA_ID.to_string(),
            created_at: Utc::now().to_rfc3339(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            timings: Timings::default(),
            cache: CacheInfo::default(),
            sizes: SizeMetrics::default(),
            cache_key_components: None,
        }
    }

    /// Set timing metrics
    pub fn with_timings(mut self, timings: Timings) -> Self {
        self.timings = timings;
        self
    }

    /// Set cache info
    pub fn with_cache(mut self, cache: CacheInfo) -> Self {
        self.cache = cache;
        self
    }

    /// Set size metrics
    pub fn with_sizes(mut self, sizes: SizeMetrics) -> Self {
        self.sizes = sizes;
        self
    }

    /// Set cache key components
    pub fn with_cache_key_components(mut self, components: CacheKeyComponents) -> Self {
        self.cache_key_components = Some(components);
        self
    }

    /// Write metrics.json to a directory with atomic write-then-rename
    pub fn write_to_file(&self, artifact_dir: &Path) -> Result<(), io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let final_path = artifact_dir.join("metrics.json");
        let temp_path = artifact_dir.join(".metrics.json.tmp");

        fs::write(&temp_path, &json)?;
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }

    /// Read metrics from file
    pub fn from_file(path: &Path) -> Result<Self, io::Error> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// Builder for metrics collection
pub struct MetricsBuilder {
    run_id: String,
    job_id: String,
    job_key: String,
    timings: Timings,
    cache: CacheInfo,
    sizes: SizeMetrics,
    cache_key_components: Option<CacheKeyComponents>,
    start_time: std::time::Instant,
}

impl MetricsBuilder {
    /// Create a new builder
    pub fn new(run_id: &str, job_id: &str, job_key: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            timings: Timings::default(),
            cache: CacheInfo::default(),
            sizes: SizeMetrics::default(),
            cache_key_components: None,
            start_time: std::time::Instant::now(),
        }
    }

    /// Record bundle creation time
    pub fn record_bundle_time(&mut self, ms: u64) {
        self.timings.bundle_ms = Some(ms);
    }

    /// Record upload time
    pub fn record_upload_time(&mut self, ms: u64) {
        self.timings.upload_ms = Some(ms);
    }

    /// Record queue wait time
    pub fn record_queue_time(&mut self, ms: u64) {
        self.timings.queue_ms = Some(ms);
    }

    /// Record execution time
    pub fn record_execute_time(&mut self, ms: u64) {
        self.timings.execute_ms = Some(ms);
    }

    /// Record fetch time
    pub fn record_fetch_time(&mut self, ms: u64) {
        self.timings.fetch_ms = Some(ms);
    }

    /// Set derived data cache hit
    pub fn set_derived_data_hit(&mut self, hit: bool) {
        self.cache.derived_data_hit = hit;
    }

    /// Set SPM cache hit
    pub fn set_spm_hit(&mut self, hit: bool) {
        self.cache.spm_hit = hit;
    }

    /// Set result cache hit
    pub fn set_result_cache_hit(&mut self, hit: bool) {
        self.cache.result_cache_hit = hit;
    }

    /// Add a cache path
    pub fn add_cache_path(&mut self, cache_type: &str, path: &str) {
        self.cache.cache_paths.push(CachePath {
            cache_type: cache_type.to_string(),
            path: path.to_string(),
        });
    }

    /// Set source bundle size
    pub fn set_source_bundle_size(&mut self, bytes: u64) {
        self.sizes.source_bundle_bytes = Some(bytes);
    }

    /// Set artifact size
    pub fn set_artifact_size(&mut self, bytes: u64) {
        self.sizes.artifact_bytes = Some(bytes);
    }

    /// Set xcresult size
    pub fn set_xcresult_size(&mut self, bytes: u64) {
        self.sizes.xcresult_bytes = Some(bytes);
    }

    /// Set cache key components
    pub fn set_cache_key_components(&mut self, components: CacheKeyComponents) {
        self.cache_key_components = Some(components);
    }

    /// Build the final metrics
    pub fn build(mut self) -> JobMetrics {
        // Record total time
        self.timings.total_ms = self.start_time.elapsed().as_millis() as u64;

        JobMetrics {
            schema_version: METRICS_SCHEMA_VERSION,
            schema_id: METRICS_SCHEMA_ID.to_string(),
            created_at: Utc::now().to_rfc3339(),
            run_id: self.run_id,
            job_id: self.job_id,
            job_key: self.job_key,
            timings: self.timings,
            cache: self.cache,
            sizes: self.sizes,
            cache_key_components: self.cache_key_components,
        }
    }
}

/// Generate metrics.json for a job
pub fn generate_metrics(
    artifact_dir: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
    timings: Timings,
    cache: CacheInfo,
    sizes: SizeMetrics,
    cache_key_components: Option<CacheKeyComponents>,
) -> Result<JobMetrics, io::Error> {
    let metrics = JobMetrics::new(run_id, job_id, job_key)
        .with_timings(timings)
        .with_cache(cache)
        .with_sizes(sizes);

    let metrics = if let Some(components) = cache_key_components {
        metrics.with_cache_key_components(components)
    } else {
        metrics
    };

    metrics.write_to_file(artifact_dir)?;
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_metrics_new() {
        let metrics = JobMetrics::new("run-001", "job-001", "abc123");

        assert_eq!(metrics.schema_version, METRICS_SCHEMA_VERSION);
        assert_eq!(metrics.schema_id, METRICS_SCHEMA_ID);
        assert_eq!(metrics.run_id, "run-001");
        assert_eq!(metrics.job_id, "job-001");
        assert_eq!(metrics.job_key, "abc123");
    }

    #[test]
    fn test_metrics_write_and_read() {
        let temp_dir = TempDir::new().unwrap();

        let timings = Timings {
            bundle_ms: Some(100),
            upload_ms: Some(500),
            queue_ms: Some(50),
            execute_ms: Some(30000),
            fetch_ms: Some(200),
            total_ms: 31000,
        };

        let cache = CacheInfo {
            derived_data_hit: false,
            spm_hit: true,
            result_cache_hit: false,
            cache_paths: vec![CachePath {
                cache_type: "spm".to_string(),
                path: "/var/cache/spm".to_string(),
            }],
        };

        let sizes = SizeMetrics {
            source_bundle_bytes: Some(1024 * 1024),
            artifact_bytes: Some(5 * 1024 * 1024),
            xcresult_bytes: Some(2 * 1024 * 1024),
        };

        let metrics = JobMetrics::new("run-001", "job-001", "abc123")
            .with_timings(timings)
            .with_cache(cache)
            .with_sizes(sizes);

        metrics.write_to_file(temp_dir.path()).unwrap();

        let path = temp_dir.path().join("metrics.json");
        assert!(path.exists());

        let loaded = JobMetrics::from_file(&path).unwrap();
        assert_eq!(loaded.run_id, "run-001");
        assert_eq!(loaded.timings.execute_ms, Some(30000));
        assert!(loaded.cache.spm_hit);
        assert_eq!(loaded.cache.cache_paths.len(), 1);
    }

    #[test]
    fn test_metrics_builder() {
        let mut builder = MetricsBuilder::new("run-001", "job-001", "abc123");

        builder.record_bundle_time(100);
        builder.record_upload_time(500);
        builder.record_execute_time(30000);
        builder.set_spm_hit(true);
        builder.add_cache_path("spm", "/var/cache/spm");
        builder.set_source_bundle_size(1024 * 1024);

        // Simulate some time passing
        std::thread::sleep(std::time::Duration::from_millis(10));

        let metrics = builder.build();

        assert_eq!(metrics.timings.bundle_ms, Some(100));
        assert_eq!(metrics.timings.upload_ms, Some(500));
        assert!(metrics.timings.total_ms >= 10);
        assert!(metrics.cache.spm_hit);
        assert_eq!(metrics.sizes.source_bundle_bytes, Some(1024 * 1024));
    }

    #[test]
    fn test_cache_key_components() {
        let components = CacheKeyComponents {
            job_key: "abc123".to_string(),
            xcode_build: "16C5032a".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        };

        let metrics = JobMetrics::new("run-001", "job-001", "abc123")
            .with_cache_key_components(components);

        assert!(metrics.cache_key_components.is_some());
        let c = metrics.cache_key_components.unwrap();
        assert_eq!(c.xcode_build, "16C5032a");
    }

    #[test]
    fn test_generate_metrics() {
        let temp_dir = TempDir::new().unwrap();

        let metrics = generate_metrics(
            temp_dir.path(),
            "run-001",
            "job-001",
            "abc123",
            Timings::default(),
            CacheInfo::default(),
            SizeMetrics::default(),
            None,
        ).unwrap();

        assert!(temp_dir.path().join("metrics.json").exists());
        assert_eq!(metrics.job_id, "job-001");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let metrics = JobMetrics::new("run-001", "job-001", "abc123")
            .with_timings(Timings {
                total_ms: 1000,
                ..Default::default()
            });

        let json = serde_json::to_string(&metrics).unwrap();
        let parsed: JobMetrics = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.run_id, metrics.run_id);
        assert_eq!(parsed.timings.total_ms, 1000);
    }

    #[test]
    fn test_optional_fields_serialization() {
        let metrics = JobMetrics::new("run-001", "job-001", "abc123");

        let json = serde_json::to_string_pretty(&metrics).unwrap();

        // Optional None fields should not appear
        assert!(!json.contains("bundle_ms"));
        assert!(!json.contains("cache_key_components"));
        // But required fields should
        assert!(json.contains("total_ms"));
        assert!(json.contains("derived_data_hit"));
    }
}
