//! Transport Layer for RPC Client
//!
//! Abstracts the SSH connection for testability. Provides:
//! - Transport trait: interface for RPC communication
//! - MockTransport: in-process mock worker for unit tests
//! - SshTransport: real SSH connection for production

use std::io;

use crate::mock::MockWorker;
use crate::protocol::envelope::{RpcRequest, RpcResponse};

/// Transport trait for RPC communication
pub trait Transport: Send + Sync {
    /// Execute an RPC request and return the response
    fn execute(&self, request: &RpcRequest) -> Result<RpcResponse, TransportError>;

    /// Execute a binary-framed request (for upload_source)
    fn execute_framed(
        &self,
        request: &RpcRequest,
        content: &[u8],
    ) -> Result<RpcResponse, TransportError>;

    /// Execute a fetch request and return the response with binary content
    fn execute_fetch(
        &self,
        request: &RpcRequest,
    ) -> Result<(RpcResponse, Option<Vec<u8>>), TransportError>;
}

/// Transport errors
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("SSH error: {0}")]
    Ssh(String),
}

/// Mock transport for testing - connects directly to MockWorker in-process
pub struct MockTransport {
    worker: MockWorker,
}

impl MockTransport {
    /// Create a new mock transport with a fresh mock worker
    pub fn new() -> Self {
        Self {
            worker: MockWorker::new(),
        }
    }

    /// Create a mock transport with a pre-configured worker
    pub fn with_worker(worker: MockWorker) -> Self {
        Self { worker }
    }

    /// Get a reference to the underlying mock worker for test configuration
    pub fn worker(&self) -> &MockWorker {
        &self.worker
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for MockTransport {
    fn execute(&self, request: &RpcRequest) -> Result<RpcResponse, TransportError> {
        Ok(self.worker.handle_request(request))
    }

    fn execute_framed(
        &self,
        request: &RpcRequest,
        _content: &[u8],
    ) -> Result<RpcResponse, TransportError> {
        // Mock doesn't need actual binary framing - the MockWorker handles it
        Ok(self.worker.handle_request(request))
    }

    fn execute_fetch(
        &self,
        request: &RpcRequest,
    ) -> Result<(RpcResponse, Option<Vec<u8>>), TransportError> {
        let response = self.worker.handle_request(request);
        // Mock returns empty artifact content for now
        let content = if response.ok { Some(vec![]) } else { None };
        Ok((response, content))
    }
}

/// SSH transport configuration
#[derive(Debug, Clone)]
pub struct SshConfig {
    /// Remote host
    pub host: String,
    /// SSH user
    pub user: String,
    /// SSH port (default 22)
    pub port: u16,
    /// Path to SSH private key
    pub key_path: Option<String>,
    /// Connection timeout in seconds
    pub connect_timeout_seconds: u32,
    /// Server alive interval for detecting dead connections
    pub server_alive_interval: u32,
    /// Server alive count max
    pub server_alive_count_max: u32,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: "rch".to_string(),
            port: 22,
            key_path: None,
            connect_timeout_seconds: 30,
            server_alive_interval: 15,
            server_alive_count_max: 2,
        }
    }
}

/// SSH transport for production use
///
/// Executes RPC requests over SSH using forced-command execution.
/// Format: Single JSON request on stdin → single JSON response on stdout.
pub struct SshTransport {
    config: SshConfig,
}

impl SshTransport {
    /// Create a new SSH transport with the given configuration
    pub fn new(config: SshConfig) -> Self {
        Self { config }
    }

    /// Build SSH command arguments
    fn build_ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".to_string(),
            format!("ConnectTimeout={}", self.config.connect_timeout_seconds),
            "-o".to_string(),
            format!("ServerAliveInterval={}", self.config.server_alive_interval),
            "-o".to_string(),
            format!("ServerAliveCountMax={}", self.config.server_alive_count_max),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-p".to_string(),
            self.config.port.to_string(),
        ];

        if let Some(ref key_path) = self.config.key_path {
            args.push("-i".to_string());
            args.push(key_path.clone());
        }

        args.push(format!("{}@{}", self.config.user, self.config.host));
        args.push("rch-worker".to_string());
        args.push("xcode".to_string());
        args.push("rpc".to_string());

        args
    }
}

impl Transport for SshTransport {
    fn execute(&self, request: &RpcRequest) -> Result<RpcResponse, TransportError> {
        use std::process::{Command, Stdio};

        let args = self.build_ssh_args();
        let request_json = serde_json::to_string(request)?;

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TransportError::Ssh(format!("Failed to spawn SSH: {}", e)))?;

        // Write request to stdin
        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            writeln!(stdin, "{}", request_json)?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| TransportError::Ssh(format!("SSH process error: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TransportError::Ssh(format!(
                "SSH exited with {}: {}",
                output.status, stderr
            )));
        }

        let response: RpcResponse = serde_json::from_slice(&output.stdout)
            .map_err(|e| TransportError::Protocol(format!("Invalid response JSON: {}", e)))?;

        Ok(response)
    }

    fn execute_framed(
        &self,
        request: &RpcRequest,
        content: &[u8],
    ) -> Result<RpcResponse, TransportError> {
        use std::process::{Command, Stdio};

        let args = self.build_ssh_args();
        let request_json = serde_json::to_string(request)?;

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TransportError::Ssh(format!("Failed to spawn SSH: {}", e)))?;

        // Write framed request: JSON header line + binary content
        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            writeln!(stdin, "{}", request_json)?;
            stdin.write_all(content)?;
            stdin.flush()?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| TransportError::Ssh(format!("SSH process error: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TransportError::Ssh(format!(
                "SSH exited with {}: {}",
                output.status, stderr
            )));
        }

        let response: RpcResponse = serde_json::from_slice(&output.stdout)
            .map_err(|e| TransportError::Protocol(format!("Invalid response JSON: {}", e)))?;

        Ok(response)
    }

    fn execute_fetch(
        &self,
        request: &RpcRequest,
    ) -> Result<(RpcResponse, Option<Vec<u8>>), TransportError> {
        use std::process::{Command, Stdio};

        let args = self.build_ssh_args();
        let request_json = serde_json::to_string(request)?;

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TransportError::Ssh(format!("Failed to spawn SSH: {}", e)))?;

        // Write request to stdin
        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            writeln!(stdin, "{}", request_json)?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| TransportError::Ssh(format!("SSH process error: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TransportError::Ssh(format!(
                "SSH exited with {}: {}",
                output.status, stderr
            )));
        }

        // Parse binary-framed response: JSON header line + binary content
        // Find the first newline to split header from content
        let stdout = &output.stdout;
        if let Some(newline_pos) = stdout.iter().position(|&b| b == b'\n') {
            let header = &stdout[..newline_pos];
            let content = &stdout[newline_pos + 1..];

            let response: RpcResponse = serde_json::from_slice(header)
                .map_err(|e| TransportError::Protocol(format!("Invalid response header: {}", e)))?;

            let binary_content = if response.ok && !content.is_empty() {
                Some(content.to_vec())
            } else {
                None
            };

            Ok((response, binary_content))
        } else {
            // No binary content, just JSON response
            let response: RpcResponse = serde_json::from_slice(stdout)
                .map_err(|e| TransportError::Protocol(format!("Invalid response JSON: {}", e)))?;
            Ok((response, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::envelope::Operation;
    use serde_json::json;

    fn make_request(op: Operation, protocol_version: i32, payload: serde_json::Value) -> RpcRequest {
        RpcRequest {
            protocol_version,
            op,
            request_id: "test-001".to_string(),
            payload,
        }
    }

    #[test]
    fn test_mock_transport_execute() {
        let transport = MockTransport::new();
        let request = make_request(Operation::Probe, 0, json!({}));

        let response = transport.execute(&request).unwrap();
        assert!(response.ok);
        assert_eq!(response.protocol_version, 0);
    }

    #[test]
    fn test_mock_transport_with_worker_config() {
        let worker = MockWorker::new();
        worker.store_source("sha256-test", vec![1, 2, 3]);

        let transport = MockTransport::with_worker(worker);

        // Has source check
        let request = make_request(Operation::HasSource, 1, json!({"source_sha256": "sha256-test"}));
        let response = transport.execute(&request).unwrap();
        assert!(response.ok);
        assert!(response.payload.unwrap()["exists"].as_bool().unwrap());
    }

    #[test]
    fn test_ssh_config_defaults() {
        let config = SshConfig::default();
        assert_eq!(config.port, 22);
        assert_eq!(config.user, "rch");
        assert_eq!(config.connect_timeout_seconds, 30);
    }
}
