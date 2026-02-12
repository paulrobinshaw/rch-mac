//! Destination resolution for RCH Xcode Lane
//!
//! Resolves destination constraints from config against worker capabilities.json.
//! Handles resolving "latest" to concrete versions and finding matching simulator
//! runtime identifiers.

use crate::worker::{Capabilities, Runtime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Errors during destination resolution
#[derive(Debug, Error)]
pub enum DestinationError {
    /// No matching destination found on worker
    #[error("no matching destination found: {reason}")]
    WorkerIncompatible { reason: String },

    /// Invalid destination constraint format
    #[error("invalid destination constraint: {0}")]
    InvalidConstraint(String),

    /// Missing required field in constraint
    #[error("missing required field in destination constraint: {0}")]
    MissingField(String),
}

/// Provisioning mode for simulators
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Provisioning {
    /// Use an existing simulator
    #[default]
    Existing,
    /// Create an ephemeral simulator for the job
    Ephemeral,
}


/// Parsed destination constraint from config
#[derive(Debug, Clone)]
pub struct DestinationConstraint {
    /// Platform (e.g., "iOS Simulator", "macOS")
    pub platform: String,
    /// Device name (e.g., "iPhone 16")
    pub name: Option<String>,
    /// OS version constraint ("latest" or specific version like "18.0")
    pub os: Option<String>,
    /// Provisioning mode (existing or ephemeral)
    pub provisioning: Provisioning,
    /// Original constraint string for auditing
    pub original: String,
}

/// Resolved destination with all required fields for job_key_inputs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedDestination {
    /// Platform (e.g., "iOS Simulator", "macOS")
    pub platform: String,

    /// Device name when applicable
    pub name: String,

    /// Resolved concrete OS version (MUST NOT be "latest")
    pub os_version: String,

    /// Provisioning mode
    pub provisioning: Provisioning,

    /// Original constraint string from config
    pub original_constraint: String,

    /// Runtime identifier (for simulators)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_identifier: Option<String>,

    /// Runtime build string (for simulators)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_build: Option<String>,

    /// Device type identifier (for simulators)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type_identifier: Option<String>,

    /// Actual UDID used (optional, filled in at execution time)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udid: Option<String>,
}

impl DestinationConstraint {
    /// Parse a destination constraint string (e.g., "platform=iOS Simulator,name=iPhone 16,OS=latest")
    pub fn parse(constraint: &str) -> Result<Self, DestinationError> {
        let mut fields: HashMap<String, String> = HashMap::new();

        // Parse comma-separated key=value pairs
        for part in constraint.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let eq_pos = part
                .find('=')
                .ok_or_else(|| DestinationError::InvalidConstraint(format!(
                    "expected key=value pair, got: {}", part
                )))?;

            let key = part[..eq_pos].trim().to_lowercase();
            let value = part[eq_pos + 1..].trim().to_string();

            fields.insert(key, value);
        }

        // Extract platform (required)
        let platform = fields
            .remove("platform")
            .ok_or_else(|| DestinationError::MissingField("platform".to_string()))?;

        // Extract optional fields
        let name = fields.remove("name");
        let os = fields.remove("os");

        // Default provisioning is "existing" for MVP
        let provisioning = Provisioning::Existing;

        Ok(Self {
            platform,
            name,
            os,
            provisioning,
            original: constraint.to_string(),
        })
    }

    /// Check if this constraint requires a simulator
    pub fn is_simulator(&self) -> bool {
        let platform_lower = self.platform.to_lowercase();
        platform_lower.contains("simulator")
    }
}

/// Resolve a destination constraint against worker capabilities
pub fn resolve_destination(
    constraint: &DestinationConstraint,
    capabilities: &Capabilities,
) -> Result<ResolvedDestination, DestinationError> {
    if constraint.is_simulator() {
        resolve_simulator_destination(constraint, capabilities)
    } else {
        resolve_device_destination(constraint, capabilities)
    }
}

/// Resolve a simulator destination
fn resolve_simulator_destination(
    constraint: &DestinationConstraint,
    capabilities: &Capabilities,
) -> Result<ResolvedDestination, DestinationError> {
    // Determine the target platform type (iOS, tvOS, watchOS, etc.)
    let platform_type = extract_platform_type(&constraint.platform);

    // Find matching runtimes for this platform
    let matching_runtimes: Vec<&Runtime> = capabilities
        .runtimes
        .iter()
        .filter(|r| {
            r.available && runtime_matches_platform(r, &platform_type)
        })
        .collect();

    if matching_runtimes.is_empty() {
        return Err(DestinationError::WorkerIncompatible {
            reason: format!("no available {} runtimes on worker", platform_type),
        });
    }

    // Resolve OS version
    let target_version = resolve_os_version(&constraint.os, &matching_runtimes)?;

    // Find the runtime for the resolved version
    let runtime = matching_runtimes
        .iter()
        .find(|r| r.version == target_version)
        .ok_or_else(|| DestinationError::WorkerIncompatible {
            reason: format!("no runtime found for {} {}", platform_type, target_version),
        })?;

    // Find matching simulator device
    let (device_name, device_type) = find_matching_device(constraint, runtime, capabilities)?;

    Ok(ResolvedDestination {
        platform: constraint.platform.clone(),
        name: device_name,
        os_version: target_version,
        provisioning: constraint.provisioning,
        original_constraint: constraint.original.clone(),
        sim_runtime_identifier: Some(runtime.identifier.clone()),
        sim_runtime_build: Some(runtime.build.clone()),
        device_type_identifier: Some(device_type),
        udid: None,
    })
}

/// Resolve a non-simulator (real device or macOS) destination
fn resolve_device_destination(
    constraint: &DestinationConstraint,
    capabilities: &Capabilities,
) -> Result<ResolvedDestination, DestinationError> {
    let platform_lower = constraint.platform.to_lowercase();

    if platform_lower == "macos" || platform_lower == "mac" {
        // macOS destination - use the worker's macOS version
        let os_version = if constraint.os.as_deref() == Some("latest") || constraint.os.is_none() {
            capabilities.macos.version.clone()
        } else {
            let requested = constraint.os.as_ref().unwrap();
            if !capabilities.macos.version.starts_with(requested) {
                return Err(DestinationError::WorkerIncompatible {
                    reason: format!(
                        "worker macOS version {} does not match requested {}",
                        capabilities.macos.version, requested
                    ),
                });
            }
            capabilities.macos.version.clone()
        };

        Ok(ResolvedDestination {
            platform: constraint.platform.clone(),
            name: "My Mac".to_string(),
            os_version,
            provisioning: constraint.provisioning,
            original_constraint: constraint.original.clone(),
            sim_runtime_identifier: None,
            sim_runtime_build: None,
            device_type_identifier: None,
            udid: None,
        })
    } else {
        // Real device (iOS device, etc.) - not fully supported in MVP
        Err(DestinationError::WorkerIncompatible {
            reason: format!(
                "real device destinations ({}) are not supported in MVP",
                constraint.platform
            ),
        })
    }
}

/// Extract platform type from platform string (e.g., "iOS Simulator" -> "iOS")
fn extract_platform_type(platform: &str) -> String {
    let platform_lower = platform.to_lowercase();
    if platform_lower.contains("ios") {
        "iOS".to_string()
    } else if platform_lower.contains("tvos") {
        "tvOS".to_string()
    } else if platform_lower.contains("watchos") {
        "watchOS".to_string()
    } else if platform_lower.contains("visionos") || platform_lower.contains("xros") {
        "visionOS".to_string()
    } else {
        platform.to_string()
    }
}

/// Check if a runtime matches the platform type
fn runtime_matches_platform(runtime: &Runtime, platform_type: &str) -> bool {
    let name_lower = runtime.name.to_lowercase();
    let platform_lower = platform_type.to_lowercase();

    // Check if runtime name contains the platform type
    name_lower.contains(&platform_lower)
        || runtime.identifier.to_lowercase().contains(&platform_lower)
}

/// Resolve OS version from constraint ("latest" -> highest available, or validate specific version)
fn resolve_os_version(
    os_constraint: &Option<String>,
    runtimes: &[&Runtime],
) -> Result<String, DestinationError> {
    match os_constraint.as_deref() {
        None | Some("latest") => {
            // Find highest available version
            let highest = runtimes
                .iter()
                .max_by(|a, b| compare_versions(&a.version, &b.version))
                .ok_or_else(|| DestinationError::WorkerIncompatible {
                    reason: "no runtimes available".to_string(),
                })?;
            Ok(highest.version.clone())
        }
        Some(version) => {
            // Find matching runtime for specific version
            let matching = runtimes.iter().find(|r| {
                r.version == version || r.version.starts_with(&format!("{}.", version))
            });

            match matching {
                Some(runtime) => Ok(runtime.version.clone()),
                None => {
                    let available: Vec<&str> = runtimes.iter().map(|r| r.version.as_str()).collect();
                    Err(DestinationError::WorkerIncompatible {
                        reason: format!(
                            "requested OS version {} not found; available: {}",
                            version,
                            available.join(", ")
                        ),
                    })
                }
            }
        }
    }
}

/// Compare version strings (e.g., "17.5" vs "18.0")
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<u32> = a.split('.').filter_map(|s| s.parse().ok()).collect();
    let b_parts: Vec<u32> = b.split('.').filter_map(|s| s.parse().ok()).collect();

    for i in 0..std::cmp::max(a_parts.len(), b_parts.len()) {
        let a_val = a_parts.get(i).copied().unwrap_or(0);
        let b_val = b_parts.get(i).copied().unwrap_or(0);
        match a_val.cmp(&b_val) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Find a matching device for the destination constraint
fn find_matching_device(
    constraint: &DestinationConstraint,
    runtime: &Runtime,
    capabilities: &Capabilities,
) -> Result<(String, String), DestinationError> {
    // If a specific device name is requested, try to find it
    if let Some(ref name) = constraint.name {
        // Look for a simulator with this name and runtime
        if let Some(sim) = capabilities.simulators.iter().find(|s| {
            s.name.to_lowercase() == name.to_lowercase() && s.runtime == runtime.identifier
        }) {
            return Ok((sim.name.clone(), sim.device_type.clone()));
        }

        // If no exact match, try to find a device type that matches the name pattern
        let device_type = derive_device_type_from_name(name);
        return Ok((name.clone(), device_type));
    }

    // No specific device requested - find a default device for this runtime
    let default_device = capabilities
        .simulators
        .iter()
        .find(|s| s.runtime == runtime.identifier);

    match default_device {
        Some(sim) => Ok((sim.name.clone(), sim.device_type.clone())),
        None => {
            // No simulators found - generate a reasonable default
            let platform_type = extract_platform_type(&constraint.platform);
            let default_name = default_device_name(&platform_type);
            let device_type = derive_device_type_from_name(&default_name);
            Ok((default_name, device_type))
        }
    }
}

/// Derive device type identifier from device name
fn derive_device_type_from_name(name: &str) -> String {
    // Convert name like "iPhone 16 Pro Max" to "com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro-Max"
    let normalized = name
        .replace(' ', "-")
        .replace("(", "")
        .replace(")", "")
        .replace(".", "-");
    format!("com.apple.CoreSimulator.SimDeviceType.{}", normalized)
}

/// Get a reasonable default device name for a platform
fn default_device_name(platform_type: &str) -> String {
    match platform_type.to_lowercase().as_str() {
        "ios" => "iPhone 16".to_string(),
        "tvos" => "Apple TV".to_string(),
        "watchos" => "Apple Watch Series 10".to_string(),
        "visionos" => "Apple Vision Pro".to_string(),
        _ => "Unknown Device".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{Capacity, MacOSInfo, Simulator};

    fn sample_capabilities() -> Capabilities {
        Capabilities {
            schema_version: 1,
            schema_id: "rch-xcode/capabilities@1".to_string(),
            created_at: chrono::Utc::now(),
            rch_xcode_lane_version: "0.1.0".to_string(),
            protocol_min: 1,
            protocol_max: 1,
            features: vec![],
            xcode_versions: vec![],
            active_xcode: None,
            macos: MacOSInfo {
                version: "15.3.1".to_string(),
                build: "24D60".to_string(),
                architecture: "arm64".to_string(),
            },
            simulators: vec![
                Simulator {
                    name: "iPhone 16".to_string(),
                    udid: "11111111-1111-1111-1111-111111111111".to_string(),
                    device_type: "com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string(),
                    runtime: "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
                    state: "Shutdown".to_string(),
                },
                Simulator {
                    name: "iPhone 16 Pro".to_string(),
                    udid: "22222222-2222-2222-2222-222222222222".to_string(),
                    device_type: "com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro".to_string(),
                    runtime: "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
                    state: "Shutdown".to_string(),
                },
            ],
            runtimes: vec![
                Runtime {
                    name: "iOS 17.5".to_string(),
                    identifier: "com.apple.CoreSimulator.SimRuntime.iOS-17-5".to_string(),
                    version: "17.5".to_string(),
                    build: "21F79".to_string(),
                    available: true,
                },
                Runtime {
                    name: "iOS 18.2".to_string(),
                    identifier: "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
                    version: "18.2".to_string(),
                    build: "22C150".to_string(),
                    available: true,
                },
            ],
            tooling: Default::default(),
            capacity: Capacity {
                max_concurrent_jobs: 2,
                disk_free_bytes: 100_000_000_000,
                disk_total_bytes: None,
                memory_available_bytes: None,
            },
            limits: Default::default(),
        }
    }

    #[test]
    fn test_parse_constraint_full() {
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator,name=iPhone 16,OS=18.0"
        ).unwrap();

        assert_eq!(constraint.platform, "iOS Simulator");
        assert_eq!(constraint.name, Some("iPhone 16".to_string()));
        assert_eq!(constraint.os, Some("18.0".to_string()));
        assert!(constraint.is_simulator());
    }

    #[test]
    fn test_parse_constraint_minimal() {
        let constraint = DestinationConstraint::parse("platform=macOS").unwrap();

        assert_eq!(constraint.platform, "macOS");
        assert!(constraint.name.is_none());
        assert!(constraint.os.is_none());
        assert!(!constraint.is_simulator());
    }

    #[test]
    fn test_parse_constraint_with_spaces() {
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator, name=iPhone 16 Pro Max, OS=latest"
        ).unwrap();

        assert_eq!(constraint.platform, "iOS Simulator");
        assert_eq!(constraint.name, Some("iPhone 16 Pro Max".to_string()));
        assert_eq!(constraint.os, Some("latest".to_string()));
    }

    #[test]
    fn test_parse_constraint_missing_platform() {
        let result = DestinationConstraint::parse("name=iPhone 16");
        assert!(result.is_err());
        match result {
            Err(DestinationError::MissingField(field)) => assert_eq!(field, "platform"),
            _ => panic!("expected MissingField error"),
        }
    }

    #[test]
    fn test_resolve_simulator_latest() {
        let capabilities = sample_capabilities();
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator,name=iPhone 16,OS=latest"
        ).unwrap();

        let resolved = resolve_destination(&constraint, &capabilities).unwrap();

        assert_eq!(resolved.platform, "iOS Simulator");
        assert_eq!(resolved.name, "iPhone 16");
        assert_eq!(resolved.os_version, "18.2"); // Highest available
        assert_eq!(
            resolved.sim_runtime_identifier,
            Some("com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string())
        );
        assert_eq!(resolved.sim_runtime_build, Some("22C150".to_string()));
    }

    #[test]
    fn test_resolve_simulator_specific_version() {
        let capabilities = sample_capabilities();
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator,name=iPhone 16,OS=17.5"
        ).unwrap();

        let resolved = resolve_destination(&constraint, &capabilities).unwrap();

        assert_eq!(resolved.os_version, "17.5");
        assert_eq!(
            resolved.sim_runtime_identifier,
            Some("com.apple.CoreSimulator.SimRuntime.iOS-17-5".to_string())
        );
    }

    #[test]
    fn test_resolve_simulator_version_not_found() {
        let capabilities = sample_capabilities();
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator,name=iPhone 16,OS=16.0"
        ).unwrap();

        let result = resolve_destination(&constraint, &capabilities);

        assert!(result.is_err());
        match result {
            Err(DestinationError::WorkerIncompatible { reason }) => {
                assert!(reason.contains("16.0"));
                assert!(reason.contains("not found"));
            }
            _ => panic!("expected WorkerIncompatible error"),
        }
    }

    #[test]
    fn test_resolve_macos() {
        let capabilities = sample_capabilities();
        let constraint = DestinationConstraint::parse("platform=macOS").unwrap();

        let resolved = resolve_destination(&constraint, &capabilities).unwrap();

        assert_eq!(resolved.platform, "macOS");
        assert_eq!(resolved.name, "My Mac");
        assert_eq!(resolved.os_version, "15.3.1");
        assert!(resolved.sim_runtime_identifier.is_none());
    }

    #[test]
    fn test_version_comparison() {
        assert!(compare_versions("18.2", "17.5") == std::cmp::Ordering::Greater);
        assert!(compare_versions("17.5", "18.2") == std::cmp::Ordering::Less);
        assert!(compare_versions("18.0", "18.0") == std::cmp::Ordering::Equal);
        assert!(compare_versions("18.0.1", "18.0") == std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_derive_device_type() {
        assert_eq!(
            derive_device_type_from_name("iPhone 16"),
            "com.apple.CoreSimulator.SimDeviceType.iPhone-16"
        );
        assert_eq!(
            derive_device_type_from_name("iPhone 16 Pro Max"),
            "com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro-Max"
        );
    }

    #[test]
    fn test_resolved_destination_never_has_latest() {
        let capabilities = sample_capabilities();

        // Test with explicit "latest"
        let constraint = DestinationConstraint::parse(
            "platform=iOS Simulator,OS=latest"
        ).unwrap();
        let resolved = resolve_destination(&constraint, &capabilities).unwrap();
        assert_ne!(resolved.os_version, "latest");

        // Test with no OS specified (implicit latest)
        let constraint = DestinationConstraint::parse("platform=iOS Simulator").unwrap();
        let resolved = resolve_destination(&constraint, &capabilities).unwrap();
        assert_ne!(resolved.os_version, "latest");
    }
}
