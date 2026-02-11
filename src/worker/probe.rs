//! Probe Operation Handler
//!
//! Implements the `probe` operation which returns worker capabilities.
//! This is the foundation for M0: Worker connectivity.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

use super::rpc::WorkerConfig;

/// Version of the RCH Xcode Lane implementation
const LANE_VERSION: &str = "0.1.0";

/// Schema version for capabilities.json
const CAPABILITIES_SCHEMA_VERSION: &str = "1.0.0";

/// Schema ID for capabilities.json
const CAPABILITIES_SCHEMA_ID: &str = "rch-xcode/capabilities@1";

/// Probe operation handler
pub struct ProbeHandler;

impl ProbeHandler {
    /// Create a new probe handler
    pub fn new() -> Self {
        Self
    }

    /// Handle a probe request, returning capabilities.json
    pub fn handle(&self, config: &WorkerConfig) -> Result<Value, ProbeError> {
        let capabilities = self.collect_capabilities(config)?;
        Ok(serde_json::to_value(capabilities)?)
    }

    /// Collect all worker capabilities
    fn collect_capabilities(&self, config: &WorkerConfig) -> Result<Capabilities, ProbeError> {
        let macos = self.get_macos_info()?;
        let xcode_versions = self.get_xcode_versions()?;
        let runtimes = self.get_simulator_runtimes()?;
        let disk = self.get_disk_info()?;

        Ok(Capabilities {
            kind: "probe".to_string(),
            schema_id: CAPABILITIES_SCHEMA_ID.to_string(),
            schema_version: CAPABILITIES_SCHEMA_VERSION.to_string(),
            created_at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            rch_xcode_lane_version: LANE_VERSION.to_string(),
            protocol_min: config.protocol_min,
            protocol_max: config.protocol_max,
            features: config.features.clone(),
            macos,
            xcode_versions,
            active_developer_dir: self.get_active_developer_dir(),
            runtimes,
            capacity: Capacity {
                max_concurrent_jobs: config.max_concurrent_jobs,
                current_jobs: 0, // TODO: Track actual job count
                disk_free_bytes: disk.0,
                disk_total_bytes: disk.1,
            },
        })
    }

    /// Get macOS version information via sw_vers
    fn get_macos_info(&self) -> Result<MacOSInfo, ProbeError> {
        // Try sw_vers command
        let output = Command::new("sw_vers").output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut version = String::new();
                let mut build = String::new();

                for line in stdout.lines() {
                    if line.starts_with("ProductVersion:") {
                        version = line.split(':').nth(1).unwrap_or("").trim().to_string();
                    } else if line.starts_with("BuildVersion:") {
                        build = line.split(':').nth(1).unwrap_or("").trim().to_string();
                    }
                }

                // Get architecture
                let arch_output = Command::new("uname").arg("-m").output();
                let arch = match arch_output {
                    Ok(o) if o.status.success() => {
                        String::from_utf8_lossy(&o.stdout).trim().to_string()
                    }
                    _ => "unknown".to_string(),
                };

                Ok(MacOSInfo { version, build, arch })
            }
            _ => {
                // Not on macOS or sw_vers not available
                Ok(MacOSInfo {
                    version: "unknown".to_string(),
                    build: "unknown".to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                })
            }
        }
    }

    /// Get installed Xcode versions via xcode-select and mdfind
    fn get_xcode_versions(&self) -> Result<Vec<XcodeVersion>, ProbeError> {
        let mut versions = Vec::new();

        // Try to find Xcode installations
        let output = Command::new("mdfind")
            .args(["kMDItemCFBundleIdentifier", "=", "com.apple.dt.Xcode"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for path in stdout.lines() {
                    if let Some(info) = self.get_xcode_info(path) {
                        versions.push(info);
                    }
                }
            }
            _ => {
                // mdfind not available (not macOS), return empty
            }
        }

        Ok(versions)
    }

    /// Get info for a specific Xcode installation
    fn get_xcode_info(&self, path: &str) -> Option<XcodeVersion> {
        // Get version from xcodebuild -version at that path
        let developer_dir = format!("{}/Contents/Developer", path);

        let output = Command::new("xcodebuild")
            .arg("-version")
            .env("DEVELOPER_DIR", &developer_dir)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut version = String::new();
        let mut build = String::new();

        for line in stdout.lines() {
            if line.starts_with("Xcode ") {
                version = line.strip_prefix("Xcode ")?.to_string();
            } else if line.starts_with("Build version ") {
                build = line.strip_prefix("Build version ")?.to_string();
            }
        }

        if version.is_empty() {
            return None;
        }

        Some(XcodeVersion {
            version,
            build,
            path: path.to_string(),
            developer_dir,
        })
    }

    /// Get the active DEVELOPER_DIR
    fn get_active_developer_dir(&self) -> String {
        Command::new("xcode-select")
            .arg("-p")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }

    /// Get simulator runtimes via xcrun simctl list runtimes
    fn get_simulator_runtimes(&self) -> Result<Vec<SimulatorRuntime>, ProbeError> {
        let output = Command::new("xcrun")
            .args(["simctl", "list", "runtimes", "--json"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();

                let runtimes = json["runtimes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|r| {
                                Some(SimulatorRuntime {
                                    name: r["name"].as_str()?.to_string(),
                                    identifier: r["identifier"].as_str()?.to_string(),
                                    build_version: r["buildversion"].as_str().unwrap_or("").to_string(),
                                    is_available: r["isAvailable"].as_bool().unwrap_or(false),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                Ok(runtimes)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Get disk space information
    fn get_disk_info(&self) -> Result<(u64, u64), ProbeError> {
        // Try df command
        let output = Command::new("df")
            .args(["-k", "/"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Parse df output (second line, skip header)
                if let Some(line) = stdout.lines().nth(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        let total = parts[1].parse::<u64>().unwrap_or(0) * 1024;
                        let available = parts[3].parse::<u64>().unwrap_or(0) * 1024;
                        return Ok((available, total));
                    }
                }
                Ok((0, 0))
            }
            _ => Ok((0, 0)),
        }
    }
}

/// Worker capabilities structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Capabilities {
    /// Kind identifier
    pub kind: String,
    /// Schema ID
    pub schema_id: String,
    /// Schema version
    pub schema_version: String,
    /// Creation timestamp (RFC 3339 UTC)
    pub created_at: String,
    /// Lane implementation version
    pub rch_xcode_lane_version: String,
    /// Minimum supported protocol version
    pub protocol_min: i32,
    /// Maximum supported protocol version
    pub protocol_max: i32,
    /// Supported features
    pub features: Vec<String>,
    /// macOS information
    pub macos: MacOSInfo,
    /// Installed Xcode versions
    pub xcode_versions: Vec<XcodeVersion>,
    /// Currently active DEVELOPER_DIR
    pub active_developer_dir: String,
    /// Available simulator runtimes
    pub runtimes: Vec<SimulatorRuntime>,
    /// Capacity information
    pub capacity: Capacity,
}

/// macOS information
#[derive(Debug, Serialize, Deserialize)]
pub struct MacOSInfo {
    pub version: String,
    pub build: String,
    pub arch: String,
}

/// Xcode version information
#[derive(Debug, Serialize, Deserialize)]
pub struct XcodeVersion {
    pub version: String,
    pub build: String,
    pub path: String,
    pub developer_dir: String,
}

/// Simulator runtime information
#[derive(Debug, Serialize, Deserialize)]
pub struct SimulatorRuntime {
    pub name: String,
    pub identifier: String,
    pub build_version: String,
    pub is_available: bool,
}

/// Capacity information
#[derive(Debug, Serialize, Deserialize)]
pub struct Capacity {
    pub max_concurrent_jobs: u32,
    pub current_jobs: u32,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
}

/// Probe operation errors
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("Failed to serialize capabilities: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("System command failed: {0}")]
    SystemCommand(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_handler() {
        let handler = ProbeHandler::new();
        let config = WorkerConfig::default();

        let result = handler.handle(&config);
        assert!(result.is_ok());

        let caps = result.unwrap();
        assert_eq!(caps["kind"], "probe");
        assert_eq!(caps["schema_id"], CAPABILITIES_SCHEMA_ID);
        assert!(caps["protocol_min"].is_number());
        assert!(caps["protocol_max"].is_number());
    }
}
