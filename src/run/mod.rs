//! Run builder for RCH Xcode Lane
//!
//! Resolves repo verify actions into an ordered run plan, allocates job_ids,
//! and manages step execution according to PLAN.md spec.
//!
//! Per PLAN.md:
//! - run_plan.json is emitted before execution starts
//! - All steps use the same worker (selected once at plan time)
//! - Rejected steps still appear in plan with rejected=true
//! - Sequential execution, abort on failure unless continue_on_failure=true

pub mod streaming;

use crate::job::{generate_job_id, generate_run_id, Action};
use crate::selection::WorkerSelection;
use crate::summary::Status;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schema version for run_plan.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier for run_plan.json
pub const SCHEMA_ID: &str = "rch-xcode/run_plan@1";

/// A step in the run plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step index (0-based)
    pub index: usize,

    /// Action for this step
    pub action: Action,

    /// Pre-allocated job_id for this step
    pub job_id: String,

    /// Whether the classifier rejected this step at plan time
    #[serde(default)]
    pub rejected: bool,

    /// Rejection reasons (if rejected)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub rejection_reasons: Vec<String>,
}

/// The run plan artifact (run_plan.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPlan {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When this plan was created
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Ordered steps in the run
    pub steps: Vec<PlanStep>,

    /// Name of the selected worker
    pub selected_worker: String,

    /// SSH host of the selected worker
    pub selected_worker_host: String,

    /// Whether to continue execution after a step failure
    #[serde(default)]
    pub continue_on_failure: bool,

    /// Negotiated protocol version with the worker
    pub protocol_version: u32,
}

impl RunPlan {
    /// Get the number of steps
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Get a step by index
    pub fn get_step(&self, index: usize) -> Option<&PlanStep> {
        self.steps.get(index)
    }

    /// Check if any steps were rejected
    pub fn has_rejected_steps(&self) -> bool {
        self.steps.iter().any(|s| s.rejected)
    }

    /// Count of rejected steps
    pub fn rejected_count(&self) -> usize {
        self.steps.iter().filter(|s| s.rejected).count()
    }
}

impl std::fmt::Display for RunPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Run Plan ===")?;
        writeln!(f)?;
        writeln!(f, "Run ID: {}", self.run_id)?;
        writeln!(f, "Worker: {} ({})", self.selected_worker, self.selected_worker_host)?;
        writeln!(f, "Protocol: v{}", self.protocol_version)?;
        writeln!(f)?;
        writeln!(f, "Steps ({}):", self.steps.len())?;
        for step in &self.steps {
            let status = if step.rejected { "REJECTED" } else { "OK" };
            writeln!(f, "  [{}] {} - {} (job: {})",
                step.index,
                step.action,
                status,
                step.job_id
            )?;
            if step.rejected && !step.rejection_reasons.is_empty() {
                for reason in &step.rejection_reasons {
                    writeln!(f, "        Reason: {}", reason)?;
                }
            }
        }
        if self.continue_on_failure {
            writeln!(f)?;
            writeln!(f, "(continue-on-failure enabled)")?;
        }
        Ok(())
    }
}

/// Errors during run building
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// No actions specified
    #[error("no actions specified for run")]
    NoActions,

    /// Classifier error
    #[error("classifier error: {0}")]
    Classifier(String),

    /// Worker selection error
    #[error("worker selection error: {0}")]
    WorkerSelection(String),

    /// Invalid action string
    #[error("invalid action: {0}")]
    InvalidAction(String),

    /// All steps were rejected
    #[error("all steps were rejected by classifier")]
    AllRejected,

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Builder for creating run plans
pub struct RunPlanBuilder {
    run_id: String,
    actions: Vec<Action>,
    continue_on_failure: bool,
}

impl RunPlanBuilder {
    /// Create a new run builder with the given actions
    pub fn new(actions: Vec<Action>) -> Self {
        Self {
            run_id: generate_run_id(),
            actions,
            continue_on_failure: false,
        }
    }

    /// Create from string action names
    pub fn from_action_strings(actions: &[&str]) -> Result<Self, RunError> {
        let parsed: Result<Vec<Action>, _> = actions
            .iter()
            .map(|s| s.parse::<Action>().map_err(|_| RunError::InvalidAction(s.to_string())))
            .collect();

        let actions = parsed?;

        if actions.is_empty() {
            return Err(RunError::NoActions);
        }

        Ok(Self::new(actions))
    }

    /// Create for a single action (rch xcode run --action <action>)
    pub fn single_action(action: Action) -> Self {
        Self::new(vec![action])
    }

    /// Set a specific run_id (for testing or resumption)
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = run_id.into();
        self
    }

    /// Set whether to continue after step failure
    pub fn continue_on_failure(mut self, continue_on_failure: bool) -> Self {
        self.continue_on_failure = continue_on_failure;
        self
    }

    /// Get the run_id (for use before build)
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Build the run plan with classifier results
    ///
    /// This allocates job_ids and records rejection status for each step
    pub fn build_with_classifier_results(
        self,
        classifier_results: Vec<(bool, Vec<String>)>,
        worker_selection: &WorkerSelection,
    ) -> Result<RunPlan, RunError> {
        if self.actions.is_empty() {
            return Err(RunError::NoActions);
        }

        if classifier_results.len() != self.actions.len() {
            return Err(RunError::Classifier(
                "classifier results count does not match actions".to_string(),
            ));
        }

        let mut steps = Vec::with_capacity(self.actions.len());

        for (index, (action, (accepted, reasons))) in self
            .actions
            .iter()
            .zip(classifier_results.into_iter())
            .enumerate()
        {
            let job_id = generate_job_id();

            steps.push(PlanStep {
                index,
                action: *action,
                job_id,
                rejected: !accepted,
                rejection_reasons: if accepted { vec![] } else { reasons },
            });
        }

        Ok(RunPlan {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: self.run_id,
            steps,
            selected_worker: worker_selection.selected_worker.clone(),
            selected_worker_host: worker_selection.selected_worker_host.clone(),
            continue_on_failure: self.continue_on_failure,
            protocol_version: worker_selection.negotiated_protocol_version,
        })
    }

    /// Build the run plan assuming all steps are accepted
    ///
    /// This is a convenience method for when classifier has already been run
    pub fn build_all_accepted(self, worker_selection: &WorkerSelection) -> Result<RunPlan, RunError> {
        let classifier_results: Vec<(bool, Vec<String>)> =
            self.actions.iter().map(|_| (true, vec![])).collect();
        self.build_with_classifier_results(classifier_results, worker_selection)
    }
}

/// Execution state for a run
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// Run is pending
    Pending,
    /// Run is executing steps
    Running,
    /// Run completed successfully
    Succeeded,
    /// Run failed (at least one step failed)
    Failed,
    /// Run was cancelled
    Cancelled,
}

/// Step execution result
#[derive(Debug, Clone)]
pub struct StepResult {
    /// The step that was executed
    pub step: PlanStep,
    /// Final status
    pub status: Status,
    /// Whether this step was skipped
    pub skipped: bool,
}

/// Run execution context
pub struct RunExecution {
    plan: RunPlan,
    results: Vec<StepResult>,
    state: ExecutionState,
}

impl RunExecution {
    /// Create a new run execution from a plan
    pub fn new(plan: RunPlan) -> Self {
        Self {
            plan,
            results: Vec::new(),
            state: ExecutionState::Pending,
        }
    }

    /// Get the run plan
    pub fn plan(&self) -> &RunPlan {
        &self.plan
    }

    /// Get current execution state
    pub fn state(&self) -> ExecutionState {
        self.state
    }

    /// Get completed step results
    pub fn results(&self) -> &[StepResult] {
        &self.results
    }

    /// Get the next step to execute, if any
    pub fn next_step(&self) -> Option<&PlanStep> {
        let completed = self.results.len();
        self.plan.steps.get(completed)
    }

    /// Check if there are any non-rejected steps
    pub fn has_executable_steps(&self) -> bool {
        self.plan.steps.iter().any(|s| !s.rejected)
    }

    /// Record a step result
    pub fn record_result(&mut self, result: StepResult) {
        self.results.push(result);
        self.update_state();
    }

    /// Mark the run as cancelled
    pub fn cancel(&mut self) {
        self.state = ExecutionState::Cancelled;
    }

    /// Update execution state based on results
    fn update_state(&mut self) {
        let has_failure = self.results.iter().any(|r| {
            matches!(r.status, Status::Failed | Status::Rejected) && !r.skipped
        });

        let all_done = self.results.len() >= self.plan.steps.len();

        if has_failure && !self.plan.continue_on_failure {
            self.state = ExecutionState::Failed;
        } else if all_done {
            if has_failure {
                self.state = ExecutionState::Failed;
            } else {
                self.state = ExecutionState::Succeeded;
            }
        } else {
            self.state = ExecutionState::Running;
        }
    }

    /// Count of steps that succeeded
    pub fn steps_succeeded(&self) -> usize {
        self.results.iter().filter(|r| r.status == Status::Success).count()
    }

    /// Count of steps that failed
    pub fn steps_failed(&self) -> usize {
        self.results.iter().filter(|r| r.status == Status::Failed).count()
    }

    /// Count of steps that were rejected
    pub fn steps_rejected(&self) -> usize {
        self.results.iter().filter(|r| r.status == Status::Rejected).count()
    }

    /// Count of steps that were skipped
    pub fn steps_skipped(&self) -> usize {
        self.results.iter().filter(|r| r.skipped).count()
    }

    /// Count of steps that were cancelled
    pub fn steps_cancelled(&self) -> usize {
        self.results.iter().filter(|r| r.status == Status::Cancelled).count()
    }

    /// Should execution continue after the last result?
    pub fn should_continue(&self) -> bool {
        if self.state == ExecutionState::Cancelled {
            return false;
        }

        if let Some(last) = self.results.last() {
            if matches!(last.status, Status::Failed | Status::Rejected) {
                return self.plan.continue_on_failure;
            }
        }

        self.results.len() < self.plan.steps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::{ProtocolRange, SelectionMode, SnapshotSource};

    fn mock_worker_selection() -> WorkerSelection {
        WorkerSelection {
            schema_version: 1,
            schema_id: "rch-xcode/worker_selection@1".to_string(),
            created_at: Utc::now(),
            run_id: "test-run-id-12345678".to_string(),
            negotiated_protocol_version: 1,
            worker_protocol_range: ProtocolRange { min: 1, max: 1 },
            selected_worker: "macmini-01".to_string(),
            selected_worker_host: "macmini.local".to_string(),
            selection_mode: SelectionMode::Deterministic,
            candidate_count: 1,
            probe_failures: vec![],
            snapshot_age_seconds: 0,
            snapshot_source: SnapshotSource::Fresh,
            adaptive_metrics: None,
        }
    }

    #[test]
    fn test_run_plan_builder_single_action() {
        let builder = RunPlanBuilder::single_action(Action::Build);
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        assert_eq!(plan.schema_version, 1);
        assert_eq!(plan.schema_id, "rch-xcode/run_plan@1");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].action, Action::Build);
        assert_eq!(plan.steps[0].index, 0);
        assert!(!plan.steps[0].rejected);
        assert!(!plan.continue_on_failure);
    }

    #[test]
    fn test_run_plan_builder_verify_actions() {
        let actions = vec![Action::Build, Action::Test];
        let builder = RunPlanBuilder::new(actions);
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].action, Action::Build);
        assert_eq!(plan.steps[1].action, Action::Test);
        assert_ne!(plan.steps[0].job_id, plan.steps[1].job_id);
    }

    #[test]
    fn test_run_plan_builder_from_strings() {
        let builder = RunPlanBuilder::from_action_strings(&["build", "test"]).unwrap();
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn test_run_plan_builder_invalid_action() {
        let result = RunPlanBuilder::from_action_strings(&["archive"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_plan_builder_no_actions() {
        let result = RunPlanBuilder::from_action_strings(&[]);
        assert!(matches!(result, Err(RunError::NoActions)));
    }

    #[test]
    fn test_run_plan_builder_continue_on_failure() {
        let builder = RunPlanBuilder::new(vec![Action::Build])
            .continue_on_failure(true);
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        assert!(plan.continue_on_failure);
    }

    #[test]
    fn test_run_plan_builder_with_run_id() {
        let builder = RunPlanBuilder::new(vec![Action::Build])
            .with_run_id("custom-run-id-12345678");
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        assert_eq!(plan.run_id, "custom-run-id-12345678");
    }

    #[test]
    fn test_run_plan_with_rejected_step() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test]);
        let worker_selection = mock_worker_selection();

        let classifier_results = vec![
            (true, vec![]),
            (false, vec!["SCHEME_MISMATCH".to_string()]),
        ];

        let plan = builder
            .build_with_classifier_results(classifier_results, &worker_selection)
            .unwrap();

        assert!(!plan.steps[0].rejected);
        assert!(plan.steps[1].rejected);
        assert_eq!(plan.steps[1].rejection_reasons, vec!["SCHEME_MISMATCH"]);
        assert!(plan.has_rejected_steps());
        assert_eq!(plan.rejected_count(), 1);
    }

    #[test]
    fn test_run_plan_serialization() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test]);
        let worker_selection = mock_worker_selection();

        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        let json = serde_json::to_string_pretty(&plan).unwrap();
        assert!(json.contains(r#""schema_version": 1"#));
        assert!(json.contains(r#""schema_id": "rch-xcode/run_plan@1""#));
        assert!(json.contains(r#""selected_worker": "macmini-01""#));

        let parsed: RunPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.steps.len(), 2);
    }

    #[test]
    fn test_run_execution_basic() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test]);
        let worker_selection = mock_worker_selection();
        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        let execution = RunExecution::new(plan);

        assert_eq!(execution.state(), ExecutionState::Pending);
        assert!(execution.has_executable_steps());

        let next = execution.next_step().unwrap();
        assert_eq!(next.action, Action::Build);
    }

    #[test]
    fn test_run_execution_step_results() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test]);
        let worker_selection = mock_worker_selection();
        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        let mut execution = RunExecution::new(plan.clone());

        execution.record_result(StepResult {
            step: plan.steps[0].clone(),
            status: Status::Success,
            skipped: false,
        });

        assert_eq!(execution.state(), ExecutionState::Running);
        assert!(execution.should_continue());

        execution.record_result(StepResult {
            step: plan.steps[1].clone(),
            status: Status::Success,
            skipped: false,
        });

        assert_eq!(execution.state(), ExecutionState::Succeeded);
        assert!(!execution.should_continue());
        assert_eq!(execution.steps_succeeded(), 2);
    }

    #[test]
    fn test_run_execution_failure_stops() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test]);
        let worker_selection = mock_worker_selection();
        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        let mut execution = RunExecution::new(plan.clone());

        execution.record_result(StepResult {
            step: plan.steps[0].clone(),
            status: Status::Failed,
            skipped: false,
        });

        assert_eq!(execution.state(), ExecutionState::Failed);
        assert!(!execution.should_continue());
        assert_eq!(execution.steps_failed(), 1);
    }

    #[test]
    fn test_run_execution_continue_on_failure() {
        let builder = RunPlanBuilder::new(vec![Action::Build, Action::Test])
            .continue_on_failure(true);
        let worker_selection = mock_worker_selection();
        let plan = builder.build_all_accepted(&worker_selection).unwrap();

        let mut execution = RunExecution::new(plan.clone());

        execution.record_result(StepResult {
            step: plan.steps[0].clone(),
            status: Status::Failed,
            skipped: false,
        });

        assert!(execution.should_continue());

        execution.record_result(StepResult {
            step: plan.steps[1].clone(),
            status: Status::Success,
            skipped: false,
        });

        assert_eq!(execution.state(), ExecutionState::Failed);
    }

    #[test]
    fn test_plan_step_serialization() {
        let step = PlanStep {
            index: 0,
            action: Action::Build,
            job_id: "01hwvx7q8yabcdefghij".to_string(),
            rejected: false,
            rejection_reasons: vec![],
        };

        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains(r#""action":"build""#));
        assert!(json.contains(r#""rejected":false"#));
        assert!(!json.contains("rejection_reasons"));
    }

    #[test]
    fn test_rejected_step_serialization() {
        let step = PlanStep {
            index: 0,
            action: Action::Build,
            job_id: "01hwvx7q8yabcdefghij".to_string(),
            rejected: true,
            rejection_reasons: vec!["SCHEME_MISMATCH:BadScheme".to_string()],
        };

        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains(r#""rejected":true"#));
        assert!(json.contains("rejection_reasons"));
        assert!(json.contains("SCHEME_MISMATCH:BadScheme"));
    }
}
