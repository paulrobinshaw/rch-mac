//! Worker RPC Handler
//!
//! Implements the stdin/stdout JSON RPC handler for the worker entrypoint.
//! This is the main entry point invoked via SSH forced-command:
//!
//!   rch-worker xcode rpc
//!
//! The handler reads a single JSON request from stdin, dispatches to the
//! appropriate operation handler, and writes a single JSON response to stdout.

use std::io::{self, BufRead, Write};

use crate::protocol::{
    envelope::{Operation, RpcErrorPayload, RpcRequest, RpcResponse},
    errors::RpcError,
};

use super::probe::ProbeHandler;

/// Worker configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Minimum supported protocol version
    pub protocol_min: i32,
    /// Maximum supported protocol version
    pub protocol_max: i32,
    /// Maximum concurrent jobs
    pub max_concurrent_jobs: u32,
    /// Maximum upload size in bytes
    pub max_upload_bytes: u64,
    /// Supported features
    pub features: Vec<String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            protocol_min: 1,
            protocol_max: 1,
            max_concurrent_jobs: 1,
            max_upload_bytes: 1024 * 1024 * 1024, // 1 GB
            features: vec![
                "probe".to_string(),
                "has_source".to_string(),
                "upload_source".to_string(),
                "tail".to_string(),
            ],
        }
    }
}

/// Main RPC handler for the worker
pub struct RpcHandler {
    config: WorkerConfig,
    probe_handler: ProbeHandler,
}

impl RpcHandler {
    /// Create a new RPC handler with the given configuration
    pub fn new(config: WorkerConfig) -> Self {
        Self {
            config,
            probe_handler: ProbeHandler::new(),
        }
    }

    /// Run the RPC handler, reading from stdin and writing to stdout
    pub fn run(&self) -> io::Result<()> {
        self.run_with_io(&mut io::stdin().lock(), &mut io::stdout().lock())
    }

    /// Run the RPC handler with custom I/O (for testing)
    pub fn run_with_io<R: BufRead, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<()> {
        // Read the request
        let request = match self.read_request(reader) {
            Ok(req) => req,
            Err(e) => {
                // On parse error, return an error response with protocol_version: 0
                let response = RpcResponse::error(
                    0,
                    String::new(),
                    RpcErrorPayload::new("INVALID_REQUEST", e.to_string()),
                );
                self.write_response(writer, &response)?;
                return Ok(());
            }
        };

        // Validate protocol version
        if let Err(e) = self.validate_protocol_version(&request) {
            let response = RpcResponse::error(
                request.protocol_version,
                request.request_id.clone(),
                e.to_payload(),
            );
            self.write_response(writer, &response)?;
            return Ok(());
        }

        // Dispatch to operation handler
        let response = self.dispatch(&request);
        self.write_response(writer, &response)?;

        Ok(())
    }

    /// Read and parse the RPC request from the reader
    fn read_request<R: BufRead>(&self, reader: &mut R) -> Result<RpcRequest, RpcError> {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| RpcError::InvalidRequest(format!("Failed to read request: {}", e)))?;

        // Check for binary framing: if this looks like it might be a framed request,
        // we need to parse the header first to check for payload.stream
        let request: RpcRequest = serde_json::from_str(&line)
            .map_err(|e| RpcError::InvalidRequest(format!("Invalid JSON: {}", e)))?;

        // Check if this is a binary-framed request
        if let Some(payload) = request.payload.as_object() {
            if payload.contains_key("stream") {
                // Binary framing detected - for now, we'll handle this in operation-specific handlers
                // The stream data would be read after this point
            }
        }

        Ok(request)
    }

    /// Validate the protocol version in the request
    fn validate_protocol_version(&self, request: &RpcRequest) -> Result<(), RpcError> {
        // probe requests MUST use protocol_version: 0
        if request.op == Operation::Probe {
            if request.protocol_version != 0 {
                return Err(RpcError::UnsupportedProtocol {
                    version: request.protocol_version,
                    min: 0,
                    max: 0,
                });
            }
            return Ok(());
        }

        // All other operations MUST NOT use protocol_version: 0
        if request.protocol_version == 0 {
            return Err(RpcError::UnsupportedProtocol {
                version: 0,
                min: self.config.protocol_min,
                max: self.config.protocol_max,
            });
        }

        // Check if version is within supported range
        if request.protocol_version < self.config.protocol_min
            || request.protocol_version > self.config.protocol_max
        {
            return Err(RpcError::UnsupportedProtocol {
                version: request.protocol_version,
                min: self.config.protocol_min,
                max: self.config.protocol_max,
            });
        }

        Ok(())
    }

    /// Dispatch the request to the appropriate operation handler
    fn dispatch(&self, request: &RpcRequest) -> RpcResponse {
        let protocol_version = if request.op == Operation::Probe {
            0 // probe responses must use protocol_version: 0
        } else {
            request.protocol_version
        };

        match request.op {
            Operation::Probe => {
                match self.probe_handler.handle(&self.config) {
                    Ok(payload) => RpcResponse::success(
                        protocol_version,
                        request.request_id.clone(),
                        payload,
                    ),
                    Err(e) => RpcResponse::error(
                        protocol_version,
                        request.request_id.clone(),
                        RpcErrorPayload::new("INTERNAL_ERROR", e.to_string()),
                    ),
                }
            }
            // TODO: Implement other operations
            Operation::Reserve
            | Operation::Release
            | Operation::Submit
            | Operation::Status
            | Operation::Tail
            | Operation::Cancel
            | Operation::Fetch
            | Operation::HasSource
            | Operation::UploadSource => {
                RpcResponse::error(
                    protocol_version,
                    request.request_id.clone(),
                    RpcErrorPayload::new(
                        "FEATURE_MISSING",
                        format!("Operation '{}' not yet implemented", request.op.as_str()),
                    ),
                )
            }
        }
    }

    /// Write the response to the writer
    fn write_response<W: Write>(&self, writer: &mut W, response: &RpcResponse) -> io::Result<()> {
        let json = serde_json::to_string(response)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(writer, "{}", json)?;
        writer.flush()
    }
}

impl Operation {
    /// Get the string representation of the operation
    pub fn as_str(&self) -> &'static str {
        match self {
            Operation::Probe => "probe",
            Operation::Reserve => "reserve",
            Operation::Release => "release",
            Operation::Submit => "submit",
            Operation::Status => "status",
            Operation::Tail => "tail",
            Operation::Cancel => "cancel",
            Operation::Fetch => "fetch",
            Operation::HasSource => "has_source",
            Operation::UploadSource => "upload_source",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn create_handler() -> RpcHandler {
        RpcHandler::new(WorkerConfig::default())
    }

    #[test]
    fn test_probe_request() {
        let handler = create_handler();

        let input = r#"{"protocol_version":0,"op":"probe","request_id":"test-001","payload":{}}
"#;
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();

        handler.run_with_io(&mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let response: RpcResponse = serde_json::from_str(&output_str).unwrap();

        assert!(response.ok);
        assert_eq!(response.protocol_version, 0);
        assert_eq!(response.request_id, "test-001");
        assert!(response.payload.is_some());
    }

    #[test]
    fn test_probe_with_wrong_version() {
        let handler = create_handler();

        // probe with protocol_version != 0 should fail
        let input = r#"{"protocol_version":1,"op":"probe","request_id":"test-002","payload":{}}
"#;
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();

        handler.run_with_io(&mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let response: RpcResponse = serde_json::from_str(&output_str).unwrap();

        assert!(!response.ok);
        assert_eq!(response.error.as_ref().unwrap().code, "UNSUPPORTED_PROTOCOL");
    }

    #[test]
    fn test_non_probe_with_version_zero() {
        let handler = create_handler();

        // non-probe op with protocol_version: 0 should fail
        let input = r#"{"protocol_version":0,"op":"status","request_id":"test-003","payload":{}}
"#;
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();

        handler.run_with_io(&mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let response: RpcResponse = serde_json::from_str(&output_str).unwrap();

        assert!(!response.ok);
        assert_eq!(response.error.as_ref().unwrap().code, "UNSUPPORTED_PROTOCOL");
    }

    #[test]
    fn test_invalid_json() {
        let handler = create_handler();

        let input = "not valid json\n";
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();

        handler.run_with_io(&mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let response: RpcResponse = serde_json::from_str(&output_str).unwrap();

        assert!(!response.ok);
        assert_eq!(response.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }
}
