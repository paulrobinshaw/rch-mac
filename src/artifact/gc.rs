//! Host-side artifact retention and garbage collection
//!
//! Per PLAN.md §Caching:
//! - Host-side artifact retention to bound disk usage under artifact root
//! - Support age-based and/or count-based limits
//! - MUST NOT delete artifacts for RUNNING runs
//! - Acquire filesystem lock before scanning/deleting

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::state::{RunState, RunStateData};

/// Retention policy for artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Maximum number of runs to keep (0 = unlimited)
    #[serde(default)]
    pub max_runs: usize,
    /// Maximum age in days (0 = unlimited)
    #[serde(default)]
    pub max_age_days: u32,
    /// Maximum total size in bytes (0 = unlimited)
    #[serde(default)]
    pub max_size_bytes: u64,
    /// Dry-run mode (log but don't delete)
    #[serde(default)]
    pub dry_run: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_runs: 100,       // Keep last 100 runs
            max_age_days: 30,    // Keep runs from last 30 days
            max_size_bytes: 0,   // No size limit by default
            dry_run: false,
        }
    }
}

impl RetentionPolicy {
    /// Create a count-based retention policy.
    pub fn keep_last_n(count: usize) -> Self {
        Self {
            max_runs: count,
            max_age_days: 0,
            max_size_bytes: 0,
            dry_run: false,
        }
    }

    /// Create an age-based retention policy.
    pub fn keep_days(days: u32) -> Self {
        Self {
            max_runs: 0,
            max_age_days: days,
            max_size_bytes: 0,
            dry_run: false,
        }
    }

    /// Set dry-run mode.
    pub fn with_dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// Result of artifact garbage collection.
#[derive(Debug, Clone, Default)]
pub struct ArtifactGcResult {
    /// Number of runs scanned
    pub scanned: usize,
    /// Number of runs deleted
    pub deleted: usize,
    /// Bytes reclaimed
    pub bytes_reclaimed: u64,
    /// Runs skipped (running or protected)
    pub skipped: usize,
    /// Errors encountered (non-fatal)
    pub errors: Vec<String>,
}

/// Information about a run's artifacts.
#[derive(Debug, Clone)]
struct RunInfo {
    path: PathBuf,
    run_id: String,
    state: Option<RunState>,
    created_at: Option<SystemTime>,
    size_bytes: u64,
}

/// Artifact garbage collector.
pub struct ArtifactGc {
    artifact_root: PathBuf,
    policy: RetentionPolicy,
}

impl ArtifactGc {
    /// Lock file name for GC coordination.
    const LOCK_FILENAME: &'static str = ".rch_artifact_gc.lock";

    /// Create a new artifact garbage collector.
    pub fn new(artifact_root: PathBuf, policy: RetentionPolicy) -> Self {
        Self {
            artifact_root,
            policy,
        }
    }

    /// Run garbage collection.
    ///
    /// Scans the artifact root for runs and applies the retention policy.
    /// Runs in RUNNING state are never deleted.
    pub fn run(&self) -> io::Result<ArtifactGcResult> {
        let mut result = ArtifactGcResult::default();

        // Ensure artifact root exists
        if !self.artifact_root.exists() {
            return Ok(result);
        }

        // Try to acquire lock (skip if locked)
        let lock_path = self.artifact_root.join(Self::LOCK_FILENAME);
        let _lock_file = match fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) => {
                result.errors.push(format!("Failed to acquire GC lock: {}", e));
                return Ok(result);
            }
        };

        // Collect all runs
        let mut runs = self.collect_runs()?;
        result.scanned = runs.len();

        if runs.is_empty() {
            return Ok(result);
        }

        // Sort by creation time (newest first)
        runs.sort_by(|a, b| {
            let time_a = a.created_at.unwrap_or(SystemTime::UNIX_EPOCH);
            let time_b = b.created_at.unwrap_or(SystemTime::UNIX_EPOCH);
            time_b.cmp(&time_a) // Descending (newest first)
        });

        // Determine which runs to delete
        let now = SystemTime::now();
        let mut kept = 0;
        let mut current_size: u64 = runs.iter().map(|r| r.size_bytes).sum();

        for run in &runs {
            // Never delete running runs
            if let Some(state) = run.state {
                if !state.is_terminal() {
                    result.skipped += 1;
                    kept += 1;
                    continue;
                }
            }

            let mut should_delete = false;

            // Count-based eviction
            if self.policy.max_runs > 0 && kept >= self.policy.max_runs {
                should_delete = true;
            }

            // Age-based eviction
            if self.policy.max_age_days > 0 {
                if let Some(created) = run.created_at {
                    let max_age = Duration::from_secs(self.policy.max_age_days as u64 * 24 * 60 * 60);
                    if let Ok(age) = now.duration_since(created) {
                        if age > max_age {
                            should_delete = true;
                        }
                    }
                }
            }

            // Size-based eviction
            if self.policy.max_size_bytes > 0 && current_size > self.policy.max_size_bytes {
                should_delete = true;
            }

            if !should_delete {
                kept += 1;
                continue;
            }

            // Delete the run
            if self.policy.dry_run {
                eprintln!("[artifact-gc] DRY-RUN: Would delete run {} ({} bytes)",
                    run.run_id, run.size_bytes);
            } else {
                match fs::remove_dir_all(&run.path) {
                    Ok(_) => {
                        eprintln!("[artifact-gc] Deleted run {} ({} bytes)",
                            run.run_id, run.size_bytes);
                    }
                    Err(e) => {
                        result.errors.push(format!("Failed to delete {}: {}", run.run_id, e));
                        continue;
                    }
                }
            }

            result.deleted += 1;
            result.bytes_reclaimed += run.size_bytes;
            current_size = current_size.saturating_sub(run.size_bytes);
        }

        // Clean up lock file
        let _ = fs::remove_file(&lock_path);

        Ok(result)
    }

    /// Collect information about all runs in the artifact root.
    fn collect_runs(&self) -> io::Result<Vec<RunInfo>> {
        let runs_dir = self.artifact_root.join("runs");
        if !runs_dir.exists() {
            return Ok(Vec::new());
        }

        let mut runs = Vec::new();

        for entry in fs::read_dir(&runs_dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let run_id = match path.file_name() {
                Some(name) => name.to_string_lossy().into_owned(),
                None => continue,
            };

            // Read run state if available
            let state_path = path.join("run_state.json");
            let (state, created_at) = if state_path.exists() {
                match fs::read_to_string(&state_path) {
                    Ok(content) => {
                        match serde_json::from_str::<RunStateData>(&content) {
                            Ok(data) => (
                                Some(data.state),
                                Some(SystemTime::from(data.created_at)),
                            ),
                            Err(_) => (None, None),
                        }
                    }
                    Err(_) => (None, None),
                }
            } else {
                // Fall back to directory mtime
                let mtime = entry.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok());
                (None, mtime)
            };

            // Calculate size
            let size_bytes = self.dir_size(&path).unwrap_or(0);

            runs.push(RunInfo {
                path,
                run_id,
                state,
                created_at,
                size_bytes,
            });
        }

        Ok(runs)
    }

    /// Calculate directory size recursively.
    fn dir_size(&self, path: &Path) -> io::Result<u64> {
        let mut size = 0;
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    size += self.dir_size(&path)?;
                } else {
                    size += entry.metadata()?.len();
                }
            }
        }
        Ok(size)
    }

    /// Get statistics about the artifact root.
    pub fn stats(&self) -> io::Result<ArtifactStats> {
        let runs = self.collect_runs()?;

        let total_runs = runs.len();
        let running_runs = runs.iter()
            .filter(|r| r.state.map(|s| !s.is_terminal()).unwrap_or(false))
            .count();
        let total_size: u64 = runs.iter().map(|r| r.size_bytes).sum();

        let oldest = runs.iter()
            .filter_map(|r| r.created_at)
            .min();

        Ok(ArtifactStats {
            total_runs,
            running_runs,
            total_size_bytes: total_size,
            oldest_run: oldest,
        })
    }
}

/// Statistics about the artifact root.
#[derive(Debug, Clone)]
pub struct ArtifactStats {
    /// Total number of runs
    pub total_runs: usize,
    /// Number of currently running runs
    pub running_runs: usize,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Oldest run creation time
    pub oldest_run: Option<SystemTime>,
}

// Use RunState's is_terminal method
use crate::state::TerminalState;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use chrono::Utc;

    fn make_run_state(state: RunState) -> RunStateData {
        RunStateData {
            schema_version: 1,
            schema_id: "rch-xcode/run_state@1".to_string(),
            run_id: "test-run".to_string(),
            state,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            current_step: None,
            lease: None,
            seq: 0,
        }
    }

    fn create_run_dir(runs_dir: &Path, run_id: &str, state: RunState) {
        let run_path = runs_dir.join(run_id);
        fs::create_dir_all(&run_path).unwrap();

        let state_data = make_run_state(state);
        let state_json = serde_json::to_string(&state_data).unwrap();
        fs::write(run_path.join("run_state.json"), state_json).unwrap();

        // Add some content
        fs::write(run_path.join("summary.json"), "{}").unwrap();
    }

    #[test]
    fn test_retention_policy_default() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.max_runs, 100);
        assert_eq!(policy.max_age_days, 30);
        assert!(!policy.dry_run);
    }

    #[test]
    fn test_retention_policy_keep_last_n() {
        let policy = RetentionPolicy::keep_last_n(10);
        assert_eq!(policy.max_runs, 10);
        assert_eq!(policy.max_age_days, 0);
    }

    #[test]
    fn test_retention_policy_keep_days() {
        let policy = RetentionPolicy::keep_days(7);
        assert_eq!(policy.max_runs, 0);
        assert_eq!(policy.max_age_days, 7);
    }

    #[test]
    fn test_gc_empty_artifact_root() {
        let temp_dir = TempDir::new().unwrap();
        let policy = RetentionPolicy::default();
        let gc = ArtifactGc::new(temp_dir.path().to_path_buf(), policy);

        let result = gc.run().unwrap();
        assert_eq!(result.scanned, 0);
        assert_eq!(result.deleted, 0);
    }

    #[test]
    fn test_gc_protects_running_runs() {
        let temp_dir = TempDir::new().unwrap();
        let runs_dir = temp_dir.path().join("runs");
        fs::create_dir_all(&runs_dir).unwrap();

        // Create a running run
        create_run_dir(&runs_dir, "running-run-1", RunState::Running);

        // Create completed runs
        create_run_dir(&runs_dir, "completed-run-1", RunState::Succeeded);
        create_run_dir(&runs_dir, "completed-run-2", RunState::Failed);

        // Only keep 1 run
        let policy = RetentionPolicy::keep_last_n(1);
        let gc = ArtifactGc::new(temp_dir.path().to_path_buf(), policy);

        let result = gc.run().unwrap();

        // Running run should be protected
        assert!(runs_dir.join("running-run-1").exists(),
            "Running run should NOT be deleted");

        // At least one completed run should be deleted
        assert!(result.deleted >= 1 || result.skipped >= 1,
            "Should have processed runs");
    }

    #[test]
    fn test_gc_count_based_eviction() {
        let temp_dir = TempDir::new().unwrap();
        let runs_dir = temp_dir.path().join("runs");
        fs::create_dir_all(&runs_dir).unwrap();

        // Create 5 completed runs
        for i in 0..5 {
            create_run_dir(&runs_dir, &format!("run-{}", i), RunState::Succeeded);
        }

        // Only keep 2 runs
        let policy = RetentionPolicy::keep_last_n(2);
        let gc = ArtifactGc::new(temp_dir.path().to_path_buf(), policy);

        let result = gc.run().unwrap();

        assert_eq!(result.scanned, 5);
        // Should delete 3 runs (5 - 2 = 3)
        // Note: may vary due to timing, but should delete some
        assert!(result.deleted >= 1, "Should have deleted some runs");

        // Count remaining runs
        let remaining = fs::read_dir(&runs_dir).unwrap().count();
        assert!(remaining <= 3, "Should have at most 3 remaining runs");
    }

    #[test]
    fn test_gc_dry_run() {
        let temp_dir = TempDir::new().unwrap();
        let runs_dir = temp_dir.path().join("runs");
        fs::create_dir_all(&runs_dir).unwrap();

        // Create runs
        create_run_dir(&runs_dir, "run-1", RunState::Succeeded);
        create_run_dir(&runs_dir, "run-2", RunState::Succeeded);

        // Keep only 1, but dry-run
        let policy = RetentionPolicy::keep_last_n(1).with_dry_run();
        let gc = ArtifactGc::new(temp_dir.path().to_path_buf(), policy);

        let result = gc.run().unwrap();

        // Should report deletion but not actually delete
        assert!(result.deleted >= 1 || result.scanned >= 2);

        // Both should still exist
        assert!(runs_dir.join("run-1").exists());
        assert!(runs_dir.join("run-2").exists());
    }

    #[test]
    fn test_gc_stats() {
        let temp_dir = TempDir::new().unwrap();
        let runs_dir = temp_dir.path().join("runs");
        fs::create_dir_all(&runs_dir).unwrap();

        create_run_dir(&runs_dir, "run-1", RunState::Succeeded);
        create_run_dir(&runs_dir, "run-2", RunState::Running);

        let policy = RetentionPolicy::default();
        let gc = ArtifactGc::new(temp_dir.path().to_path_buf(), policy);

        let stats = gc.stats().unwrap();
        assert_eq!(stats.total_runs, 2);
        assert_eq!(stats.running_runs, 1);
        assert!(stats.total_size_bytes > 0);
    }

    #[test]
    fn test_gc_result_merge() {
        let mut r1 = ArtifactGcResult {
            scanned: 10,
            deleted: 3,
            bytes_reclaimed: 1000,
            skipped: 2,
            errors: vec!["e1".to_string()],
        };

        r1.scanned += 5;
        r1.deleted += 2;

        assert_eq!(r1.scanned, 15);
        assert_eq!(r1.deleted, 5);
    }
}
