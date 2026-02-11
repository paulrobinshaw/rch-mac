//! xcodebuild backend executor for the RCH worker.
//!
//! Implements the execution logic per bead axz.10. This module handles:
//! - Creating isolated working directories per job
//! - Extracting source bundles from the content-addressed store
//! - Constructing xcodebuild commands from job.json
//! - Executing xcodebuild with streaming log capture
//! - Writing normative artifacts (summary.json, toolchain.json, etc.)
//! - Handling cancellation via SIGTERM

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Environment variable allowlist - only these are passed to xcodebuild.
/// Per bead axz.10: drop-by-default, pass only known-safe env vars.
pub const ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "DEVELOPER_DIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "USER",
    "LOGNAME",
];

/// Schema version for toolchain.json
pub const TOOLCHAIN_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for toolchain.json
pub const TOOLCHAIN_SCHEMA_ID: &str = "rch-xcode/toolchain@1";

/// Schema version for destination.json
pub const DESTINATION_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for destination.json
pub const DESTINATION_SCHEMA_ID: &str = "rch-xcode/destination@1";

/// Schema version for effective_config.json
pub const EFFECTIVE_CONFIG_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for effective_config.json
pub const EFFECTIVE_CONFIG_SCHEMA_ID: &str = "rch-xcode/effective_config@1";

/// Errors from executor operations.
#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("source bundle not found: {0}")]
    SourceNotFound(String),

    #[error("extraction failed: {0}")]
    ExtractionFailed(String),

    #[error("invalid job.json: {0}")]
    InvalidJob(String),

    #[error("xcodebuild failed to start: {0}")]
    SpawnFailed(String),

    #[error("cancelled")]
    Cancelled,

    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Result type for executor operations.
pub type ExecutorResult<T> = Result<T, ExecutorError>;

/// Configuration for a job execution.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Root directory for job working directories.
    pub jobs_root: PathBuf,
    /// Root directory for the source store.
    pub source_store_root: PathBuf,
    /// Shared DerivedData cache path (for shared mode).
    pub shared_derived_data: Option<PathBuf>,
    /// Grace period in seconds for SIGTERM before SIGKILL.
    pub termination_grace_seconds: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            jobs_root: PathBuf::from("/var/lib/rch/jobs"),
            source_store_root: PathBuf::from("/var/lib/rch/sources"),
            shared_derived_data: Some(PathBuf::from("/var/lib/rch/DerivedData")),
            termination_grace_seconds: 10,
        }
    }
}

/// Job specification from the host (subset of fields needed for execution).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInput {
    /// Run identifier
    pub run_id: String,
    /// Job identifier
    pub job_id: String,
    /// Action: "build" or "test"
    pub action: String,
    /// Job key (SHA-256 hash)
    pub job_key: String,
    /// Job key inputs (contains sanitized_argv, toolchain, destination, source_sha256)
    pub job_key_inputs: JobKeyInputs,
    /// Effective config from host
    #[serde(default)]
    pub effective_config: Option<serde_json::Value>,
    /// Original destination constraint string
    #[serde(default)]
    pub original_constraint: Option<String>,
}

/// Job key inputs structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobKeyInputs {
    /// SHA-256 of the source bundle
    pub source_sha256: String,
    /// Sanitized xcodebuild argv (action is first element)
    pub sanitized_argv: Vec<String>,
    /// Toolchain identity
    pub toolchain: ToolchainInput,
    /// Destination identity
    pub destination: DestinationInput,
}

/// Toolchain input from job_key_inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainInput {
    pub xcode_build: String,
    pub developer_dir: String,
    pub macos_version: String,
    pub macos_build: String,
    pub arch: String,
}

/// Destination input from job_key_inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationInput {
    pub platform: String,
    pub name: String,
    pub os_version: String,
    pub provisioning: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_runtime_build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type_identifier: Option<String>,
}

/// Result of a job execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// Job status
    pub status: ExecutionStatus,
    /// Exit code (stable exit code per PLAN.md)
    pub exit_code: i32,
    /// Backend (xcodebuild) exit code
    pub backend_exit_code: Option<i32>,
    /// Termination signal if killed
    pub backend_term_signal: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Human-readable summary
    pub human_summary: String,
    /// Failure kind (if failed)
    pub failure_kind: Option<String>,
    /// Failure subkind (if applicable)
    pub failure_subkind: Option<String>,
}

/// Execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Success,
    Failed,
    Cancelled,
}

/// The xcodebuild executor.
pub struct Executor {
    config: ExecutorConfig,
    /// Cancellation flag
    cancelled: Arc<AtomicBool>,
}

impl Executor {
    /// Create a new executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        Self {
            config,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get a cancellation flag that can be shared.
    pub fn cancellation_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Request cancellation.
    pub fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Execute a job.
    ///
    /// This is the main entry point for job execution:
    /// 1. Create isolated working directory
    /// 2. Extract source bundle
    /// 3. Construct xcodebuild command
    /// 4. Execute with log streaming
    /// 5. Write artifacts
    pub fn execute(&self, job: &JobInput) -> ExecutorResult<ExecutionResult> {
        let start_time = Instant::now();

        // Create job directories
        let job_dir = self.config.jobs_root.join(&job.job_id);
        let work_dir = job_dir.join("work");
        let artifact_dir = job_dir.join("artifacts");

        fs::create_dir_all(&work_dir)?;
        fs::create_dir_all(&artifact_dir)?;

        // Check for cancellation
        if self.is_cancelled() {
            return Ok(self.make_cancelled_result(start_time.elapsed()));
        }

        // Extract source bundle
        let source_sha256 = &job.job_key_inputs.source_sha256;
        if let Err(e) = self.extract_source(source_sha256, &work_dir) {
            let duration_ms = start_time.elapsed().as_millis() as u64;
            self.write_failure_summary(
                &artifact_dir,
                job,
                "TRANSFER",
                Some("EXTRACTION_FAILED"),
                &format!("Source extraction failed: {}", e),
                duration_ms,
                None,
                None,
            )?;
            return Ok(ExecutionResult {
                status: ExecutionStatus::Failed,
                exit_code: 30, // TRANSFER
                backend_exit_code: None,
                backend_term_signal: None,
                duration_ms,
                human_summary: format!("Source extraction failed: {}", e),
                failure_kind: Some("TRANSFER".to_string()),
                failure_subkind: Some("EXTRACTION_FAILED".to_string()),
            });
        }

        // Check for cancellation
        if self.is_cancelled() {
            let duration_ms = start_time.elapsed().as_millis() as u64;
            self.write_cancelled_summary(&artifact_dir, job, duration_ms)?;
            return Ok(self.make_cancelled_result(start_time.elapsed()));
        }

        // Build xcodebuild command
        let (cmd, args) = self.build_xcodebuild_command(job, &artifact_dir)?;

        // Build environment
        let env = self.build_environment(&job.job_key_inputs.toolchain);

        // Open build.log for streaming
        let log_path = artifact_dir.join("build.log");
        let log_file = File::create(&log_path)?;
        let mut log_writer = BufWriter::new(log_file);

        // Log command line
        writeln!(log_writer, "=== RCH Xcode Lane - Job {} ===", job.job_id)?;
        writeln!(log_writer, "run_id: {}", job.run_id)?;
        writeln!(log_writer, "job_key: {}", job.job_key)?;
        writeln!(log_writer, "action: {}", job.action)?;
        writeln!(log_writer, "command: {} {}", cmd, args.join(" "))?;
        writeln!(log_writer, "working_dir: {}", work_dir.display())?;
        writeln!(log_writer, "started_at: {}", Utc::now().to_rfc3339())?;
        writeln!(log_writer, "=== Begin xcodebuild output ===")?;
        log_writer.flush()?;

        // Execute xcodebuild
        let exec_result = self.run_xcodebuild(
            &cmd,
            &args,
            &work_dir,
            &env,
            &log_path,
            start_time,
        );

        // Write end marker to log
        {
            let mut log_file = fs::OpenOptions::new().append(true).open(&log_path)?;
            writeln!(log_file, "=== End xcodebuild output ===")?;
            writeln!(log_file, "ended_at: {}", Utc::now().to_rfc3339())?;
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Process result and write artifacts
        let result = match exec_result {
            Ok((status, backend_exit_code, term_signal)) => {
                self.process_execution_result(
                    &artifact_dir,
                    job,
                    status,
                    backend_exit_code,
                    term_signal,
                    duration_ms,
                )?
            }
            Err(e) => {
                self.write_failure_summary(
                    &artifact_dir,
                    job,
                    "EXECUTOR",
                    None,
                    &format!("Execution failed: {}", e),
                    duration_ms,
                    None,
                    None,
                )?;
                ExecutionResult {
                    status: ExecutionStatus::Failed,
                    exit_code: 40, // EXECUTOR
                    backend_exit_code: None,
                    backend_term_signal: None,
                    duration_ms,
                    human_summary: format!("Execution failed: {}", e),
                    failure_kind: Some("EXECUTOR".to_string()),
                    failure_subkind: None,
                }
            }
        };

        // Write additional artifacts
        self.write_toolchain_json(&artifact_dir, job)?;
        self.write_destination_json(&artifact_dir, job)?;
        if let Some(ref config) = job.effective_config {
            self.write_effective_config_json(&artifact_dir, job, config)?;
        }

        Ok(result)
    }

    /// Extract source bundle to working directory.
    fn extract_source(&self, source_sha256: &str, work_dir: &Path) -> ExecutorResult<()> {
        // Build path to bundle in the source store
        let prefix = &source_sha256[..2.min(source_sha256.len())];
        let bundle_path = self.config.source_store_root
            .join(prefix)
            .join(source_sha256)
            .join("bundle.tar");

        if !bundle_path.exists() {
            return Err(ExecutorError::SourceNotFound(source_sha256.to_string()));
        }

        // Use tar to extract (PAX format)
        let status = Command::new("tar")
            .args(["-xf", bundle_path.to_str().unwrap()])
            .current_dir(work_dir)
            .status()
            .map_err(|e| ExecutorError::ExtractionFailed(e.to_string()))?;

        if !status.success() {
            return Err(ExecutorError::ExtractionFailed(format!(
                "tar exited with status: {:?}",
                status.code()
            )));
        }

        Ok(())
    }

    /// Build the xcodebuild command and arguments.
    fn build_xcodebuild_command(
        &self,
        job: &JobInput,
        artifact_dir: &Path,
    ) -> ExecutorResult<(String, Vec<String>)> {
        // Start with sanitized_argv which already has action as first element
        let mut args = job.job_key_inputs.sanitized_argv.clone();

        // For test jobs, inject -resultBundlePath
        if job.action == "test" {
            let xcresult_path = artifact_dir.join("result.xcresult");
            args.push("-resultBundlePath".to_string());
            args.push(xcresult_path.to_str().unwrap().to_string());
        }

        // Handle DerivedData based on config
        // Extract cache mode from effective_config if present
        let derived_data_mode = job.effective_config
            .as_ref()
            .and_then(|c| c.get("config"))
            .and_then(|c| c.get("cache"))
            .and_then(|c| c.get("derived_data"))
            .and_then(|v| v.as_str())
            .unwrap_or("shared");

        match derived_data_mode {
            "per_job" => {
                let dd_path = artifact_dir.join("DerivedData");
                args.push("-derivedDataPath".to_string());
                args.push(dd_path.to_str().unwrap().to_string());
            }
            "shared" => {
                if let Some(ref shared_path) = self.config.shared_derived_data {
                    args.push("-derivedDataPath".to_string());
                    args.push(shared_path.to_str().unwrap().to_string());
                }
            }
            "off" => {
                // Let Xcode use its default
            }
            _ => {
                // Unknown mode, default to shared
                if let Some(ref shared_path) = self.config.shared_derived_data {
                    args.push("-derivedDataPath".to_string());
                    args.push(shared_path.to_str().unwrap().to_string());
                }
            }
        }

        Ok(("xcodebuild".to_string(), args))
    }

    /// Build the environment for xcodebuild.
    fn build_environment(&self, toolchain: &ToolchainInput) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Set DEVELOPER_DIR from toolchain
        env.insert("DEVELOPER_DIR".to_string(), toolchain.developer_dir.clone());

        // Copy allowed environment variables from current process
        for key in ENV_ALLOWLIST {
            if *key == "DEVELOPER_DIR" {
                continue; // Already set from toolchain
            }
            if let Ok(value) = std::env::var(key) {
                env.insert(key.to_string(), value);
            }
        }

        // Log dropped variables (keys only)
        let current_vars: Vec<_> = std::env::vars()
            .filter(|(k, _)| !ENV_ALLOWLIST.contains(&k.as_str()))
            .map(|(k, _)| k)
            .collect();

        if !current_vars.is_empty() {
            eprintln!("[executor] Dropped environment variables: {}", current_vars.join(", "));
        }

        env
    }

    /// Run xcodebuild with streaming output.
    fn run_xcodebuild(
        &self,
        cmd: &str,
        args: &[String],
        work_dir: &Path,
        env: &HashMap<String, String>,
        log_path: &Path,
        _start_time: Instant,
    ) -> ExecutorResult<(ExitStatus, Option<i32>, Option<String>)> {
        // Clear environment and set only allowed vars
        let mut command = Command::new(cmd);
        command
            .args(args)
            .current_dir(work_dir)
            .env_clear()
            .envs(env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()
            .map_err(|e| ExecutorError::SpawnFailed(e.to_string()))?;

        // Stream output to log file
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Open log file for appending
        let log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let log_file = Arc::new(std::sync::Mutex::new(log_file));

        // Stream stdout
        let log_clone = Arc::clone(&log_file);
        let stdout_handle = std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        if let Ok(mut f) = log_clone.lock() {
                            let _ = writeln!(f, "{}", line);
                        }
                    }
                }
            }
        });

        // Stream stderr
        let log_clone = Arc::clone(&log_file);
        let stderr_handle = std::thread::spawn(move || {
            if let Some(stderr) = stderr {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        if let Ok(mut f) = log_clone.lock() {
                            let _ = writeln!(f, "[stderr] {}", line);
                        }
                    }
                }
            }
        });

        // Monitor for cancellation while waiting
        let status = loop {
            if self.is_cancelled() {
                // Try graceful termination first
                self.terminate_child(&mut child)?;
                return Ok((
                    child.wait()?,
                    None,
                    Some("SIGTERM".to_string()),
                ));
            }

            // Check if process has exited
            match child.try_wait()? {
                Some(status) => break status,
                None => {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        };

        // Wait for output streaming threads
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

        let backend_exit_code = status.code();
        let term_signal = if !status.success() && backend_exit_code.is_none() {
            // Process was killed by signal
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                status.signal().map(|s| format!("SIG{}", s))
            }
            #[cfg(not(unix))]
            {
                None
            }
        } else {
            None
        };

        Ok((status, backend_exit_code, term_signal))
    }

    /// Terminate a child process gracefully then forcefully.
    fn terminate_child(&self, child: &mut Child) -> ExecutorResult<()> {
        // Send SIGTERM
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;

            let pid = Pid::from_raw(child.id() as i32);
            let _ = signal::kill(pid, Signal::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }

        // Wait for grace period
        let grace_duration = Duration::from_secs(self.config.termination_grace_seconds);
        let start = Instant::now();

        while start.elapsed() < grace_duration {
            match child.try_wait()? {
                Some(_) => return Ok(()),
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }

        // Force kill
        let _ = child.kill();
        let _ = child.wait();

        Ok(())
    }

    /// Process execution result and write summary.
    fn process_execution_result(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        status: ExitStatus,
        backend_exit_code: Option<i32>,
        term_signal: Option<String>,
        duration_ms: u64,
    ) -> ExecutorResult<ExecutionResult> {
        if self.is_cancelled() {
            self.write_cancelled_summary(artifact_dir, job, duration_ms)?;
            return Ok(ExecutionResult {
                status: ExecutionStatus::Cancelled,
                exit_code: 80,
                backend_exit_code,
                backend_term_signal: term_signal,
                duration_ms,
                human_summary: "Job cancelled".to_string(),
                failure_kind: Some("CANCELLED".to_string()),
                failure_subkind: None,
            });
        }

        if status.success() {
            self.write_success_summary(artifact_dir, job, duration_ms)?;
            Ok(ExecutionResult {
                status: ExecutionStatus::Success,
                exit_code: 0,
                backend_exit_code: Some(0),
                backend_term_signal: None,
                duration_ms,
                human_summary: format!("{} succeeded", job.action),
                failure_kind: None,
                failure_subkind: None,
            })
        } else {
            let human_summary = format!(
                "xcodebuild {} failed with exit code {}",
                job.action,
                backend_exit_code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string())
            );

            self.write_failure_summary(
                artifact_dir,
                job,
                "XCODEBUILD",
                None,
                &human_summary,
                duration_ms,
                backend_exit_code,
                term_signal.as_deref(),
            )?;

            Ok(ExecutionResult {
                status: ExecutionStatus::Failed,
                exit_code: 50, // XCODEBUILD_FAILED
                backend_exit_code,
                backend_term_signal: term_signal,
                duration_ms,
                human_summary,
                failure_kind: Some("XCODEBUILD".to_string()),
                failure_subkind: None,
            })
        }
    }

    /// Write success summary.json
    fn write_success_summary(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        duration_ms: u64,
    ) -> ExecutorResult<()> {
        let summary = serde_json::json!({
            "schema_version": 1,
            "schema_id": "rch-xcode/summary@1",
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "status": "success",
            "exit_code": 0,
            "backend_exit_code": 0,
            "backend": "xcodebuild",
            "human_summary": format!("{} succeeded", job.action),
            "duration_ms": duration_ms,
            "artifact_profile": "minimal"
        });

        self.write_json_artifact(artifact_dir, "summary.json", &summary)
    }

    /// Write failure summary.json
    fn write_failure_summary(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        failure_kind: &str,
        failure_subkind: Option<&str>,
        human_summary: &str,
        duration_ms: u64,
        backend_exit_code: Option<i32>,
        backend_term_signal: Option<&str>,
    ) -> ExecutorResult<()> {
        let exit_code = match failure_kind {
            "CLASSIFIER_REJECTED" => 10,
            "SSH" => 20,
            "TRANSFER" => 30,
            "EXECUTOR" => 40,
            "XCODEBUILD" => 50,
            "MCP" => 60,
            "ARTIFACTS" => 70,
            "CANCELLED" => 80,
            "WORKER_BUSY" => 90,
            "WORKER_INCOMPATIBLE" => 91,
            "BUNDLER" => 92,
            "ATTESTATION" => 93,
            _ => 40,
        };

        let mut summary = serde_json::json!({
            "schema_version": 1,
            "schema_id": "rch-xcode/summary@1",
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "status": "failed",
            "failure_kind": failure_kind,
            "exit_code": exit_code,
            "backend": "xcodebuild",
            "human_summary": human_summary,
            "duration_ms": duration_ms,
            "artifact_profile": "minimal"
        });

        if let Some(subkind) = failure_subkind {
            summary["failure_subkind"] = serde_json::json!(subkind);
        }
        if let Some(code) = backend_exit_code {
            summary["backend_exit_code"] = serde_json::json!(code);
        }
        if let Some(signal) = backend_term_signal {
            summary["backend_term_signal"] = serde_json::json!(signal);
        }

        self.write_json_artifact(artifact_dir, "summary.json", &summary)
    }

    /// Write cancelled summary.json
    fn write_cancelled_summary(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        duration_ms: u64,
    ) -> ExecutorResult<()> {
        let summary = serde_json::json!({
            "schema_version": 1,
            "schema_id": "rch-xcode/summary@1",
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "status": "cancelled",
            "failure_kind": "CANCELLED",
            "exit_code": 80,
            "backend": "xcodebuild",
            "human_summary": "Job cancelled",
            "duration_ms": duration_ms,
            "artifact_profile": "minimal"
        });

        self.write_json_artifact(artifact_dir, "summary.json", &summary)
    }

    /// Write toolchain.json artifact.
    fn write_toolchain_json(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
    ) -> ExecutorResult<()> {
        let toolchain = &job.job_key_inputs.toolchain;
        let json = serde_json::json!({
            "schema_version": TOOLCHAIN_SCHEMA_VERSION,
            "schema_id": TOOLCHAIN_SCHEMA_ID,
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "xcode_build": toolchain.xcode_build,
            "xcode_version": null, // Could be derived from xcode_build
            "developer_dir": toolchain.developer_dir,
            "macos_version": toolchain.macos_version,
            "macos_build": toolchain.macos_build,
            "arch": toolchain.arch
        });

        self.write_json_artifact(artifact_dir, "toolchain.json", &json)
    }

    /// Write destination.json artifact.
    fn write_destination_json(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
    ) -> ExecutorResult<()> {
        let dest = &job.job_key_inputs.destination;
        let mut json = serde_json::json!({
            "schema_version": DESTINATION_SCHEMA_VERSION,
            "schema_id": DESTINATION_SCHEMA_ID,
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "platform": dest.platform,
            "name": dest.name,
            "os_version": dest.os_version,
            "provisioning": dest.provisioning
        });

        if let Some(ref orig) = job.original_constraint {
            json["original_constraint"] = serde_json::json!(orig);
        }
        if let Some(ref id) = dest.sim_runtime_identifier {
            json["sim_runtime_identifier"] = serde_json::json!(id);
        }
        if let Some(ref build) = dest.sim_runtime_build {
            json["sim_runtime_build"] = serde_json::json!(build);
        }
        if let Some(ref dtype) = dest.device_type_identifier {
            json["device_type_identifier"] = serde_json::json!(dtype);
        }

        self.write_json_artifact(artifact_dir, "destination.json", &json)
    }

    /// Write effective_config.json artifact.
    fn write_effective_config_json(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        config: &serde_json::Value,
    ) -> ExecutorResult<()> {
        // The effective_config from job.json already has the right structure
        // Just add run_id, job_id, job_key if not present
        let mut json = config.clone();

        if let serde_json::Value::Object(ref mut map) = json {
            if !map.contains_key("run_id") {
                map.insert("run_id".to_string(), serde_json::json!(job.run_id));
            }
            if !map.contains_key("job_id") {
                map.insert("job_id".to_string(), serde_json::json!(job.job_id));
            }
            if !map.contains_key("job_key") {
                map.insert("job_key".to_string(), serde_json::json!(job.job_key));
            }
        }

        self.write_json_artifact(artifact_dir, "effective_config.json", &json)
    }

    /// Write a JSON artifact with atomic write-then-rename.
    fn write_json_artifact(
        &self,
        artifact_dir: &Path,
        filename: &str,
        value: &serde_json::Value,
    ) -> ExecutorResult<()> {
        let json = serde_json::to_string_pretty(value)
            .map_err(|e| ExecutorError::Serialization(e.to_string()))?;

        let final_path = artifact_dir.join(filename);
        let temp_path = artifact_dir.join(format!(".{}.tmp", filename));

        fs::write(&temp_path, &json)?;
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }

    /// Create a cancelled result.
    fn make_cancelled_result(&self, elapsed: Duration) -> ExecutionResult {
        ExecutionResult {
            status: ExecutionStatus::Cancelled,
            exit_code: 80,
            backend_exit_code: None,
            backend_term_signal: Some("SIGTERM".to_string()),
            duration_ms: elapsed.as_millis() as u64,
            human_summary: "Job cancelled".to_string(),
            failure_kind: Some("CANCELLED".to_string()),
            failure_subkind: None,
        }
    }

    /// Get the artifact directory for a job.
    pub fn artifact_dir(&self, job_id: &str) -> PathBuf {
        self.config.jobs_root.join(job_id).join("artifacts")
    }

    /// Clean up job directory.
    pub fn cleanup_job(&self, job_id: &str) -> ExecutorResult<()> {
        let job_dir = self.config.jobs_root.join(job_id);
        if job_dir.exists() {
            fs::remove_dir_all(&job_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a test job input with default values.
    fn make_test_job(job_id: &str, action: &str) -> JobInput {
        JobInput {
            run_id: "run-001".to_string(),
            job_id: job_id.to_string(),
            action: action.to_string(),
            job_key: "abc123def456789012345678901234567890123456789012345678901234".to_string(),
            job_key_inputs: JobKeyInputs {
                source_sha256: "source123456789012345678901234567890123456789012345678901234".to_string(),
                sanitized_argv: vec![
                    action.to_string(),
                    "-scheme".to_string(),
                    "MyApp".to_string(),
                    "-workspace".to_string(),
                    "MyApp.xcworkspace".to_string(),
                ],
                toolchain: ToolchainInput {
                    xcode_build: "16C5032a".to_string(),
                    developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
                    macos_version: "15.3".to_string(),
                    macos_build: "24D60".to_string(),
                    arch: "arm64".to_string(),
                },
                destination: DestinationInput {
                    platform: "iOS Simulator".to_string(),
                    name: "iPhone 16".to_string(),
                    os_version: "18.2".to_string(),
                    provisioning: "existing".to_string(),
                    sim_runtime_identifier: Some("com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string()),
                    sim_runtime_build: Some("22C150".to_string()),
                    device_type_identifier: Some("com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string()),
                },
            },
            effective_config: None,
            original_constraint: Some("platform=iOS Simulator,name=iPhone 16,OS=18.2".to_string()),
        }
    }

    /// Create an executor with temporary directories.
    fn make_test_executor(temp_dir: &TempDir) -> (Executor, ExecutorConfig) {
        let config = ExecutorConfig {
            jobs_root: temp_dir.path().join("jobs"),
            source_store_root: temp_dir.path().join("sources"),
            shared_derived_data: Some(temp_dir.path().join("DerivedData")),
            termination_grace_seconds: 10,
        };
        fs::create_dir_all(&config.jobs_root).unwrap();
        fs::create_dir_all(&config.source_store_root).unwrap();
        (Executor::new(config.clone()), config)
    }

    // Test 1: Environment allowlist
    #[test]
    fn test_env_allowlist() {
        // Verify expected env vars are in allowlist
        assert!(ENV_ALLOWLIST.contains(&"HOME"));
        assert!(ENV_ALLOWLIST.contains(&"PATH"));
        assert!(ENV_ALLOWLIST.contains(&"DEVELOPER_DIR"));
        assert!(ENV_ALLOWLIST.contains(&"TMPDIR"));
        // Verify potentially dangerous vars are NOT in allowlist
        assert!(!ENV_ALLOWLIST.contains(&"AWS_SECRET_ACCESS_KEY"));
        assert!(!ENV_ALLOWLIST.contains(&"PASSWORD"));
    }

    // Test 2: Executor config defaults
    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.termination_grace_seconds, 10);
        assert!(config.shared_derived_data.is_some());
    }

    // Test 3: Job input deserialization
    #[test]
    fn test_job_input_deserialization() {
        let json = r#"{
            "run_id": "run-001",
            "job_id": "job-001",
            "action": "build",
            "job_key": "abc123",
            "job_key_inputs": {
                "source_sha256": "def456",
                "sanitized_argv": ["build", "-scheme", "MyApp"],
                "toolchain": {
                    "xcode_build": "16C5032a",
                    "developer_dir": "/Applications/Xcode.app/Contents/Developer",
                    "macos_version": "15.3",
                    "macos_build": "24D60",
                    "arch": "arm64"
                },
                "destination": {
                    "platform": "iOS Simulator",
                    "name": "iPhone 16",
                    "os_version": "18.2",
                    "provisioning": "existing"
                }
            }
        }"#;

        let job: JobInput = serde_json::from_str(json).unwrap();
        assert_eq!(job.job_id, "job-001");
        assert_eq!(job.action, "build");
        assert_eq!(job.job_key_inputs.sanitized_argv[0], "build");
    }

    // Test 4: Build environment sets DEVELOPER_DIR
    #[test]
    fn test_build_environment() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);

        let toolchain = ToolchainInput {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        };

        let env = executor.build_environment(&toolchain);

        // DEVELOPER_DIR should be set from toolchain
        assert_eq!(
            env.get("DEVELOPER_DIR"),
            Some(&"/Applications/Xcode.app/Contents/Developer".to_string())
        );
    }

    // Test 5: xcodebuild command construction - no duplicate action
    #[test]
    fn test_build_command_no_duplicate_action() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (cmd, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        assert_eq!(cmd, "xcodebuild");
        // sanitized_argv already has "build" as first element
        // Command should NOT prepend action again
        assert_eq!(args[0], "build");
        // Count occurrences of "build" - should be exactly 1
        let build_count = args.iter().filter(|a| *a == "build").count();
        assert_eq!(build_count, 1, "Action should not be duplicated");
    }

    // Test 6: -resultBundlePath injection for test jobs
    #[test]
    fn test_result_bundle_path_injection_test_job() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "test");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        // Test job should have -resultBundlePath
        assert!(args.contains(&"-resultBundlePath".to_string()),
            "Test jobs should have -resultBundlePath injected");

        // Find the index and verify the path
        let idx = args.iter().position(|a| a == "-resultBundlePath").unwrap();
        assert!(args[idx + 1].ends_with("result.xcresult"),
            "resultBundlePath should point to result.xcresult");
    }

    // Test 7: -resultBundlePath NOT injected for build jobs
    #[test]
    fn test_no_result_bundle_path_for_build_job() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        // Build job should NOT have -resultBundlePath
        assert!(!args.contains(&"-resultBundlePath".to_string()),
            "Build jobs should NOT have -resultBundlePath");
    }

    // Test 8: -derivedDataPath per cache mode - shared
    #[test]
    fn test_derived_data_path_shared_mode() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_executor(&temp_dir);
        let mut job = make_test_job("job-001", "build");

        // Set cache mode to shared
        job.effective_config = Some(serde_json::json!({
            "config": {
                "cache": {
                    "derived_data": "shared"
                }
            }
        }));

        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        // Should use shared DerivedData path
        assert!(args.contains(&"-derivedDataPath".to_string()));
        let idx = args.iter().position(|a| a == "-derivedDataPath").unwrap();
        assert_eq!(args[idx + 1], config.shared_derived_data.unwrap().to_str().unwrap());
    }

    // Test 9: -derivedDataPath per cache mode - per_job
    #[test]
    fn test_derived_data_path_per_job_mode() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let mut job = make_test_job("job-001", "build");

        // Set cache mode to per_job
        job.effective_config = Some(serde_json::json!({
            "config": {
                "cache": {
                    "derived_data": "per_job"
                }
            }
        }));

        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        // Should use job-specific DerivedData path
        assert!(args.contains(&"-derivedDataPath".to_string()));
        let idx = args.iter().position(|a| a == "-derivedDataPath").unwrap();
        assert!(args[idx + 1].contains("DerivedData"),
            "per_job mode should use artifact_dir/DerivedData");
    }

    // Test 10: -derivedDataPath per cache mode - off
    #[test]
    fn test_derived_data_path_off_mode() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let mut job = make_test_job("job-001", "build");

        // Set cache mode to off
        job.effective_config = Some(serde_json::json!({
            "config": {
                "cache": {
                    "derived_data": "off"
                }
            }
        }));

        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_xcodebuild_command(&job, &artifact_dir).unwrap();

        // Should NOT have -derivedDataPath
        assert!(!args.contains(&"-derivedDataPath".to_string()),
            "off mode should not inject -derivedDataPath");
    }

    // Test 11: Working directory isolation
    #[test]
    fn test_working_directory_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_executor(&temp_dir);

        // Two different job IDs should have non-overlapping directories
        let job1_work_dir = config.jobs_root.join("job-001").join("work");
        let job2_work_dir = config.jobs_root.join("job-002").join("work");

        assert_ne!(job1_work_dir, job2_work_dir);
        assert!(!job1_work_dir.to_string_lossy().contains("job-002"));
        assert!(!job2_work_dir.to_string_lossy().contains("job-001"));
    }

    // Test 12: Exit code mapping - success
    #[test]
    fn test_exit_code_mapping_success() {
        // exit 0 → success/0
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let status = std::process::ExitStatus::default(); // This won't work directly, need workaround
        // Since we can't easily construct ExitStatus, test via the summary writing
        executor.write_success_summary(&artifact_dir, &job, 1000).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        assert!(summary_path.exists());

        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();
        assert_eq!(summary["exit_code"], 0);
        assert_eq!(summary["status"], "success");
    }

    // Test 13: Exit code mapping - xcodebuild failure
    #[test]
    fn test_exit_code_mapping_xcodebuild_failure() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        // Write failure with backend exit code 65
        executor.write_failure_summary(
            &artifact_dir,
            &job,
            "XCODEBUILD",
            None,
            "Build failed",
            1000,
            Some(65),
            None,
        ).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["exit_code"], 50, "XCODEBUILD failure should map to exit code 50");
        assert_eq!(summary["backend_exit_code"], 65);
        assert_eq!(summary["failure_kind"], "XCODEBUILD");
    }

    // Test 14: Exit code mapping - cancelled
    #[test]
    fn test_exit_code_mapping_cancelled() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_cancelled_summary(&artifact_dir, &job, 1000).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["exit_code"], 80, "Cancelled should map to exit code 80");
        assert_eq!(summary["failure_kind"], "CANCELLED");
        assert_eq!(summary["status"], "cancelled");
    }

    // Test 15: Exit code mapping - executor failure
    #[test]
    fn test_exit_code_mapping_executor_failure() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_failure_summary(
            &artifact_dir,
            &job,
            "EXECUTOR",
            None,
            "Failed to spawn xcodebuild",
            100,
            None,
            None,
        ).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["exit_code"], 40, "EXECUTOR failure should map to exit code 40");
    }

    // Test 16: Exit code mapping - transfer failure with extraction subkind
    #[test]
    fn test_exit_code_mapping_transfer_extraction_failed() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_failure_summary(
            &artifact_dir,
            &job,
            "TRANSFER",
            Some("EXTRACTION_FAILED"),
            "Source extraction failed",
            100,
            None,
            None,
        ).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["exit_code"], 30, "TRANSFER failure should map to exit code 30");
        assert_eq!(summary["failure_kind"], "TRANSFER");
        assert_eq!(summary["failure_subkind"], "EXTRACTION_FAILED");
    }

    // Test 17: summary.json field completeness
    #[test]
    fn test_summary_json_field_completeness() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_success_summary(&artifact_dir, &job, 5000).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        // Verify ALL normative fields are present
        assert!(summary.get("schema_version").is_some(), "Missing schema_version");
        assert!(summary.get("schema_id").is_some(), "Missing schema_id");
        assert!(summary.get("run_id").is_some(), "Missing run_id");
        assert!(summary.get("job_id").is_some(), "Missing job_id");
        assert!(summary.get("job_key").is_some(), "Missing job_key");
        assert!(summary.get("created_at").is_some(), "Missing created_at");
        assert!(summary.get("status").is_some(), "Missing status");
        assert!(summary.get("exit_code").is_some(), "Missing exit_code");
        assert!(summary.get("backend").is_some(), "Missing backend");
        assert!(summary.get("human_summary").is_some(), "Missing human_summary");
        assert!(summary.get("duration_ms").is_some(), "Missing duration_ms");

        // Verify schema values
        assert_eq!(summary["schema_version"], 1);
        assert_eq!(summary["schema_id"], "rch-xcode/summary@1");
        assert_eq!(summary["backend"], "xcodebuild");
    }

    // Test 18: toolchain.json emission
    #[test]
    fn test_toolchain_json_emission() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_toolchain_json(&artifact_dir, &job).unwrap();

        let toolchain_path = artifact_dir.join("toolchain.json");
        assert!(toolchain_path.exists(), "toolchain.json should be created");

        let toolchain: serde_json::Value = serde_json::from_str(&fs::read_to_string(&toolchain_path).unwrap()).unwrap();

        // Verify all fields
        assert_eq!(toolchain["schema_version"], TOOLCHAIN_SCHEMA_VERSION);
        assert_eq!(toolchain["schema_id"], TOOLCHAIN_SCHEMA_ID);
        assert_eq!(toolchain["run_id"], job.run_id);
        assert_eq!(toolchain["job_id"], job.job_id);
        assert_eq!(toolchain["xcode_build"], job.job_key_inputs.toolchain.xcode_build);
        assert_eq!(toolchain["developer_dir"], job.job_key_inputs.toolchain.developer_dir);
        assert_eq!(toolchain["macos_version"], job.job_key_inputs.toolchain.macos_version);
        assert_eq!(toolchain["macos_build"], job.job_key_inputs.toolchain.macos_build);
        assert_eq!(toolchain["arch"], job.job_key_inputs.toolchain.arch);
    }

    // Test 19: destination.json emission
    #[test]
    fn test_destination_json_emission() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "test");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_destination_json(&artifact_dir, &job).unwrap();

        let dest_path = artifact_dir.join("destination.json");
        assert!(dest_path.exists(), "destination.json should be created");

        let dest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&dest_path).unwrap()).unwrap();

        // Verify all fields
        assert_eq!(dest["schema_version"], DESTINATION_SCHEMA_VERSION);
        assert_eq!(dest["schema_id"], DESTINATION_SCHEMA_ID);
        assert_eq!(dest["platform"], job.job_key_inputs.destination.platform);
        assert_eq!(dest["name"], job.job_key_inputs.destination.name);
        assert_eq!(dest["os_version"], job.job_key_inputs.destination.os_version);
        assert_eq!(dest["provisioning"], job.job_key_inputs.destination.provisioning);

        // Simulator-specific fields
        assert_eq!(dest["sim_runtime_identifier"], job.job_key_inputs.destination.sim_runtime_identifier.as_deref().unwrap());
    }

    // Test 20: Cancellation flag
    #[test]
    fn test_cancellation_flag() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);

        assert!(!executor.is_cancelled(), "Should not be cancelled initially");

        executor.request_cancel();

        assert!(executor.is_cancelled(), "Should be cancelled after request_cancel");
    }

    // Test 21: Cancellation flag is shared
    #[test]
    fn test_cancellation_flag_shared() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);

        let flag = executor.cancellation_flag();

        assert!(!flag.load(Ordering::SeqCst), "Shared flag should not be cancelled initially");

        executor.request_cancel();

        assert!(flag.load(Ordering::SeqCst), "Shared flag should reflect cancellation");
    }

    // Test 22: Artifact atomic writes (write-then-rename)
    #[test]
    fn test_artifact_atomic_writes() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        // Write an artifact
        executor.write_success_summary(&artifact_dir, &job, 1000).unwrap();

        // Check that the temp file doesn't exist (it should have been renamed)
        let temp_file = artifact_dir.join(".summary.json.tmp");
        assert!(!temp_file.exists(), "Temp file should not exist after write");

        // Check that the final file exists
        let final_file = artifact_dir.join("summary.json");
        assert!(final_file.exists(), "Final file should exist after write");
    }

    // Test 23: Artifact directory path
    #[test]
    fn test_artifact_dir_path() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_executor(&temp_dir);

        let artifact_dir = executor.artifact_dir("job-test-123");
        let expected = config.jobs_root.join("job-test-123").join("artifacts");

        assert_eq!(artifact_dir, expected);
    }

    // Test 24: Job cleanup
    #[test]
    fn test_cleanup_job() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_executor(&temp_dir);

        // Create job directory structure
        let job_dir = config.jobs_root.join("job-cleanup-test");
        let work_dir = job_dir.join("work");
        let artifact_dir = job_dir.join("artifacts");
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&artifact_dir).unwrap();

        // Create some files
        fs::write(work_dir.join("test.txt"), "test content").unwrap();
        fs::write(artifact_dir.join("summary.json"), "{}").unwrap();

        assert!(job_dir.exists());

        // Clean up
        executor.cleanup_job("job-cleanup-test").unwrap();

        assert!(!job_dir.exists(), "Job directory should be removed after cleanup");
    }

    // Test 25: Cleanup non-existent job is no-op
    #[test]
    fn test_cleanup_nonexistent_job() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);

        // Should not error when cleaning up a job that doesn't exist
        let result = executor.cleanup_job("nonexistent-job-id");
        assert!(result.is_ok());
    }

    // Test 26: Make cancelled result
    #[test]
    fn test_make_cancelled_result() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);

        let elapsed = Duration::from_secs(5);
        let result = executor.make_cancelled_result(elapsed);

        assert_eq!(result.status, ExecutionStatus::Cancelled);
        assert_eq!(result.exit_code, 80);
        assert_eq!(result.backend_term_signal, Some("SIGTERM".to_string()));
        assert_eq!(result.failure_kind, Some("CANCELLED".to_string()));
        assert_eq!(result.duration_ms, 5000);
    }

    // Test 27: Source not found error
    #[test]
    fn test_source_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let work_dir = temp_dir.path().join("work");
        fs::create_dir_all(&work_dir).unwrap();

        let result = executor.extract_source("nonexistent_sha256", &work_dir);

        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutorError::SourceNotFound(sha) => {
                assert_eq!(sha, "nonexistent_sha256");
            }
            _ => panic!("Expected SourceNotFound error"),
        }
    }

    // Test 28: All failure kinds map to correct exit codes
    #[test]
    fn test_all_failure_kind_exit_codes() {
        let mappings = [
            ("CLASSIFIER_REJECTED", 10),
            ("SSH", 20),
            ("TRANSFER", 30),
            ("EXECUTOR", 40),
            ("XCODEBUILD", 50),
            ("MCP", 60),
            ("ARTIFACTS", 70),
            ("CANCELLED", 80),
            ("WORKER_BUSY", 90),
            ("WORKER_INCOMPATIBLE", 91),
            ("BUNDLER", 92),
            ("ATTESTATION", 93),
        ];

        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_executor(&temp_dir);
        let job = make_test_job("job-001", "build");

        for (failure_kind, expected_exit_code) in mappings {
            let artifact_dir = temp_dir.path().join(format!("artifacts-{}", failure_kind.to_lowercase()));
            fs::create_dir_all(&artifact_dir).unwrap();

            executor.write_failure_summary(
                &artifact_dir,
                &job,
                failure_kind,
                None,
                &format!("{} failure", failure_kind),
                100,
                None,
                None,
            ).unwrap();

            let summary_path = artifact_dir.join("summary.json");
            let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

            assert_eq!(
                summary["exit_code"], expected_exit_code,
                "Failure kind {} should map to exit code {}",
                failure_kind, expected_exit_code
            );
        }
    }
}
