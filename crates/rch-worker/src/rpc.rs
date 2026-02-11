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

use rch_protocol::{
    ops::names,
    ErrorCode, RpcError, RpcRequest, RpcResponse,
    PROTOCOL_VERSION_PROBE, PROTOCOL_MIN, PROTOCOL_MAX,
};

use crate::config::WorkerConfig;
use crate::handlers;

/// Main RPC handler for the worker.
pub struct RpcHandler {
    config: WorkerConfig,
}

impl RpcHandler {
    /// Create a new RPC handler with the given configuration.
    pub fn new(config: WorkerConfig) -> Self {
        Self { config }
    }

    /// Run the RPC handler, reading from stdin and writing to stdout.
    pub fn run(&self) -> io::Result<()> {
        self.run_with_io(&mut io::stdin().lock(), &mut io::stdout().lock())
    }

    /// Run the RPC handler with custom I/O (for testing).
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
                    PROTOCOL_VERSION_PROBE,
                    String::new(),
                    e,
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
                e,
            );
            self.write_response(writer, &response)?;
            return Ok(());
        }

        // Dispatch to operation handler
        let response = self.dispatch(&request);
        self.write_response(writer, &response)?;

        Ok(())
    }

    /// Read and parse the RPC request from the reader.
    fn read_request<R: BufRead>(&self, reader: &mut R) -> Result<RpcRequest, RpcError> {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| RpcError::invalid_request(format!("failed to read request: {}", e)))?;

        // Parse the JSON request
        let request: RpcRequest = serde_json::from_str(&line)
            .map_err(|e| RpcError::invalid_request(format!("invalid JSON: {}", e)))?;

        Ok(request)
    }

    /// Validate the protocol version in the request.
    fn validate_protocol_version(&self, request: &RpcRequest) -> Result<(), RpcError> {
        // probe requests MUST use protocol_version: 0
        if request.op == names::PROBE {
            if request.protocol_version != PROTOCOL_VERSION_PROBE {
                return Err(RpcError::unsupported_protocol(
                    request.protocol_version,
                    PROTOCOL_VERSION_PROBE,
                    PROTOCOL_VERSION_PROBE,
                ));
            }
            return Ok(());
        }

        // All other operations MUST NOT use protocol_version: 0
        if request.protocol_version == PROTOCOL_VERSION_PROBE {
            return Err(RpcError::unsupported_protocol(
                PROTOCOL_VERSION_PROBE,
                self.config.protocol_min,
                self.config.protocol_max,
            ));
        }

        // Check if version is within supported range
        if request.protocol_version < self.config.protocol_min
            || request.protocol_version > self.config.protocol_max
        {
            return Err(RpcError::unsupported_protocol(
                request.protocol_version,
                self.config.protocol_min,
                self.config.protocol_max,
            ));
        }

        Ok(())
    }

    /// Dispatch the request to the appropriate operation handler.
    fn dispatch(&self, request: &RpcRequest) -> RpcResponse {
        let protocol_version = if request.op == names::PROBE {
            PROTOCOL_VERSION_PROBE // probe responses must use protocol_version: 0
        } else {
            request.protocol_version
        };

        let result = match request.op.as_str() {
            names::PROBE => handlers::probe::handle(&self.config),
            names::RESERVE => handlers::reserve::handle(request, &self.config),
            names::RELEASE => handlers::release::handle(request, &self.config),
            names::SUBMIT => handlers::submit::handle(request, &self.config),
            names::STATUS => handlers::status::handle(request, &self.config),
            names::TAIL => handlers::tail::handle(request, &self.config),
            names::CANCEL => handlers::cancel::handle(request, &self.config),
            names::HAS_SOURCE => handlers::has_source::handle(request, &self.config),
            names::UPLOAD_SOURCE => handlers::upload_source::handle(request, &self.config),
            names::FETCH => handlers::fetch::handle(request, &self.config),
            _ => Err(RpcError::unknown_operation(&request.op)),
        };

        match result {
            Ok(payload) => RpcResponse::success(
                protocol_version,
                request.request_id.clone(),
                payload,
            ),
            Err(e) => RpcResponse::error(
                protocol_version,
                request.request_id.clone(),
                e,
            ),
        }
    }

    /// Write the response to the writer.
    fn write_response<W: Write>(&self, writer: &mut W, response: &RpcResponse) -> io::Result<()> {
        let json = serde_json::to_string(response)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(writer, "{}", json)?;
        writer.flush()
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
        assert_eq!(response.error.as_ref().unwrap().code, ErrorCode::UnsupportedProtocol);
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
        assert_eq!(response.error.as_ref().unwrap().code, ErrorCode::UnsupportedProtocol);
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
        assert_eq!(response.error.as_ref().unwrap().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn test_unknown_operation() {
        let handler = create_handler();

        let input = r#"{"protocol_version":1,"op":"unknown_op","request_id":"test-004","payload":{}}
"#;
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();

        handler.run_with_io(&mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let response: RpcResponse = serde_json::from_str(&output_str).unwrap();

        assert!(!response.ok);
        assert_eq!(response.error.as_ref().unwrap().code, ErrorCode::UnknownOperation);
    }
}
