//! Pipeline orchestration for RCH Xcode Lane
//!
//! This module implements the full verify/run pipeline per bead axz.17:
//! - Classify actions
//! - Select worker
//! - Build run plan
//! - Bundle sources
//! - Execute jobs with streaming logs
//! - Collect artifacts and emit summaries
//!
//! The pipeline supports both multi-step verify (build + test) and
//! single-step run (--action build/test).

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::artifact::verify_artifacts;
use crate::bundle::{Bundler, BundleMode, BundleError, BundleResult, SourceManifest};
use crate::classifier::{Classifier, ClassifierConfig, ClassifierResult, RepoConfig};
use crate::config::{EffectiveConfig, ConfigSource};
use crate::destination::{resolve_destination, DestinationConstraint, ResolvedDestination};
use crate::host::rpc::{RpcClient, RpcClientConfig, RpcError, TailResponse, CancelReason};
use crate::host::transport::{SshConfig, SshTransport, Transport, TransportError};
use crate::inventory::WorkerInventory;
use crate::job::{Action, JobSpec, JobSpecBuilder, JobKeyInputs, JobKeyToolchain, JobKeyDestination};
use crate::run::{RunPlan, RunPlanBuilder, RunExecution, PlanStep, StepResult, ExecutionState};
use crate::run::streaming::{LogStreamer, LogStreamerConfig, StreamMode, has_tail_feature, is_terminal_state};
use crate::selection::{select_worker, SelectionConstraints, WorkerSelection, SelectionError};
use crate::signal::{SignalHandler, SignalAction, CancellationCoordinator};
use crate::state::{RunState, RunStateData, JobState, JobStateData};
use crate::summary::{Status, RunSummary, JobSummary, FailureKind, FailureSubkind, ExitCode};
use crate::toolchain::{resolve_toolchain, ToolchainIdentity, XcodeConstraint};
use crate::worker::Capabilities;

/// Pipeline errors
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("classifier error: {0}")]
    Classifier(String),

    #[error("worker selection error: {0}")]
    WorkerSelection(#[from] SelectionError),

    #[error("bundling error: {0}")]
    Bundling(#[from] BundleError),

    #[error("RPC error: {0}")]
    Rpc(#[from] RpcError),

    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("run error: {0}")]
    Run(#[from] crate::run::RunError),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("all steps rejected by classifier")]
    AllRejected,

    #[error("cancelled")]
    Cancelled,

    #[error("timeout")]
    Timeout,

    #[error("no workers available")]
    NoWorkers,

    #[error("job failed: {0}")]
    JobFailed(String),

    #[error("state error: {0}")]
    State(#[from] crate::state::RunStateError),

    #[error("artifact integrity error: {0}")]
    ArtifactIntegrity(String),
}

impl PipelineError {
    /// Get the exit code for this error
    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::Config(_) => 1,
            PipelineError::Classifier(_) => 10,
            PipelineError::WorkerSelection(_) => 20,
            PipelineError::Bundling(_) => 92,
            PipelineError::Rpc(e) => e.exit_code(),
            PipelineError::Transport(_) => 20,
            PipelineError::Run(_) => 40,
            PipelineError::Io(_) => 1,
            PipelineError::Serialization(_) => 1,
            PipelineError::AllRejected => 10,
            PipelineError::Cancelled => 80,
            PipelineError::Timeout => 80,
            PipelineError::NoWorkers => 91,
            PipelineError::JobFailed(_) => 50,
            PipelineError::State(_) => 40,
            PipelineError::ArtifactIntegrity(_) => 70,
        }
    }
}

/// Result type for pipeline operations
pub type PipelineResult<T> = Result<T, PipelineError>;

/// Pipeline configuration
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Path to repo config file
    pub repo_config_path: PathBuf,

    /// Path to worker inventory
    pub inventory_path: Option<PathBuf>,

    /// Path to artifacts directory
    pub artifacts_dir: PathBuf,

    /// Overall timeout in seconds
    pub overall_timeout_seconds: u64,

    /// Idle log timeout in seconds
    pub idle_timeout_seconds: u64,

    /// Continue execution after step failure
    pub continue_on_failure: bool,

    /// Tail poll interval
    pub tail_poll_interval: Duration,

    /// Verbose output
    pub verbose: bool,

    /// Dry-run mode - print plan without executing
    pub dry_run: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self {
            repo_config_path: PathBuf::from(".rch/xcode.toml"),
            inventory_path: None,
            artifacts_dir: PathBuf::from(format!("{}/.local/share/rch/artifacts/xcode", home)),
            overall_timeout_seconds: 1800,
            idle_timeout_seconds: 300,
            continue_on_failure: false,
            tail_poll_interval: Duration::from_secs(1),
            verbose: false,
            dry_run: false,
        }
    }
}

/// Pipeline execution context
pub struct Pipeline {
    config: PipelineConfig,
    repo_config: Option<RepoConfig>,
    classifier: Option<Classifier>,
    worker_selection: Option<WorkerSelection>,
    capabilities: Option<serde_json::Value>,
    run_plan: Option<RunPlan>,
    source_manifest: Option<SourceManifest>,
    source_sha256: Option<String>,
    bundle_path: Option<PathBuf>,
    rpc_client: Option<RpcClient>,
}

impl Pipeline {
    /// Create a new pipeline with the given configuration
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            config,
            repo_config: None,
            classifier: None,
            worker_selection: None,
            capabilities: None,
            run_plan: None,
            source_manifest: None,
            source_sha256: None,
            bundle_path: None,
            rpc_client: None,
        }
    }

    /// Dry-run mode: classify actions, select worker, build plan without executing
    ///
    /// This performs all the setup steps (load config, classify, select worker,
    /// build run plan) but does not bundle sources or submit to the worker.
    pub fn dry_run(&mut self, action: Option<Action>) -> PipelineResult<RunPlan> {
        // Load config
        self.load_config()?;

        let repo_config = self.repo_config.as_ref().unwrap();

        // Get actions to run
        let actions: Vec<Action> = if let Some(a) = action {
            vec![a]
        } else {
            // Get verify actions from config
            repo_config.verify.iter()
                .filter_map(|v| v.action.parse::<Action>().ok())
                .collect()
        };

        if actions.is_empty() {
            return Err(PipelineError::Config("No actions specified".to_string()));
        }

        if self.config.verbose {
            eprintln!("Dry-run: classifying {} action(s)...", actions.len());
        }

        // 1. Classify actions
        let classifier_results = self.classify_actions(&actions)?;

        // Check if all rejected
        if classifier_results.iter().all(|(accepted, _)| !*accepted) {
            return Err(PipelineError::AllRejected);
        }

        if self.config.verbose {
            eprintln!("Dry-run: selecting worker...");
        }

        // 2. Select worker
        self.select_worker()?;

        if self.config.verbose {
            eprintln!("Dry-run: building run plan...");
        }

        // 3. Build run plan
        let run_plan = RunPlanBuilder::new(actions)
            .continue_on_failure(self.config.continue_on_failure)
            .build_with_classifier_results(classifier_results, self.worker_selection.as_ref().unwrap())?;

        Ok(run_plan)
    }

    /// Execute verify (multiple actions from config)
    pub fn execute_verify(&mut self) -> PipelineResult<RunSummary> {
        // Load config
        self.load_config()?;

        let repo_config = self.repo_config.as_ref().unwrap();

        // Get verify actions from config
        let actions: Vec<Action> = repo_config.verify.iter()
            .filter_map(|v| v.action.parse::<Action>().ok())
            .collect();

        if actions.is_empty() {
            return Err(PipelineError::Config("No verify actions configured".to_string()));
        }

        self.execute_actions(actions)
    }

    /// Execute a single action
    pub fn execute_action(&mut self, action: Action) -> PipelineResult<RunSummary> {
        // Load config
        self.load_config()?;

        self.execute_actions(vec![action])
    }

    /// Execute the given actions
    fn execute_actions(&mut self, actions: Vec<Action>) -> PipelineResult<RunSummary> {
        let start_time = Instant::now();

        // 1. Classify actions
        if self.config.verbose {
            eprintln!("Classifying {} action(s)...", actions.len());
        }
        let classifier_results = self.classify_actions(&actions)?;

        // Check if all rejected
        if classifier_results.iter().all(|(accepted, _)| !*accepted) {
            return Err(PipelineError::AllRejected);
        }

        // 2. Select worker
        if self.config.verbose {
            eprintln!("Selecting worker...");
        }
        self.select_worker()?;

        // 3. Build run plan
        if self.config.verbose {
            eprintln!("Building run plan...");
        }
        let run_plan = RunPlanBuilder::new(actions.clone())
            .continue_on_failure(self.config.continue_on_failure)
            .build_with_classifier_results(classifier_results, self.worker_selection.as_ref().unwrap())?;

        let run_id = run_plan.run_id.clone();
        self.run_plan = Some(run_plan);

        // Create artifact directory
        let run_artifact_dir = self.config.artifacts_dir.join(&run_id);
        fs::create_dir_all(&run_artifact_dir)?;

        // 4. Write run_plan.json
        self.write_run_plan(&run_artifact_dir)?;

        // 5. Create source bundle
        if self.config.verbose {
            eprintln!("Creating source bundle...");
        }
        self.create_source_bundle(&run_artifact_dir)?;

        // 6. Connect to worker and execute
        if self.config.verbose {
            eprintln!("Connecting to worker...");
        }
        self.connect_to_worker()?;

        // 7. Execute run
        let summary = self.execute_run(&run_artifact_dir, start_time)?;

        Ok(summary)
    }

    /// Load configuration
    fn load_config(&mut self) -> PipelineResult<()> {
        if !self.config.repo_config_path.exists() {
            return Err(PipelineError::Config(format!(
                "Repo config not found: {}",
                self.config.repo_config_path.display()
            )));
        }

        let repo_config = RepoConfig::from_file(&self.config.repo_config_path)
            .map_err(|e| PipelineError::Config(e.to_string()))?;

        let classifier_config = repo_config.to_classifier_config();
        self.classifier = Some(Classifier::new(classifier_config));
        self.repo_config = Some(repo_config);

        Ok(())
    }

    /// Classify actions
    fn classify_actions(&self, actions: &[Action]) -> PipelineResult<Vec<(bool, Vec<String>)>> {
        let classifier = self.classifier.as_ref()
            .ok_or_else(|| PipelineError::Classifier("Classifier not initialized".to_string()))?;
        let repo_config = self.repo_config.as_ref()
            .ok_or_else(|| PipelineError::Config("Config not loaded".to_string()))?;

        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            // Build xcodebuild command for this action
            let mut cmd = vec!["xcodebuild".to_string(), action.to_string()];

            if let Some(ref ws) = repo_config.workspace {
                cmd.push("-workspace".to_string());
                cmd.push(ws.clone());
            } else if let Some(ref proj) = repo_config.project {
                cmd.push("-project".to_string());
                cmd.push(proj.clone());
            }

            if !repo_config.schemes.is_empty() {
                cmd.push("-scheme".to_string());
                cmd.push(repo_config.schemes[0].clone());
            }

            if !repo_config.destinations.is_empty() {
                cmd.push("-destination".to_string());
                cmd.push(repo_config.destinations[0].clone());
            }

            let result = classifier.classify(&cmd);
            let accepted = result.accepted;
            let reasons: Vec<String> = result.rejection_reasons.iter()
                .map(|r| format!("{:?}", r))
                .collect();

            results.push((accepted, reasons));
        }

        Ok(results)
    }

    /// Select worker
    fn select_worker(&mut self) -> PipelineResult<()> {
        // Load inventory
        let inventory = match &self.config.inventory_path {
            Some(path) => WorkerInventory::load(path),
            None => WorkerInventory::load_default(),
        }.map_err(|e| PipelineError::NoWorkers)?;

        // Filter by tags for Xcode
        let candidates: Vec<_> = inventory.filter_by_tags(&["macos", "xcode"]);

        if candidates.is_empty() {
            return Err(PipelineError::NoWorkers);
        }

        // For now, select the first candidate with probe
        let worker = candidates[0];

        // Build selection result (simplified - full implementation would probe)
        let selection = WorkerSelection {
            schema_version: 1,
            schema_id: "rch-xcode/worker_selection@1".to_string(),
            created_at: Utc::now(),
            run_id: "".to_string(), // Will be set by run builder
            negotiated_protocol_version: 1,
            worker_protocol_range: crate::selection::ProtocolRange { min: 1, max: 1 },
            selected_worker: worker.name.clone(),
            selected_worker_host: worker.host.clone(),
            selection_mode: crate::selection::SelectionMode::Deterministic,
            candidate_count: candidates.len() as u32,
            probe_failures: vec![],
            snapshot_age_seconds: 0,
            snapshot_source: crate::selection::SnapshotSource::Fresh,
        };

        self.worker_selection = Some(selection);
        Ok(())
    }

    /// Write run_plan.json
    fn write_run_plan(&self, run_dir: &Path) -> PipelineResult<()> {
        let run_plan = self.run_plan.as_ref()
            .ok_or_else(|| PipelineError::Run(crate::run::RunError::NoActions))?;

        let json = serde_json::to_string_pretty(run_plan)?;
        let path = run_dir.join("run_plan.json");
        fs::write(&path, &json)?;

        if self.config.verbose {
            eprintln!("Wrote: {}", path.display());
        }

        Ok(())
    }

    /// Create source bundle
    fn create_source_bundle(&mut self, run_dir: &Path) -> PipelineResult<()> {
        // Determine source directory (current directory by default)
        let source_dir = std::env::current_dir()?;

        // Get config bundle.max_bytes (0 = no limit from config)
        let config_max_bytes = self.repo_config
            .as_ref()
            .map(|c| c.bundle.max_bytes)
            .unwrap_or(0);

        // Get worker capabilities.max_upload_bytes if available (0 = no limit from worker)
        let worker_max_upload_bytes = self.capabilities
            .as_ref()
            .and_then(|c| c.get("max_upload_bytes"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Compute effective limit: min of both where 0 means no limit
        let effective_limit = BundleResult::effective_limit(config_max_bytes, worker_max_upload_bytes);

        // Create bundler with size limit
        let mut bundler = Bundler::new(source_dir.clone())
            .with_mode(BundleMode::Worktree);

        if effective_limit > 0 {
            bundler = bundler.with_max_bytes(effective_limit);
        }

        // Get run_id from the plan
        let run_plan = self.run_plan.as_ref()
            .ok_or_else(|| PipelineError::Run(crate::run::RunError::NoActions))?;

        // Create bundle (will fail with SizeExceeded if over limit)
        let result = bundler.create_bundle(&run_plan.run_id)?;

        // Write tar to file
        let bundle_path = run_dir.join("source.tar");
        result.write_tar(&bundle_path)?;

        self.source_manifest = Some(result.manifest.clone());
        self.source_sha256 = Some(result.source_sha256.clone());
        self.bundle_path = Some(bundle_path.clone());

        // Write source_manifest.json
        let manifest_json = serde_json::to_string_pretty(&result.manifest)?;
        fs::write(run_dir.join("source_manifest.json"), &manifest_json)?;

        if self.config.verbose {
            eprintln!("Bundle created: {} ({} files, {} bytes)",
                bundle_path.display(),
                result.manifest.entries.len(),
                result.tar_bytes.len()
            );
        }

        Ok(())
    }

    /// Connect to worker
    fn connect_to_worker(&mut self) -> PipelineResult<()> {
        let selection = self.worker_selection.as_ref()
            .ok_or_else(|| PipelineError::NoWorkers)?;

        // Load inventory to get full worker details
        let inventory = match &self.config.inventory_path {
            Some(path) => WorkerInventory::load(path),
            None => WorkerInventory::load_default(),
        }.map_err(|e| PipelineError::NoWorkers)?;

        let worker = inventory.get(&selection.selected_worker)
            .ok_or_else(|| PipelineError::NoWorkers)?;

        // Create SSH transport
        let ssh_config = SshConfig {
            host: worker.host.clone(),
            user: worker.user.clone(),
            port: worker.port,
            key_path: worker.ssh_key_path.clone(),
            connect_timeout_seconds: 30,
            server_alive_interval: 15,
            server_alive_count_max: 2,
        };
        let transport = SshTransport::new(ssh_config);

        let mut client = RpcClient::new(Arc::new(transport));

        // Probe to negotiate protocol
        let capabilities = client.probe()?;
        self.capabilities = Some(capabilities);
        self.rpc_client = Some(client);

        Ok(())
    }

    /// Execute the run
    fn execute_run(&mut self, run_dir: &Path, start_time: Instant) -> PipelineResult<RunSummary> {
        let run_plan = self.run_plan.take()
            .ok_or_else(|| PipelineError::Run(crate::run::RunError::NoActions))?;
        let source_sha256 = self.source_sha256.as_ref()
            .ok_or_else(|| PipelineError::Bundling(BundleError::IoError(
                std::io::Error::new(std::io::ErrorKind::NotFound, "No source bundle")
            )))?
            .clone();

        // Write initial run_state.json
        let mut run_state = RunStateData::new(run_plan.run_id.clone());
        run_state.transition(RunState::Running)?;
        self.write_run_state(run_dir, &run_state)?;

        let mut execution = RunExecution::new(run_plan.clone());
        let mut step_summaries = Vec::new();

        // Check if source exists on worker, upload if needed
        self.ensure_source_on_worker(&source_sha256)?;

        // Execute each step
        while let Some(step) = execution.next_step() {
            // Clone step to release the borrow on execution
            let step = step.clone();

            if step.rejected {
                // Step was rejected by classifier - skip it
                let result = StepResult {
                    step: step.clone(),
                    status: Status::Rejected,
                    skipped: true,
                };
                execution.record_result(result);

                let run_id = run_plan.run_id.clone();
                step_summaries.push(JobSummary::rejected(
                    run_id,
                    step.job_id.clone(),
                    "Rejected by classifier".to_string(),
                ));

                continue;
            }

            // Execute step
            let step_dir = run_dir.join("steps").join(step.action.to_string()).join(&step.job_id);
            fs::create_dir_all(&step_dir)?;

            if self.config.verbose {
                eprintln!("Executing step {}: {} (job {})", step.index, step.action, step.job_id);
            }

            match self.execute_step(&step, &step_dir, &source_sha256, &run_plan.run_id) {
                Ok(summary) => {
                    let status = summary.status;
                    step_summaries.push(summary);

                    let result = StepResult {
                        step: step.clone(),
                        status,
                        skipped: false,
                    };
                    execution.record_result(result);
                }
                Err(e) => {
                    use crate::summary::Backend;
                    let summary = JobSummary::failure(
                        run_plan.run_id.clone(),
                        step.job_id.clone(),
                        format!("key-{}", step.job_id), // Simplified job_key
                        Backend::Xcodebuild,
                        FailureKind::Executor,
                        None,
                        e.to_string(),
                        0,
                    );
                    step_summaries.push(summary);

                    let result = StepResult {
                        step: step.clone(),
                        status: Status::Failed,
                        skipped: false,
                    };
                    execution.record_result(result);
                }
            }

            // Check if we should continue
            if !execution.should_continue() {
                break;
            }

            // Check timeout
            if start_time.elapsed().as_secs() > self.config.overall_timeout_seconds {
                return Err(PipelineError::Timeout);
            }
        }

        // Update run state based on execution result
        let final_state = match execution.state() {
            ExecutionState::Succeeded => RunState::Succeeded,
            ExecutionState::Failed => RunState::Failed,
            ExecutionState::Cancelled => RunState::Cancelled,
            _ => RunState::Failed,
        };
        run_state.transition(final_state)?;
        self.write_run_state(run_dir, &run_state)?;

        // Create run summary
        let summary = RunSummary::from_job_summaries(
            run_plan.run_id.clone(),
            &step_summaries,
            start_time.elapsed().as_millis() as u64,
        );

        // Write run_summary.json
        let summary_json = serde_json::to_string_pretty(&summary)?;
        fs::write(run_dir.join("run_summary.json"), &summary_json)?;

        Ok(summary)
    }

    /// Ensure source is on worker
    fn ensure_source_on_worker(&mut self, source_sha256: &str) -> PipelineResult<()> {
        let client = self.rpc_client.as_ref()
            .ok_or_else(|| PipelineError::Transport(TransportError::ConnectionFailed("No connection".to_string())))?;

        // Check if source exists
        if !client.has_source(source_sha256)? {
            // Upload source
            if self.config.verbose {
                eprintln!("Uploading source bundle...");
            }

            let bundle_path = self.bundle_path.as_ref()
                .ok_or_else(|| PipelineError::Bundling(BundleError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, "No bundle path"))))?;

            let content = fs::read(bundle_path)?;
            client.upload_source(source_sha256, &content)?;

            if self.config.verbose {
                eprintln!("Source bundle uploaded.");
            }
        }

        Ok(())
    }

    /// Execute a single step
    fn execute_step(&self, step: &PlanStep, step_dir: &Path, source_sha256: &str, run_id: &str) -> PipelineResult<JobSummary> {
        let client = self.rpc_client.as_ref()
            .ok_or_else(|| PipelineError::Transport(TransportError::ConnectionFailed("No connection".to_string())))?;

        // Submit job
        let submit_response = client.submit(
            &step.job_id,
            &format!("key-{}", &step.job_id), // Simplified job_key
            source_sha256,
            None, // No lease
        )?;

        if self.config.verbose {
            eprintln!("  Job submitted: {} (state: {})", submit_response.job_id, submit_response.state);
        }

        // Stream logs while job runs
        let capabilities = self.capabilities.as_ref();
        let has_tail = capabilities.map(|c| has_tail_feature(c)).unwrap_or(false);

        let streamer_config = LogStreamerConfig {
            poll_interval: self.config.tail_poll_interval,
            max_bytes_per_request: None,
            max_events_per_request: None,
        };
        let mut streamer = LogStreamer::new(streamer_config, has_tail);

        let start = Instant::now();

        loop {
            // Check status
            let status = client.status(&step.job_id)?;

            if is_terminal_state(&status.state) {
                // Job completed
                if self.config.verbose {
                    eprintln!("  Job completed: {} (state: {})", step.job_id, status.state);
                }

                use crate::summary::Backend;
                let duration_ms = start.elapsed().as_millis() as u64;
                let job_key = format!("key-{}", step.job_id);

                let mut summary = match status.state.as_str() {
                    "SUCCEEDED" => JobSummary::success(
                        run_id.to_string(),
                        step.job_id.clone(),
                        job_key.clone(),
                        Backend::Xcodebuild,
                        duration_ms,
                    ),
                    "CANCELLED" => JobSummary::cancelled(
                        run_id.to_string(),
                        step.job_id.clone(),
                        job_key.clone(),
                        Backend::Xcodebuild,
                        duration_ms,
                    ),
                    _ => JobSummary::failure(
                        run_id.to_string(),
                        step.job_id.clone(),
                        job_key.clone(),
                        Backend::Xcodebuild,
                        FailureKind::Xcodebuild,
                        None,
                        format!("{} failed", step.action),
                        duration_ms,
                    ),
                };

                // Fetch artifacts if available and verify integrity
                if status.artifacts_available {
                    if let Some(integrity_errors) = self.fetch_artifacts(&step.job_id, step_dir)? {
                        // Artifact integrity verification failed per rch-mac-y6s.2
                        // Override summary with ARTIFACTS/INTEGRITY_MISMATCH failure
                        summary = JobSummary::failure(
                            run_id.to_string(),
                            step.job_id.clone(),
                            job_key,
                            Backend::Xcodebuild,
                            FailureKind::Artifacts,
                            Some(FailureSubkind::IntegrityMismatch),
                            "Artifact integrity verification failed".to_string(),
                            duration_ms,
                        ).with_integrity_errors(integrity_errors);
                    }
                }

                return Ok(summary);
            }

            // Not terminal - stream logs if supported
            if has_tail {
                let (cursor, max_bytes, max_events) = streamer.tail_request_params();
                let cursor_u64 = cursor.and_then(|c| c.parse().ok());

                if let Ok(tail_response) = client.tail(&step.job_id, cursor_u64, max_bytes) {
                    let log_chunk = if !tail_response.entries.is_empty() {
                        Some(tail_response.entries.iter()
                            .map(|e| e.line.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"))
                    } else {
                        None
                    };

                    let next_cursor = tail_response.next_cursor.map(|c| c.to_string());

                    let update = streamer.process_tail_response(next_cursor, log_chunk.clone(), vec![]);

                    // Print logs to stdout
                    if let Some(chunk) = log_chunk {
                        print!("{}", chunk);
                        io::stdout().flush()?;
                    }
                }
            }

            // Check idle timeout
            if streamer.time_since_activity() > Duration::from_secs(self.config.idle_timeout_seconds) {
                // Cancel job due to idle timeout
                client.cancel(&step.job_id, Some(CancelReason::TimeoutIdle))?;
                return Err(PipelineError::Timeout);
            }

            // Check overall timeout
            if start.elapsed().as_secs() > self.config.overall_timeout_seconds {
                client.cancel(&step.job_id, Some(CancelReason::TimeoutOverall))?;
                return Err(PipelineError::Timeout);
            }

            // Wait before next poll
            std::thread::sleep(self.config.tail_poll_interval);
        }
    }

    /// Fetch artifacts for a job and verify integrity
    ///
    /// Returns Ok(None) if no errors, Ok(Some(errors)) if integrity errors found.
    /// Per rch-mac-y6s.2: After fetching job artifacts, host MUST:
    /// 1) Recompute artifact_root_sha256 from fetched manifest.json entries and verify match
    /// 2) Verify sha256 and size for every entry against fetched files
    /// 3) Verify no extra files exist beyond manifest entries plus the excluded triple
    fn fetch_artifacts(&self, job_id: &str, step_dir: &Path) -> PipelineResult<Option<Vec<String>>> {
        let client = self.rpc_client.as_ref()
            .ok_or_else(|| PipelineError::Transport(TransportError::ConnectionFailed("No connection".to_string())))?;

        let fetch_response = client.fetch(job_id)?;

        if let Some(content) = fetch_response.content {
            // Extract artifacts (assuming tar format)
            let artifacts_path = step_dir.join("artifacts.tar");
            fs::write(&artifacts_path, &content)?;

            // Extract
            let status = std::process::Command::new("tar")
                .args(["-xf", artifacts_path.to_str().unwrap()])
                .current_dir(step_dir)
                .status()?;

            if status.success() {
                fs::remove_file(&artifacts_path)?;
            }

            // Verify artifact integrity per rch-mac-y6s.2
            // Uses the verify_artifacts function which performs:
            // 1) Recompute artifact_root_sha256 and verify match
            // 2) Verify sha256 and size for every entry
            // 3) Check for extra files beyond manifest + excluded triple
            let manifest_path = step_dir.join("manifest.json");
            if manifest_path.exists() {
                match verify_artifacts(step_dir) {
                    Ok(result) => {
                        if !result.passed {
                            if let Some((_, _, errors)) = result.failure_info() {
                                return Ok(Some(errors));
                            }
                        }
                    }
                    Err(e) => {
                        return Ok(Some(vec![format!("Failed to verify artifacts: {}", e)]));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Write run_state.json
    fn write_run_state(&self, run_dir: &Path, state: &RunStateData) -> PipelineResult<()> {
        let json = serde_json::to_string_pretty(state)?;
        fs::write(run_dir.join("run_state.json"), &json)?;
        Ok(())
    }
}

/// Tail command execution
pub fn execute_tail(
    id: &str,
    inventory_path: Option<PathBuf>,
    follow: bool,
    verbose: bool,
) -> PipelineResult<()> {
    // Determine if this is a run_id or job_id
    // For now, treat it as a job_id

    // Load inventory
    let inventory = match &inventory_path {
        Some(path) => WorkerInventory::load(path),
        None => WorkerInventory::load_default(),
    }.map_err(|_| PipelineError::NoWorkers)?;

    // Get first available worker (simplified - should look up from run artifacts)
    let candidates: Vec<_> = inventory.filter_by_tags(&["macos", "xcode"]);
    if candidates.is_empty() {
        return Err(PipelineError::NoWorkers);
    }
    let worker = candidates[0];

    // Connect to worker
    let ssh_config = SshConfig {
        host: worker.host.clone(),
        user: worker.user.clone(),
        port: worker.port,
        key_path: worker.ssh_key_path.clone(),
        connect_timeout_seconds: 30,
        server_alive_interval: 15,
        server_alive_count_max: 2,
    };
    let transport = SshTransport::new(ssh_config);

    let mut client = RpcClient::new(Arc::new(transport));
    let capabilities = client.probe()?;

    let has_tail = has_tail_feature(&capabilities);
    if !has_tail {
        eprintln!("Warning: Worker does not support tail feature, using status polling");
    }

    let config = LogStreamerConfig::default();
    let mut streamer = LogStreamer::new(config, has_tail);

    loop {
        if has_tail {
            let (cursor, max_bytes, _) = streamer.tail_request_params();
            let cursor_u64 = cursor.and_then(|c| c.parse().ok());

            match client.tail(id, cursor_u64, max_bytes) {
                Ok(response) => {
                    // Print logs
                    for entry in &response.entries {
                        println!("{}", entry.line);
                    }

                    let log_chunk = if !response.entries.is_empty() {
                        Some(response.entries.iter()
                            .map(|e| e.line.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"))
                    } else {
                        None
                    };

                    let next_cursor = response.next_cursor.map(|c| c.to_string());
                    let update = streamer.process_tail_response(next_cursor, log_chunk, vec![]);

                    if update.complete {
                        if verbose {
                            eprintln!("[tail complete]");
                        }
                        if !follow {
                            break;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Tail error: {}", e);
                    break;
                }
            }
        } else {
            // Fallback to status check
            match client.status(id) {
                Ok(status) => {
                    let update = streamer.process_status_response(&status.state, is_terminal_state(&status.state));
                    if update.complete && !follow {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Status error: {}", e);
                    break;
                }
            }
        }

        std::thread::sleep(streamer.poll_interval());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert_eq!(config.overall_timeout_seconds, 1800);
        assert_eq!(config.idle_timeout_seconds, 300);
        assert!(!config.continue_on_failure);
    }

    #[test]
    fn test_pipeline_error_exit_codes() {
        assert_eq!(PipelineError::Cancelled.exit_code(), 80);
        assert_eq!(PipelineError::Timeout.exit_code(), 80);
        assert_eq!(PipelineError::NoWorkers.exit_code(), 91);
        assert_eq!(PipelineError::AllRejected.exit_code(), 10);
    }
}
