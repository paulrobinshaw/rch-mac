//! Mock Worker Implementation
//!
//! Configurable mock worker for testing all RPC operations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::protocol::envelope::{Operation, RpcErrorPayload, RpcRequest, RpcResponse};
use crate::worker::probe::{SimulatorRuntime, XcodeVersion};
use crate::worker::rpc::WorkerConfig;

use super::failure::{FailureConfig, FailureInjector};
use super::state::{Job, JobState, Lease, MockState};

/// Upload session for tracking resumable uploads
#[derive(Debug, Clone)]
pub struct MockUploadSession {
    pub upload_id: String,
    pub source_sha256: String,
    pub content_length: u64,
    pub received_bytes: Vec<u8>,
}

/// Configurable mock worker for testing
pub struct MockWorker {
    /// Worker configuration
    config: WorkerConfig,
    /// Mutable state (wrapped for interior mutability)
    state: Arc<Mutex<MockState>>,
    /// Failure injector
    failures: Arc<Mutex<FailureInjector>>,
    /// Configurable capabilities
    capabilities: Arc<Mutex<MockCapabilities>>,
    /// Job progression overrides (job_id -> sequence of states to advance through)
    job_progressions: Arc<Mutex<HashMap<String, Vec<JobState>>>>,
    /// Jobs held in a state (won't advance until released)
    held_jobs: Arc<Mutex<HashMap<String, JobState>>>,
    /// Upload sessions for resumable uploads
    upload_sessions: Arc<Mutex<HashMap<String, MockUploadSession>>>,
}

/// Configurable capabilities for the mock worker
#[derive(Debug, Clone)]
pub struct MockCapabilities {
    pub macos_version: String,
    pub macos_build: String,
    pub macos_arch: String,
    pub xcode_versions: Vec<XcodeVersion>,
    pub runtimes: Vec<SimulatorRuntime>,
    pub max_concurrent_jobs: u32,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    /// Maximum upload size in bytes (0 = no limit)
    pub max_upload_bytes: u64,
}

impl Default for MockCapabilities {
    fn default() -> Self {
        Self {
            macos_version: "14.0".to_string(),
            macos_build: "23A344".to_string(),
            macos_arch: "arm64".to_string(),
            xcode_versions: vec![XcodeVersion {
                version: "15.0".to_string(),
                build: "15A240d".to_string(),
                path: "/Applications/Xcode.app".to_string(),
                developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            }],
            runtimes: vec![SimulatorRuntime {
                name: "iOS 17.0".to_string(),
                identifier: "com.apple.CoreSimulator.SimRuntime.iOS-17-0".to_string(),
                build_version: "21A328".to_string(),
                is_available: true,
            }],
            max_concurrent_jobs: 2,
            disk_free_bytes: 100 * 1024 * 1024 * 1024, // 100 GB
            disk_total_bytes: 500 * 1024 * 1024 * 1024, // 500 GB
            max_upload_bytes: 0, // 0 = no limit
        }
    }
}

impl MockCapabilities {
    /// Set the maximum upload size in bytes (0 = no limit)
    pub fn with_max_upload_bytes(mut self, bytes: u64) -> Self {
        self.max_upload_bytes = bytes;
        self
    }
}

impl MockWorker {
    /// Create a new mock worker with default configuration
    pub fn new() -> Self {
        Self::with_config(WorkerConfig::default())
    }

    /// Create a new mock worker with custom configuration
    pub fn with_config(config: WorkerConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(MockState::new())),
            failures: Arc::new(Mutex::new(FailureInjector::new())),
            capabilities: Arc::new(Mutex::new(MockCapabilities::default())),
            job_progressions: Arc::new(Mutex::new(HashMap::new())),
            held_jobs: Arc::new(Mutex::new(HashMap::new())),
            upload_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // === Public API for test configuration ===

    /// Set the maximum concurrent jobs (affects BUSY responses)
    pub fn set_capacity(&self, max_jobs: u32) {
        let mut caps = self.capabilities.lock().unwrap();
        caps.max_concurrent_jobs = max_jobs;
    }

    /// Set the maximum upload size in bytes (0 = no limit)
    ///
    /// When set, upload_source will reject payloads exceeding this limit
    /// with PAYLOAD_TOO_LARGE error.
    pub fn set_max_upload_bytes(&self, max_bytes: u64) {
        let mut caps = self.capabilities.lock().unwrap();
        caps.max_upload_bytes = max_bytes;
    }

    /// Inject an error for the next call to an operation
    pub fn inject_error(&self, op: Operation, code: &str, message: &str) {
        let mut failures = self.failures.lock().unwrap();
        failures.inject_error(op, code, message);
    }

    /// Inject a failure configuration for an operation
    pub fn inject_failure(&self, op: Operation, config: FailureConfig) {
        let mut failures = self.failures.lock().unwrap();
        failures.inject(op, config);
    }

    /// Clear all failure injections
    pub fn clear_failures(&self) {
        let mut failures = self.failures.lock().unwrap();
        failures.clear();
    }

    /// Set job progression sequence
    pub fn set_job_progression(&self, job_id: &str, states: Vec<JobState>) {
        let mut progressions = self.job_progressions.lock().unwrap();
        progressions.insert(job_id.to_string(), states);
    }

    /// Hold a job in a specific state
    pub fn hold_job_in_state(&self, job_id: &str, state: JobState) {
        let mut held = self.held_jobs.lock().unwrap();
        held.insert(job_id.to_string(), state);
    }

    /// Release a held job
    pub fn release_job(&self, job_id: &str) {
        let mut held = self.held_jobs.lock().unwrap();
        held.remove(job_id);
    }

    /// Evict a source (for TOCTOU testing)
    pub fn evict_source(&self, sha256: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        state.evict_source(sha256)
    }

    /// Delete artifacts for a job
    pub fn delete_artifacts(&self, job_id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        state.delete_artifacts(job_id)
    }

    /// Store a source bundle directly (for test setup)
    pub fn store_source(&self, sha256: &str, content: Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        state.store_source(sha256.to_string(), content);
    }

    /// Get job state (for test assertions)
    pub fn get_job_state(&self, job_id: &str) -> Option<JobState> {
        let state = self.state.lock().unwrap();
        state.get_job(job_id).map(|j| j.state)
    }

    // === Request handling ===

    /// Handle an RPC request (in-process library mode)
    pub fn handle_request(&self, request: &RpcRequest) -> RpcResponse {
        // Check for injected failures first
        if let Some(failure) = self.check_failure(&request.op) {
            return self.make_failure_response(request, failure);
        }

        // Validate protocol version
        if let Err(e) = self.validate_protocol_version(request) {
            return e;
        }

        // Dispatch to operation handler
        self.dispatch(request)
    }

    /// Handle a JSON request string (convenience method)
    pub fn handle_json(&self, json_request: &str) -> Result<String, serde_json::Error> {
        let request: RpcRequest = serde_json::from_str(json_request)?;
        let response = self.handle_request(&request);
        serde_json::to_string(&response)
    }

    // === Internal helpers ===

    fn check_failure(&self, op: &Operation) -> Option<FailureConfig> {
        let mut failures = self.failures.lock().unwrap();
        failures.check(op).cloned()
    }

    fn make_failure_response(&self, request: &RpcRequest, failure: FailureConfig) -> RpcResponse {
        let protocol_version = if request.op == Operation::Probe {
            0
        } else {
            request.protocol_version
        };

        let code = failure.error_code.unwrap_or_else(|| "INTERNAL_ERROR".to_string());
        let message = failure.error_message.unwrap_or_else(|| "Injected failure".to_string());

        let mut error = RpcErrorPayload::new(code, message);
        if let Some(retry_after) = failure.retry_after_seconds {
            error = error.with_data("retry_after_seconds", json!(retry_after));
        }

        RpcResponse::error(protocol_version, request.request_id.clone(), error)
    }

    fn validate_protocol_version(&self, request: &RpcRequest) -> Result<(), RpcResponse> {
        // probe MUST use protocol_version: 0
        if request.op == Operation::Probe {
            if request.protocol_version != 0 {
                return Err(RpcResponse::error(
                    request.protocol_version,
                    request.request_id.clone(),
                    RpcErrorPayload::new("UNSUPPORTED_PROTOCOL", "probe requires protocol_version: 0")
                        .with_data("min", json!(0))
                        .with_data("max", json!(0)),
                ));
            }
            return Ok(());
        }

        // All other ops MUST NOT use protocol_version: 0
        if request.protocol_version == 0 {
            return Err(RpcResponse::error(
                0,
                request.request_id.clone(),
                RpcErrorPayload::new("UNSUPPORTED_PROTOCOL", "Non-probe operations require protocol_version > 0")
                    .with_data("min", json!(self.config.protocol_min))
                    .with_data("max", json!(self.config.protocol_max)),
            ));
        }

        // Check version range
        if request.protocol_version < self.config.protocol_min
            || request.protocol_version > self.config.protocol_max
        {
            return Err(RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("UNSUPPORTED_PROTOCOL", "Protocol version not supported")
                    .with_data("min", json!(self.config.protocol_min))
                    .with_data("max", json!(self.config.protocol_max)),
            ));
        }

        Ok(())
    }

    fn dispatch(&self, request: &RpcRequest) -> RpcResponse {
        match request.op {
            Operation::Probe => self.handle_probe(request),
            Operation::Reserve => self.handle_reserve(request),
            Operation::Release => self.handle_release(request),
            Operation::Submit => self.handle_submit(request),
            Operation::Status => self.handle_status(request),
            Operation::Tail => self.handle_tail(request),
            Operation::Cancel => self.handle_cancel(request),
            Operation::Fetch => self.handle_fetch(request),
            Operation::HasSource => self.handle_has_source(request),
            Operation::UploadSource => self.handle_upload_source(request),
        }
    }

    // === Operation handlers ===

    fn handle_probe(&self, request: &RpcRequest) -> RpcResponse {
        let caps = self.capabilities.lock().unwrap();
        let state = self.state.lock().unwrap();

        let payload = json!({
            "kind": "probe",
            "schema_id": "rch-xcode/capabilities@1",
            "schema_version": "1.0.0",
            "created_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "rch_xcode_lane_version": "0.1.0",
            "protocol_min": self.config.protocol_min,
            "protocol_max": self.config.protocol_max,
            "features": self.config.features,
            "macos": {
                "version": caps.macos_version,
                "build": caps.macos_build,
                "arch": caps.macos_arch,
            },
            "xcode_versions": caps.xcode_versions,
            "active_developer_dir": caps.xcode_versions.first()
                .map(|x| x.developer_dir.clone())
                .unwrap_or_default(),
            "runtimes": caps.runtimes,
            "capacity": {
                "max_concurrent_jobs": caps.max_concurrent_jobs,
                "current_jobs": state.jobs.values().filter(|j| !j.state.is_terminal()).count(),
                "disk_free_bytes": caps.disk_free_bytes,
                "disk_total_bytes": caps.disk_total_bytes,
            },
            "limits": {
                "max_upload_bytes": if caps.max_upload_bytes > 0 { json!(caps.max_upload_bytes) } else { json!(null) },
            }
        });

        RpcResponse::success(0, request.request_id.clone(), payload)
    }

    fn handle_reserve(&self, request: &RpcRequest) -> RpcResponse {
        let mut state = self.state.lock().unwrap();
        let caps = self.capabilities.lock().unwrap();

        // Check capacity
        if state.active_lease_count() >= caps.max_concurrent_jobs as usize {
            return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("BUSY", "Worker at capacity")
                    .with_data("retry_after_seconds", json!(30)),
            );
        }

        // Extract run_id from payload
        let run_id = request.payload.get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Get TTL from payload (default 1 hour)
        let ttl_seconds = request.payload.get("ttl_seconds")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);

        // Create lease
        let lease_id = state.next_id("lease");
        let lease = Lease::new(lease_id.clone(), run_id, ttl_seconds);
        let expires_at = lease.expires_at.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        state.leases.insert(lease_id.clone(), lease);

        RpcResponse::success(
            request.protocol_version,
            request.request_id.clone(),
            json!({
                "lease_id": lease_id,
                "expires_at": expires_at,
            }),
        )
    }

    fn handle_release(&self, request: &RpcRequest) -> RpcResponse {
        let mut state = self.state.lock().unwrap();

        let lease_id = request.payload.get("lease_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Release is idempotent - unknown/expired lease returns ok
        state.leases.remove(lease_id);

        RpcResponse::success(
            request.protocol_version,
            request.request_id.clone(),
            json!({}),
        )
    }

    fn handle_submit(&self, request: &RpcRequest) -> RpcResponse {
        let mut state = self.state.lock().unwrap();

        // Extract required fields
        let job_id = match request.payload.get("job_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id"),
            ),
        };

        let job_key = match request.payload.get("job_key").and_then(|v| v.as_str()) {
            Some(key) => key.to_string(),
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_key"),
            ),
        };

        let source_sha256 = match request.payload.get("source_sha256").and_then(|v| v.as_str()) {
            Some(sha) => sha.to_string(),
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: source_sha256"),
            ),
        };

        let lease_id = request.payload.get("lease_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Check for idempotency - same job_id
        if let Some(existing) = state.jobs.get(&job_id) {
            if existing.job_key == job_key {
                // Same job_id + same job_key → return existing
                return RpcResponse::success(
                    request.protocol_version,
                    request.request_id.clone(),
                    json!({
                        "job_id": job_id,
                        "state": existing.state,
                        "created_at": existing.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    }),
                );
            } else {
                // Same job_id + different job_key → reject
                return RpcResponse::error(
                    request.protocol_version,
                    request.request_id.clone(),
                    RpcErrorPayload::new("INVALID_REQUEST", "job_id already exists with different job_key"),
                );
            }
        }

        // Check if source exists
        if !state.has_source(&source_sha256) {
            return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("SOURCE_MISSING", "Source bundle not found")
                    .with_data("source_sha256", json!(source_sha256)),
            );
        }

        // Create job
        let job = Job::new(job_id.clone(), job_key, source_sha256, lease_id);
        let created_at = job.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        state.jobs.insert(job_id.clone(), job);

        RpcResponse::success(
            request.protocol_version,
            request.request_id.clone(),
            json!({
                "job_id": job_id,
                "state": JobState::Queued,
                "created_at": created_at,
            }),
        )
    }

    fn handle_status(&self, request: &RpcRequest) -> RpcResponse {
        let state = self.state.lock().unwrap();

        let job_id = match request.payload.get("job_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id"),
            ),
        };

        match state.jobs.get(job_id) {
            Some(job) => RpcResponse::success(
                request.protocol_version,
                request.request_id.clone(),
                json!({
                    "job_id": job_id,
                    "state": job.state,
                    "exit_code": job.exit_code,
                    "created_at": job.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    "updated_at": job.updated_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    "artifacts_available": job.artifacts_available,
                }),
            ),
            None => RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Job not found")
                    .with_data("job_id", json!(job_id)),
            ),
        }
    }

    fn handle_tail(&self, request: &RpcRequest) -> RpcResponse {
        let state = self.state.lock().unwrap();

        let job_id = match request.payload.get("job_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id"),
            ),
        };

        let cursor = request.payload.get("cursor")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let limit = request.payload.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;

        match state.jobs.get(job_id) {
            Some(job) => {
                let entries: Vec<_> = job.logs.iter()
                    .skip(cursor)
                    .take(limit)
                    .map(|e| json!({
                        "timestamp": e.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
                        "stream": e.stream,
                        "line": e.line,
                    }))
                    .collect();

                let next_cursor = if cursor + entries.len() < job.logs.len() {
                    Some(cursor + entries.len())
                } else if job.state.is_terminal() {
                    None // No more logs coming
                } else {
                    Some(job.logs.len()) // More logs may come
                };

                RpcResponse::success(
                    request.protocol_version,
                    request.request_id.clone(),
                    json!({
                        "entries": entries,
                        "next_cursor": next_cursor,
                    }),
                )
            }
            None => RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Job not found"),
            ),
        }
    }

    fn handle_cancel(&self, request: &RpcRequest) -> RpcResponse {
        let mut state = self.state.lock().unwrap();

        let job_id = match request.payload.get("job_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id"),
            ),
        };

        match state.jobs.get_mut(&job_id) {
            Some(job) => {
                if job.state.is_terminal() {
                    // Already terminal, return current state
                    return RpcResponse::success(
                        request.protocol_version,
                        request.request_id.clone(),
                        json!({
                            "job_id": job_id,
                            "state": job.state,
                            "already_terminal": true,
                        }),
                    );
                }

                // Transition to CANCEL_REQUESTED
                job.transition(JobState::CancelRequested);

                RpcResponse::success(
                    request.protocol_version,
                    request.request_id.clone(),
                    json!({
                        "job_id": job_id,
                        "state": job.state,
                    }),
                )
            }
            None => RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Job not found"),
            ),
        }
    }

    fn handle_fetch(&self, request: &RpcRequest) -> RpcResponse {
        let state = self.state.lock().unwrap();

        let job_id = match request.payload.get("job_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: job_id"),
            ),
        };

        // Check if job exists
        let job = match state.jobs.get(job_id) {
            Some(j) => j,
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Job not found"),
            ),
        };

        // Check if job is terminal
        if !job.state.is_terminal() {
            return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Job not in terminal state"),
            );
        }

        // Check if artifacts exist
        match state.artifacts.get(job_id) {
            Some(content) => {
                // In a real implementation, this would return binary-framed response
                // For the mock, we return metadata about the artifacts
                RpcResponse::success(
                    request.protocol_version,
                    request.request_id.clone(),
                    json!({
                        "job_id": job_id,
                        "stream": {
                            "content_length": content.len(),
                            "content_sha256": format!("mock-sha256-{}", job_id),
                            "compression": "none",
                            "format": "tar",
                        }
                    }),
                )
            }
            None => RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("ARTIFACTS_GONE", "Artifacts have been deleted"),
            ),
        }
    }

    fn handle_has_source(&self, request: &RpcRequest) -> RpcResponse {
        let state = self.state.lock().unwrap();

        let source_sha256 = match request.payload.get("source_sha256").and_then(|v| v.as_str()) {
            Some(sha) => sha,
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: source_sha256"),
            ),
        };

        RpcResponse::success(
            request.protocol_version,
            request.request_id.clone(),
            json!({
                "exists": state.has_source(source_sha256),
            }),
        )
    }

    fn handle_upload_source(&self, request: &RpcRequest) -> RpcResponse {
        let mut state = self.state.lock().unwrap();
        let caps = self.capabilities.lock().unwrap();
        let mut sessions = self.upload_sessions.lock().unwrap();

        let source_sha256 = match request.payload.get("source_sha256").and_then(|v| v.as_str()) {
            Some(sha) => sha.to_string(),
            None => return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("INVALID_REQUEST", "Missing required field: source_sha256"),
            ),
        };

        // Get content_length from stream metadata
        let content_length = request.payload.get("stream")
            .and_then(|s| s.get("content_length"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Check max_upload_bytes limit
        if caps.max_upload_bytes > 0 && content_length > caps.max_upload_bytes {
            return RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                RpcErrorPayload::new("PAYLOAD_TOO_LARGE", "Upload exceeds maximum allowed size")
                    .with_data("max_bytes", json!(caps.max_upload_bytes))
                    .with_data("content_length", json!(content_length)),
            );
        }

        // Check for resumable upload
        let (upload_id, offset) = if let Some(resume) = request.payload.get("resume") {
            let id = resume.get("upload_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let off = resume.get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            (id, off)
        } else {
            // Generate new upload_id
            let id = format!("upload-{}", state.next_id("upload"));
            (id, 0)
        };

        // Get or create upload session
        let session = sessions.entry(upload_id.clone()).or_insert_with(|| {
            MockUploadSession {
                upload_id: upload_id.clone(),
                source_sha256: source_sha256.clone(),
                content_length,
                received_bytes: Vec::with_capacity(content_length as usize),
            }
        });

        // In a real implementation, this would read the binary-framed stream
        // For the mock, we simulate receiving content
        let content = request.payload.get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_else(|| vec![0u8; (content_length - offset) as usize]);

        // Append content from offset
        if offset as usize == session.received_bytes.len() {
            session.received_bytes.extend_from_slice(&content);
        }

        let next_offset = session.received_bytes.len() as u64;
        let complete = next_offset >= content_length;

        // If complete, store the source
        if complete {
            state.store_source(source_sha256.clone(), session.received_bytes.clone());
            sessions.remove(&upload_id);
        }

        RpcResponse::success(
            request.protocol_version,
            request.request_id.clone(),
            json!({
                "source_sha256": source_sha256,
                "upload_id": upload_id,
                "next_offset": next_offset,
                "complete": complete,
                "stored": complete,
            }),
        )
    }

    /// Get an upload session (for testing)
    pub fn get_upload_session(&self, upload_id: &str) -> Option<MockUploadSession> {
        let sessions = self.upload_sessions.lock().unwrap();
        sessions.get(upload_id).cloned()
    }
}

impl Default for MockWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn make_request(op: Operation, protocol_version: i32, payload: Value) -> RpcRequest {
        RpcRequest {
            protocol_version,
            op,
            request_id: "test-req".to_string(),
            payload,
        }
    }

    #[test]
    fn test_probe() {
        let worker = MockWorker::new();
        let request = make_request(Operation::Probe, 0, json!({}));

        let response = worker.handle_request(&request);
        assert!(response.ok);
        assert_eq!(response.protocol_version, 0);

        let payload = response.payload.unwrap();
        assert_eq!(payload["kind"], "probe");
        assert!(payload["protocol_min"].is_number());
        assert!(payload["protocol_max"].is_number());
    }

    #[test]
    fn test_probe_wrong_version() {
        let worker = MockWorker::new();
        let request = make_request(Operation::Probe, 1, json!({}));

        let response = worker.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
    }

    #[test]
    fn test_reserve_and_release() {
        let worker = MockWorker::new();

        // Reserve
        let request = make_request(Operation::Reserve, 1, json!({"run_id": "run-001"}));
        let response = worker.handle_request(&request);
        assert!(response.ok);

        let lease_id = response.payload.unwrap()["lease_id"].as_str().unwrap().to_string();

        // Release
        let request = make_request(Operation::Release, 1, json!({"lease_id": lease_id}));
        let response = worker.handle_request(&request);
        assert!(response.ok);

        // Release again (idempotent)
        let response = worker.handle_request(&request);
        assert!(response.ok);
    }

    #[test]
    fn test_reserve_busy() {
        let worker = MockWorker::new();
        worker.set_capacity(1);

        // First reserve should succeed
        let request = make_request(Operation::Reserve, 1, json!({"run_id": "run-001"}));
        let response = worker.handle_request(&request);
        assert!(response.ok);

        // Second reserve should fail with BUSY
        let request = make_request(Operation::Reserve, 1, json!({"run_id": "run-002"}));
        let response = worker.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "BUSY");
    }

    #[test]
    fn test_submit_source_missing() {
        let worker = MockWorker::new();

        let request = make_request(Operation::Submit, 1, json!({
            "job_id": "job-001",
            "job_key": "key-abc",
            "source_sha256": "sha256-xyz",
        }));

        let response = worker.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "SOURCE_MISSING");
    }

    #[test]
    fn test_submit_success() {
        let worker = MockWorker::new();
        worker.store_source("sha256-xyz", vec![1, 2, 3]);

        let request = make_request(Operation::Submit, 1, json!({
            "job_id": "job-001",
            "job_key": "key-abc",
            "source_sha256": "sha256-xyz",
        }));

        let response = worker.handle_request(&request);
        assert!(response.ok);
        assert_eq!(response.payload.unwrap()["state"], "QUEUED");
    }

    #[test]
    fn test_submit_idempotency() {
        let worker = MockWorker::new();
        worker.store_source("sha256-xyz", vec![1, 2, 3]);

        let request = make_request(Operation::Submit, 1, json!({
            "job_id": "job-001",
            "job_key": "key-abc",
            "source_sha256": "sha256-xyz",
        }));

        // First submit
        let response = worker.handle_request(&request);
        assert!(response.ok);

        // Same job_id + same job_key → return existing
        let response = worker.handle_request(&request);
        assert!(response.ok);

        // Same job_id + different job_key → reject
        let request = make_request(Operation::Submit, 1, json!({
            "job_id": "job-001",
            "job_key": "different-key",
            "source_sha256": "sha256-xyz",
        }));
        let response = worker.handle_request(&request);
        assert!(!response.ok);
        assert!(response.error.unwrap().message.contains("different job_key"));
    }

    #[test]
    fn test_has_source() {
        let worker = MockWorker::new();

        // Not found
        let request = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-xyz"}));
        let response = worker.handle_request(&request);
        assert!(response.ok);
        assert!(!response.payload.unwrap()["exists"].as_bool().unwrap());

        // Store source
        worker.store_source("sha256-xyz", vec![1, 2, 3]);

        // Found
        let response = worker.handle_request(&request);
        assert!(response.ok);
        assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
    }

    #[test]
    fn test_failure_injection() {
        let worker = MockWorker::new();

        // Inject failure
        worker.inject_error(Operation::Probe, "TEST_ERROR", "Injected test error");

        let request = make_request(Operation::Probe, 0, json!({}));
        let response = worker.handle_request(&request);

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "TEST_ERROR");

        // Clear and try again
        worker.clear_failures();
        let response = worker.handle_request(&request);
        assert!(response.ok);
    }

    #[test]
    fn test_protocol_version_non_probe() {
        let worker = MockWorker::new();

        // Non-probe with version 0 should fail
        let request = make_request(Operation::Status, 0, json!({"job_id": "test"}));
        let response = worker.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "UNSUPPORTED_PROTOCOL");
    }
}
