//! Operation dispatcher.
//!
//! Routes incoming requests to the appropriate operation handler.

use std::io::BufRead;
use rch_protocol::{
    RpcRequest, RpcResponse, RpcError, ErrorCode,
    PROTOCOL_VERSION_PROBE, PROTOCOL_MIN, PROTOCOL_MAX,
    ops::names,
};

use crate::probe;

/// Dispatch a request to the appropriate handler.
pub fn dispatch<R: BufRead>(request: RpcRequest, reader: &mut R) -> Result<RpcResponse, RpcError> {
    let op = request.op.as_str();
    let request_id = request.request_id.clone();
    let protocol_version = request.protocol_version;
    
    // Validate protocol version
    // probe accepts only version 0
    // all other ops reject version 0 and require [PROTOCOL_MIN, PROTOCOL_MAX]
    if op == names::PROBE {
        if protocol_version != PROTOCOL_VERSION_PROBE {
            return Ok(RpcResponse::error(
                PROTOCOL_VERSION_PROBE,
                request_id,
                RpcError::unsupported_protocol(protocol_version, 0, 0),
            ));
        }
    } else {
        // Non-probe operations reject protocol_version 0
        if protocol_version == PROTOCOL_VERSION_PROBE {
            return Ok(RpcResponse::error(
                PROTOCOL_VERSION_PROBE,
                request_id,
                RpcError::new(
                    ErrorCode::UnsupportedProtocol,
                    "protocol_version 0 is only valid for probe operation",
                ),
            ));
        }
        
        // Check version is within supported range
        if protocol_version < PROTOCOL_MIN || protocol_version > PROTOCOL_MAX {
            return Ok(RpcResponse::error(
                protocol_version,
                request_id,
                RpcError::unsupported_protocol(protocol_version, PROTOCOL_MIN, PROTOCOL_MAX),
            ));
        }
    }
    
    // Dispatch to operation handler
    match op {
        names::PROBE => {
            let payload = probe::handle_probe(&request)?;
            Ok(RpcResponse::success(PROTOCOL_VERSION_PROBE, request_id, payload))
        }
        
        // M2+ operations - stubs that return FEATURE_MISSING for now
        names::RESERVE | names::RELEASE => {
            Ok(RpcResponse::error(
                protocol_version,
                request_id,
                RpcError::new(ErrorCode::FeatureMissing, format!("{} operation not yet implemented", op)),
            ))
        }
        
        names::SUBMIT | names::STATUS | names::TAIL | names::CANCEL |
        names::HAS_SOURCE | names::UPLOAD_SOURCE | names::FETCH => {
            // These will be wired when their respective handlers are implemented
            Ok(RpcResponse::error(
                protocol_version,
                request_id,
                RpcError::new(ErrorCode::FeatureMissing, format!("{} operation not yet implemented", op)),
            ))
        }
        
        _ => {
            Ok(RpcResponse::error(
                protocol_version,
                request_id,
                RpcError::unknown_operation(op),
            ))
        }
    }
}
