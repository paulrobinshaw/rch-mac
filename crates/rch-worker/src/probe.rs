//! Probe operation handler.
//!
//! Collects system information and returns capabilities.

use std::process::Command;
use chrono::Utc;
use rch_protocol::{
    RpcRequest, RpcError, ErrorCode,
    LANE_VERSION, PROTOCOL_MIN, PROTOCOL_MAX,
    ops::{ProbeResponse, Capabilities, XcodeInfo, SimulatorRuntime, Capacity},
};

/// Handle the probe operation.
pub fn handle_probe(_request: &RpcRequest) -> Result<serde_json::Value, RpcError> {
    let now = Utc::now();
    
    // Collect system information
    let (macos_version, macos_build) = get_macos_info()?;
    let arch = get_architecture();
    let xcode_versions = get_xcode_versions();
    let simulator_runtimes = get_simulator_runtimes();
    let capacity = get_capacity();
    
    // Build capabilities
    let capabilities = Capabilities {
        schema_version: 1,
        schema_id: "rch-xcode/capabilities@1".to_string(),
        created_at: now,
        macos_version,
        macos_build,
        arch,
        xcode_versions,
        simulator_runtimes,
        capacity,
    };
    
    // Build probe response
    let response = ProbeResponse {
        schema_version: 1,
        schema_id: "rch-xcode/probe@1".to_string(),
        created_at: now,
        rch_xcode_lane_version: LANE_VERSION.to_string(),
        protocol_min: PROTOCOL_MIN,
        protocol_max: PROTOCOL_MAX,
        features: get_supported_features(),
        capabilities,
    };
    
    serde_json::to_value(response).map_err(|e| {
        RpcError::new(ErrorCode::InvalidRequest, format!("failed to serialize probe response: {}", e))
    })
}

/// Get macOS version and build number via sw_vers.
fn get_macos_info() -> Result<(String, String), RpcError> {
    // Try sw_vers for version
    let version = run_command("sw_vers", &["-productVersion"])
        .unwrap_or_else(|_| "unknown".to_string());
    
    // Try sw_vers for build
    let build = run_command("sw_vers", &["-buildVersion"])
        .unwrap_or_else(|_| "unknown".to_string());
    
    Ok((version.trim().to_string(), build.trim().to_string()))
}

/// Get machine architecture.
fn get_architecture() -> String {
    run_command("uname", &["-m"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| std::env::consts::ARCH.to_string())
}

/// Get installed Xcode versions via xcode-select and mdfind.
fn get_xcode_versions() -> Vec<XcodeInfo> {
    let mut versions = Vec::new();
    
    // Get current Xcode path
    if let Ok(path) = run_command("xcode-select", &["-p"]) {
        let developer_dir = path.trim().to_string();
        // Extract Xcode.app path from developer dir
        let xcode_path = if developer_dir.contains("/Contents/Developer") {
            developer_dir.replace("/Contents/Developer", "")
        } else {
            developer_dir.clone()
        };
        
        // Get Xcode version info
        if let Ok(version_output) = run_command("xcodebuild", &["-version"]) {
            let lines: Vec<&str> = version_output.lines().collect();
            if lines.len() >= 2 {
                // First line: "Xcode 16.2"
                // Second line: "Build version 16C5032a"
                let version = lines[0].replace("Xcode ", "").trim().to_string();
                let build = lines[1].replace("Build version ", "").trim().to_string();
                
                versions.push(XcodeInfo {
                    version,
                    build,
                    path: xcode_path,
                    developer_dir,
                });
            }
        }
    }
    
    versions
}

/// Get available simulator runtimes via xcrun simctl.
fn get_simulator_runtimes() -> Vec<SimulatorRuntime> {
    let mut runtimes = Vec::new();
    
    // Run simctl list runtimes -j for JSON output
    if let Ok(output) = run_command("xcrun", &["simctl", "list", "runtimes", "-j"]) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&output) {
            if let Some(runtime_list) = json.get("runtimes").and_then(|r| r.as_array()) {
                for runtime in runtime_list {
                    if let (Some(name), Some(identifier), Some(version)) = (
                        runtime.get("name").and_then(|n| n.as_str()),
                        runtime.get("identifier").and_then(|i| i.as_str()),
                        runtime.get("version").and_then(|v| v.as_str()),
                    ) {
                        // Determine platform from identifier
                        let platform = if identifier.contains("iOS") {
                            "iOS"
                        } else if identifier.contains("tvOS") {
                            "tvOS"
                        } else if identifier.contains("watchOS") {
                            "watchOS"
                        } else if identifier.contains("visionOS") {
                            "visionOS"
                        } else {
                            "unknown"
                        };
                        
                        let build = runtime.get("buildversion")
                            .and_then(|b| b.as_str())
                            .map(|s| s.to_string());
                        
                        // Get device types (simplified - would need another simctl call)
                        let device_types = Vec::new();
                        
                        runtimes.push(SimulatorRuntime {
                            name: name.to_string(),
                            identifier: identifier.to_string(),
                            version: version.to_string(),
                            build,
                            platform: platform.to_string(),
                            device_types,
                        });
                    }
                }
            }
        }
    }
    
    runtimes
}

/// Get worker capacity information.
fn get_capacity() -> Capacity {
    let mut capacity = Capacity::default();
    
    // Get disk usage (simplified - would use statvfs on real implementation)
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = run_command("df", &["-k", "/"]) {
            // Parse df output to get disk space
            for line in output.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    if let (Ok(total), Ok(available)) = (
                        parts[1].parse::<u64>(),
                        parts[3].parse::<u64>(),
                    ) {
                        capacity.disk_total_bytes = total * 1024;
                        capacity.disk_available_bytes = available * 1024;
                    }
                }
                break;
            }
        }
    }
    
    capacity
}

/// Get list of supported features.
fn get_supported_features() -> Vec<String> {
    // For M0, only probe is fully supported
    // Other features will be added as implemented
    vec![
        "probe".to_string(),
        // Future features will be added here:
        // "tail", "fetch", "events", "has_source", "upload_source", "attestation_signing"
    ]
}

/// Run a command and return its stdout.
fn run_command(cmd: &str, args: &[&str]) -> Result<String, std::io::Error> {
    let output = Command::new(cmd)
        .args(args)
        .output()?;
    
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("command failed with exit code: {:?}", output.status.code()),
        ))
    }
}
