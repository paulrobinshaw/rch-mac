//! Toolchain resolution for host-side Xcode matching
//!
//! Resolves Xcode constraints from config against worker capabilities.
//! Per PLAN.md normative spec:
//! - Exact build match preferred
//! - Version match with highest version as fallback (with warning)
//! - No match → fail with WORKER_INCOMPATIBLE

use serde::{Deserialize, Serialize};

use crate::worker::{Capabilities, MacOSInfo, XcodeVersion};

/// Schema version for toolchain.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/toolchain@1";

/// Resolved toolchain identity for job_key_inputs.toolchain
///
/// Per PLAN.md normative spec, job_key_inputs.toolchain MUST include:
/// - xcode_build (e.g., "16C5032a")
/// - developer_dir (absolute path on worker)
/// - macos_version (e.g., "15.3.1")
/// - macos_build (e.g., "24D60")
/// - arch (e.g., "arm64")
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainIdentity {
    /// Xcode build identifier (e.g., "15F31d", "16C5032a")
    pub xcode_build: String,

    /// DEVELOPER_DIR value (absolute path on worker)
    pub developer_dir: String,

    /// macOS marketing version (e.g., "14.5", "15.3.1")
    pub macos_version: String,

    /// macOS build identifier (e.g., "23F79", "24D60")
    pub macos_build: String,

    /// CPU architecture (e.g., "arm64")
    pub arch: String,
}

impl ToolchainIdentity {
    /// Create from resolved Xcode version and macOS info
    pub fn new(xcode: &XcodeVersion, macos: &MacOSInfo) -> Self {
        Self {
            xcode_build: xcode.build.clone(),
            developer_dir: xcode.developer_dir.clone(),
            macos_version: macos.version.clone(),
            macos_build: macos.build.clone(),
            arch: macos.architecture.clone(),
        }
    }

    /// Compute filesystem-safe toolchain key
    ///
    /// Per PLAN.md: `xcode_<build>__macos_<major>__<arch>`
    /// Example: "xcode_15F31d__macos_15__arm64"
    pub fn toolchain_key(&self) -> String {
        // Extract major version from macos_version (e.g., "15.3.1" -> "15")
        let macos_major = self
            .macos_version
            .split('.')
            .next()
            .unwrap_or(&self.macos_version);

        format!(
            "xcode_{}__macos_{}__{}",
            self.xcode_build, macos_major, self.arch
        )
    }
}

/// Xcode constraint from config
///
/// Per PLAN.md: "The repo config or CLI MAY specify a required Xcode build
/// number (preferred), version, or range."
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XcodeConstraint {
    /// Required exact build number (e.g., "15F31d") - preferred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,

    /// Required version (e.g., "15.4") - fallback if no build specified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Minimum version (e.g., "15.0") - for version ranges
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
}

impl XcodeConstraint {
    /// Create a constraint requiring an exact build
    pub fn exact_build(build: impl Into<String>) -> Self {
        Self {
            build: Some(build.into()),
            version: None,
            min_version: None,
        }
    }

    /// Create a constraint requiring a specific version
    pub fn exact_version(version: impl Into<String>) -> Self {
        Self {
            build: None,
            version: Some(version.into()),
            min_version: None,
        }
    }

    /// Create a constraint with a minimum version
    pub fn min_version(min: impl Into<String>) -> Self {
        Self {
            build: None,
            version: None,
            min_version: Some(min.into()),
        }
    }

    /// Check if this constraint is empty (matches any Xcode)
    pub fn is_empty(&self) -> bool {
        self.build.is_none() && self.version.is_none() && self.min_version.is_none()
    }
}

/// Toolchain resolution errors
#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    /// No Xcode matches the constraint
    #[error("No Xcode version matches constraint: {constraint}")]
    NoMatch { constraint: String },

    /// No Xcode versions available on worker
    #[error("Worker has no Xcode installations")]
    NoXcodeInstalled,

    /// Version parsing error
    #[error("Invalid version format: {version}")]
    InvalidVersion { version: String },
}

/// Result of toolchain resolution
#[derive(Debug)]
pub struct ToolchainResolution {
    /// The resolved toolchain identity
    pub identity: ToolchainIdentity,

    /// The matched Xcode version entry
    pub xcode: XcodeVersion,

    /// Whether this was an exact build match (vs version fallback)
    pub exact_match: bool,

    /// Warning message if not an exact match
    pub warning: Option<String>,
}

/// Resolve toolchain from capabilities using constraint
///
/// Resolution algorithm per PLAN.md:
/// 1. If constraint specifies build: find exact build match
/// 2. If constraint specifies version: find exact version match, prefer highest build
/// 3. If constraint specifies min_version: find all >= min, prefer highest version/build
/// 4. If constraint is empty: use active_xcode or highest available
/// 5. If multiple match: prefer exact build match, then highest version
/// 6. If no match: fail with WORKER_INCOMPATIBLE
pub fn resolve_toolchain(
    capabilities: &Capabilities,
    constraint: &XcodeConstraint,
) -> Result<ToolchainResolution, ToolchainError> {
    if capabilities.xcode_versions.is_empty() {
        return Err(ToolchainError::NoXcodeInstalled);
    }

    // Try to find a matching Xcode
    let (xcode, exact_match, warning) = if let Some(ref build) = constraint.build {
        // Exact build match required
        match find_by_build(&capabilities.xcode_versions, build) {
            Some(xcode) => (xcode, true, None),
            None => {
                return Err(ToolchainError::NoMatch {
                    constraint: format!("build={}", build),
                })
            }
        }
    } else if let Some(ref version) = constraint.version {
        // Version match, prefer exact build if multiple exist
        match find_by_version(&capabilities.xcode_versions, version) {
            Some(xcode) => {
                // Check if there are multiple with same version
                let count = capabilities
                    .xcode_versions
                    .iter()
                    .filter(|x| x.version == *version)
                    .count();
                let warning = if count > 1 {
                    Some(format!(
                        "Multiple Xcode {} installations found, using build {}",
                        version, xcode.build
                    ))
                } else {
                    None
                };
                (xcode, true, warning)
            }
            None => {
                return Err(ToolchainError::NoMatch {
                    constraint: format!("version={}", version),
                })
            }
        }
    } else if let Some(ref min_version) = constraint.min_version {
        // Minimum version, find highest matching
        match find_min_version(&capabilities.xcode_versions, min_version) {
            Some(xcode) => {
                let warning = Some(format!(
                    "Using Xcode {} (build {}) for min_version={} constraint",
                    xcode.version, xcode.build, min_version
                ));
                (xcode, false, warning)
            }
            None => {
                return Err(ToolchainError::NoMatch {
                    constraint: format!("min_version={}", min_version),
                })
            }
        }
    } else {
        // No constraint: use active or highest available
        let xcode = capabilities
            .active_xcode
            .as_ref()
            .unwrap_or_else(|| find_highest(&capabilities.xcode_versions).unwrap());
        (xcode, false, None)
    };

    let identity = ToolchainIdentity::new(xcode, &capabilities.macos);

    Ok(ToolchainResolution {
        identity,
        xcode: xcode.clone(),
        exact_match,
        warning,
    })
}

/// Find Xcode by exact build number
fn find_by_build<'a>(xcodes: &'a [XcodeVersion], build: &str) -> Option<&'a XcodeVersion> {
    xcodes.iter().find(|x| x.build == build)
}

/// Find Xcode by version, preferring highest build if multiple exist
fn find_by_version<'a>(xcodes: &'a [XcodeVersion], version: &str) -> Option<&'a XcodeVersion> {
    xcodes
        .iter()
        .filter(|x| x.version == version)
        .max_by(|a, b| a.build.cmp(&b.build))
}

/// Find highest Xcode meeting minimum version
fn find_min_version<'a>(xcodes: &'a [XcodeVersion], min_version: &str) -> Option<&'a XcodeVersion> {
    xcodes
        .iter()
        .filter(|x| compare_versions(&x.version, min_version) >= std::cmp::Ordering::Equal)
        .max_by(|a, b| {
            compare_versions(&a.version, &b.version).then_with(|| a.build.cmp(&b.build))
        })
}

/// Find highest Xcode version available
fn find_highest(xcodes: &[XcodeVersion]) -> Option<&XcodeVersion> {
    xcodes.iter().max_by(|a, b| {
        compare_versions(&a.version, &b.version).then_with(|| a.build.cmp(&b.build))
    })
}

/// Compare semantic versions (e.g., "15.4" vs "16.0")
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for (ap, bp) in a_parts.iter().zip(b_parts.iter()) {
        match ap.cmp(bp) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    a_parts.len().cmp(&b_parts.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::Capacity;

    fn sample_capabilities() -> Capabilities {
        Capabilities {
            schema_version: 1,
            schema_id: "rch-xcode/capabilities@1".to_string(),
            created_at: chrono::Utc::now(),
            rch_xcode_lane_version: "0.1.0".to_string(),
            protocol_min: 1,
            protocol_max: 1,
            features: vec![],
            xcode_versions: vec![
                XcodeVersion {
                    version: "15.4".to_string(),
                    build: "15F31d".to_string(),
                    path: "/Applications/Xcode-15.4.app".to_string(),
                    developer_dir: "/Applications/Xcode-15.4.app/Contents/Developer".to_string(),
                },
                XcodeVersion {
                    version: "16.0".to_string(),
                    build: "16A5171c".to_string(),
                    path: "/Applications/Xcode-16.0.app".to_string(),
                    developer_dir: "/Applications/Xcode-16.0.app/Contents/Developer".to_string(),
                },
                XcodeVersion {
                    version: "16.2".to_string(),
                    build: "16C5032a".to_string(),
                    path: "/Applications/Xcode.app".to_string(),
                    developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
                },
            ],
            active_xcode: Some(XcodeVersion {
                version: "16.2".to_string(),
                build: "16C5032a".to_string(),
                path: "/Applications/Xcode.app".to_string(),
                developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            }),
            macos: MacOSInfo {
                version: "15.3.1".to_string(),
                build: "24D60".to_string(),
                architecture: "arm64".to_string(),
            },
            simulators: vec![],
            runtimes: vec![],
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
    fn test_toolchain_identity_new() {
        let xcode = XcodeVersion {
            version: "16.2".to_string(),
            build: "16C5032a".to_string(),
            path: "/Applications/Xcode.app".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
        };
        let macos = MacOSInfo {
            version: "15.3.1".to_string(),
            build: "24D60".to_string(),
            architecture: "arm64".to_string(),
        };

        let identity = ToolchainIdentity::new(&xcode, &macos);

        assert_eq!(identity.xcode_build, "16C5032a");
        assert_eq!(
            identity.developer_dir,
            "/Applications/Xcode.app/Contents/Developer"
        );
        assert_eq!(identity.macos_version, "15.3.1");
        assert_eq!(identity.macos_build, "24D60");
        assert_eq!(identity.arch, "arm64");
    }

    #[test]
    fn test_toolchain_key() {
        let identity = ToolchainIdentity {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3.1".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        };

        assert_eq!(identity.toolchain_key(), "xcode_16C5032a__macos_15__arm64");
    }

    #[test]
    fn test_resolve_exact_build() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::exact_build("16C5032a");

        let result = resolve_toolchain(&caps, &constraint).unwrap();

        assert!(result.exact_match);
        assert_eq!(result.xcode.build, "16C5032a");
        assert_eq!(result.identity.xcode_build, "16C5032a");
        assert!(result.warning.is_none());
    }

    #[test]
    fn test_resolve_exact_version() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::exact_version("15.4");

        let result = resolve_toolchain(&caps, &constraint).unwrap();

        assert!(result.exact_match);
        assert_eq!(result.xcode.version, "15.4");
        assert_eq!(result.xcode.build, "15F31d");
    }

    #[test]
    fn test_resolve_min_version() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::min_version("16.0");

        let result = resolve_toolchain(&caps, &constraint).unwrap();

        assert!(!result.exact_match);
        // Should get highest >= 16.0, which is 16.2
        assert_eq!(result.xcode.version, "16.2");
        assert!(result.warning.is_some());
    }

    #[test]
    fn test_resolve_no_constraint() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::default();

        let result = resolve_toolchain(&caps, &constraint).unwrap();

        // Should use active_xcode
        assert_eq!(result.xcode.version, "16.2");
        assert!(!result.exact_match);
    }

    #[test]
    fn test_resolve_build_not_found() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::exact_build("NONEXISTENT");

        let result = resolve_toolchain(&caps, &constraint);

        assert!(result.is_err());
        match result {
            Err(ToolchainError::NoMatch { constraint }) => {
                assert!(constraint.contains("NONEXISTENT"));
            }
            _ => panic!("Expected NoMatch error"),
        }
    }

    #[test]
    fn test_resolve_version_not_found() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::exact_version("99.0");

        let result = resolve_toolchain(&caps, &constraint);

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_min_version_not_satisfied() {
        let caps = sample_capabilities();
        let constraint = XcodeConstraint::min_version("99.0");

        let result = resolve_toolchain(&caps, &constraint);

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_no_xcode_installed() {
        let mut caps = sample_capabilities();
        caps.xcode_versions.clear();
        caps.active_xcode = None;

        let constraint = XcodeConstraint::default();
        let result = resolve_toolchain(&caps, &constraint);

        assert!(matches!(result, Err(ToolchainError::NoXcodeInstalled)));
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(
            compare_versions("15.4", "16.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("16.0", "15.4"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("16.2", "16.2"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("16.2.1", "16.2"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("16", "16.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_xcode_constraint_is_empty() {
        assert!(XcodeConstraint::default().is_empty());
        assert!(!XcodeConstraint::exact_build("15F31d").is_empty());
        assert!(!XcodeConstraint::exact_version("15.4").is_empty());
        assert!(!XcodeConstraint::min_version("15.0").is_empty());
    }

    #[test]
    fn test_toolchain_identity_serialization() {
        let identity = ToolchainIdentity {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3.1".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        };

        let json = serde_json::to_string(&identity).unwrap();
        assert!(json.contains(r#""xcode_build":"16C5032a""#));
        assert!(json.contains(r#""macos_version":"15.3.1""#));
        assert!(json.contains(r#""arch":"arm64""#));

        let parsed: ToolchainIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, identity);
    }
}
