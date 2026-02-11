//! RCH Worker Entrypoint
//!
//! Usage: rch-worker xcode rpc
//!
//! Reads a single JSON RPC request from stdin, dispatches to the
//! appropriate handler, and writes a JSON response to stdout.
//! Designed to be invoked via SSH forced-command.

use std::process::ExitCode;
use rch_worker::{RpcHandler, WorkerConfig};

fn main() -> ExitCode {
    // For now, we only support the "xcode rpc" subcommand
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() >= 3 && args[1] == "xcode" && args[2] == "rpc" {
        let config = WorkerConfig::default();
        let handler = RpcHandler::new(config);
        
        if let Err(e) = handler.run() {
            eprintln!("RPC handler error: {}", e);
            return ExitCode::FAILURE;
        }
        
        ExitCode::SUCCESS
    } else {
        eprintln!("Usage: rch-worker xcode rpc");
        eprintln!();
        eprintln!("Runs the RPC handler, reading JSON from stdin and writing to stdout.");
        ExitCode::FAILURE
    }
}
