//! Worker configuration.

/// Worker configuration settings.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Minimum supported protocol version.
    pub protocol_min: i32,
    /// Maximum supported protocol version.
    pub protocol_max: i32,
    /// Maximum concurrent jobs allowed.
    pub max_concurrent_jobs: u32,
    /// Maximum upload size in bytes.
    pub max_upload_bytes: u64,
    /// Supported features.
    pub features: Vec<String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            protocol_min: 1,
            protocol_max: 1,
            max_concurrent_jobs: 1,
            max_upload_bytes: 1024 * 1024 * 1024, // 1 GB
            features: vec![
                "probe".to_string(),
                "has_source".to_string(),
                "upload_source".to_string(),
                "tail".to_string(),
            ],
        }
    }
}
