//! Ephemeral simulator provisioning for RCH worker (M7, bead b7s.2)
//!
//! Per PLAN.md:
//! - When destination.provisioning=ephemeral: provision clean simulator per job
//! - Naming convention: rch-ephemeral-<job_id>
//! - Record created UDID in artifacts (NOT in job_key)
//! - Delete simulator after artifact collection or cancellation
//! - Log warning and set ephemeral_cleanup_failed=true if cleanup fails
//! - Startup sweep: delete orphaned ephemeral simulators

use serde::{Deserialize, Serialize};
use std::io;
use std::process::Command;
use thiserror::Error;

/// Ephemeral simulator naming prefix
pub const EPHEMERAL_PREFIX: &str = "rch-ephemeral-";

/// Errors from simulator operations
#[derive(Debug, Error)]
pub enum SimulatorError {
    #[error("simctl command failed: {0}")]
    SimctlFailed(String),

    #[error("failed to parse simctl output: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("no matching device type found for: {0}")]
    NoMatchingDeviceType(String),

    #[error("no matching runtime found for: {0}")]
    NoMatchingRuntime(String),
}

/// Result type for simulator operations
pub type SimulatorResult<T> = Result<T, SimulatorError>;

/// Created ephemeral simulator info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralSimulator {
    /// UDID of the created simulator
    pub udid: String,

    /// Name (rch-ephemeral-<job_id>)
    pub name: String,

    /// Device type identifier
    pub device_type_identifier: String,

    /// Runtime identifier
    pub runtime_identifier: String,
}

/// Provision an ephemeral simulator for a job.
///
/// Creates a new simulator with naming convention `rch-ephemeral-<job_id>`.
/// The caller is responsible for cleanup via `delete_simulator`.
pub fn provision_ephemeral(
    job_id: &str,
    device_type: &str,
    runtime_identifier: &str,
) -> SimulatorResult<EphemeralSimulator> {
    let name = format!("{}{}", EPHEMERAL_PREFIX, job_id);

    // Create simulator: xcrun simctl create <name> <device_type> <runtime>
    let output = Command::new("xcrun")
        .args(["simctl", "create", &name, device_type, runtime_identifier])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimulatorError::SimctlFailed(format!(
            "simctl create failed: {}",
            stderr.trim()
        )));
    }

    // Output is the UDID
    let udid = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if udid.is_empty() {
        return Err(SimulatorError::ParseError(
            "simctl create returned empty UDID".to_string(),
        ));
    }

    // Boot the simulator
    let boot_output = Command::new("xcrun")
        .args(["simctl", "boot", &udid])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    // Boot may fail if already booted (not fatal)
    if !boot_output.status.success() {
        let stderr = String::from_utf8_lossy(&boot_output.stderr);
        // Only log, don't fail - simulator might already be booted
        eprintln!("Warning: simctl boot: {}", stderr.trim());
    }

    Ok(EphemeralSimulator {
        udid,
        name,
        device_type_identifier: device_type.to_string(),
        runtime_identifier: runtime_identifier.to_string(),
    })
}

/// Delete a simulator by UDID.
///
/// Shuts down the simulator first if running, then deletes it.
/// Returns Ok(()) if successful, Err if deletion fails.
pub fn delete_simulator(udid: &str) -> SimulatorResult<()> {
    // First try to shutdown (ignore errors - might not be running)
    let _ = Command::new("xcrun")
        .args(["simctl", "shutdown", udid])
        .output();

    // Delete the simulator
    let output = Command::new("xcrun")
        .args(["simctl", "delete", udid])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimulatorError::SimctlFailed(format!(
            "simctl delete failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Delete a simulator by name.
pub fn delete_simulator_by_name(name: &str) -> SimulatorResult<()> {
    // First try to shutdown (ignore errors)
    let _ = Command::new("xcrun")
        .args(["simctl", "shutdown", name])
        .output();

    // Delete the simulator
    let output = Command::new("xcrun")
        .args(["simctl", "delete", name])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimulatorError::SimctlFailed(format!(
            "simctl delete failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Simulator info from simctl list output
#[derive(Debug, Clone, Deserialize)]
struct SimctlDevice {
    name: String,
    udid: String,
    #[serde(default)]
    state: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SimctlDevices {
    devices: std::collections::HashMap<String, Vec<SimctlDevice>>,
}

/// Find orphaned ephemeral simulators from previous runs.
///
/// Returns list of UDIDs for simulators matching the ephemeral naming convention.
pub fn find_orphaned_ephemeral() -> SimulatorResult<Vec<(String, String)>> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devices", "-j"])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimulatorError::SimctlFailed(format!(
            "simctl list failed: {}",
            stderr.trim()
        )));
    }

    let parsed: SimctlDevices = serde_json::from_slice(&output.stdout)
        .map_err(|e| SimulatorError::ParseError(format!("failed to parse devices: {}", e)))?;

    let mut orphaned = Vec::new();

    for (_runtime, devices) in parsed.devices {
        for device in devices {
            if device.name.starts_with(EPHEMERAL_PREFIX) {
                orphaned.push((device.udid, device.name));
            }
        }
    }

    Ok(orphaned)
}

/// Clean up all orphaned ephemeral simulators.
///
/// This should be called at worker startup to clean up simulators from crashed runs.
/// Returns count of successfully deleted simulators.
pub fn cleanup_orphaned() -> (usize, Vec<String>) {
    let mut deleted = 0;
    let mut errors = Vec::new();

    match find_orphaned_ephemeral() {
        Ok(orphans) => {
            for (udid, name) in orphans {
                match delete_simulator(&udid) {
                    Ok(()) => {
                        eprintln!("Cleaned up orphaned simulator: {} ({})", name, udid);
                        deleted += 1;
                    }
                    Err(e) => {
                        errors.push(format!("{}: {}", name, e));
                    }
                }
            }
        }
        Err(e) => {
            errors.push(format!("Failed to list orphaned simulators: {}", e));
        }
    }

    (deleted, errors)
}

/// Check if a simulator with the given name exists.
pub fn simulator_exists(name: &str) -> bool {
    if let Ok(orphans) = find_orphaned_ephemeral() {
        orphans.iter().any(|(_, n)| n == name)
    } else {
        false
    }
}

/// Find device type identifier matching a device name.
///
/// E.g., "iPhone 16" -> "com.apple.CoreSimulator.SimDeviceType.iPhone-16"
pub fn find_device_type(name: &str) -> SimulatorResult<String> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devicetypes", "-j"])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        return Err(SimulatorError::SimctlFailed(
            "simctl list devicetypes failed".to_string(),
        ));
    }

    #[derive(Deserialize)]
    struct DeviceType {
        name: String,
        identifier: String,
    }

    #[derive(Deserialize)]
    struct DeviceTypes {
        devicetypes: Vec<DeviceType>,
    }

    let parsed: DeviceTypes = serde_json::from_slice(&output.stdout)
        .map_err(|e| SimulatorError::ParseError(format!("failed to parse devicetypes: {}", e)))?;

    // Find matching device type
    for dt in parsed.devicetypes {
        if dt.name == name {
            return Ok(dt.identifier);
        }
    }

    Err(SimulatorError::NoMatchingDeviceType(name.to_string()))
}

/// Find runtime identifier matching a platform and OS version.
///
/// E.g., ("iOS", "18.2") -> "com.apple.CoreSimulator.SimRuntime.iOS-18-2"
pub fn find_runtime(platform: &str, os_version: &str) -> SimulatorResult<String> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "runtimes", "-j"])
        .output()
        .map_err(|e| SimulatorError::Io(e))?;

    if !output.status.success() {
        return Err(SimulatorError::SimctlFailed(
            "simctl list runtimes failed".to_string(),
        ));
    }

    #[derive(Deserialize)]
    struct Runtime {
        name: String,
        identifier: String,
        version: String,
        #[serde(rename = "isAvailable")]
        is_available: Option<bool>,
    }

    #[derive(Deserialize)]
    struct Runtimes {
        runtimes: Vec<Runtime>,
    }

    let parsed: Runtimes = serde_json::from_slice(&output.stdout)
        .map_err(|e| SimulatorError::ParseError(format!("failed to parse runtimes: {}", e)))?;

    // Normalize platform name (e.g., "iOS Simulator" -> "iOS")
    let normalized_platform = platform
        .replace(" Simulator", "")
        .replace("Simulator", "");

    // Find matching runtime
    for rt in parsed.runtimes {
        if rt.is_available != Some(true) {
            continue;
        }
        if rt.name.contains(&normalized_platform) && rt.version == os_version {
            return Ok(rt.identifier);
        }
    }

    Err(SimulatorError::NoMatchingRuntime(format!(
        "{} {}",
        platform, os_version
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ephemeral_name_format() {
        let job_id = "job-12345";
        let name = format!("{}{}", EPHEMERAL_PREFIX, job_id);
        assert_eq!(name, "rch-ephemeral-job-12345");
        assert!(name.starts_with(EPHEMERAL_PREFIX));
    }

    #[test]
    fn test_ephemeral_prefix_constant() {
        assert_eq!(EPHEMERAL_PREFIX, "rch-ephemeral-");
    }

    // Note: Actual simctl tests require macOS with Xcode installed
    // These would be integration tests, not unit tests
}
