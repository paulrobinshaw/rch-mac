//! Executor Environment Audit (executor_env.json)
//!
//! Emits executor_env.json per job for security auditing. Records which
//! environment variable keys were passed to the backend, which were dropped,
//! and which were overridden by the worker.
//!
//! Per PLAN.md: Values must not be recorded by default (opt-in only, redacted by default).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

/// Schema version for executor_env.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier for executor_env.json
pub const SCHEMA_ID: &str = "rch-xcode/executor_env@1";

/// Environment variable override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvOverride {
    /// The environment variable key
    pub key: String,
    /// Reason for the override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Executor environment audit (executor_env.json)
///
/// Records which environment variables were passed to the backend process,
/// which were dropped for security, and which were explicitly overridden.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorEnv {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When this audit was created
    pub created_at: DateTime<Utc>,

    /// Parent run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash)
    pub job_key: String,

    /// Environment variable keys passed to the backend (no values)
    pub passed_keys: Vec<String>,

    /// Environment variable keys present in worker env but not passed
    pub dropped_keys: Vec<String>,

    /// Environment variables explicitly set/overridden by worker
    pub overrides: Vec<EnvOverride>,
}

impl ExecutorEnv {
    /// Create a new executor environment audit
    pub fn new(run_id: String, job_id: String, job_key: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            job_key,
            passed_keys: Vec::new(),
            dropped_keys: Vec::new(),
            overrides: Vec::new(),
        }
    }

    /// Add a passed key
    pub fn add_passed_key(&mut self, key: impl Into<String>) {
        self.passed_keys.push(key.into());
    }

    /// Add a dropped key
    pub fn add_dropped_key(&mut self, key: impl Into<String>) {
        self.dropped_keys.push(key.into());
    }

    /// Add an override
    pub fn add_override(&mut self, key: impl Into<String>, reason: Option<String>) {
        self.overrides.push(EnvOverride {
            key: key.into(),
            reason,
        });
    }

    /// Set passed keys from an iterator
    pub fn with_passed_keys(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.passed_keys = keys.into_iter().map(|k| k.into()).collect();
        self
    }

    /// Set dropped keys from an iterator
    pub fn with_dropped_keys(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.dropped_keys = keys.into_iter().map(|k| k.into()).collect();
        self
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON
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
}

/// Environment variable filter for security
///
/// Determines which environment variables from the worker process
/// should be passed to the backend process.
#[derive(Debug, Clone)]
pub struct EnvFilter {
    /// Keys to always pass through (allowlist)
    allowed_keys: HashSet<String>,
    /// Keys to always drop (denylist)
    denied_keys: HashSet<String>,
    /// Whether to pass through unlisted keys
    pass_unlisted: bool,
}

impl Default for EnvFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvFilter {
    /// Create a new filter with sensible defaults for Xcode builds
    pub fn new() -> Self {
        // Minimal allowlist: keys needed for xcodebuild to function
        let allowed = [
            "HOME",
            "USER",
            "PATH",
            "SHELL",
            "TERM",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TMPDIR",
            "DEVELOPER_DIR",
            "SDKROOT",
            "TOOLCHAIN_DIR",
            "XCODE_DEVELOPER_DIR_PATH",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        // Denylist: keys that should never be passed
        let denied = [
            // Secrets
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "GITLAB_TOKEN",
            "CI_JOB_TOKEN",
            "NPM_TOKEN",
            "DOCKER_PASSWORD",
            // SSH
            "SSH_AUTH_SOCK",
            "SSH_AGENT_PID",
            // Potentially sensitive
            "HISTFILE",
            "HISTSIZE",
            "HISTCONTROL",
            // Platform-specific
            "SUDO_USER",
            "SUDO_UID",
            "SUDO_GID",
            "SUDO_COMMAND",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        Self {
            allowed_keys: allowed,
            denied_keys: denied,
            pass_unlisted: false, // Secure default: deny unlisted
        }
    }

    /// Create a permissive filter that passes most variables
    pub fn permissive() -> Self {
        Self {
            allowed_keys: HashSet::new(),
            denied_keys: Self::new().denied_keys, // Keep denylist
            pass_unlisted: true,
        }
    }

    /// Add a key to the allowlist
    pub fn allow(mut self, key: impl Into<String>) -> Self {
        self.allowed_keys.insert(key.into());
        self
    }

    /// Add a key to the denylist
    pub fn deny(mut self, key: impl Into<String>) -> Self {
        self.denied_keys.insert(key.into());
        self
    }

    /// Set whether to pass unlisted keys
    pub fn pass_unlisted(mut self, pass: bool) -> Self {
        self.pass_unlisted = pass;
        self
    }

    /// Check if a key should be passed
    pub fn should_pass(&self, key: &str) -> bool {
        // Denylist takes precedence
        if self.denied_keys.contains(key) {
            return false;
        }
        // Allowlist or unlisted check
        self.allowed_keys.contains(key) || self.pass_unlisted
    }

    /// Filter environment variables and return audit info
    ///
    /// Returns (passed_keys, dropped_keys) for audit purposes.
    pub fn filter_env(
        &self,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
        let mut passed = Vec::new();
        let mut passed_keys = Vec::new();
        let mut dropped_keys = Vec::new();

        for (key, value) in env {
            if self.should_pass(&key) {
                passed_keys.push(key.clone());
                passed.push((key, value));
            } else {
                dropped_keys.push(key);
            }
        }

        // Sort for deterministic output
        passed_keys.sort();
        dropped_keys.sort();

        (passed, passed_keys, dropped_keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_env_new() {
        let env = ExecutorEnv::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        assert_eq!(env.schema_version, SCHEMA_VERSION);
        assert_eq!(env.schema_id, SCHEMA_ID);
        assert_eq!(env.run_id, "run-123");
        assert_eq!(env.job_id, "job-456");
        assert_eq!(env.job_key, "key-789");
        assert!(env.passed_keys.is_empty());
        assert!(env.dropped_keys.is_empty());
        assert!(env.overrides.is_empty());
    }

    #[test]
    fn test_executor_env_with_keys() {
        let env = ExecutorEnv::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        )
        .with_passed_keys(["HOME", "PATH", "DEVELOPER_DIR"])
        .with_dropped_keys(["AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN"]);

        assert_eq!(env.passed_keys.len(), 3);
        assert_eq!(env.dropped_keys.len(), 2);
    }

    #[test]
    fn test_executor_env_serialization() {
        let mut env = ExecutorEnv::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );

        env.add_passed_key("HOME");
        env.add_passed_key("PATH");
        env.add_dropped_key("AWS_SECRET_ACCESS_KEY");
        env.add_override("DEVELOPER_DIR", Some("Set by worker for Xcode selection".to_string()));

        let json = env.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/executor_env@1""#));
        assert!(json.contains(r#""passed_keys""#));
        assert!(json.contains(r#""dropped_keys""#));
        assert!(json.contains(r#""overrides""#));
        assert!(json.contains("HOME"));
        assert!(json.contains("AWS_SECRET_ACCESS_KEY"));
        assert!(json.contains("DEVELOPER_DIR"));
    }

    #[test]
    fn test_executor_env_round_trip() {
        let mut env = ExecutorEnv::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
        );
        env.add_passed_key("HOME");
        env.add_override("DEVELOPER_DIR", None);

        let json = env.to_json().unwrap();
        let parsed = ExecutorEnv::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, env.run_id);
        assert_eq!(parsed.job_id, env.job_id);
        assert_eq!(parsed.passed_keys, env.passed_keys);
        assert_eq!(parsed.overrides.len(), 1);
    }

    #[test]
    fn test_env_filter_default() {
        let filter = EnvFilter::new();

        // Allowed keys
        assert!(filter.should_pass("HOME"));
        assert!(filter.should_pass("PATH"));
        assert!(filter.should_pass("DEVELOPER_DIR"));

        // Denied keys
        assert!(!filter.should_pass("AWS_SECRET_ACCESS_KEY"));
        assert!(!filter.should_pass("GITHUB_TOKEN"));
        assert!(!filter.should_pass("SSH_AUTH_SOCK"));

        // Unlisted keys (default: denied)
        assert!(!filter.should_pass("CUSTOM_VAR"));
    }

    #[test]
    fn test_env_filter_permissive() {
        let filter = EnvFilter::permissive();

        // Denied keys still blocked
        assert!(!filter.should_pass("AWS_SECRET_ACCESS_KEY"));
        assert!(!filter.should_pass("GITHUB_TOKEN"));

        // Unlisted keys passed
        assert!(filter.should_pass("CUSTOM_VAR"));
        assert!(filter.should_pass("MY_BUILD_FLAG"));
    }

    #[test]
    fn test_env_filter_custom() {
        let filter = EnvFilter::new()
            .allow("CUSTOM_ALLOWED")
            .deny("PATH"); // Override default allow

        assert!(filter.should_pass("CUSTOM_ALLOWED"));
        assert!(!filter.should_pass("PATH")); // Now denied
        assert!(filter.should_pass("HOME")); // Still allowed
    }

    #[test]
    fn test_env_filter_filter_env() {
        let filter = EnvFilter::new();

        let env = vec![
            ("HOME".to_string(), "/Users/test".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
            ("CUSTOM_VAR".to_string(), "value".to_string()),
        ];

        let (passed, passed_keys, dropped_keys) = filter.filter_env(env);

        // HOME and PATH passed
        assert_eq!(passed.len(), 2);
        assert!(passed_keys.contains(&"HOME".to_string()));
        assert!(passed_keys.contains(&"PATH".to_string()));

        // AWS_SECRET_ACCESS_KEY and CUSTOM_VAR dropped
        assert!(dropped_keys.contains(&"AWS_SECRET_ACCESS_KEY".to_string()));
        assert!(dropped_keys.contains(&"CUSTOM_VAR".to_string()));
    }

    #[test]
    fn test_env_filter_output_sorted() {
        let filter = EnvFilter::new();

        let env = vec![
            ("PATH".to_string(), "val".to_string()),
            ("HOME".to_string(), "val".to_string()),
            ("USER".to_string(), "val".to_string()),
        ];

        let (_, passed_keys, _) = filter.filter_env(env);

        // Keys should be sorted
        let mut sorted = passed_keys.clone();
        sorted.sort();
        assert_eq!(passed_keys, sorted);
    }
}
