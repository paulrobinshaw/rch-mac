//! Run resumption after host restart (M6 feature)
//!
//! Per rch-mac-0bw.3:
//! - Read run_plan.json to recover step list and job_ids
//! - Check job_index.json existence (commit marker) per step
//! - If present: treat step as COMPLETE
//! - If absent: query worker via status
//! - Record resumed_at in run_state.json
//! - Re-probe worker, verify protocol_version still in range
//! - Verify worker still has required Xcode build
//! - Must NOT switch workers mid-run

use std::fs;
use std::path::Path;

use super::{RunError, RunPlan, PlanStep};

/// Status of a step during resumption check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumptionStepStatus {
    /// Step completed (job_index.json exists)
    Complete,
    /// Step needs status check from worker
    NeedsStatusCheck,
    /// Step needs to be re-submitted
    NeedsResubmit,
    /// Step was rejected (skip it)
    Rejected,
}

/// Result of checking resumption state for a run
#[derive(Debug, Clone)]
pub struct ResumptionState {
    /// The loaded run plan
    pub plan: RunPlan,

    /// Status of each step
    pub step_statuses: Vec<(usize, ResumptionStepStatus)>,

    /// Index of the first incomplete step (where to resume)
    pub resume_from: Option<usize>,

    /// Steps that are already complete
    pub complete_count: usize,
}

impl ResumptionState {
    /// Check if all steps are complete (nothing to resume)
    pub fn is_fully_complete(&self) -> bool {
        self.resume_from.is_none()
    }

    /// Get the number of steps that need work
    pub fn steps_remaining(&self) -> usize {
        self.plan.steps.len() - self.complete_count
    }

    /// Get steps that need status check
    pub fn steps_needing_status_check(&self) -> Vec<&PlanStep> {
        self.step_statuses
            .iter()
            .filter(|(_, status)| *status == ResumptionStepStatus::NeedsStatusCheck)
            .filter_map(|(idx, _)| self.plan.get_step(*idx))
            .collect()
    }
}

/// Check the resumption state for a run directory.
///
/// This reads run_plan.json and checks each step's job_index.json existence.
pub fn check_resumption_state(run_dir: &Path) -> Result<ResumptionState, RunError> {
    // Load run_plan.json
    let plan_path = run_dir.join("run_plan.json");
    if !plan_path.exists() {
        return Err(RunError::PlanNotFound {
            path: plan_path.display().to_string(),
        });
    }

    let plan_json = fs::read_to_string(&plan_path)
        .map_err(|e| RunError::Io(e))?;

    let plan: RunPlan = serde_json::from_str(&plan_json)
        .map_err(|e| RunError::Serialization(e.to_string()))?;

    // Check each step's completion status
    let mut step_statuses = Vec::with_capacity(plan.steps.len());
    let mut complete_count = 0;
    let mut resume_from = None;

    for step in &plan.steps {
        if step.rejected {
            step_statuses.push((step.index, ResumptionStepStatus::Rejected));
            continue;
        }

        // Check if job_index.json exists (the commit marker)
        let job_index_path = run_dir
            .join("steps")
            .join(&step.job_id)
            .join("job_index.json");

        if job_index_path.exists() {
            step_statuses.push((step.index, ResumptionStepStatus::Complete));
            complete_count += 1;
        } else {
            // Need to check with worker or re-submit
            step_statuses.push((step.index, ResumptionStepStatus::NeedsStatusCheck));
            if resume_from.is_none() {
                resume_from = Some(step.index);
            }
        }
    }

    Ok(ResumptionState {
        plan,
        step_statuses,
        resume_from,
        complete_count,
    })
}

/// Verify that a worker's protocol version is compatible with the run plan.
///
/// Returns Ok if compatible, Err(ProtocolDrift) if not.
pub fn verify_protocol_compatibility(
    plan: &RunPlan,
    worker_protocol_min: u32,
    worker_protocol_max: u32,
) -> Result<(), RunError> {
    let planned_version = plan.protocol_version;

    if planned_version < worker_protocol_min || planned_version > worker_protocol_max {
        return Err(RunError::ProtocolDrift {
            planned: planned_version,
            min: worker_protocol_min,
            max: worker_protocol_max,
        });
    }

    Ok(())
}

/// Verify that a worker still has the required Xcode version.
///
/// Returns Ok if available, Err(ToolchainChanged) if not.
///
/// Note: This requires the plan to include the required Xcode version,
/// which should be extracted from the effective_config at resume time.
pub fn verify_toolchain_available(
    required_xcode: &str,
    available_xcode_versions: &[String],
) -> Result<(), RunError> {
    if available_xcode_versions.iter().any(|v| v == required_xcode) {
        Ok(())
    } else {
        Err(RunError::ToolchainChanged {
            required: required_xcode.to_string(),
            available: available_xcode_versions.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use crate::run::RunPlan;
    use chrono::Utc;
    use crate::job::Action;

    fn make_test_plan(run_id: &str) -> RunPlan {
        RunPlan {
            schema_version: 1,
            schema_id: "rch-xcode/run_plan@1".to_string(),
            created_at: Utc::now(),
            run_id: run_id.to_string(),
            steps: vec![
                PlanStep {
                    index: 0,
                    action: Action::Build,
                    job_id: "job-001".to_string(),
                    rejected: false,
                    rejection_reasons: vec![],
                },
                PlanStep {
                    index: 1,
                    action: Action::Test,
                    job_id: "job-002".to_string(),
                    rejected: false,
                    rejection_reasons: vec![],
                },
            ],
            selected_worker: "macmini-01".to_string(),
            selected_worker_host: "macmini.local".to_string(),
            continue_on_failure: false,
            protocol_version: 1,
        }
    }

    #[test]
    fn test_check_resumption_state_no_plan() {
        let temp_dir = TempDir::new().unwrap();

        let result = check_resumption_state(temp_dir.path());

        assert!(matches!(result, Err(RunError::PlanNotFound { .. })));
    }

    #[test]
    fn test_check_resumption_state_no_complete_steps() {
        let temp_dir = TempDir::new().unwrap();
        let plan = make_test_plan("test-run");

        // Write run_plan.json
        let plan_path = temp_dir.path().join("run_plan.json");
        fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();

        let result = check_resumption_state(temp_dir.path()).unwrap();

        assert_eq!(result.complete_count, 0);
        assert_eq!(result.resume_from, Some(0));
        assert!(!result.is_fully_complete());
    }

    #[test]
    fn test_check_resumption_state_first_step_complete() {
        let temp_dir = TempDir::new().unwrap();
        let plan = make_test_plan("test-run");

        // Write run_plan.json
        let plan_path = temp_dir.path().join("run_plan.json");
        fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();

        // Create job_index.json for first step
        let job_dir = temp_dir.path().join("steps").join("job-001");
        fs::create_dir_all(&job_dir).unwrap();
        fs::write(job_dir.join("job_index.json"), "{}").unwrap();

        let result = check_resumption_state(temp_dir.path()).unwrap();

        assert_eq!(result.complete_count, 1);
        assert_eq!(result.resume_from, Some(1));
        assert!(!result.is_fully_complete());
    }

    #[test]
    fn test_check_resumption_state_all_complete() {
        let temp_dir = TempDir::new().unwrap();
        let plan = make_test_plan("test-run");

        // Write run_plan.json
        let plan_path = temp_dir.path().join("run_plan.json");
        fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();

        // Create job_index.json for both steps
        for job_id in &["job-001", "job-002"] {
            let job_dir = temp_dir.path().join("steps").join(job_id);
            fs::create_dir_all(&job_dir).unwrap();
            fs::write(job_dir.join("job_index.json"), "{}").unwrap();
        }

        let result = check_resumption_state(temp_dir.path()).unwrap();

        assert_eq!(result.complete_count, 2);
        assert!(result.resume_from.is_none());
        assert!(result.is_fully_complete());
    }

    #[test]
    fn test_verify_protocol_compatibility_ok() {
        let plan = make_test_plan("test-run");

        let result = verify_protocol_compatibility(&plan, 1, 2);

        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_protocol_compatibility_drift() {
        let mut plan = make_test_plan("test-run");
        plan.protocol_version = 2;

        let result = verify_protocol_compatibility(&plan, 3, 5);

        assert!(matches!(result, Err(RunError::ProtocolDrift { planned: 2, min: 3, max: 5 })));
    }

    #[test]
    fn test_verify_toolchain_available_ok() {
        let result = verify_toolchain_available(
            "16.2",
            &["15.4".to_string(), "16.2".to_string()],
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_toolchain_changed() {
        let result = verify_toolchain_available(
            "16.2",
            &["15.4".to_string(), "16.0".to_string()],
        );

        assert!(matches!(result, Err(RunError::ToolchainChanged { required, .. }) if required == "16.2"));
    }
}
