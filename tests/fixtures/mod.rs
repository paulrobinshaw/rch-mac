//! Test fixtures for golden-file assertions (rch-mac-b7s.7)
//!
//! This module provides test fixtures for:
//! - Classifier corpus (accepted/rejected xcodebuild invocations)
//! - Source bundle reproducibility (deterministic hashing)
//! - Xcode project fixture (minimal build + test targets)

use std::path::{Path, PathBuf};

/// Path to the classifier corpus fixture
pub fn classifier_corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/classifier_corpus/corpus.json")
}

/// Path to the source bundle fixture
pub fn source_bundle_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/source_bundle")
}

/// Path to the golden expectations file
pub fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/source_bundle/golden.json")
}

/// Classifier test case from corpus.json
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifierTestCase {
    pub id: String,
    pub description: String,
    pub argv: Vec<String>,
    pub expected: ClassifierExpectation,
}

/// Expected classifier result
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifierExpectation {
    pub accepted: bool,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub configuration: Option<String>,
    #[serde(default)]
    pub rejection_reason: Option<String>,
}

/// Classifier corpus configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifierCorpusConfig {
    pub workspace: Option<String>,
    pub project: Option<String>,
    pub scheme: String,
    pub destination: Option<String>,
    pub allowed_configurations: Vec<String>,
}

/// Full classifier corpus
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClassifierCorpus {
    pub schema_version: u32,
    pub description: String,
    pub test_cases: Vec<ClassifierTestCase>,
    pub config: ClassifierCorpusConfig,
}

impl ClassifierCorpus {
    /// Load corpus from the fixture file
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = classifier_corpus_path();
        let content = std::fs::read_to_string(&path)?;
        let corpus: ClassifierCorpus = serde_json::from_str(&content)?;
        Ok(corpus)
    }
}

/// Golden file expectations
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoldenExpectations {
    pub schema_version: u32,
    pub description: String,
    pub fixture_path: String,
    pub expected_source_sha256: String,
    pub notes: Vec<String>,
    pub job_key_inputs: serde_json::Value,
    pub expected_artifacts: Vec<String>,
    pub expected_exit_codes: serde_json::Value,
    pub schema_versions: serde_json::Value,
}

impl GoldenExpectations {
    /// Load golden expectations from file
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = golden_path();
        let content = std::fs::read_to_string(&path)?;
        let golden: GoldenExpectations = serde_json::from_str(&content)?;
        Ok(golden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifier_corpus_loads() {
        let corpus = ClassifierCorpus::load().expect("Failed to load corpus");
        assert!(corpus.test_cases.len() > 10, "Expected at least 10 test cases");
        assert_eq!(corpus.config.scheme, "MyApp");
    }

    #[test]
    fn test_golden_expectations_loads() {
        let golden = GoldenExpectations::load().expect("Failed to load golden");
        assert_eq!(golden.schema_version, 1);
        assert!(golden.expected_artifacts.contains(&"manifest.json".to_string()));
    }

    #[test]
    fn test_source_bundle_exists() {
        let path = source_bundle_path();
        assert!(path.exists(), "Source bundle fixture not found at {:?}", path);
        assert!(path.join("MyApp.xcworkspace").exists());
        assert!(path.join("MyApp.xcodeproj").exists());
        assert!(path.join("MyApp/AppDelegate.swift").exists());
        assert!(path.join("MyAppTests/MyAppTests.swift").exists());
    }
}
