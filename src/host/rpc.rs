//! Host RPC Client
//!
//! Implements the host-side RPC client for communicating with workers.
//! Handles protocol negotiation, retry logic, and error mapping.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::envelope::{Operation, RpcErrorPayload, RpcRequest, RpcResponse};

use super::transport::{Transport, TransportError};

/// RPC client configuration
#[derive(Debug, Clone)]
pub struct RpcClientConfig {
    /// Connection timeout in seconds
    pub connect_timeout_seconds: u32,
    /// Maximum retries for SSH connection
    pub ssh_connect_retries: u32,
    /// Initial retry delay in milliseconds
    pub retry_initial_delay_ms: u64,
    /// Maximum retry delay in milliseconds
    pub retry_max_delay_ms: u64,
    /// Maximum retries for upload operations
    pub upload_retries: u32,
    /// Maximum retries on BUSY response
    pub busy_retries: u32,
}

impl Default for RpcClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 30,
            ssh_connect_retries: 3,
            retry_initial_delay_ms: 2000,
            retry_max_delay_ms: 30000,
            upload_retries: 3,
            busy_retries: 3,
        }
    }
}

/// RPC client errors
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Worker incompatible: {0}")]
    WorkerIncompatible(String),

    #[error("Worker busy: retry after {retry_after_seconds}s")]
    WorkerBusy { retry_after_seconds: u32 },

    #[error("Source missing: {source_sha256}")]
    SourceMissing { source_sha256: String },

    #[error("Artifacts gone for job {job_id}")]
    ArtifactsGone { job_id: String },

    #[error("Lease expired: {lease_id}")]
    LeaseExpired { lease_id: String },

    #[error("Payload too large: max {max_bytes} bytes")]
    PayloadTooLarge { max_bytes: u64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Version negotiation failed: host [{host_min},{host_max}] vs worker [{worker_min},{worker_max}]")]
    VersionNegotiationFailed {
        host_min: i32,
        host_max: i32,
        worker_min: i32,
        worker_max: i32,
    },

    #[error("Max retries exceeded")]
    MaxRetriesExceeded,
}

/// Failure kind for exit code mapping (per PLAN.md taxonomy)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// SSH/transport failures (exit code 20)
    Ssh = 20,
    /// Executor failures (exit code 40)
    Executor = 40,
    /// Artifact failures (exit code 70)
    ArtifactsFailed = 70,
    /// Worker busy (exit code 90)
    WorkerBusy = 90,
    /// Worker incompatible (exit code 91)
    WorkerIncompatible = 91,
    /// Bundler failures (exit code 92)
    Bundler = 92,
}

impl RpcError {
    /// Map error to failure kind for exit code
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            RpcError::Transport(_) => FailureKind::Ssh,
            RpcError::Protocol(_) => FailureKind::Ssh,
            RpcError::WorkerIncompatible(_) => FailureKind::WorkerIncompatible,
            RpcError::VersionNegotiationFailed { .. } => FailureKind::WorkerIncompatible,
            RpcError::WorkerBusy { .. } => FailureKind::WorkerBusy,
            RpcError::SourceMissing { .. } => FailureKind::Executor,
            RpcError::ArtifactsGone { .. } => FailureKind::ArtifactsFailed,
            RpcError::LeaseExpired { .. } => FailureKind::Executor,
            RpcError::PayloadTooLarge { .. } => FailureKind::Bundler,
            RpcError::InvalidRequest(_) => FailureKind::Executor,
            RpcError::MaxRetriesExceeded => FailureKind::WorkerBusy,
        }
    }

    /// Get exit code for this error
    pub fn exit_code(&self) -> i32 {
        self.failure_kind() as i32
    }
}

/// Result type for RPC operations
pub type RpcResult<T> = Result<T, RpcError>;

/// Host RPC client
pub struct RpcClient {
    transport: Arc<dyn Transport>,
    config: RpcClientConfig,
    /// Negotiated protocol version (set after probe)
    negotiated_version: Option<i32>,
    /// Host supported protocol range
    host_protocol_min: i32,
    host_protocol_max: i32,
    /// Request ID counter
    request_counter: std::sync::atomic::AtomicU64,
}

impl RpcClient {
    /// Create a new RPC client with the given transport
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::with_config(transport, RpcClientConfig::default())
    }

    /// Create a new RPC client with custom configuration
    pub fn with_config(transport: Arc<dyn Transport>, config: RpcClientConfig) -> Self {
        Self {
            transport,
            config,
            negotiated_version: None,
            host_protocol_min: 1,
            host_protocol_max: 1,
            request_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Generate a unique request ID
    fn next_request_id(&self) -> String {
        let counter = self.request_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("req-{:x}-{:08x}", timestamp, counter)
    }

    /// Get the negotiated protocol version, or fail if not negotiated yet
    fn get_protocol_version(&self) -> RpcResult<i32> {
        self.negotiated_version.ok_or_else(|| {
            RpcError::Protocol("Protocol version not negotiated - call probe() first".to_string())
        })
    }

    /// Parse error response into RpcError
    fn parse_error(&self, error: &RpcErrorPayload, context: &str) -> RpcError {
        let code = error.code.as_str();
        match code {
            "BUSY" => {
                let retry_after = error.data.as_ref()
                    .and_then(|d| d.get("retry_after_seconds"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30) as u32;
                RpcError::WorkerBusy { retry_after_seconds: retry_after }
            }
            "UNSUPPORTED_PROTOCOL" | "FEATURE_MISSING" => {
                RpcError::WorkerIncompatible(error.message.clone())
            }
            "SOURCE_MISSING" => {
                let sha = error.data.as_ref()
                    .and_then(|d| d.get("source_sha256"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                RpcError::SourceMissing { source_sha256: sha }
            }
            "ARTIFACTS_GONE" => {
                RpcError::ArtifactsGone { job_id: context.to_string() }
            }
            "LEASE_EXPIRED" => {
                RpcError::LeaseExpired { lease_id: context.to_string() }
            }
            "PAYLOAD_TOO_LARGE" => {
                let max = error.data.as_ref()
                    .and_then(|d| d.get("max_bytes"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                RpcError::PayloadTooLarge { max_bytes: max }
            }
            "INVALID_REQUEST" => {
                RpcError::InvalidRequest(error.message.clone())
            }
            _ => RpcError::Protocol(format!("{}: {}", code, error.message))
        }
    }

    // === Public RPC Operations ===

    /// Probe the worker for capabilities
    ///
    /// This must be called first to negotiate protocol version.
    pub fn probe(&mut self) -> RpcResult<Value> {
        let request = RpcRequest {
            protocol_version: 0, // probe always uses v0
            op: Operation::Probe,
            request_id: self.next_request_id(),
            payload: json!({}),
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Probe failed with no error details")
            });
            return Err(self.parse_error(&error, "probe"));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Probe response missing payload".to_string())
        })?;

        // Extract protocol range from capabilities
        let worker_min = payload.get("protocol_min")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;
        let worker_max = payload.get("protocol_max")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        // Negotiate version: use max of intersection
        let intersection_min = self.host_protocol_min.max(worker_min);
        let intersection_max = self.host_protocol_max.min(worker_max);

        if intersection_min > intersection_max {
            return Err(RpcError::VersionNegotiationFailed {
                host_min: self.host_protocol_min,
                host_max: self.host_protocol_max,
                worker_min,
                worker_max,
            });
        }

        self.negotiated_version = Some(intersection_max);
        Ok(payload)
    }

    /// Reserve a lease on the worker
    pub fn reserve(&self, run_id: &str, ttl_seconds: Option<i64>) -> RpcResult<ReserveResponse> {
        let version = self.get_protocol_version()?;

        let mut payload = json!({"run_id": run_id});
        if let Some(ttl) = ttl_seconds {
            payload["ttl_seconds"] = json!(ttl);
        }

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Reserve,
            request_id: self.next_request_id(),
            payload,
        };

        let response = self.execute_with_busy_retry(&request)?;

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Reserve response missing payload".to_string())
        })?;

        Ok(ReserveResponse {
            lease_id: payload["lease_id"].as_str().unwrap_or("").to_string(),
            expires_at: payload["expires_at"].as_str().unwrap_or("").to_string(),
        })
    }

    /// Release a lease
    pub fn release(&self, lease_id: &str) -> RpcResult<()> {
        let version = self.get_protocol_version()?;

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Release,
            request_id: self.next_request_id(),
            payload: json!({"lease_id": lease_id}),
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Release failed")
            });
            return Err(self.parse_error(&error, lease_id));
        }

        Ok(())
    }

    /// Check if worker has a source bundle
    pub fn has_source(&self, source_sha256: &str) -> RpcResult<bool> {
        let version = self.get_protocol_version()?;

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::HasSource,
            request_id: self.next_request_id(),
            payload: json!({"source_sha256": source_sha256}),
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "HasSource failed")
            });
            return Err(self.parse_error(&error, source_sha256));
        }

        let exists = response.payload
            .and_then(|p| p.get("exists").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        Ok(exists)
    }

    /// Upload a source bundle to the worker
    pub fn upload_source(&self, source_sha256: &str, content: &[u8]) -> RpcResult<()> {
        self.upload_source_resumable(source_sha256, content, None)
            .map(|_| ())
    }

    /// Upload a source bundle to the worker with optional resume support
    ///
    /// If the worker supports `upload_resumable` feature and a resume request is provided,
    /// the upload will resume from the specified offset.
    pub fn upload_source_resumable(
        &self,
        source_sha256: &str,
        content: &[u8],
        resume: Option<super::resumable::ResumeRequest>,
    ) -> RpcResult<UploadSourceResponse> {
        let version = self.get_protocol_version()?;

        let mut payload = json!({
            "source_sha256": source_sha256,
            "stream": {
                "content_length": content.len(),
                "content_sha256": format!("sha256-{}", source_sha256),
                "compression": "none",
                "format": "tar"
            }
        });

        // Add resume info if provided
        let (upload_content, offset) = if let Some(ref r) = resume {
            payload["resume"] = json!({
                "upload_id": r.upload_id,
                "offset": r.offset
            });
            // Only send content from the offset onwards
            let start = r.offset as usize;
            if start >= content.len() {
                // Already complete
                return Ok(UploadSourceResponse {
                    upload_id: Some(r.upload_id.clone()),
                    next_offset: content.len() as u64,
                    complete: true,
                });
            }
            (&content[start..], r.offset)
        } else {
            (content, 0)
        };

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::UploadSource,
            request_id: self.next_request_id(),
            payload,
        };

        // Retry upload on transient failures
        let mut retries = 0;
        loop {
            match self.transport.execute_framed(&request, upload_content) {
                Ok(response) => {
                    if response.ok {
                        let payload = response.payload.unwrap_or(json!({}));
                        return Ok(UploadSourceResponse {
                            upload_id: payload.get("upload_id").and_then(|v| v.as_str()).map(String::from),
                            next_offset: payload.get("next_offset").and_then(|v| v.as_u64()).unwrap_or(offset + upload_content.len() as u64),
                            complete: payload.get("complete").and_then(|v| v.as_bool()).unwrap_or(true),
                        });
                    }
                    let error = response.error.unwrap_or_else(|| {
                        RpcErrorPayload::new("UNKNOWN", "Upload failed")
                    });
                    return Err(self.parse_error(&error, source_sha256));
                }
                Err(_) if retries < self.config.upload_retries => {
                    retries += 1;
                    let delay = self.calculate_backoff(retries);
                    std::thread::sleep(Duration::from_millis(delay));
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Submit a job to the worker
    ///
    /// Note: This operation is NOT automatically retried. Retries require a new run.
    pub fn submit(&self, job_id: &str, job_key: &str, source_sha256: &str, lease_id: Option<&str>) -> RpcResult<SubmitResponse> {
        let version = self.get_protocol_version()?;

        let mut payload = json!({
            "job_id": job_id,
            "job_key": job_key,
            "source_sha256": source_sha256
        });

        if let Some(lid) = lease_id {
            payload["lease_id"] = json!(lid);
        }

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Submit,
            request_id: self.next_request_id(),
            payload,
        };

        // Submit is NOT retried automatically (non-idempotent)
        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Submit failed")
            });
            return Err(self.parse_error(&error, job_id));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Submit response missing payload".to_string())
        })?;

        Ok(SubmitResponse {
            job_id: payload["job_id"].as_str().unwrap_or(job_id).to_string(),
            state: payload["state"].as_str().unwrap_or("QUEUED").to_string(),
            created_at: payload["created_at"].as_str().unwrap_or("").to_string(),
        })
    }

    /// Get job status
    pub fn status(&self, job_id: &str) -> RpcResult<StatusResponse> {
        let version = self.get_protocol_version()?;

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Status,
            request_id: self.next_request_id(),
            payload: json!({"job_id": job_id}),
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Status failed")
            });
            return Err(self.parse_error(&error, job_id));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Status response missing payload".to_string())
        })?;

        Ok(StatusResponse {
            job_id: payload["job_id"].as_str().unwrap_or(job_id).to_string(),
            state: payload["state"].as_str().unwrap_or("UNKNOWN").to_string(),
            exit_code: payload.get("exit_code").and_then(|v| v.as_i64()).map(|c| c as i32),
            created_at: payload["created_at"].as_str().unwrap_or("").to_string(),
            updated_at: payload["updated_at"].as_str().unwrap_or("").to_string(),
            artifacts_available: payload.get("artifacts_available").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }

    /// Tail job logs
    pub fn tail(&self, job_id: &str, cursor: Option<u64>, limit: Option<u64>) -> RpcResult<TailResponse> {
        let version = self.get_protocol_version()?;

        let mut payload = json!({"job_id": job_id});
        if let Some(c) = cursor {
            payload["cursor"] = json!(c);
        }
        if let Some(l) = limit {
            payload["limit"] = json!(l);
        }

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Tail,
            request_id: self.next_request_id(),
            payload,
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Tail failed")
            });
            return Err(self.parse_error(&error, job_id));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Tail response missing payload".to_string())
        })?;

        let entries = payload["entries"]
            .as_array()
            .map(|arr| arr.iter().map(|e| LogEntry {
                timestamp: e["timestamp"].as_str().unwrap_or("").to_string(),
                stream: e["stream"].as_str().unwrap_or("").to_string(),
                line: e["line"].as_str().unwrap_or("").to_string(),
            }).collect())
            .unwrap_or_default();

        Ok(TailResponse {
            entries,
            next_cursor: payload.get("next_cursor").and_then(|v| v.as_u64()),
        })
    }

    /// Cancel a job
    pub fn cancel(&self, job_id: &str, reason: Option<CancelReason>) -> RpcResult<CancelResponse> {
        let version = self.get_protocol_version()?;

        let mut payload = json!({"job_id": job_id});
        if let Some(r) = reason {
            payload["reason"] = json!(r.as_str());
        }

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Cancel,
            request_id: self.next_request_id(),
            payload,
        };

        let response = self.transport.execute(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Cancel failed")
            });
            return Err(self.parse_error(&error, job_id));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Cancel response missing payload".to_string())
        })?;

        Ok(CancelResponse {
            job_id: payload["job_id"].as_str().unwrap_or(job_id).to_string(),
            state: payload["state"].as_str().unwrap_or("UNKNOWN").to_string(),
            already_terminal: payload.get("already_terminal").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }

    /// Fetch job artifacts
    pub fn fetch(&self, job_id: &str) -> RpcResult<FetchResponse> {
        let version = self.get_protocol_version()?;

        let request = RpcRequest {
            protocol_version: version,
            op: Operation::Fetch,
            request_id: self.next_request_id(),
            payload: json!({"job_id": job_id}),
        };

        let (response, content) = self.transport.execute_fetch(&request)?;

        if !response.ok {
            let error = response.error.unwrap_or_else(|| {
                RpcErrorPayload::new("UNKNOWN", "Fetch failed")
            });
            return Err(self.parse_error(&error, job_id));
        }

        let payload = response.payload.ok_or_else(|| {
            RpcError::Protocol("Fetch response missing payload".to_string())
        })?;

        Ok(FetchResponse {
            job_id: payload["job_id"].as_str().unwrap_or(job_id).to_string(),
            content,
            stream_metadata: payload.get("stream").cloned(),
        })
    }

    // === Internal Helpers ===

    /// Execute a request with BUSY retry handling
    fn execute_with_busy_retry(&self, request: &RpcRequest) -> RpcResult<RpcResponse> {
        let mut retries = 0;

        loop {
            let response = self.transport.execute(request)?;

            if response.ok {
                return Ok(response);
            }

            let error = response.error.as_ref().unwrap();
            if error.code == "BUSY" && retries < self.config.busy_retries {
                retries += 1;
                let retry_after = error.data.as_ref()
                    .and_then(|d| d.get("retry_after_seconds"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30);

                let delay = (retry_after * 1000).min(self.config.retry_max_delay_ms);
                std::thread::sleep(Duration::from_millis(delay));
                continue;
            }

            return Ok(response);
        }
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff(&self, attempt: u32) -> u64 {
        let base = self.config.retry_initial_delay_ms;
        let delay = base * 2u64.pow(attempt - 1);
        delay.min(self.config.retry_max_delay_ms)
    }
}

// === Response Types ===

/// Reserve operation response
#[derive(Debug, Clone)]
pub struct ReserveResponse {
    pub lease_id: String,
    pub expires_at: String,
}

/// Submit operation response
#[derive(Debug, Clone)]
pub struct SubmitResponse {
    pub job_id: String,
    pub state: String,
    pub created_at: String,
}

/// Status operation response
#[derive(Debug, Clone)]
pub struct StatusResponse {
    pub job_id: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
    pub artifacts_available: bool,
}

/// Tail operation response
#[derive(Debug, Clone)]
pub struct TailResponse {
    pub entries: Vec<LogEntry>,
    pub next_cursor: Option<u64>,
}

/// Log entry from tail
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub stream: String,
    pub line: String,
}

/// Cancel operation response
#[derive(Debug, Clone)]
pub struct CancelResponse {
    pub job_id: String,
    pub state: String,
    pub already_terminal: bool,
}

/// Fetch operation response
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub job_id: String,
    pub content: Option<Vec<u8>>,
    pub stream_metadata: Option<Value>,
}

/// Upload source response (for resumable uploads)
#[derive(Debug, Clone)]
pub struct UploadSourceResponse {
    /// Upload session ID (for resumable uploads)
    pub upload_id: Option<String>,
    /// Next offset to resume from
    pub next_offset: u64,
    /// Whether upload is complete
    pub complete: bool,
}

/// Cancel reason
#[derive(Debug, Clone, Copy)]
pub enum CancelReason {
    User,
    TimeoutOverall,
    TimeoutIdle,
    Signal,
}

impl CancelReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            CancelReason::User => "USER",
            CancelReason::TimeoutOverall => "TIMEOUT_OVERALL",
            CancelReason::TimeoutIdle => "TIMEOUT_IDLE",
            CancelReason::Signal => "SIGNAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::transport::MockTransport;

    fn create_client() -> RpcClient {
        let transport = Arc::new(MockTransport::new());
        RpcClient::new(transport)
    }

    #[test]
    fn test_probe_and_negotiate() {
        let mut client = create_client();

        let capabilities = client.probe().unwrap();

        assert!(capabilities.get("protocol_min").is_some());
        assert!(capabilities.get("protocol_max").is_some());
        assert!(client.negotiated_version.is_some());
    }

    #[test]
    fn test_reserve_requires_probe_first() {
        let client = create_client();

        let result = client.reserve("run-001", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_source() {
        let transport = MockTransport::new();
        transport.worker().store_source("sha256-test", vec![1, 2, 3]);

        let mut client = RpcClient::new(Arc::new(transport));
        client.probe().unwrap();

        let exists = client.has_source("sha256-test").unwrap();
        assert!(exists);

        let exists = client.has_source("sha256-missing").unwrap();
        assert!(!exists);
    }

    #[test]
    fn test_reserve_and_release() {
        let mut client = create_client();
        client.probe().unwrap();

        let reserve = client.reserve("run-001", Some(3600)).unwrap();
        assert!(!reserve.lease_id.is_empty());

        // Release should succeed
        client.release(&reserve.lease_id).unwrap();

        // Release again should also succeed (idempotent)
        client.release(&reserve.lease_id).unwrap();
    }

    #[test]
    fn test_submit_source_missing() {
        let mut client = create_client();
        client.probe().unwrap();

        let result = client.submit("job-001", "key-001", "missing-sha", None);
        assert!(matches!(result, Err(RpcError::SourceMissing { .. })));
    }

    #[test]
    fn test_submit_success() {
        let transport = MockTransport::new();
        transport.worker().store_source("sha256-test", vec![1, 2, 3]);

        let mut client = RpcClient::new(Arc::new(transport));
        client.probe().unwrap();

        let response = client.submit("job-001", "key-001", "sha256-test", None).unwrap();
        assert_eq!(response.job_id, "job-001");
        assert_eq!(response.state, "QUEUED");
    }

    #[test]
    fn test_status() {
        let transport = MockTransport::new();
        transport.worker().store_source("sha256-test", vec![1, 2, 3]);

        let mut client = RpcClient::new(Arc::new(transport));
        client.probe().unwrap();

        client.submit("job-status", "key-001", "sha256-test", None).unwrap();

        let status = client.status("job-status").unwrap();
        assert_eq!(status.job_id, "job-status");
    }

    #[test]
    fn test_cancel() {
        let transport = MockTransport::new();
        transport.worker().store_source("sha256-test", vec![1, 2, 3]);

        let mut client = RpcClient::new(Arc::new(transport));
        client.probe().unwrap();

        client.submit("job-cancel", "key-001", "sha256-test", None).unwrap();

        let response = client.cancel("job-cancel", Some(CancelReason::User)).unwrap();
        assert_eq!(response.job_id, "job-cancel");
        assert_eq!(response.state, "CANCEL_REQUESTED");
    }

    #[test]
    fn test_error_mapping() {
        let err = RpcError::WorkerBusy { retry_after_seconds: 30 };
        assert_eq!(err.failure_kind(), FailureKind::WorkerBusy);
        assert_eq!(err.exit_code(), 90);

        let err = RpcError::SourceMissing { source_sha256: "test".to_string() };
        assert_eq!(err.failure_kind(), FailureKind::Executor);
        assert_eq!(err.exit_code(), 40);

        let err = RpcError::ArtifactsGone { job_id: "test".to_string() };
        assert_eq!(err.failure_kind(), FailureKind::ArtifactsFailed);
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn test_request_id_generation() {
        let client = create_client();

        let id1 = client.next_request_id();
        let id2 = client.next_request_id();

        assert_ne!(id1, id2);
        assert!(id1.starts_with("req-"));
    }
}
