//! Effective configuration with full provenance
//!
//! The effective_config captures the merged configuration plus
//! information about where each value came from.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;

use super::defaults::BuiltinDefaults;
use super::merge::merge_layers;

/// Schema version for effective_config
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/effective_config@1";

/// Origin of a configuration source
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigOrigin {
    Builtin,
    Host,
    Repo,
    Cli,
}

/// A contributing config source with provenance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSource {
    /// Origin of this source
    pub origin: ConfigOrigin,

    /// File path (None for builtin/cli)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// SHA-256 digest of raw file bytes (None for builtin/cli)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Effective configuration with full provenance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveConfig {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When this config was computed
    pub created_at: DateTime<Utc>,

    /// Run ID (set later)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,

    /// Job ID (set later)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,

    /// Job key (set later)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_key: Option<String>,

    /// The merged configuration object
    pub config: Value,

    /// Contributing sources in precedence order
    pub sources: Vec<ConfigSource>,

    /// Redacted key paths
    pub redactions: Vec<String>,
}

/// Keys that contain secrets and should be redacted
const SECRET_KEYS: &[&str] = &[
    "password",
    "token",
    "secret",
    "private_key",
    "api_key",
    "credential",
];

impl EffectiveConfig {
    /// Build effective config from layers
    pub fn build(
        host_config_path: Option<&Path>,
        repo_config_path: Option<&Path>,
        cli_overrides: Option<Value>,
    ) -> Result<Self, ConfigError> {
        let mut layers = Vec::new();
        let mut sources = Vec::new();

        // Layer 1: Built-in defaults
        let defaults = BuiltinDefaults::default();
        layers.push(defaults.to_value());
        sources.push(ConfigSource {
            origin: ConfigOrigin::Builtin,
            path: None,
            digest: None,
        });

        // Layer 2: Host config
        if let Some(path) = host_config_path {
            if path.exists() {
                let (value, digest) = Self::load_toml_file(path)?;
                layers.push(value);
                sources.push(ConfigSource {
                    origin: ConfigOrigin::Host,
                    path: Some(path.to_string_lossy().to_string()),
                    digest: Some(digest),
                });
            }
        }

        // Layer 3: Repo config
        if let Some(path) = repo_config_path {
            if path.exists() {
                let (value, digest) = Self::load_toml_file(path)?;
                layers.push(value);
                sources.push(ConfigSource {
                    origin: ConfigOrigin::Repo,
                    path: Some(path.to_string_lossy().to_string()),
                    digest: Some(digest),
                });
            }
        }

        // Layer 4: CLI overrides
        if let Some(cli) = cli_overrides {
            layers.push(cli);
            sources.push(ConfigSource {
                origin: ConfigOrigin::Cli,
                path: None,
                digest: None,
            });
        }

        // Merge all layers
        let mut merged = merge_layers(layers);

        // Redact secrets
        let redactions = Self::redact_secrets(&mut merged);

        // Validate
        Self::validate_config(&merged)?;

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: None,
            job_id: None,
            job_key: None,
            config: merged,
            sources,
            redactions,
        })
    }

    /// Load and parse a TOML file, returning the value and digest
    fn load_toml_file(path: &Path) -> Result<(Value, String), ConfigError> {
        let bytes = fs::read(path).map_err(|e| ConfigError::IoError(e.to_string()))?;

        // Compute digest
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hex::encode(hasher.finalize());

        // Parse TOML
        let contents = String::from_utf8(bytes)
            .map_err(|e| ConfigError::ParseError(format!("Invalid UTF-8: {}", e)))?;

        let toml_value: toml::Value = toml::from_str(&contents)
            .map_err(|e| ConfigError::ParseError(format!("TOML parse error: {}", e)))?;

        // Convert to JSON Value
        let json_value = Self::toml_to_json(toml_value);

        Ok((json_value, digest))
    }

    /// Convert TOML Value to JSON Value
    fn toml_to_json(toml: toml::Value) -> Value {
        match toml {
            toml::Value::String(s) => Value::String(s),
            toml::Value::Integer(i) => Value::Number(i.into()),
            toml::Value::Float(f) => {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
            toml::Value::Boolean(b) => Value::Bool(b),
            toml::Value::Datetime(dt) => Value::String(dt.to_string()),
            toml::Value::Array(arr) => {
                Value::Array(arr.into_iter().map(Self::toml_to_json).collect())
            }
            toml::Value::Table(table) => {
                let map: serde_json::Map<String, Value> = table
                    .into_iter()
                    .map(|(k, v)| (k, Self::toml_to_json(v)))
                    .collect();
                Value::Object(map)
            }
        }
    }

    /// Redact secrets in the config, returning list of redacted paths
    fn redact_secrets(value: &mut Value) -> Vec<String> {
        let mut redactions = Vec::new();
        Self::redact_recursive(value, String::new(), &mut redactions);
        redactions
    }

    fn redact_recursive(value: &mut Value, path: String, redactions: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    let key_lower = key.to_lowercase();
                    let current_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };

                    // Check if this key contains secret-like words
                    let is_secret = SECRET_KEYS.iter().any(|s| key_lower.contains(s));

                    if is_secret && !val.is_object() && !val.is_array() {
                        *val = Value::String("[REDACTED]".to_string());
                        redactions.push(current_path);
                    } else {
                        Self::redact_recursive(val, current_path, redactions);
                    }
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter_mut().enumerate() {
                    let current_path = format!("{}[{}]", path, i);
                    Self::redact_recursive(val, current_path, redactions);
                }
            }
            _ => {}
        }
    }

    /// Validate configuration values
    fn validate_config(config: &Value) -> Result<(), ConfigError> {
        // overall_seconds must be in (0, 86400]
        if let Some(overall) = config.get("overall_seconds").and_then(|v| v.as_u64()) {
            if overall == 0 || overall > 86400 {
                return Err(ConfigError::ValidationError(
                    "overall_seconds must be in (0, 86400]".to_string(),
                ));
            }
        }

        // idle_log_seconds must be in (0, overall_seconds]
        if let Some(idle) = config.get("idle_log_seconds").and_then(|v| v.as_u64()) {
            let overall = config
                .get("overall_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(1800);

            if idle == 0 || idle > overall {
                return Err(ConfigError::ValidationError(format!(
                    "idle_log_seconds must be in (0, {}]",
                    overall
                )));
            }
        }

        // connect_timeout_seconds must be in (0, 300]
        if let Some(connect) = config.get("connect_timeout_seconds").and_then(|v| v.as_u64()) {
            if connect == 0 || connect > 300 {
                return Err(ConfigError::ValidationError(
                    "connect_timeout_seconds must be in (0, 300]".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Set run context
    pub fn with_run_id(mut self, run_id: String) -> Self {
        self.run_id = Some(run_id);
        self
    }

    /// Set job context
    pub fn with_job_context(mut self, job_id: String, job_key: String) -> Self {
        self.job_id = Some(job_id);
        self.job_key = Some(job_key);
        self
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Write to file
    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("JSON serialization failed: {}", e),
            )
        })?;
        fs::write(path, json)
    }

    /// Get a config value by path (dot-separated)
    pub fn get(&self, path: &str) -> Option<&Value> {
        let mut current = &self.config;
        for part in path.split('.') {
            current = current.get(part)?;
        }
        Some(current)
    }

    /// Get a config value as u64
    pub fn get_u64(&self, path: &str) -> Option<u64> {
        self.get(path).and_then(|v| v.as_u64())
    }

    /// Get a config value as string
    pub fn get_str(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(|v| v.as_str())
    }

    /// Get a config value as bool
    pub fn get_bool(&self, path: &str) -> Option<bool> {
        self.get(path).and_then(|v| v.as_bool())
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_build_with_defaults_only() {
        let config = EffectiveConfig::build(None, None, None).unwrap();

        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert_eq!(config.get_u64("overall_seconds"), Some(1800));
        assert_eq!(config.get_str("bundle.mode"), Some("worktree"));
    }

    #[test]
    fn test_build_with_cli_override() {
        let cli = serde_json::json!({
            "overall_seconds": 600
        });

        let config = EffectiveConfig::build(None, None, Some(cli)).unwrap();

        assert_eq!(config.get_u64("overall_seconds"), Some(600));
    }

    #[test]
    fn test_validation_overall_seconds() {
        let cli = serde_json::json!({
            "overall_seconds": 0
        });

        let result = EffectiveConfig::build(None, None, Some(cli));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("overall_seconds"));
    }

    #[test]
    fn test_validation_connect_timeout() {
        let cli = serde_json::json!({
            "connect_timeout_seconds": 500
        });

        let result = EffectiveConfig::build(None, None, Some(cli));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("connect_timeout_seconds"));
    }

    #[test]
    fn test_secret_redaction() {
        let cli = serde_json::json!({
            "api_key": "secret123",
            "password": "hunter2",
            "normal_value": "visible"
        });

        let config = EffectiveConfig::build(None, None, Some(cli)).unwrap();

        assert_eq!(config.get_str("api_key"), Some("[REDACTED]"));
        assert_eq!(config.get_str("password"), Some("[REDACTED]"));
        assert_eq!(config.get_str("normal_value"), Some("visible"));
        assert!(config.redactions.contains(&"api_key".to_string()));
        assert!(config.redactions.contains(&"password".to_string()));
    }

    #[test]
    fn test_nested_secret_redaction() {
        let cli = serde_json::json!({
            "auth": {
                "token": "secret-token",
                "username": "user"
            }
        });

        let config = EffectiveConfig::build(None, None, Some(cli)).unwrap();

        assert_eq!(config.get_str("auth.token"), Some("[REDACTED]"));
        assert_eq!(config.get_str("auth.username"), Some("user"));
    }

    #[test]
    fn test_load_toml_file() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "overall_seconds = 900").unwrap();
        writeln!(temp, "[cache]").unwrap();
        writeln!(temp, "derived_data = \"on\"").unwrap();

        let config =
            EffectiveConfig::build(Some(temp.path()), None, None).unwrap();

        assert_eq!(config.get_u64("overall_seconds"), Some(900));
        assert_eq!(config.get_str("cache.derived_data"), Some("on"));
    }

    #[test]
    fn test_sources_tracked() {
        let config = EffectiveConfig::build(None, None, None).unwrap();

        assert_eq!(config.sources.len(), 1);
        assert_eq!(config.sources[0].origin, ConfigOrigin::Builtin);
    }

    #[test]
    fn test_with_context() {
        let config = EffectiveConfig::build(None, None, None)
            .unwrap()
            .with_run_id("run-123".to_string())
            .with_job_context("job-456".to_string(), "key-789".to_string());

        assert_eq!(config.run_id, Some("run-123".to_string()));
        assert_eq!(config.job_id, Some("job-456".to_string()));
        assert_eq!(config.job_key, Some("key-789".to_string()));
    }
}
