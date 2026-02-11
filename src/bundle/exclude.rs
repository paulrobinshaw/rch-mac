//! Exclusion rules for source bundling
//!
//! Handles default exclusions and .rchignore files.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::fs;
use std::path::Path;

/// Default patterns to exclude from bundles
const DEFAULT_EXCLUDES: &[&str] = &[
    ".git/**",
    ".git",
    ".DS_Store",
    "**/.DS_Store",
    "DerivedData/**",
    "DerivedData",
    ".build/**",
    ".build",
    "**/*.xcresult",
    "**/*.xcresult/**",
    "**/.swiftpm",
    "**/.swiftpm/**",
    ".rch/artifacts/**",
    ".rch/artifacts",
    "*.xcuserdata",
    "**/*.xcuserdata",
    "**/*.xcuserdata/**",
];

/// Errors for exclusion rules
#[derive(Debug, thiserror::Error)]
pub enum ExcludeError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Glob pattern error: {0}")]
    GlobError(#[from] globset::Error),
}

/// Exclusion rules for filtering files
#[derive(Debug)]
pub struct ExcludeRules {
    glob_set: GlobSet,
}

impl Default for ExcludeRules {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

impl ExcludeRules {
    /// Create new exclusion rules with defaults
    pub fn new() -> Result<Self, ExcludeError> {
        let mut builder = GlobSetBuilder::new();

        for pattern in DEFAULT_EXCLUDES {
            builder.add(Glob::new(pattern)?);
        }

        Ok(Self {
            glob_set: builder.build()?,
        })
    }

    /// Add patterns from an ignore file
    pub fn with_ignore_file(mut self, path: &Path) -> Result<Self, ExcludeError> {
        let contents = fs::read_to_string(path)?;
        let patterns: Vec<&str> = contents
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();

        self = self.with_patterns(&patterns)?;
        Ok(self)
    }

    /// Add additional patterns
    pub fn with_patterns(self, patterns: &[&str]) -> Result<Self, ExcludeError> {
        let mut builder = GlobSetBuilder::new();

        // Re-add default patterns
        for pattern in DEFAULT_EXCLUDES {
            builder.add(Glob::new(pattern)?);
        }

        // Add new patterns
        for pattern in patterns {
            if !pattern.is_empty() {
                builder.add(Glob::new(pattern)?);
            }
        }

        Ok(Self {
            glob_set: builder.build()?,
        })
    }

    /// Check if a path should be excluded
    pub fn is_excluded(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        self.glob_set.is_match(path_str.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_excludes_git() {
        let rules = ExcludeRules::new().unwrap();

        assert!(rules.is_excluded(Path::new(".git")));
        assert!(rules.is_excluded(Path::new(".git/config")));
        assert!(rules.is_excluded(Path::new(".git/objects/pack")));
    }

    #[test]
    fn test_default_excludes_ds_store() {
        let rules = ExcludeRules::new().unwrap();

        assert!(rules.is_excluded(Path::new(".DS_Store")));
        assert!(rules.is_excluded(Path::new("subdir/.DS_Store")));
    }

    #[test]
    fn test_default_excludes_derived_data() {
        let rules = ExcludeRules::new().unwrap();

        assert!(rules.is_excluded(Path::new("DerivedData")));
        assert!(rules.is_excluded(Path::new("DerivedData/Build")));
    }

    #[test]
    fn test_default_excludes_xcresult() {
        let rules = ExcludeRules::new().unwrap();

        assert!(rules.is_excluded(Path::new("test.xcresult")));
        assert!(rules.is_excluded(Path::new("subdir/test.xcresult")));
    }

    #[test]
    fn test_default_excludes_swiftpm() {
        let rules = ExcludeRules::new().unwrap();

        assert!(rules.is_excluded(Path::new(".swiftpm")));
        assert!(rules.is_excluded(Path::new("subdir/.swiftpm")));
    }

    #[test]
    fn test_normal_files_not_excluded() {
        let rules = ExcludeRules::new().unwrap();

        assert!(!rules.is_excluded(Path::new("file.swift")));
        assert!(!rules.is_excluded(Path::new("src/main.swift")));
        assert!(!rules.is_excluded(Path::new(".rch/xcode.toml")));
    }

    #[test]
    fn test_custom_patterns() {
        let rules = ExcludeRules::new()
            .unwrap()
            .with_patterns(&["*.log", "temp/**"])
            .unwrap();

        assert!(rules.is_excluded(Path::new("debug.log")));
        assert!(rules.is_excluded(Path::new("temp/file.txt")));
        // Default excludes still work
        assert!(rules.is_excluded(Path::new(".git")));
    }

    #[test]
    fn test_ignore_file_parsing() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Comment").unwrap();
        writeln!(file, "*.tmp").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "  build/  ").unwrap();

        let rules = ExcludeRules::new()
            .unwrap()
            .with_ignore_file(file.path())
            .unwrap();

        assert!(rules.is_excluded(Path::new("test.tmp")));
        assert!(rules.is_excluded(Path::new("build/")));
    }
}
