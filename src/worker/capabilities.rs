//! Worker capabilities (capabilities.json) per PLAN.md normative spec
//!
//! Describes what a worker can do, including installed Xcode versions,
//! simulators, tooling, and capacity limits.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

/// Schema version for capabilities.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/capabilities@1";

/// Worker capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When capabilities were captured
    pub created_at: DateTime<Utc>,

    /// RCH Xcode Lane version on the worker
    pub rch_xcode_lane_version: String,

    /// Minimum supported protocol version
    pub protocol_min: u32,

    /// Maximum supported protocol version (inclusive)
    pub protocol_max: u32,

    /// Supported features
    pub features: Vec<String>,

    /// Installed Xcode versions
    pub xcode_versions: Vec<XcodeVersion>,

    /// Active Xcode (DEVELOPER_DIR)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_xcode: Option<XcodeVersion>,

    /// macOS information
    pub macos: MacOSInfo,

    /// Available simulators
    #[serde(default)]
    pub simulators: Vec<Simulator>,

    /// Available runtimes
    #[serde(default)]
    pub runtimes: Vec<Runtime>,

    /// Installed tooling versions
    #[serde(default)]
    pub tooling: ToolingVersions,

    /// Capacity information
    pub capacity: Capacity,

    /// Limits
    #[serde(default)]
    pub limits: Limits,
}

/// Xcode version information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcodeVersion {
    /// Marketing version (e.g., "15.4")
    pub version: String,

    /// Build identifier (e.g., "15F31d")
    pub build: String,

    /// Path to Xcode.app
    pub path: String,

    /// DEVELOPER_DIR value
    pub developer_dir: String,
}

/// macOS system information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacOSInfo {
    /// Marketing version (e.g., "14.5")
    pub version: String,

    /// Build identifier (e.g., "23F79")
    pub build: String,

    /// CPU architecture (e.g., "arm64")
    pub architecture: String,
}

/// Simulator device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Simulator {
    /// Device name (e.g., "iPhone 15 Pro")
    pub name: String,

    /// Device UDID
    pub udid: String,

    /// Device type identifier
    pub device_type: String,

    /// Runtime identifier this device uses
    pub runtime: String,

    /// Device state (e.g., "Shutdown", "Booted")
    pub state: String,
}

/// Simulator runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runtime {
    /// Runtime name (e.g., "iOS 17.5")
    pub name: String,

    /// Runtime identifier (e.g., "com.apple.CoreSimulator.SimRuntime.iOS-17-5")
    pub identifier: String,

    /// Version string
    pub version: String,

    /// Build identifier
    pub build: String,

    /// Whether this runtime is available
    #[serde(default = "default_true")]
    pub available: bool,
}

fn default_true() -> bool {
    true
}

/// Installed tooling versions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolingVersions {
    /// Swift version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swift: Option<String>,

    /// clang version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clang: Option<String>,

    /// CocoaPods version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cocoapods: Option<String>,

    /// Homebrew version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homebrew: Option<String>,

    /// Git version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,

    /// Ruby version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruby: Option<String>,
}

/// Worker capacity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capacity {
    /// Maximum concurrent jobs
    pub max_concurrent_jobs: u32,

    /// Free disk space in bytes
    pub disk_free_bytes: u64,

    /// Total disk space in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_total_bytes: Option<u64>,

    /// Available memory in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_available_bytes: Option<u64>,
}

/// Worker limits
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum upload size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_upload_bytes: Option<u64>,

    /// Maximum artifact size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_artifact_bytes: Option<u64>,

    /// Maximum log size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_log_bytes: Option<u64>,
}

/// Standard features supported by workers
pub mod features {
    pub const TAIL: &str = "tail";
    pub const FETCH: &str = "fetch";
    pub const EVENTS: &str = "events";
    pub const HAS_SOURCE: &str = "has_source";
    pub const UPLOAD_SOURCE: &str = "upload_source";
    pub const ATTESTATION_SIGNING: &str = "attestation_signing";
    pub const CANCEL: &str = "cancel";
}

impl Capabilities {
    /// Create a new capabilities struct with required fields
    pub fn new(
        rch_xcode_lane_version: String,
        protocol_min: u32,
        protocol_max: u32,
        macos: MacOSInfo,
        capacity: Capacity,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            rch_xcode_lane_version,
            protocol_min,
            protocol_max,
            features: vec![],
            xcode_versions: vec![],
            active_xcode: None,
            macos,
            simulators: vec![],
            runtimes: vec![],
            tooling: ToolingVersions::default(),
            capacity,
            limits: Limits::default(),
        }
    }

    /// Check if a feature is supported
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.contains(&feature.to_string())
    }

    /// Check if a protocol version is supported
    pub fn supports_protocol_version(&self, version: u32) -> bool {
        version >= self.protocol_min && version <= self.protocol_max
    }

    /// Find an Xcode version by marketing version
    pub fn find_xcode_version(&self, version: &str) -> Option<&XcodeVersion> {
        self.xcode_versions.iter().find(|x| x.version == version)
    }

    /// Find a runtime by identifier
    pub fn find_runtime(&self, identifier: &str) -> Option<&Runtime> {
        self.runtimes.iter().find(|r| r.identifier == identifier)
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write to file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e))
        })?;
        fs::write(path, json)
    }

    /// Load from file
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        Self::from_json(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON error: {}", e)))
    }

    /// Format for human-readable output
    pub fn to_human_readable(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("RCH Xcode Lane v{}\n", self.rch_xcode_lane_version));
        output.push_str(&format!(
            "Protocol: v{}-v{}\n",
            self.protocol_min, self.protocol_max
        ));
        output.push_str(&format!("Captured: {}\n\n", self.created_at.format("%Y-%m-%d %H:%M:%S UTC")));

        // macOS info
        output.push_str(&format!(
            "macOS {} ({}) on {}\n\n",
            self.macos.version, self.macos.build, self.macos.architecture
        ));

        // Xcode versions
        output.push_str("Xcode Installations:\n");
        for xcode in &self.xcode_versions {
            let active = if self.active_xcode.as_ref().map(|x| &x.path) == Some(&xcode.path) {
                " [active]"
            } else {
                ""
            };
            output.push_str(&format!(
                "  - {} ({}){}\n    {}\n",
                xcode.version, xcode.build, active, xcode.path
            ));
        }
        output.push('\n');

        // Features
        output.push_str("Features:\n");
        for feature in &self.features {
            output.push_str(&format!("  - {}\n", feature));
        }
        output.push('\n');

        // Capacity
        output.push_str("Capacity:\n");
        output.push_str(&format!(
            "  Max concurrent jobs: {}\n",
            self.capacity.max_concurrent_jobs
        ));
        output.push_str(&format!(
            "  Disk free: {:.2} GB\n",
            self.capacity.disk_free_bytes as f64 / 1_073_741_824.0
        ));

        // Runtimes
        if !self.runtimes.is_empty() {
            output.push_str("\nSimulator Runtimes:\n");
            for runtime in &self.runtimes {
                output.push_str(&format!(
                    "  - {} ({})\n",
                    runtime.name, runtime.version
                ));
            }
        }

        // Tooling
        output.push_str("\nTooling:\n");
        if let Some(ref v) = self.tooling.swift {
            output.push_str(&format!("  Swift: {}\n", v));
        }
        if let Some(ref v) = self.tooling.git {
            output.push_str(&format!("  Git: {}\n", v));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_capabilities() -> Capabilities {
        Capabilities {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            rch_xcode_lane_version: "0.1.0".to_string(),
            protocol_min: 1,
            protocol_max: 1,
            features: vec![
                "tail".to_string(),
                "fetch".to_string(),
                "has_source".to_string(),
                "upload_source".to_string(),
            ],
            xcode_versions: vec![XcodeVersion {
                version: "15.4".to_string(),
                build: "15F31d".to_string(),
                path: "/Applications/Xcode.app".to_string(),
                developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            }],
            active_xcode: Some(XcodeVersion {
                version: "15.4".to_string(),
                build: "15F31d".to_string(),
                path: "/Applications/Xcode.app".to_string(),
                developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            }),
            macos: MacOSInfo {
                version: "14.5".to_string(),
                build: "23F79".to_string(),
                architecture: "arm64".to_string(),
            },
            simulators: vec![],
            runtimes: vec![Runtime {
                name: "iOS 17.5".to_string(),
                identifier: "com.apple.CoreSimulator.SimRuntime.iOS-17-5".to_string(),
                version: "17.5".to_string(),
                build: "21F79".to_string(),
                available: true,
            }],
            tooling: ToolingVersions {
                swift: Some("5.10".to_string()),
                git: Some("2.45.0".to_string()),
                ..Default::default()
            },
            capacity: Capacity {
                max_concurrent_jobs: 2,
                disk_free_bytes: 100_000_000_000,
                disk_total_bytes: Some(500_000_000_000),
                memory_available_bytes: Some(16_000_000_000),
            },
            limits: Limits {
                max_upload_bytes: Some(1_000_000_000),
                max_artifact_bytes: Some(5_000_000_000),
                max_log_bytes: Some(100_000_000),
            },
        }
    }

    #[test]
    fn test_serialization() {
        let caps = sample_capabilities();
        let json = caps.to_json().unwrap();

        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/capabilities@1""#));
        assert!(json.contains(r#""rch_xcode_lane_version": "0.1.0""#));
    }

    #[test]
    fn test_deserialization() {
        let caps = sample_capabilities();
        let json = caps.to_json().unwrap();
        let parsed = Capabilities::from_json(&json).unwrap();

        assert_eq!(parsed.rch_xcode_lane_version, caps.rch_xcode_lane_version);
        assert_eq!(parsed.protocol_min, caps.protocol_min);
        assert_eq!(parsed.protocol_max, caps.protocol_max);
        assert_eq!(parsed.features, caps.features);
    }

    #[test]
    fn test_has_feature() {
        let caps = sample_capabilities();

        assert!(caps.has_feature("tail"));
        assert!(caps.has_feature("fetch"));
        assert!(!caps.has_feature("attestation_signing"));
    }

    #[test]
    fn test_supports_protocol_version() {
        let caps = sample_capabilities();

        assert!(!caps.supports_protocol_version(0)); // Below min
        assert!(caps.supports_protocol_version(1)); // In range
        assert!(!caps.supports_protocol_version(2)); // Above max
    }

    #[test]
    fn test_find_xcode_version() {
        let caps = sample_capabilities();

        assert!(caps.find_xcode_version("15.4").is_some());
        assert!(caps.find_xcode_version("16.0").is_none());
    }

    #[test]
    fn test_find_runtime() {
        let caps = sample_capabilities();

        assert!(caps.find_runtime("com.apple.CoreSimulator.SimRuntime.iOS-17-5").is_some());
        assert!(caps.find_runtime("com.apple.CoreSimulator.SimRuntime.iOS-18-0").is_none());
    }

    #[test]
    fn test_human_readable() {
        let caps = sample_capabilities();
        let output = caps.to_human_readable();

        assert!(output.contains("RCH Xcode Lane v0.1.0"));
        assert!(output.contains("macOS 14.5"));
        assert!(output.contains("arm64"));
        assert!(output.contains("15.4"));
        assert!(output.contains("[active]"));
    }

    #[test]
    fn test_write_and_read_file() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let caps = sample_capabilities();

        let path = dir.path().join("capabilities.json");
        caps.write_to_file(&path).unwrap();

        let loaded = Capabilities::from_file(&path).unwrap();
        assert_eq!(loaded.rch_xcode_lane_version, caps.rch_xcode_lane_version);
    }
}
