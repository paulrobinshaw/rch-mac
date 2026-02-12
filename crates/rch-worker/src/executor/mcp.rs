//! XcodeBuildMCP backend executor for the RCH worker.
//!
//! Implements the MCP (Model Context Protocol) backend per M3 milestone.
//! XcodeBuildMCP provides richer structured output than direct xcodebuild:
//! - Structured JSON events during execution
//! - Richer diagnostics and build/test summaries
//! - Multi-step orchestration support
//!
//! This executor:
//! - Invokes XcodeBuildMCP instead of direct xcodebuild
//! - Parses structured events from MCP output
//! - Uses exit code 60 for MCP failures
//! - Same artifact contract as xcodebuild backend (with backend=mcp)
//!
//! Selected via `backend = "mcp"` in `.rch/xcode.toml`.
//! NOT a fallback — explicit choice per repo config.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::{
    Event, EventEmitter, ExecutionResult, ExecutionStatus, ExecutorConfig, ExecutorError,
    ExecutorResult, JobInput, attestation, manifest, summary, ENV_ALLOWLIST,
};

/// XcodeBuildMCP protocol version we support.
/// This is used for version negotiation if MCP supports it.
pub const MCP_PROTOCOL_VERSION: u32 = 1;

/// Schema version for MCP-specific events
pub const MCP_EVENTS_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for MCP events
pub const MCP_EVENTS_SCHEMA_ID: &str = "rch-xcode/mcp-events@1";

/// MCP event types from XcodeBuildMCP output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEventType {
    /// Build/test started
    Started,
    /// Progress update (e.g., compiling file, running test)
    Progress,
    /// Warning emitted
    Warning,
    /// Error emitted
    Error,
    /// Build target completed
    TargetCompleted,
    /// Test suite started
    TestSuiteStarted,
    /// Test case started
    TestCaseStarted,
    /// Test case passed
    TestCasePassed,
    /// Test case failed
    TestCaseFailed,
    /// Test suite completed
    TestSuiteCompleted,
    /// Build/test completed
    Completed,
    /// Unknown event type
    Unknown,
}

impl McpEventType {
    /// Parse event type from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "started" | "build_started" | "test_started" => Self::Started,
            "progress" | "compiling" | "linking" => Self::Progress,
            "warning" => Self::Warning,
            "error" => Self::Error,
            "target_completed" | "target_complete" => Self::TargetCompleted,
            "test_suite_started" | "testsuite_started" => Self::TestSuiteStarted,
            "test_case_started" | "testcase_started" => Self::TestCaseStarted,
            "test_case_passed" | "testcase_passed" => Self::TestCasePassed,
            "test_case_failed" | "testcase_failed" => Self::TestCaseFailed,
            "test_suite_completed" | "testsuite_completed" => Self::TestSuiteCompleted,
            "completed" | "build_completed" | "test_completed" => Self::Completed,
            _ => Self::Unknown,
        }
    }
}

/// Parsed MCP event from XcodeBuildMCP output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEvent {
    /// Event type
    #[serde(rename = "type")]
    pub event_type: String,
    /// Timestamp (if provided by MCP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Message content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// File path (for compile/error events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Line number (for error/warning events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Target name (for build events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Test name (for test events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_name: Option<String>,
    /// Test suite name (for test events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_suite: Option<String>,
    /// Duration in milliseconds (for completed events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Exit code (for completed events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Additional data
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl McpEvent {
    /// Get the parsed event type.
    pub fn parsed_type(&self) -> McpEventType {
        McpEventType::from_str(&self.event_type)
    }

    /// Convert to a standard Event for events.jsonl.
    pub fn to_standard_event(&self) -> Event {
        let stage = match self.parsed_type() {
            McpEventType::Started => "mcp",
            McpEventType::Progress => "mcp",
            McpEventType::Warning => "mcp",
            McpEventType::Error => "mcp",
            McpEventType::TargetCompleted => "mcp",
            McpEventType::TestSuiteStarted => "mcp",
            McpEventType::TestCaseStarted => "mcp",
            McpEventType::TestCasePassed => "mcp",
            McpEventType::TestCaseFailed => "mcp",
            McpEventType::TestSuiteCompleted => "mcp",
            McpEventType::Completed => "mcp",
            McpEventType::Unknown => "mcp",
        };

        let mut data = serde_json::Map::new();
        if let Some(ref msg) = self.message {
            data.insert("message".to_string(), serde_json::json!(msg));
        }
        if let Some(ref file) = self.file {
            data.insert("file".to_string(), serde_json::json!(file));
        }
        if let Some(line) = self.line {
            data.insert("line".to_string(), serde_json::json!(line));
        }
        if let Some(ref target) = self.target {
            data.insert("target".to_string(), serde_json::json!(target));
        }
        if let Some(ref test) = self.test_name {
            data.insert("test_name".to_string(), serde_json::json!(test));
        }
        if let Some(ref suite) = self.test_suite {
            data.insert("test_suite".to_string(), serde_json::json!(suite));
        }
        if let Some(duration) = self.duration_ms {
            data.insert("duration_ms".to_string(), serde_json::json!(duration));
        }

        Event {
            ts: self.timestamp.clone().unwrap_or_else(|| Utc::now().to_rfc3339()),
            stage: stage.to_string(),
            kind: self.event_type.clone(),
            data: if data.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(data))
            },
        }
    }
}

/// MCP execution summary collected during the run.
#[derive(Debug, Default)]
pub struct McpExecutionSummary {
    /// Number of targets built
    pub targets_built: u32,
    /// Number of warnings
    pub warnings: u32,
    /// Number of errors
    pub errors: u32,
    /// Test suites run
    pub test_suites_run: u32,
    /// Tests passed
    pub tests_passed: u32,
    /// Tests failed
    pub tests_failed: u32,
    /// First error message (if any)
    pub first_error: Option<String>,
    /// Failed test names
    pub failed_tests: Vec<String>,
}

/// The XcodeBuildMCP executor.
pub struct McpExecutor {
    config: ExecutorConfig,
    /// Cancellation flag
    cancelled: Arc<AtomicBool>,
    /// Path to xcodebuildmcp binary (defaults to "xcodebuildmcp" in PATH)
    mcp_binary: PathBuf,
}

impl McpExecutor {
    /// Create a new MCP executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        Self {
            config,
            cancelled: Arc::new(AtomicBool::new(false)),
            mcp_binary: PathBuf::from("xcodebuildmcp"),
        }
    }

    /// Create an MCP executor with a custom binary path.
    pub fn with_binary(config: ExecutorConfig, binary_path: PathBuf) -> Self {
        Self {
            config,
            cancelled: Arc::new(AtomicBool::new(false)),
            mcp_binary: binary_path,
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

    /// Execute a job using XcodeBuildMCP.
    ///
    /// This is the main entry point for MCP job execution:
    /// 1. Create isolated working directory
    /// 2. Extract source bundle
    /// 3. Construct XcodeBuildMCP command
    /// 4. Execute with structured event streaming
    /// 5. Write artifacts
    pub fn execute(&self, job: &JobInput) -> ExecutorResult<ExecutionResult> {
        let start_time = Instant::now();

        // Create job directories
        let job_dir = self.config.jobs_root.join(&job.job_id);
        let work_dir = job_dir.join("work");
        let artifact_dir = job_dir.join("artifacts");

        fs::create_dir_all(&work_dir)?;
        fs::create_dir_all(&artifact_dir)?;

        // Create event emitter for structured events
        let events = EventEmitter::new(&artifact_dir);

        // Emit job started event
        let _ = events.emit_with_data("setup", "started", serde_json::json!({
            "job_id": job.job_id,
            "run_id": job.run_id,
            "action": job.action,
            "backend": "mcp",
        }));

        // Check for cancellation
        if self.is_cancelled() {
            let _ = events.emit_simple("setup", "cancelled");
            return Ok(self.make_cancelled_result(start_time.elapsed()));
        }

        // Emit extraction started event
        let _ = events.emit_with_data("extraction", "started", serde_json::json!({
            "source_sha256": &job.job_key_inputs.source_sha256,
        }));

        // Extract source bundle
        let source_sha256 = &job.job_key_inputs.source_sha256;
        if let Err(e) = self.extract_source(source_sha256, &work_dir) {
            let _ = events.emit_with_data("extraction", "failed", serde_json::json!({
                "error": e.to_string(),
            }));
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

        // Emit extraction completed event
        let _ = events.emit_simple("extraction", "completed");

        // Check for cancellation
        if self.is_cancelled() {
            let _ = events.emit_simple("setup", "cancelled");
            let duration_ms = start_time.elapsed().as_millis() as u64;
            self.write_cancelled_summary(&artifact_dir, job, duration_ms)?;
            return Ok(self.make_cancelled_result(start_time.elapsed()));
        }

        // Build XcodeBuildMCP command
        let (cmd, args) = self.build_mcp_command(job, &artifact_dir)?;

        // Build environment
        let env = self.build_environment(&job.job_key_inputs.toolchain);

        // Open build.log for streaming
        let log_path = artifact_dir.join("build.log");
        let log_file = File::create(&log_path)?;
        let mut log_writer = BufWriter::new(log_file);

        // Log command line
        writeln!(log_writer, "=== RCH Xcode Lane - Job {} (MCP Backend) ===", job.job_id)?;
        writeln!(log_writer, "run_id: {}", job.run_id)?;
        writeln!(log_writer, "job_key: {}", job.job_key)?;
        writeln!(log_writer, "action: {}", job.action)?;
        writeln!(log_writer, "backend: mcp")?;
        writeln!(log_writer, "command: {} {}", cmd.display(), args.join(" "))?;
        writeln!(log_writer, "working_dir: {}", work_dir.display())?;
        writeln!(log_writer, "started_at: {}", Utc::now().to_rfc3339())?;
        writeln!(log_writer, "=== Begin XcodeBuildMCP output ===")?;
        log_writer.flush()?;

        // Emit execution started event
        let _ = events.emit_with_data("execution", "started", serde_json::json!({
            "command": cmd.to_string_lossy(),
            "action": job.action,
            "backend": "mcp",
        }));

        // Execute XcodeBuildMCP
        let exec_result = self.run_mcp(
            &cmd,
            &args,
            &work_dir,
            &env,
            &log_path,
            &events,
            start_time,
        );

        // Write end marker to log
        {
            let mut log_file = fs::OpenOptions::new().append(true).open(&log_path)?;
            writeln!(log_file, "=== End XcodeBuildMCP output ===")?;
            writeln!(log_file, "ended_at: {}", Utc::now().to_rfc3339())?;
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Process result and write artifacts
        let result = match exec_result {
            Ok((status, backend_exit_code, term_signal, mcp_summary)) => {
                let res = self.process_execution_result(
                    &artifact_dir,
                    job,
                    status,
                    backend_exit_code,
                    term_signal,
                    duration_ms,
                    &mcp_summary,
                )?;

                // Emit execution event based on result
                match res.status {
                    ExecutionStatus::Success => {
                        let _ = events.emit_with_data("execution", "completed", serde_json::json!({
                            "exit_code": res.exit_code,
                            "duration_ms": duration_ms,
                            "backend": "mcp",
                        }));
                    }
                    ExecutionStatus::Failed => {
                        let _ = events.emit_with_data("execution", "failed", serde_json::json!({
                            "exit_code": res.exit_code,
                            "backend_exit_code": res.backend_exit_code,
                            "failure_kind": res.failure_kind,
                            "backend": "mcp",
                        }));
                    }
                    ExecutionStatus::Cancelled => {
                        let _ = events.emit_simple("execution", "cancelled");
                    }
                }

                res
            }
            Err(e) => {
                let _ = events.emit_with_data("execution", "error", serde_json::json!({
                    "error": e.to_string(),
                    "backend": "mcp",
                }));

                self.write_failure_summary(
                    &artifact_dir,
                    job,
                    "MCP",
                    None,
                    &format!("MCP execution failed: {}", e),
                    duration_ms,
                    None,
                    None,
                )?;
                ExecutionResult {
                    status: ExecutionStatus::Failed,
                    exit_code: 60, // MCP_FAILED
                    backend_exit_code: None,
                    backend_term_signal: None,
                    duration_ms,
                    human_summary: format!("MCP execution failed: {}", e),
                    failure_kind: Some("MCP".to_string()),
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

        // Generate agent-friendly summaries
        let _ = summary::generate_summaries(
            &artifact_dir,
            &job.run_id,
            &job.job_id,
            &job.job_key,
            &job.action,
        );

        // Generate manifest.json
        let manifest_result = manifest::generate_manifest(
            &artifact_dir,
            &job.run_id,
            &job.job_id,
            &job.job_key,
        );

        // Generate attestation.json
        if let Ok(ref manifest) = manifest_result {
            if let Ok(manifest_sha256) = manifest.compute_sha256() {
                let backend_version = "mcp-1.0"; // MCP version

                let _ = attestation::generate_attestation(
                    &artifact_dir,
                    &job.run_id,
                    &job.job_id,
                    &job.job_key,
                    &job.job_key_inputs.source_sha256,
                    &self.config.worker_name,
                    &self.config.worker_fingerprint,
                    self.config.capabilities_json.as_bytes(),
                    "mcp",
                    backend_version,
                    &manifest_sha256,
                );
            }
        }

        // Emit completion event
        let completion_kind = match result.status {
            ExecutionStatus::Success => "success",
            ExecutionStatus::Failed => "failure",
            ExecutionStatus::Cancelled => "cancelled",
        };
        let _ = events.emit_with_data("completion", completion_kind, serde_json::json!({
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "status": format!("{:?}", result.status),
            "backend": "mcp",
        }));

        Ok(result)
    }

    /// Extract source bundle to working directory.
    fn extract_source(&self, source_sha256: &str, work_dir: &Path) -> ExecutorResult<()> {
        let prefix = &source_sha256[..2.min(source_sha256.len())];
        let bundle_path = self.config.source_store_root
            .join(prefix)
            .join(source_sha256)
            .join("bundle.tar");

        if !bundle_path.exists() {
            return Err(ExecutorError::SourceNotFound(source_sha256.to_string()));
        }

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

    /// Build the XcodeBuildMCP command and arguments.
    fn build_mcp_command(
        &self,
        job: &JobInput,
        artifact_dir: &Path,
    ) -> ExecutorResult<(PathBuf, Vec<String>)> {
        // XcodeBuildMCP uses the same argument structure as xcodebuild
        // but with additional MCP-specific flags for structured output
        let mut args = job.job_key_inputs.sanitized_argv.clone();

        // Add MCP-specific flags for structured JSON output
        // These flags tell XcodeBuildMCP to emit events to stdout
        args.push("--mcp-format".to_string());
        args.push("json".to_string());

        // For test jobs, inject -resultBundlePath
        if job.action == "test" {
            let xcresult_path = artifact_dir.join("result.xcresult");
            args.push("-resultBundlePath".to_string());
            args.push(xcresult_path.to_str().unwrap().to_string());
        }

        // Handle DerivedData based on config
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
                if let Some(ref shared_path) = self.config.shared_derived_data {
                    args.push("-derivedDataPath".to_string());
                    args.push(shared_path.to_str().unwrap().to_string());
                }
            }
        }

        Ok((self.mcp_binary.clone(), args))
    }

    /// Build the environment for XcodeBuildMCP.
    fn build_environment(&self, toolchain: &super::ToolchainInput) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Set DEVELOPER_DIR from toolchain
        env.insert("DEVELOPER_DIR".to_string(), toolchain.developer_dir.clone());

        // Copy allowed environment variables
        for key in ENV_ALLOWLIST {
            if *key == "DEVELOPER_DIR" {
                continue;
            }
            if let Ok(value) = std::env::var(key) {
                env.insert(key.to_string(), value);
            }
        }

        // Add MCP-specific environment variables
        env.insert("MCP_OUTPUT_FORMAT".to_string(), "json".to_string());

        env
    }

    /// Run XcodeBuildMCP with streaming output and event parsing.
    #[allow(clippy::too_many_arguments)]
    fn run_mcp(
        &self,
        cmd: &Path,
        args: &[String],
        work_dir: &Path,
        env: &HashMap<String, String>,
        log_path: &Path,
        events: &EventEmitter,
        _start_time: Instant,
    ) -> ExecutorResult<(ExitStatus, Option<i32>, Option<String>, McpExecutionSummary)> {
        let mut command = Command::new(cmd);
        command
            .args(args)
            .current_dir(work_dir)
            .env_clear()
            .envs(env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()
            .map_err(|e| ExecutorError::SpawnFailed(format!("Failed to spawn XcodeBuildMCP: {}", e)))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Open log file for appending
        let log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let log_file = Arc::new(std::sync::Mutex::new(log_file));

        // MCP execution summary
        let mcp_summary = Arc::new(std::sync::Mutex::new(McpExecutionSummary::default()));

        // Stream stdout and parse MCP events
        let log_clone = Arc::clone(&log_file);
        let summary_clone = Arc::clone(&mcp_summary);
        let events_clone = events.path.clone();
        let stdout_handle = std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        // Write raw line to log
                        if let Ok(mut f) = log_clone.lock() {
                            let _ = writeln!(f, "{}", line);
                        }

                        // Try to parse as MCP event
                        if let Ok(mcp_event) = serde_json::from_str::<McpEvent>(&line) {
                            // Update summary based on event type
                            if let Ok(mut summary) = summary_clone.lock() {
                                match mcp_event.parsed_type() {
                                    McpEventType::Error => {
                                        summary.errors += 1;
                                        if summary.first_error.is_none() {
                                            summary.first_error = mcp_event.message.clone();
                                        }
                                    }
                                    McpEventType::Warning => {
                                        summary.warnings += 1;
                                    }
                                    McpEventType::TargetCompleted => {
                                        summary.targets_built += 1;
                                    }
                                    McpEventType::TestSuiteStarted => {
                                        summary.test_suites_run += 1;
                                    }
                                    McpEventType::TestCasePassed => {
                                        summary.tests_passed += 1;
                                    }
                                    McpEventType::TestCaseFailed => {
                                        summary.tests_failed += 1;
                                        if let Some(ref name) = mcp_event.test_name {
                                            summary.failed_tests.push(name.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            // Write event to events.jsonl
                            let standard_event = mcp_event.to_standard_event();
                            let emitter = EventEmitter { path: events_clone.clone() };
                            let _ = emitter.emit(&standard_event);
                        }
                    }
                }
            }
        });

        // Stream stderr to log
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
                self.terminate_child(&mut child)?;
                let final_summary = mcp_summary.lock().unwrap().clone();
                return Ok((
                    child.wait()?,
                    None,
                    Some("SIGTERM".to_string()),
                    final_summary,
                ));
            }

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

        let final_summary = mcp_summary.lock().unwrap().clone();
        Ok((status, backend_exit_code, term_signal, final_summary))
    }

    /// Terminate a child process gracefully then forcefully.
    fn terminate_child(&self, child: &mut Child) -> ExecutorResult<()> {
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

        let grace_duration = Duration::from_secs(self.config.termination_grace_seconds);
        let start = Instant::now();

        while start.elapsed() < grace_duration {
            match child.try_wait()? {
                Some(_) => return Ok(()),
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        Ok(())
    }

    /// Process execution result and write summary.
    #[allow(clippy::too_many_arguments)]
    fn process_execution_result(
        &self,
        artifact_dir: &Path,
        job: &JobInput,
        status: ExitStatus,
        backend_exit_code: Option<i32>,
        term_signal: Option<String>,
        duration_ms: u64,
        mcp_summary: &McpExecutionSummary,
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
            self.write_success_summary(artifact_dir, job, duration_ms, mcp_summary)?;
            Ok(ExecutionResult {
                status: ExecutionStatus::Success,
                exit_code: 0,
                backend_exit_code: Some(0),
                backend_term_signal: None,
                duration_ms,
                human_summary: format!("{} succeeded via MCP", job.action),
                failure_kind: None,
                failure_subkind: None,
            })
        } else {
            let human_summary = if let Some(ref first_error) = mcp_summary.first_error {
                format!("MCP {} failed: {}", job.action, first_error)
            } else {
                format!(
                    "MCP {} failed with exit code {}",
                    job.action,
                    backend_exit_code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string())
                )
            };

            self.write_failure_summary(
                artifact_dir,
                job,
                "MCP",
                None,
                &human_summary,
                duration_ms,
                backend_exit_code,
                term_signal.as_deref(),
            )?;

            Ok(ExecutionResult {
                status: ExecutionStatus::Failed,
                exit_code: 60, // MCP_FAILED
                backend_exit_code,
                backend_term_signal: term_signal,
                duration_ms,
                human_summary,
                failure_kind: Some("MCP".to_string()),
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
        mcp_summary: &McpExecutionSummary,
    ) -> ExecutorResult<()> {
        let mut summary = serde_json::json!({
            "schema_version": 1,
            "schema_id": "rch-xcode/summary@1",
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "status": "success",
            "exit_code": 0,
            "backend_exit_code": 0,
            "backend": "mcp",
            "human_summary": format!("{} succeeded via MCP", job.action),
            "duration_ms": duration_ms,
            "artifact_profile": "rich" // MCP produces rich output
        });

        // Add MCP-specific summary data
        if job.action == "test" {
            summary["tests_passed"] = serde_json::json!(mcp_summary.tests_passed);
            summary["tests_failed"] = serde_json::json!(mcp_summary.tests_failed);
            summary["test_suites_run"] = serde_json::json!(mcp_summary.test_suites_run);
        }
        if mcp_summary.targets_built > 0 {
            summary["targets_built"] = serde_json::json!(mcp_summary.targets_built);
        }
        if mcp_summary.warnings > 0 {
            summary["warnings_count"] = serde_json::json!(mcp_summary.warnings);
        }

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
            _ => 60, // Default to MCP for this executor
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
            "backend": "mcp",
            "human_summary": human_summary,
            "duration_ms": duration_ms,
            "artifact_profile": "rich"
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
            "backend": "mcp",
            "human_summary": "Job cancelled",
            "duration_ms": duration_ms,
            "artifact_profile": "rich"
        });

        self.write_json_artifact(artifact_dir, "summary.json", &summary)
    }

    /// Write toolchain.json artifact.
    fn write_toolchain_json(&self, artifact_dir: &Path, job: &JobInput) -> ExecutorResult<()> {
        let toolchain = &job.job_key_inputs.toolchain;
        let json = serde_json::json!({
            "schema_version": super::TOOLCHAIN_SCHEMA_VERSION,
            "schema_id": super::TOOLCHAIN_SCHEMA_ID,
            "run_id": job.run_id,
            "job_id": job.job_id,
            "job_key": job.job_key,
            "created_at": Utc::now().to_rfc3339(),
            "xcode_build": toolchain.xcode_build,
            "xcode_version": null,
            "developer_dir": toolchain.developer_dir,
            "macos_version": toolchain.macos_version,
            "macos_build": toolchain.macos_build,
            "arch": toolchain.arch
        });

        self.write_json_artifact(artifact_dir, "toolchain.json", &json)
    }

    /// Write destination.json artifact.
    fn write_destination_json(&self, artifact_dir: &Path, job: &JobInput) -> ExecutorResult<()> {
        let dest = &job.job_key_inputs.destination;
        let mut json = serde_json::json!({
            "schema_version": super::DESTINATION_SCHEMA_VERSION,
            "schema_id": super::DESTINATION_SCHEMA_ID,
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

impl Clone for McpExecutionSummary {
    fn clone(&self) -> Self {
        Self {
            targets_built: self.targets_built,
            warnings: self.warnings,
            errors: self.errors,
            test_suites_run: self.test_suites_run,
            tests_passed: self.tests_passed,
            tests_failed: self.tests_failed,
            first_error: self.first_error.clone(),
            failed_tests: self.failed_tests.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_test_job(job_id: &str, action: &str) -> JobInput {
        JobInput {
            run_id: "run-001".to_string(),
            job_id: job_id.to_string(),
            action: action.to_string(),
            job_key: "abc123def456789012345678901234567890123456789012345678901234".to_string(),
            job_key_inputs: super::super::JobKeyInputs {
                source_sha256: "source123456789012345678901234567890123456789012345678901234".to_string(),
                sanitized_argv: vec![
                    action.to_string(),
                    "-scheme".to_string(),
                    "MyApp".to_string(),
                    "-workspace".to_string(),
                    "MyApp.xcworkspace".to_string(),
                ],
                toolchain: super::super::ToolchainInput {
                    xcode_build: "16C5032a".to_string(),
                    developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
                    macos_version: "15.3".to_string(),
                    macos_build: "24D60".to_string(),
                    arch: "arm64".to_string(),
                },
                destination: super::super::DestinationInput {
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
            artifact_profile: super::super::ArtifactProfile::Rich, // MCP always produces rich output
        }
    }

    fn make_test_mcp_executor(temp_dir: &TempDir) -> (McpExecutor, ExecutorConfig) {
        let config = ExecutorConfig {
            jobs_root: temp_dir.path().join("jobs"),
            source_store_root: temp_dir.path().join("sources"),
            shared_derived_data: Some(temp_dir.path().join("DerivedData")),
            termination_grace_seconds: 10,
            worker_name: "test-worker".to_string(),
            worker_fingerprint: "SHA256:test-fingerprint".to_string(),
            capabilities_json: r#"{"max_upload_bytes":104857600}"#.to_string(),
        };
        fs::create_dir_all(&config.jobs_root).unwrap();
        fs::create_dir_all(&config.source_store_root).unwrap();
        (McpExecutor::new(config.clone()), config)
    }

    // Test 1: MCP event type parsing
    #[test]
    fn test_mcp_event_type_parsing() {
        assert_eq!(McpEventType::from_str("started"), McpEventType::Started);
        assert_eq!(McpEventType::from_str("build_started"), McpEventType::Started);
        assert_eq!(McpEventType::from_str("error"), McpEventType::Error);
        assert_eq!(McpEventType::from_str("warning"), McpEventType::Warning);
        assert_eq!(McpEventType::from_str("test_case_passed"), McpEventType::TestCasePassed);
        assert_eq!(McpEventType::from_str("test_case_failed"), McpEventType::TestCaseFailed);
        assert_eq!(McpEventType::from_str("completed"), McpEventType::Completed);
        assert_eq!(McpEventType::from_str("unknown_event"), McpEventType::Unknown);
    }

    // Test 2: MCP event deserialization
    #[test]
    fn test_mcp_event_deserialization() {
        let json = r#"{
            "type": "error",
            "message": "Build failed",
            "file": "src/main.swift",
            "line": 42
        }"#;

        let event: McpEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "error");
        assert_eq!(event.message, Some("Build failed".to_string()));
        assert_eq!(event.file, Some("src/main.swift".to_string()));
        assert_eq!(event.line, Some(42));
    }

    // Test 3: MCP event to standard event conversion
    #[test]
    fn test_mcp_event_to_standard_event() {
        let mcp_event = McpEvent {
            event_type: "test_case_failed".to_string(),
            timestamp: Some("2026-02-11T12:00:00Z".to_string()),
            message: Some("Test failed".to_string()),
            file: None,
            line: None,
            target: None,
            test_name: Some("testFoo".to_string()),
            test_suite: Some("MyTests".to_string()),
            duration_ms: Some(100),
            exit_code: None,
            extra: HashMap::new(),
        };

        let event = mcp_event.to_standard_event();
        assert_eq!(event.stage, "mcp");
        assert_eq!(event.kind, "test_case_failed");
        assert!(event.data.is_some());

        let data = event.data.unwrap();
        assert_eq!(data["test_name"], "testFoo");
        assert_eq!(data["test_suite"], "MyTests");
        assert_eq!(data["duration_ms"], 100);
    }

    // Test 4: MCP executor config defaults
    #[test]
    fn test_mcp_executor_config() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);

        assert_eq!(executor.mcp_binary, PathBuf::from("xcodebuildmcp"));
    }

    // Test 5: MCP executor with custom binary
    #[test]
    fn test_mcp_executor_custom_binary() {
        let config = ExecutorConfig::default();
        let executor = McpExecutor::with_binary(config, PathBuf::from("/opt/mcp/bin/xcodebuildmcp"));

        assert_eq!(executor.mcp_binary, PathBuf::from("/opt/mcp/bin/xcodebuildmcp"));
    }

    // Test 6: Build MCP command includes MCP-specific flags
    #[test]
    fn test_build_mcp_command() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (cmd, args) = executor.build_mcp_command(&job, &artifact_dir).unwrap();

        assert_eq!(cmd, PathBuf::from("xcodebuildmcp"));
        // Should have --mcp-format json
        assert!(args.contains(&"--mcp-format".to_string()));
        assert!(args.contains(&"json".to_string()));
    }

    // Test 7: Build MCP command for test job includes resultBundlePath
    #[test]
    fn test_build_mcp_command_test_job() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "test");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let (_, args) = executor.build_mcp_command(&job, &artifact_dir).unwrap();

        assert!(args.contains(&"-resultBundlePath".to_string()));
    }

    // Test 8: MCP environment includes MCP_OUTPUT_FORMAT
    #[test]
    fn test_mcp_environment() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");

        let env = executor.build_environment(&job.job_key_inputs.toolchain);

        assert_eq!(env.get("MCP_OUTPUT_FORMAT"), Some(&"json".to_string()));
        assert_eq!(
            env.get("DEVELOPER_DIR"),
            Some(&"/Applications/Xcode.app/Contents/Developer".to_string())
        );
    }

    // Test 9: Success summary has backend=mcp
    #[test]
    fn test_success_summary_backend_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let mcp_summary = McpExecutionSummary {
            targets_built: 3,
            warnings: 2,
            ..Default::default()
        };

        executor.write_success_summary(&artifact_dir, &job, 5000, &mcp_summary).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["backend"], "mcp");
        assert_eq!(summary["artifact_profile"], "rich");
        assert_eq!(summary["targets_built"], 3);
        assert_eq!(summary["warnings_count"], 2);
    }

    // Test 10: Failure summary has exit code 60 for MCP
    #[test]
    fn test_failure_summary_mcp_exit_code() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_failure_summary(
            &artifact_dir,
            &job,
            "MCP",
            None,
            "MCP build failed",
            5000,
            Some(1),
            None,
        ).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["exit_code"], 60);
        assert_eq!(summary["failure_kind"], "MCP");
        assert_eq!(summary["backend"], "mcp");
    }

    // Test 11: Cancelled summary
    #[test]
    fn test_cancelled_summary_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_cancelled_summary(&artifact_dir, &job, 1000).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["status"], "cancelled");
        assert_eq!(summary["exit_code"], 80);
        assert_eq!(summary["backend"], "mcp");
    }

    // Test 12: MCP execution summary accumulation
    #[test]
    fn test_mcp_execution_summary() {
        let mut summary = McpExecutionSummary::default();

        // Simulate processing events
        summary.errors += 1;
        summary.first_error = Some("Cannot find module".to_string());
        summary.warnings += 3;
        summary.targets_built += 2;

        assert_eq!(summary.errors, 1);
        assert_eq!(summary.warnings, 3);
        assert_eq!(summary.targets_built, 2);
        assert_eq!(summary.first_error, Some("Cannot find module".to_string()));
    }

    // Test 13: Test summary includes test counts
    #[test]
    fn test_summary_includes_test_counts() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "test");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let mcp_summary = McpExecutionSummary {
            tests_passed: 10,
            tests_failed: 2,
            test_suites_run: 3,
            failed_tests: vec!["testFoo".to_string(), "testBar".to_string()],
            ..Default::default()
        };

        executor.write_success_summary(&artifact_dir, &job, 5000, &mcp_summary).unwrap();

        let summary_path = artifact_dir.join("summary.json");
        let summary: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

        assert_eq!(summary["tests_passed"], 10);
        assert_eq!(summary["tests_failed"], 2);
        assert_eq!(summary["test_suites_run"], 3);
    }

    // Test 14: Cancellation flag
    #[test]
    fn test_mcp_cancellation_flag() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);

        assert!(!executor.is_cancelled());
        executor.request_cancel();
        assert!(executor.is_cancelled());
    }

    // Test 15: Make cancelled result
    #[test]
    fn test_make_cancelled_result_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);

        let result = executor.make_cancelled_result(Duration::from_secs(3));

        assert_eq!(result.status, ExecutionStatus::Cancelled);
        assert_eq!(result.exit_code, 80);
        assert_eq!(result.duration_ms, 3000);
    }

    // Test 16: All failure kinds map to correct exit codes
    #[test]
    fn test_all_failure_kind_exit_codes_mcp() {
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
        let (executor, _) = make_test_mcp_executor(&temp_dir);
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
            let summary: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&summary_path).unwrap()).unwrap();

            assert_eq!(
                summary["exit_code"], expected_exit_code,
                "Failure kind {} should map to exit code {}",
                failure_kind, expected_exit_code
            );
        }
    }

    // Test 17: Artifact directory path
    #[test]
    fn test_artifact_dir_path_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_mcp_executor(&temp_dir);

        let artifact_dir = executor.artifact_dir("job-test-123");
        let expected = config.jobs_root.join("job-test-123").join("artifacts");

        assert_eq!(artifact_dir, expected);
    }

    // Test 18: Cleanup job
    #[test]
    fn test_cleanup_job_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, config) = make_test_mcp_executor(&temp_dir);

        let job_dir = config.jobs_root.join("job-cleanup-test");
        fs::create_dir_all(&job_dir.join("work")).unwrap();
        fs::create_dir_all(&job_dir.join("artifacts")).unwrap();

        assert!(job_dir.exists());
        executor.cleanup_job("job-cleanup-test").unwrap();
        assert!(!job_dir.exists());
    }

    // Test 19: MCP event parsing from JSON line
    #[test]
    fn test_parse_mcp_event_json_line() {
        let line = r#"{"type":"test_case_passed","test_name":"testExample","test_suite":"MyTests","duration_ms":50}"#;

        let event: McpEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.parsed_type(), McpEventType::TestCasePassed);
        assert_eq!(event.test_name, Some("testExample".to_string()));
        assert_eq!(event.test_suite, Some("MyTests".to_string()));
        assert_eq!(event.duration_ms, Some(50));
    }

    // Test 20: toolchain.json emission
    #[test]
    fn test_toolchain_json_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "build");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_toolchain_json(&artifact_dir, &job).unwrap();

        let toolchain_path = artifact_dir.join("toolchain.json");
        assert!(toolchain_path.exists());

        let toolchain: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&toolchain_path).unwrap()).unwrap();

        assert_eq!(toolchain["schema_id"], "rch-xcode/toolchain@1");
    }

    // Test 21: destination.json emission
    #[test]
    fn test_destination_json_mcp() {
        let temp_dir = TempDir::new().unwrap();
        let (executor, _) = make_test_mcp_executor(&temp_dir);
        let job = make_test_job("job-001", "test");
        let artifact_dir = temp_dir.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        executor.write_destination_json(&artifact_dir, &job).unwrap();

        let dest_path = artifact_dir.join("destination.json");
        assert!(dest_path.exists());

        let dest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest_path).unwrap()).unwrap();

        assert_eq!(dest["schema_id"], "rch-xcode/destination@1");
        assert_eq!(dest["platform"], "iOS Simulator");
    }
}
