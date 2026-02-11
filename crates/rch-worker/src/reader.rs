//! Request reader module.
//!
//! Handles both simple JSON requests and binary-framed requests.

use std::io::{BufRead, Read};
use rch_protocol::{RpcRequest, RpcError, ErrorCode};

/// Maximum size for non-binary-framed requests (10 MB).
const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;

/// Read an RPC request from the input.
///
/// If the request contains a `payload.stream` object, this reads only the
/// JSON header line. The caller is responsible for reading the binary payload.
pub fn read_request<R: BufRead>(reader: &mut R) -> Result<RpcRequest, RpcError> {
    // Peek to determine if this might be a framed request
    // For now, we read the entire stdin as JSON
    // Binary framing will be handled by checking payload.stream after parsing
    
    let mut buffer = Vec::new();
    let mut total_read = 0;
    
    // Read line by line to handle potential framed requests
    let mut first_line = String::new();
    match reader.read_line(&mut first_line) {
        Ok(0) => {
            return Err(RpcError::invalid_request("unexpected EOF before complete JSON"));
        }
        Ok(_) => {}
        Err(e) => {
            return Err(RpcError::invalid_request(format!("failed to read input: {}", e)));
        }
    }
    
    // Try to parse the first line as complete JSON
    match serde_json::from_str::<RpcRequest>(&first_line) {
        Ok(request) => {
            // Check if this is a framed request
            if request.has_stream() {
                // For framed requests, the first line IS the complete header
                // The binary payload follows and will be read separately
                return Ok(request);
            }
            // Single-line JSON request
            return Ok(request);
        }
        Err(_) => {
            // First line wasn't complete JSON, continue reading
            buffer.extend_from_slice(first_line.as_bytes());
            total_read += first_line.len();
        }
    }
    
    // Read the rest of the input for multi-line JSON
    loop {
        if total_read >= MAX_REQUEST_SIZE {
            return Err(RpcError::new(
                ErrorCode::PayloadTooLarge,
                format!("request exceeds maximum size of {} bytes", MAX_REQUEST_SIZE),
            ));
        }
        
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(n) => {
                buffer.extend_from_slice(line.as_bytes());
                total_read += n;
            }
            Err(e) => {
                return Err(RpcError::invalid_request(format!("failed to read input: {}", e)));
            }
        }
    }
    
    // Parse the complete JSON
    serde_json::from_slice(&buffer).map_err(|e| {
        RpcError::invalid_request(format!("invalid JSON: {}", e))
    })
}

/// Read exactly `n` bytes of binary data.
pub fn read_binary_payload<R: Read>(reader: &mut R, length: u64) -> Result<Vec<u8>, RpcError> {
    let mut buffer = vec![0u8; length as usize];
    reader.read_exact(&mut buffer).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            RpcError::invalid_request("unexpected EOF before complete binary payload")
        } else {
            RpcError::invalid_request(format!("failed to read binary payload: {}", e))
        }
    })?;
    Ok(buffer)
}
