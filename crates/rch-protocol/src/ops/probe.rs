//! Probe operation types.
//!
//! The probe operation collects system information and returns capabilities.

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Probe request payload (typically empty).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeRequest {}

/// Probe response payload containing capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResponse {
    /// Schema version for capabilities.
    pub schema_version: i32,
    /// Schema identifier.
    pub schema_id: String,
    /// When this capabilities snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Lane version running on the worker.
    pub rch_xcode_lane_version: String,
    /// Minimum protocol version supported.
    pub protocol_min: i32,
    /// Maximum protocol version supported.
    pub protocol_max: i32,
    /// Feature flags supported by this worker.
    pub features: Vec<String>,
    /// Full capabilities data.
    pub capabilities: Capabilities,
}

/// Worker capabilities data (capabilities.json schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    /// Schema version.
    pub schema_version: i32,
    /// Schema identifier.
    pub schema_id: String,
    /// When this snapshot was created.
    pub created_at: DateTime<Utc>,
    /// macOS version (e.g., "15.3.1").
    pub macos_version: String,
    /// macOS build number (e.g., "24D60").
    pub macos_build: String,
    /// Machine architecture (e.g., "arm64").
    pub arch: String,
    /// Installed Xcode versions.
    pub xcode_versions: Vec<XcodeInfo>,
    /// Available simulator runtimes.
    pub simulator_runtimes: Vec<SimulatorRuntime>,
    /// Worker capacity information.
    pub capacity: Capacity,
}

/// Information about an installed Xcode version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcodeInfo {
    /// Xcode version string (e.g., "16.2").
    pub version: String,
    /// Xcode build number (e.g., "16C5032a").
    pub build: String,
    /// Path to Xcode.app.
    pub path: String,
    /// Path to DEVELOPER_DIR.
    pub developer_dir: String,
}

/// Information about an available simulator runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorRuntime {
    /// Runtime name (e.g., "iOS 19.2").
    pub name: String,
    /// Runtime identifier (e.g., "com.apple.CoreSimulator.SimRuntime.iOS-19-2").
    pub identifier: String,
    /// Runtime version (e.g., "19.2").
    pub version: String,
    /// Runtime build string if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Platform (e.g., "iOS", "tvOS", "watchOS").
    pub platform: String,
    /// Available device types for this runtime.
    pub device_types: Vec<String>,
}

/// Worker capacity information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capacity {
    /// Maximum concurrent jobs allowed.
    pub max_concurrent_jobs: u32,
    /// Current number of running jobs.
    pub current_jobs: u32,
    /// Maximum upload size in bytes.
    pub max_upload_bytes: u64,
    /// Disk space available in bytes.
    pub disk_available_bytes: u64,
    /// Disk space total in bytes.
    pub disk_total_bytes: u64,
}

impl Default for Capacity {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: 1,
            current_jobs: 0,
            max_upload_bytes: 1024 * 1024 * 1024, // 1 GB
            disk_available_bytes: 0,
            disk_total_bytes: 0,
        }
    }
}
