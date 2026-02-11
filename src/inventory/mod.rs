//! Worker Inventory Configuration
//!
//! Parses and validates the worker inventory file at `~/.config/rch/workers.toml`.
//! Each worker entry describes a remote macOS worker that can execute Xcode jobs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Worker inventory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInventory {
    /// Schema version for forward compatibility
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// List of workers
    #[serde(default, rename = "worker")]
    pub workers: Vec<WorkerEntry>,
}

fn default_schema_version() -> u32 {
    1
}

/// A single worker entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEntry {
    /// Unique identifier for this worker (must be unique across inventory)
    pub name: String,

    /// SSH hostname or IP address
    pub host: String,

    /// SSH port (default: 22)
    #[serde(default = "default_port")]
    pub port: u16,

    /// SSH user (default: "rch")
    #[serde(default = "default_user")]
    pub user: String,

    /// Tags for filtering (e.g., ["macos", "xcode"])
    #[serde(default)]
    pub tags: Vec<String>,

    /// Path to SSH private key
    #[serde(alias = "identity_file")]
    pub ssh_key_path: Option<String>,

    /// Priority for deterministic worker selection (lower = higher priority)
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// SSH known host fingerprint for verification
    pub known_host_fingerprint: Option<String>,

    /// Optional attestation public key fingerprint for signed attestation verification
    pub attestation_pubkey_fingerprint: Option<String>,
}

fn default_port() -> u16 {
    22
}

fn default_user() -> String {
    "rch".to_string()
}

fn default_priority() -> i32 {
    100
}

/// Errors that can occur when loading or validating worker inventory
#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("Failed to read inventory file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Duplicate worker name: '{0}'")]
    DuplicateName(String),

    #[error("Worker '{name}': missing required field '{field}'")]
    MissingField { name: String, field: String },

    #[error("Worker '{name}': invalid value for '{field}': {reason}")]
    InvalidValue {
        name: String,
        field: String,
        reason: String,
    },

    #[error("Inventory file not found: {0}")]
    NotFound(PathBuf),

    #[error("No workers configured")]
    Empty,
}

impl WorkerInventory {
    /// Load worker inventory from the default location (~/.config/rch/workers.toml)
    pub fn load_default() -> Result<Self, InventoryError> {
        let path = Self::default_path()?;
        Self::load(&path)
    }

    /// Get the default inventory file path
    pub fn default_path() -> Result<PathBuf, InventoryError> {
        let home = std::env::var("HOME")
            .map_err(|_| InventoryError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME environment variable not set",
            )))?;
        Ok(PathBuf::from(home).join(".config/rch/workers.toml"))
    }

    /// Load worker inventory from a specific path
    pub fn load(path: &Path) -> Result<Self, InventoryError> {
        if !path.exists() {
            return Err(InventoryError::NotFound(path.to_path_buf()));
        }

        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse worker inventory from TOML string
    pub fn parse(content: &str) -> Result<Self, InventoryError> {
        let inventory: WorkerInventory = toml::from_str(content)?;
        inventory.validate()?;
        Ok(inventory)
    }

    /// Validate the inventory
    fn validate(&self) -> Result<(), InventoryError> {
        // Check for duplicate names
        let mut seen_names = HashSet::new();
        for worker in &self.workers {
            if !seen_names.insert(&worker.name) {
                return Err(InventoryError::DuplicateName(worker.name.clone()));
            }
        }

        // Validate each worker
        for worker in &self.workers {
            worker.validate()?;
        }

        Ok(())
    }

    /// Get a worker by name
    pub fn get(&self, name: &str) -> Option<&WorkerEntry> {
        self.workers.iter().find(|w| w.name == name)
    }

    /// Filter workers by tags (all tags must match)
    pub fn filter_by_tags(&self, required_tags: &[&str]) -> Vec<&WorkerEntry> {
        self.workers
            .iter()
            .filter(|w| {
                required_tags.iter().all(|tag| w.tags.contains(&tag.to_string()))
            })
            .collect()
    }

    /// Get workers sorted by priority (lower priority value = earlier in list)
    pub fn sorted_by_priority(&self) -> Vec<&WorkerEntry> {
        let mut workers: Vec<_> = self.workers.iter().collect();
        workers.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.name.cmp(&b.name)));
        workers
    }

    /// Check if inventory is empty
    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// Get the number of workers
    pub fn len(&self) -> usize {
        self.workers.len()
    }
}

impl WorkerEntry {
    /// Validate the worker entry
    fn validate(&self) -> Result<(), InventoryError> {
        // Name must not be empty
        if self.name.is_empty() {
            return Err(InventoryError::MissingField {
                name: "(unnamed)".to_string(),
                field: "name".to_string(),
            });
        }

        // Host must not be empty
        if self.host.is_empty() {
            return Err(InventoryError::MissingField {
                name: self.name.clone(),
                field: "host".to_string(),
            });
        }

        // Port must be valid
        if self.port == 0 {
            return Err(InventoryError::InvalidValue {
                name: self.name.clone(),
                field: "port".to_string(),
                reason: "port cannot be 0".to_string(),
            });
        }

        // User must not be empty
        if self.user.is_empty() {
            return Err(InventoryError::InvalidValue {
                name: self.name.clone(),
                field: "user".to_string(),
                reason: "user cannot be empty".to_string(),
            });
        }

        // Name should be a valid identifier (alphanumeric, dash, underscore)
        if !self.name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(InventoryError::InvalidValue {
                name: self.name.clone(),
                field: "name".to_string(),
                reason: "name must contain only alphanumeric characters, dashes, and underscores".to_string(),
            });
        }

        Ok(())
    }

    /// Check if this worker has all required tags
    pub fn has_tags(&self, required: &[&str]) -> bool {
        required.iter().all(|tag| self.tags.contains(&tag.to_string()))
    }

    /// Get the expanded SSH key path (resolves ~ to home directory)
    pub fn expanded_ssh_key_path(&self) -> Option<PathBuf> {
        self.ssh_key_path.as_ref().map(|p| {
            if p.starts_with("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    return PathBuf::from(home).join(&p[2..]);
                }
            }
            PathBuf::from(p)
        })
    }
}

impl Default for WorkerInventory {
    fn default() -> Self {
        Self {
            schema_version: 1,
            workers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_inventory() {
        let content = r#"
            schema_version = 1

            [[worker]]
            name = "macmini-01"
            host = "macmini.local"
            user = "rch"
            port = 22
            tags = ["macos", "xcode"]
            ssh_key_path = "~/.ssh/rch_macmini"
            priority = 10
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        assert_eq!(inventory.schema_version, 1);
        assert_eq!(inventory.workers.len(), 1);

        let worker = &inventory.workers[0];
        assert_eq!(worker.name, "macmini-01");
        assert_eq!(worker.host, "macmini.local");
        assert_eq!(worker.user, "rch");
        assert_eq!(worker.port, 22);
        assert_eq!(worker.tags, vec!["macos", "xcode"]);
        assert_eq!(worker.priority, 10);
    }

    #[test]
    fn test_parse_multiple_workers() {
        let content = r#"
            [[worker]]
            name = "worker-1"
            host = "host1.local"
            priority = 10

            [[worker]]
            name = "worker-2"
            host = "host2.local"
            priority = 20
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        assert_eq!(inventory.workers.len(), 2);
        assert_eq!(inventory.workers[0].name, "worker-1");
        assert_eq!(inventory.workers[1].name, "worker-2");
    }

    #[test]
    fn test_default_values() {
        let content = r#"
            [[worker]]
            name = "minimal"
            host = "host.local"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let worker = &inventory.workers[0];
        assert_eq!(worker.port, 22);
        assert_eq!(worker.user, "rch");
        assert_eq!(worker.priority, 100);
        assert!(worker.tags.is_empty());
    }

    #[test]
    fn test_duplicate_name_rejected() {
        let content = r#"
            [[worker]]
            name = "same-name"
            host = "host1.local"

            [[worker]]
            name = "same-name"
            host = "host2.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(matches!(result, Err(InventoryError::DuplicateName(_))));
    }

    #[test]
    fn test_missing_name_rejected() {
        let content = r#"
            [[worker]]
            host = "host.local"
        "#;

        // TOML will fail to parse because name is required
        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_host_rejected() {
        let content = r#"
            [[worker]]
            name = "worker"
        "#;

        // TOML will fail to parse because host is required
        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_name_rejected() {
        let content = r#"
            [[worker]]
            name = ""
            host = "host.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(matches!(result, Err(InventoryError::MissingField { .. })));
    }

    #[test]
    fn test_empty_host_rejected() {
        let content = r#"
            [[worker]]
            name = "worker"
            host = ""
        "#;

        let result = WorkerInventory::parse(content);
        assert!(matches!(result, Err(InventoryError::MissingField { .. })));
    }

    #[test]
    fn test_invalid_name_rejected() {
        let content = r#"
            [[worker]]
            name = "worker name with spaces"
            host = "host.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(matches!(result, Err(InventoryError::InvalidValue { .. })));
    }

    #[test]
    fn test_filter_by_tags() {
        let content = r#"
            [[worker]]
            name = "mac-1"
            host = "host1.local"
            tags = ["macos", "xcode"]

            [[worker]]
            name = "mac-2"
            host = "host2.local"
            tags = ["macos", "xcode", "m1"]

            [[worker]]
            name = "linux-1"
            host = "host3.local"
            tags = ["linux"]
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();

        let macos = inventory.filter_by_tags(&["macos"]);
        assert_eq!(macos.len(), 2);

        let xcode = inventory.filter_by_tags(&["macos", "xcode"]);
        assert_eq!(xcode.len(), 2);

        let m1 = inventory.filter_by_tags(&["m1"]);
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0].name, "mac-2");

        let linux = inventory.filter_by_tags(&["linux"]);
        assert_eq!(linux.len(), 1);
        assert_eq!(linux[0].name, "linux-1");
    }

    #[test]
    fn test_sorted_by_priority() {
        let content = r#"
            [[worker]]
            name = "worker-b"
            host = "host1.local"
            priority = 20

            [[worker]]
            name = "worker-a"
            host = "host2.local"
            priority = 10

            [[worker]]
            name = "worker-c"
            host = "host3.local"
            priority = 10
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let sorted = inventory.sorted_by_priority();

        // worker-a and worker-c have same priority, sorted by name
        assert_eq!(sorted[0].name, "worker-a");
        assert_eq!(sorted[1].name, "worker-c");
        assert_eq!(sorted[2].name, "worker-b");
    }

    #[test]
    fn test_get_worker_by_name() {
        let content = r#"
            [[worker]]
            name = "worker-1"
            host = "host1.local"

            [[worker]]
            name = "worker-2"
            host = "host2.local"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();

        assert!(inventory.get("worker-1").is_some());
        assert!(inventory.get("worker-2").is_some());
        assert!(inventory.get("worker-3").is_none());
    }

    #[test]
    fn test_has_tags() {
        let content = r#"
            [[worker]]
            name = "worker"
            host = "host.local"
            tags = ["macos", "xcode", "m1"]
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let worker = &inventory.workers[0];

        assert!(worker.has_tags(&["macos"]));
        assert!(worker.has_tags(&["macos", "xcode"]));
        assert!(worker.has_tags(&["macos", "xcode", "m1"]));
        assert!(!worker.has_tags(&["linux"]));
        assert!(!worker.has_tags(&["macos", "linux"]));
    }

    #[test]
    fn test_identity_file_alias() {
        let content = r#"
            [[worker]]
            name = "worker"
            host = "host.local"
            identity_file = "/path/to/key"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let worker = &inventory.workers[0];
        assert_eq!(worker.ssh_key_path, Some("/path/to/key".to_string()));
    }

    #[test]
    fn test_optional_fields() {
        let content = r#"
            [[worker]]
            name = "worker"
            host = "host.local"
            known_host_fingerprint = "SHA256:abc123"
            attestation_pubkey_fingerprint = "SHA256:def456"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let worker = &inventory.workers[0];
        assert_eq!(worker.known_host_fingerprint, Some("SHA256:abc123".to_string()));
        assert_eq!(worker.attestation_pubkey_fingerprint, Some("SHA256:def456".to_string()));
    }

    #[test]
    fn test_expanded_ssh_key_path() {
        let worker = WorkerEntry {
            name: "test".to_string(),
            host: "host.local".to_string(),
            port: 22,
            user: "rch".to_string(),
            tags: vec![],
            ssh_key_path: Some("~/.ssh/test_key".to_string()),
            priority: 100,
            known_host_fingerprint: None,
            attestation_pubkey_fingerprint: None,
        };

        let expanded = worker.expanded_ssh_key_path();
        assert!(expanded.is_some());
        // The path should not start with ~
        let path = expanded.unwrap();
        assert!(!path.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn test_empty_inventory() {
        let content = r#"
            schema_version = 1
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        assert!(inventory.is_empty());
        assert_eq!(inventory.len(), 0);
    }
}
