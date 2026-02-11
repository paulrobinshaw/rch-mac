//! Built-in lane defaults (layer 1)
//!
//! Hardcoded defaults for all configuration values.

use serde::{Deserialize, Serialize};

/// Built-in default configuration values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinDefaults {
    /// Overall timeout in seconds (default: 1800 = 30 minutes)
    pub overall_seconds: u64,

    /// Idle log timeout in seconds (default: 300 = 5 minutes)
    pub idle_log_seconds: u64,

    /// Connection timeout in seconds (default: 30)
    pub connect_timeout_seconds: u64,

    /// Source bundle mode (default: "worktree")
    pub bundle_mode: String,

    /// Artifact profile (default: "minimal")
    pub artifact_profile: String,

    /// Continue on failure (default: false)
    pub continue_on_failure: bool,

    /// Worker selection mode (default: "deterministic")
    pub worker_selection_mode: String,

    /// Derived data cache (default: "off")
    pub cache_derived_data: String,

    /// SPM cache (default: "off")
    pub cache_spm: String,
}

impl Default for BuiltinDefaults {
    fn default() -> Self {
        Self {
            overall_seconds: 1800,
            idle_log_seconds: 300,
            connect_timeout_seconds: 30,
            bundle_mode: "worktree".to_string(),
            artifact_profile: "minimal".to_string(),
            continue_on_failure: false,
            worker_selection_mode: "deterministic".to_string(),
            cache_derived_data: "off".to_string(),
            cache_spm: "off".to_string(),
        }
    }
}

impl BuiltinDefaults {
    /// Convert to JSON Value for merging
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "overall_seconds": self.overall_seconds,
            "idle_log_seconds": self.idle_log_seconds,
            "connect_timeout_seconds": self.connect_timeout_seconds,
            "bundle": {
                "mode": self.bundle_mode
            },
            "artifact_profile": self.artifact_profile,
            "continue_on_failure": self.continue_on_failure,
            "worker_selection": {
                "mode": self.worker_selection_mode
            },
            "cache": {
                "derived_data": self.cache_derived_data,
                "spm": self.cache_spm
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let defaults = BuiltinDefaults::default();
        assert_eq!(defaults.overall_seconds, 1800);
        assert_eq!(defaults.idle_log_seconds, 300);
        assert_eq!(defaults.connect_timeout_seconds, 30);
        assert_eq!(defaults.bundle_mode, "worktree");
        assert_eq!(defaults.artifact_profile, "minimal");
        assert!(!defaults.continue_on_failure);
    }

    #[test]
    fn test_to_value() {
        let defaults = BuiltinDefaults::default();
        let value = defaults.to_value();

        assert_eq!(value["overall_seconds"], 1800);
        assert_eq!(value["bundle"]["mode"], "worktree");
        assert_eq!(value["cache"]["derived_data"], "off");
    }
}
