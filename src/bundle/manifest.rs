//! Source manifest for bundled files
//!
//! Records all files included in a source bundle with their hashes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

/// Entry type in the manifest
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
}

/// A single entry in the source manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative path within the bundle
    pub path: String,

    /// Size in bytes (0 for directories and symlinks)
    pub size: u64,

    /// SHA-256 hash of file contents (empty for directories and symlinks)
    pub sha256: String,

    /// Type of entry
    #[serde(rename = "type")]
    pub entry_type: EntryType,

    /// Symlink target (only present for symlinks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

/// Source manifest (source_manifest.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceManifest {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the manifest was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// SHA-256 of the canonical tar bytes
    pub source_sha256: String,

    /// All entries in the bundle
    pub entries: Vec<ManifestEntry>,
}

impl SourceManifest {
    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
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

    /// Get total size of all files
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// Get count of each entry type
    pub fn entry_counts(&self) -> (usize, usize, usize) {
        let files = self.entries.iter().filter(|e| e.entry_type == EntryType::File).count();
        let dirs = self.entries.iter().filter(|e| e.entry_type == EntryType::Directory).count();
        let symlinks = self.entries.iter().filter(|e| e.entry_type == EntryType::Symlink).count();
        (files, dirs, symlinks)
    }

    /// Find an entry by path
    pub fn find_entry(&self, path: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_manifest() -> SourceManifest {
        SourceManifest {
            schema_version: 1,
            schema_id: "rch-xcode/source_manifest@1".to_string(),
            created_at: Utc::now(),
            run_id: "test-run-123".to_string(),
            source_sha256: "abc123".to_string(),
            entries: vec![
                ManifestEntry {
                    path: "file1.txt".to_string(),
                    size: 100,
                    sha256: "hash1".to_string(),
                    entry_type: EntryType::File,
                    symlink_target: None,
                },
                ManifestEntry {
                    path: "dir1".to_string(),
                    size: 0,
                    sha256: String::new(),
                    entry_type: EntryType::Directory,
                    symlink_target: None,
                },
                ManifestEntry {
                    path: "link1".to_string(),
                    size: 0,
                    sha256: String::new(),
                    entry_type: EntryType::Symlink,
                    symlink_target: Some("file1.txt".to_string()),
                },
            ],
        }
    }

    #[test]
    fn test_serialization() {
        let manifest = sample_manifest();
        let json = manifest.to_json().unwrap();

        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"run_id\": \"test-run-123\""));
        assert!(json.contains("\"file1.txt\""));
    }

    #[test]
    fn test_deserialization() {
        let manifest = sample_manifest();
        let json = manifest.to_json().unwrap();

        let parsed = SourceManifest::from_json(&json).unwrap();
        assert_eq!(parsed.run_id, manifest.run_id);
        assert_eq!(parsed.entries.len(), manifest.entries.len());
    }

    #[test]
    fn test_total_size() {
        let manifest = sample_manifest();
        assert_eq!(manifest.total_size(), 100);
    }

    #[test]
    fn test_entry_counts() {
        let manifest = sample_manifest();
        let (files, dirs, symlinks) = manifest.entry_counts();

        assert_eq!(files, 1);
        assert_eq!(dirs, 1);
        assert_eq!(symlinks, 1);
    }

    #[test]
    fn test_find_entry() {
        let manifest = sample_manifest();

        assert!(manifest.find_entry("file1.txt").is_some());
        assert!(manifest.find_entry("nonexistent").is_none());
    }

    #[test]
    fn test_entry_type_serialization() {
        let entry = ManifestEntry {
            path: "test".to_string(),
            size: 0,
            sha256: String::new(),
            entry_type: EntryType::Symlink,
            symlink_target: Some("target".to_string()),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"type\":\"symlink\""));
        assert!(json.contains("\"symlink_target\":\"target\""));
    }
}
