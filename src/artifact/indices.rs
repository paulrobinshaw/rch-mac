//! Artifact indices (run_index.json, job_index.json) per PLAN.md normative spec
//!
//! Provides stable discovery paths for all artifacts. job_index.json serves as
//! the commit marker — its existence proves the artifact set is complete and consistent.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use crate::summary::ArtifactProfile;

/// Schema version for run_index.json
pub const RUN_INDEX_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for run_index.json
pub const RUN_INDEX_SCHEMA_ID: &str = "rch-xcode/run_index@1";

/// Schema version for job_index.json
pub const JOB_INDEX_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for job_index.json
pub const JOB_INDEX_SCHEMA_ID: &str = "rch-xcode/job_index@1";

/// Step pointer in run_index.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepPointer {
    /// Step index (0-based)
    pub index: usize,

    /// Action type (build, test)
    pub action: String,

    /// Job identifier
    pub job_id: String,

    /// Relative path to job_index.json
    pub job_index_path: String,
}

/// Run index (run_index.json)
///
/// Provides stable discovery for all run-scoped artifacts and
/// an ordered list of steps with pointers to job indices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunIndex {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the index was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Relative path to run_plan.json
    pub run_plan: String,

    /// Relative path to run_state.json
    pub run_state: String,

    /// Relative path to run_summary.json
    pub run_summary: String,

    /// Relative path to source_manifest.json
    pub source_manifest: String,

    /// Relative path to worker_selection.json
    pub worker_selection: String,

    /// Relative path to capabilities.json snapshot
    pub capabilities: String,

    /// Ordered list of steps with pointers to job indices
    pub steps: Vec<StepPointer>,
}

impl RunIndex {
    /// Create a new run index
    pub fn new(run_id: String) -> Self {
        Self {
            schema_version: RUN_INDEX_SCHEMA_VERSION,
            schema_id: RUN_INDEX_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            run_plan: "run_plan.json".to_string(),
            run_state: "run_state.json".to_string(),
            run_summary: "run_summary.json".to_string(),
            source_manifest: "source_manifest.json".to_string(),
            worker_selection: "worker_selection.json".to_string(),
            capabilities: "capabilities.json".to_string(),
            steps: Vec::new(),
        }
    }

    /// Add a step pointer
    pub fn add_step(&mut self, index: usize, action: &str, job_id: &str) {
        let job_index_path = format!("steps/{}/{}/job_index.json", action, job_id);
        self.steps.push(StepPointer {
            index,
            action: action.to_string(),
            job_id: job_id.to_string(),
            job_index_path,
        });
    }

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
}

/// Artifact pointer in job_index.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPointer {
    /// Artifact name (without .json extension for clarity)
    pub name: String,

    /// Relative path to artifact
    pub path: String,

    /// Whether artifact is present
    pub present: bool,
}

impl ArtifactPointer {
    /// Create a pointer for a present artifact
    pub fn present(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            present: true,
        }
    }

    /// Create a pointer for a missing optional artifact
    pub fn missing(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            present: false,
        }
    }
}

/// Job index (job_index.json)
///
/// Provides stable discovery for all job-scoped artifacts.
/// Its existence serves as the COMMIT MARKER for artifact set completeness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobIndex {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When the index was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Job identifier
    pub job_id: String,

    /// Job key (deterministic hash)
    pub job_key: String,

    /// Action type
    pub action: String,

    /// Artifact profile produced
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_profile: Option<ArtifactProfile>,

    /// Required artifacts (always present for successful jobs)
    pub required: RequiredJobArtifacts,

    /// Optional artifacts (presence varies)
    pub optional: Vec<ArtifactPointer>,
}

/// Required artifacts for a job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredJobArtifacts {
    /// job.json (job spec)
    pub job: String,

    /// job_state.json
    pub job_state: String,

    /// summary.json
    pub summary: String,

    /// manifest.json
    pub manifest: String,

    /// attestation.json
    pub attestation: String,

    /// toolchain.json
    pub toolchain: String,

    /// destination.json
    pub destination: String,

    /// effective_config.json
    pub effective_config: String,

    /// invocation.json
    pub invocation: String,

    /// job_key_inputs.json
    pub job_key_inputs: String,

    /// build.log
    pub build_log: String,
}

impl Default for RequiredJobArtifacts {
    fn default() -> Self {
        Self {
            job: "job.json".to_string(),
            job_state: "job_state.json".to_string(),
            summary: "summary.json".to_string(),
            manifest: "manifest.json".to_string(),
            attestation: "attestation.json".to_string(),
            toolchain: "toolchain.json".to_string(),
            destination: "destination.json".to_string(),
            effective_config: "effective_config.json".to_string(),
            invocation: "invocation.json".to_string(),
            job_key_inputs: "job_key_inputs.json".to_string(),
            build_log: "build.log".to_string(),
        }
    }
}

impl JobIndex {
    /// Create a new job index
    pub fn new(run_id: String, job_id: String, job_key: String, action: String) -> Self {
        Self {
            schema_version: JOB_INDEX_SCHEMA_VERSION,
            schema_id: JOB_INDEX_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id,
            job_id,
            job_key,
            action,
            artifact_profile: None,
            required: RequiredJobArtifacts::default(),
            optional: Vec::new(),
        }
    }

    /// Set the artifact profile
    pub fn with_artifact_profile(mut self, profile: ArtifactProfile) -> Self {
        self.artifact_profile = Some(profile);
        self
    }

    /// Add optional artifacts based on what's present in the job directory
    pub fn scan_optional_artifacts(&mut self, job_dir: &Path) {
        let optional_artifacts = [
            ("metrics", "metrics.json"),
            ("executor_env", "executor_env.json"),
            ("classifier_policy", "classifier_policy.json"),
            ("events", "events.jsonl"),
            ("test_summary", "test_summary.json"),
            ("build_summary", "build_summary.json"),
            ("junit", "junit.xml"),
            ("xcresult", "result.xcresult"),
        ];

        for (name, filename) in optional_artifacts {
            let path = job_dir.join(filename);
            if path.exists() {
                self.optional
                    .push(ArtifactPointer::present(name, filename));
            } else {
                self.optional
                    .push(ArtifactPointer::missing(name, filename));
            }
        }
    }

    /// Add an optional artifact pointer
    pub fn add_optional(&mut self, name: &str, path: &str, present: bool) {
        if present {
            self.optional.push(ArtifactPointer::present(name, path));
        } else {
            self.optional.push(ArtifactPointer::missing(name, path));
        }
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write to file
    ///
    /// IMPORTANT: Writing job_index.json is the COMMIT MARKER for artifact set completeness.
    /// Only call this after all other job artifacts have been written successfully.
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

    /// Check if this job's artifacts are complete
    ///
    /// Returns true if job_index.json exists at the expected location.
    pub fn is_complete(job_dir: &Path) -> bool {
        job_dir.join("job_index.json").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_run_index_new() {
        let index = RunIndex::new("run-123".to_string());

        assert_eq!(index.schema_version, RUN_INDEX_SCHEMA_VERSION);
        assert_eq!(index.schema_id, RUN_INDEX_SCHEMA_ID);
        assert_eq!(index.run_id, "run-123");
        assert!(index.steps.is_empty());
    }

    #[test]
    fn test_run_index_add_steps() {
        let mut index = RunIndex::new("run-123".to_string());
        index.add_step(0, "build", "job-001");
        index.add_step(1, "test", "job-002");

        assert_eq!(index.steps.len(), 2);
        assert_eq!(index.steps[0].action, "build");
        assert_eq!(index.steps[0].job_index_path, "steps/build/job-001/job_index.json");
        assert_eq!(index.steps[1].action, "test");
        assert_eq!(index.steps[1].job_index_path, "steps/test/job-002/job_index.json");
    }

    #[test]
    fn test_run_index_serialization() {
        let mut index = RunIndex::new("run-123".to_string());
        index.add_step(0, "build", "job-001");

        let json = index.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/run_index@1""#));
        assert!(json.contains(r#""run_plan": "run_plan.json""#));
        assert!(json.contains(r#""steps""#));
    }

    #[test]
    fn test_run_index_round_trip() {
        let mut index = RunIndex::new("run-123".to_string());
        index.add_step(0, "build", "job-001");
        index.add_step(1, "test", "job-002");

        let json = index.to_json().unwrap();
        let parsed = RunIndex::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, index.run_id);
        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].job_id, "job-001");
    }

    #[test]
    fn test_job_index_new() {
        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        );

        assert_eq!(index.schema_version, JOB_INDEX_SCHEMA_VERSION);
        assert_eq!(index.schema_id, JOB_INDEX_SCHEMA_ID);
        assert_eq!(index.run_id, "run-123");
        assert_eq!(index.job_id, "job-456");
        assert_eq!(index.job_key, "key-789");
        assert_eq!(index.action, "build");
        assert!(index.optional.is_empty());
    }

    #[test]
    fn test_job_index_with_artifact_profile() {
        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        )
        .with_artifact_profile(ArtifactProfile::Rich);

        assert_eq!(index.artifact_profile, Some(ArtifactProfile::Rich));
    }

    #[test]
    fn test_job_index_scan_optional() {
        let dir = TempDir::new().unwrap();

        // Create some optional artifacts
        fs::write(dir.path().join("metrics.json"), "{}").unwrap();
        fs::write(dir.path().join("events.jsonl"), "").unwrap();

        let mut index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        );
        index.scan_optional_artifacts(dir.path());

        // Check that present artifacts are marked correctly
        let metrics = index.optional.iter().find(|a| a.name == "metrics").unwrap();
        assert!(metrics.present);

        let events = index.optional.iter().find(|a| a.name == "events").unwrap();
        assert!(events.present);

        // Check that missing artifacts are marked correctly
        let junit = index.optional.iter().find(|a| a.name == "junit").unwrap();
        assert!(!junit.present);
    }

    #[test]
    fn test_job_index_serialization() {
        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "test".to_string(),
        )
        .with_artifact_profile(ArtifactProfile::Minimal);

        let json = index.to_json().unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/job_index@1""#));
        assert!(json.contains(r#""required""#));
        assert!(json.contains(r#""manifest": "manifest.json""#));
        assert!(json.contains(r#""artifact_profile": "minimal""#));
    }

    #[test]
    fn test_job_index_round_trip() {
        let mut index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        );
        index.add_optional("metrics", "metrics.json", true);
        index.add_optional("junit", "junit.xml", false);

        let json = index.to_json().unwrap();
        let parsed = JobIndex::from_json(&json).unwrap();

        assert_eq!(parsed.run_id, index.run_id);
        assert_eq!(parsed.job_id, index.job_id);
        assert_eq!(parsed.optional.len(), 2);
    }

    #[test]
    fn test_job_index_file_io() {
        let dir = TempDir::new().unwrap();

        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "test".to_string(),
        );

        let path = dir.path().join("job_index.json");
        index.write_to_file(&path).unwrap();

        let loaded = JobIndex::from_file(&path).unwrap();
        assert_eq!(loaded.job_id, index.job_id);
    }

    #[test]
    fn test_job_index_is_complete() {
        let dir = TempDir::new().unwrap();

        // Not complete initially
        assert!(!JobIndex::is_complete(dir.path()));

        // Create job_index.json
        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        );
        index.write_to_file(&dir.path().join("job_index.json")).unwrap();

        // Now complete
        assert!(JobIndex::is_complete(dir.path()));
    }

    #[test]
    fn test_required_artifacts_default() {
        let required = RequiredJobArtifacts::default();

        assert_eq!(required.job, "job.json");
        assert_eq!(required.summary, "summary.json");
        assert_eq!(required.manifest, "manifest.json");
        assert_eq!(required.attestation, "attestation.json");
        assert_eq!(required.build_log, "build.log");
    }

    #[test]
    fn test_artifact_pointer() {
        let present = ArtifactPointer::present("metrics", "metrics.json");
        assert!(present.present);
        assert_eq!(present.name, "metrics");
        assert_eq!(present.path, "metrics.json");

        let missing = ArtifactPointer::missing("junit", "junit.xml");
        assert!(!missing.present);
        assert_eq!(missing.name, "junit");
    }
}
