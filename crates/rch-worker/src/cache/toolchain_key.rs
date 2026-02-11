//! Toolchain key computation for cache directory naming
//!
//! Per PLAN.md: Cache directories must be additionally keyed by toolchain identity
//! (at minimum: Xcode build number and macOS major version) to prevent
//! cross-toolchain corruption.

use serde::{Deserialize, Serialize};

/// Toolchain identity for cache keying.
///
/// This produces a filesystem-safe key that uniquely identifies the build environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolchainKey {
    /// Xcode build number (e.g., "16C5032a")
    pub xcode_build: String,
    /// macOS major version (e.g., "15")
    pub macos_major: String,
    /// Architecture (e.g., "arm64")
    pub arch: String,
}

impl ToolchainKey {
    /// Create a new toolchain key from components.
    pub fn new(xcode_build: &str, macos_version: &str, arch: &str) -> Self {
        // Extract major version from macos_version (e.g., "15.3" -> "15")
        let macos_major = macos_version
            .split('.')
            .next()
            .unwrap_or(macos_version)
            .to_string();

        Self {
            xcode_build: xcode_build.to_string(),
            macos_major,
            arch: arch.to_string(),
        }
    }

    /// Convert to a filesystem-safe directory name.
    ///
    /// Format: `xcode_<build>__macos_<major>__<arch>`
    ///
    /// Example: `xcode_16C5032a__macos_15__arm64`
    pub fn to_dir_name(&self) -> String {
        format!(
            "xcode_{}__macos_{}__{}",
            Self::sanitize(&self.xcode_build),
            Self::sanitize(&self.macos_major),
            Self::sanitize(&self.arch)
        )
    }

    /// Sanitize a string for filesystem safety.
    ///
    /// - Replaces non-alphanumeric chars (except hyphen) with underscore
    /// - Converts to lowercase for consistency
    fn sanitize(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolchain_key_basic() {
        let key = ToolchainKey::new("16C5032a", "15.3", "arm64");

        assert_eq!(key.xcode_build, "16C5032a");
        assert_eq!(key.macos_major, "15");
        assert_eq!(key.arch, "arm64");
    }

    #[test]
    fn test_toolchain_key_extracts_major_version() {
        let key = ToolchainKey::new("16C5032a", "15.3.1", "arm64");
        assert_eq!(key.macos_major, "15");

        let key2 = ToolchainKey::new("16C5032a", "14", "arm64");
        assert_eq!(key2.macos_major, "14");
    }

    #[test]
    fn test_toolchain_key_to_dir_name() {
        let key = ToolchainKey::new("16C5032a", "15.3", "arm64");
        assert_eq!(key.to_dir_name(), "xcode_16c5032a__macos_15__arm64");
    }

    #[test]
    fn test_toolchain_key_sanitizes_special_chars() {
        let key = ToolchainKey::new("16C5/032a", "15.3", "arm64");
        let dir_name = key.to_dir_name();
        assert!(!dir_name.contains('/'));
        assert!(!dir_name.contains('.'));
    }

    #[test]
    fn test_toolchain_key_equality() {
        let key1 = ToolchainKey::new("16C5032a", "15.3", "arm64");
        let key2 = ToolchainKey::new("16C5032a", "15.3", "arm64");
        let key3 = ToolchainKey::new("16C5032a", "15.3", "x86_64");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_toolchain_key_serialization() {
        let key = ToolchainKey::new("16C5032a", "15.3", "arm64");
        let json = serde_json::to_string(&key).unwrap();
        let parsed: ToolchainKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, parsed);
    }
}
