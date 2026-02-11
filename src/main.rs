//! RCH Xcode Lane CLI
//!
//! Entry point for the `rch-xcode` command-line tool.

use clap::{Parser, Subcommand};
use rch_xcode_lane::worker::Capabilities;
use rch_xcode_lane::{Classifier, ClassifierConfig, RpcRequest, RepoConfig, WorkerInventory};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{self, Command, Stdio};

#[derive(Parser)]
#[command(name = "rch-xcode")]
#[command(about = "Remote Xcode build/test lane", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Explain a classifier decision without executing
    Explain {
        /// Output in human-readable format instead of JSON
        #[arg(long)]
        human: bool,

        /// Path to repo config file (default: .rch/xcode.toml)
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,

        /// The xcodebuild command to explain (after --)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },

    /// Verify the project configuration
    Verify {
        /// Path to repo config file (default: .rch/xcode.toml)
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },

    /// Worker management commands
    Workers {
        #[command(subcommand)]
        action: WorkersCommands,
    },

    /// Cancel a running job or run
    Cancel {
        /// Run ID or Job ID to cancel
        id: String,

        /// Cancel reason (user, signal, timeout)
        #[arg(long, default_value = "user")]
        reason: String,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WorkersCommands {
    /// List configured workers
    List {
        /// Filter workers by tags (comma-separated, e.g., "macos,xcode")
        #[arg(long, short = 't', value_delimiter = ',')]
        tag: Option<Vec<String>>,

        /// Path to workers inventory file (default: ~/.config/rch/workers.toml)
        #[arg(long, short = 'i')]
        inventory: Option<PathBuf>,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Probe a worker for capabilities
    Probe {
        /// Worker name from inventory
        worker: String,

        /// Path to workers inventory file (default: ~/.config/rch/workers.toml)
        #[arg(long, short = 'i')]
        inventory: Option<PathBuf>,

        /// Output in JSON format
        #[arg(long)]
        json: bool,

        /// Save capabilities to cache file
        #[arg(long)]
        save: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Explain { human, config, cmd } => {
            run_explain(human, config, cmd);
        }
        Commands::Verify { config } => {
            run_verify(config);
        }
        Commands::Workers { action } => match action {
            WorkersCommands::List { tag, inventory, json } => {
                run_workers_list(tag, inventory, json);
            }
            WorkersCommands::Probe {
                worker,
                inventory,
                json,
                save,
            } => {
                run_workers_probe(&worker, inventory, json, save);
            }
        },
        Commands::Cancel { id, reason, json } => {
            run_cancel(&id, &reason, json);
        }
    }
}

fn run_explain(human: bool, config_path: Option<PathBuf>, cmd: Vec<String>) {
    // Load config
    let classifier_config = match load_classifier_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            process::exit(1);
        }
    };

    // Create classifier and run explain
    let classifier = Classifier::new(classifier_config);
    let explanation = classifier.explain(&cmd);

    // Output
    if human {
        println!("{}", explanation.to_human());
    } else {
        match explanation.to_json() {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing output: {}", e);
                process::exit(1);
            }
        }
    }

    // Exit with appropriate code
    if explanation.accepted {
        process::exit(0);
    } else {
        process::exit(1);
    }
}

fn run_verify(config_path: Option<PathBuf>) {
    let path = config_path.unwrap_or_else(|| PathBuf::from(".rch/xcode.toml"));

    match RepoConfig::from_file(&path) {
        Ok(config) => {
            println!("Configuration valid: {}", path.display());
            println!();
            if let Some(ref ws) = config.workspace {
                println!("  Workspace: {}", ws);
            }
            if let Some(ref proj) = config.project {
                println!("  Project: {}", proj);
            }
            println!("  Schemes: {}", config.schemes.join(", "));
            if !config.destinations.is_empty() {
                println!("  Destinations: {}", config.destinations.len());
            }
            if !config.configurations.is_empty() {
                println!("  Configurations: {}", config.configurations.join(", "));
            }
            if !config.verify.is_empty() {
                println!("  Verify actions: {}", config.verify.len());
            }
        }
        Err(e) => {
            eprintln!("Configuration error: {}", e);
            process::exit(1);
        }
    }
}

fn load_classifier_config(config_path: Option<PathBuf>) -> Result<ClassifierConfig, String> {
    let path = config_path.unwrap_or_else(|| PathBuf::from(".rch/xcode.toml"));

    if path.exists() {
        RepoConfig::from_file(&path)
            .map(|c| c.to_classifier_config())
            .map_err(|e| e.to_string())
    } else {
        // Use default config if no file exists
        Ok(ClassifierConfig::default())
    }
}

fn run_workers_list(tags: Option<Vec<String>>, inventory_path: Option<PathBuf>, json_output: bool) {
    // Load inventory
    let inventory = match inventory_path {
        Some(path) => WorkerInventory::load(&path),
        None => WorkerInventory::load_default(),
    };

    let inventory = match inventory {
        Ok(inv) => inv,
        Err(e) => {
            eprintln!("Error loading worker inventory: {}", e);
            process::exit(1);
        }
    };

    // Filter by tags if specified
    let workers: Vec<_> = if let Some(ref tag_list) = tags {
        let tag_refs: Vec<&str> = tag_list.iter().map(|s| s.as_str()).collect();
        inventory.filter_by_tags(&tag_refs)
    } else {
        inventory.workers.iter().collect()
    };

    // Sort by priority
    let mut workers = workers;
    workers.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.name.cmp(&b.name)));

    if json_output {
        // JSON output
        let output: Vec<serde_json::Value> = workers
            .iter()
            .map(|w| {
                serde_json::json!({
                    "name": w.name,
                    "host": w.host,
                    "port": w.port,
                    "user": w.user,
                    "tags": w.tags,
                    "priority": w.priority,
                    "ssh_key_path": w.ssh_key_path,
                    "known_host_fingerprint": w.known_host_fingerprint,
                    "attestation_pubkey_fingerprint": w.attestation_pubkey_fingerprint,
                })
            })
            .collect();

        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing output: {}", e);
                process::exit(1);
            }
        }
    } else {
        // Human-readable output
        if workers.is_empty() {
            if tags.is_some() {
                println!("No workers found matching the specified tags.");
            } else {
                println!("No workers configured.");
            }
            return;
        }

        println!("Configured workers ({} total):\n", workers.len());

        for worker in workers {
            println!("  {} ({})", worker.name, worker.host);
            println!("    User: {}@{}:{}", worker.user, worker.host, worker.port);
            if !worker.tags.is_empty() {
                println!("    Tags: {}", worker.tags.join(", "));
            }
            println!("    Priority: {}", worker.priority);
            if let Some(ref key) = worker.ssh_key_path {
                println!("    SSH Key: {}", key);
            }
            if let Some(ref fp) = worker.known_host_fingerprint {
                println!("    Host Fingerprint: {}", fp);
            }
            if let Some(ref fp) = worker.attestation_pubkey_fingerprint {
                println!("    Attestation Key: {}", fp);
            }
            println!();
        }
    }
}

fn run_workers_probe(worker_name: &str, inventory_path: Option<PathBuf>, json_output: bool, save: bool) {
    // Load inventory
    let inventory = match inventory_path {
        Some(ref path) => WorkerInventory::load(path),
        None => WorkerInventory::load_default(),
    };

    let inventory = match inventory {
        Ok(inv) => inv,
        Err(e) => {
            eprintln!("Error loading worker inventory: {}", e);
            process::exit(1);
        }
    };

    // Find the worker
    let worker = match inventory.get(worker_name) {
        Some(w) => w,
        None => {
            eprintln!("Worker '{}' not found in inventory.", worker_name);
            eprintln!("Available workers: {}",
                inventory.workers.iter().map(|w| w.name.as_str()).collect::<Vec<_>>().join(", "));
            process::exit(1);
        }
    };

    // Build SSH command
    let mut ssh_args = vec![
        "-o".to_string(), "BatchMode=yes".to_string(),
        "-o".to_string(), "ConnectTimeout=30".to_string(),
        "-o".to_string(), "StrictHostKeyChecking=accept-new".to_string(),
    ];

    // Add identity file if specified
    if let Some(ref key_path) = worker.ssh_key_path {
        let expanded = if key_path.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                format!("{}/{}", home, &key_path[2..])
            } else {
                key_path.clone()
            }
        } else {
            key_path.clone()
        };
        ssh_args.push("-i".to_string());
        ssh_args.push(expanded);
    }

    // Add port if not default
    if worker.port != 22 {
        ssh_args.push("-p".to_string());
        ssh_args.push(worker.port.to_string());
    }

    // Add user@host
    ssh_args.push(format!("{}@{}", worker.user, worker.host));

    // Create probe request
    let probe_request = RpcRequest {
        protocol_version: 0,
        request_id: format!("probe-{}", uuid_simple()),
        op: rch_xcode_lane::Operation::Probe,
        payload: serde_json::Value::Object(serde_json::Map::new()),
    };

    let request_json = match serde_json::to_string(&probe_request) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Error serializing probe request: {}", e);
            process::exit(1);
        }
    };

    // Execute SSH command
    eprintln!("Probing worker '{}'...", worker_name);

    let mut child = match Command::new("ssh")
        .args(&ssh_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Failed to spawn SSH process: {}", e);
            process::exit(20); // SSH exit code
        }
    };

    // Write request to stdin
    if let Some(ref mut stdin) = child.stdin {
        if let Err(e) = writeln!(stdin, "{}", request_json) {
            eprintln!("Failed to write to SSH stdin: {}", e);
            process::exit(20);
        }
    }
    drop(child.stdin.take()); // Close stdin to signal EOF

    // Read response from stdout
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut reader = BufReader::new(stdout);
    let mut response_line = String::new();

    match reader.read_line(&mut response_line) {
        Ok(0) => {
            eprintln!("No response from worker");
            // Read stderr for error details
            if let Some(mut stderr) = child.stderr.take() {
                let mut err_output = String::new();
                if std::io::Read::read_to_string(&mut stderr, &mut err_output).is_ok() && !err_output.is_empty() {
                    eprintln!("SSH stderr: {}", err_output.trim());
                }
            }
            process::exit(20);
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("Failed to read from SSH stdout: {}", e);
            process::exit(20);
        }
    }

    // Wait for SSH to complete
    let status = child.wait().unwrap_or_else(|e| {
        eprintln!("Failed to wait for SSH process: {}", e);
        process::exit(20);
    });

    if !status.success() {
        eprintln!("SSH command failed with status: {}", status);
        process::exit(20);
    }

    // Parse the response
    let response: serde_json::Value = match serde_json::from_str(&response_line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse probe response: {}", e);
            eprintln!("Raw response: {}", response_line);
            process::exit(1);
        }
    };

    // Extract capabilities from response payload
    let capabilities_json = match response.get("payload") {
        Some(payload) => payload,
        None => {
            eprintln!("Probe response missing payload");
            eprintln!("Response: {}", serde_json::to_string_pretty(&response).unwrap_or_default());
            process::exit(1);
        }
    };

    // Try to parse as Capabilities
    let capabilities: Capabilities = match serde_json::from_value(capabilities_json.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: Could not parse capabilities struct: {}", e);
            // Fall back to raw JSON output
            if json_output {
                println!("{}", serde_json::to_string_pretty(capabilities_json).unwrap());
            } else {
                println!("Raw capabilities:\n{}", serde_json::to_string_pretty(capabilities_json).unwrap());
            }
            process::exit(0);
        }
    };

    // Save to cache if requested
    if save {
        let cache_dir = match std::env::var("HOME") {
            Ok(home) => PathBuf::from(home).join(".cache/rch/capabilities"),
            Err(_) => {
                eprintln!("Warning: Could not determine home directory for cache");
                PathBuf::from("/tmp/rch/capabilities")
            }
        };

        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            eprintln!("Warning: Could not create cache directory: {}", e);
        } else {
            let cache_file = cache_dir.join(format!("{}.json", worker_name));
            match capabilities.write_to_file(&cache_file) {
                Ok(_) => eprintln!("Saved capabilities to: {}", cache_file.display()),
                Err(e) => eprintln!("Warning: Could not save capabilities: {}", e),
            }
        }
    }

    // Output
    if json_output {
        match capabilities.to_json() {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing capabilities: {}", e);
                process::exit(1);
            }
        }
    } else {
        println!("{}", capabilities.to_human_readable());
    }
}

/// Generate a simple UUID-like string for request IDs
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:016x}{:08x}", now.as_nanos(), std::process::id())
}

fn run_cancel(id: &str, reason_str: &str, json_output: bool) {
    use rch_xcode_lane::host::rpc::CancelReason;

    // Parse reason
    let reason = match reason_str.to_lowercase().as_str() {
        "user" => CancelReason::User,
        "signal" => CancelReason::Signal,
        "timeout" | "timeout_overall" => CancelReason::TimeoutOverall,
        "timeout_idle" => CancelReason::TimeoutIdle,
        _ => {
            eprintln!("Invalid reason '{}'. Valid: user, signal, timeout, timeout_idle", reason_str);
            process::exit(1);
        }
    };

    // Determine if this is a run_id or job_id
    // For now, we treat it as a job_id and would need worker connection for actual cancel
    // In a full implementation, this would:
    // 1. Check if it's a run_id (look for run_plan.json in artifacts)
    // 2. If run_id, get all job_ids from the run and cancel each
    // 3. If job_id, cancel just that job
    // 4. Connect to worker and send cancel RPC

    if json_output {
        println!("{{");
        println!("  \"id\": \"{}\",", id);
        println!("  \"reason\": \"{}\",", reason.as_str());
        println!("  \"status\": \"cancel_requested\",");
        println!("  \"message\": \"Cancel command received. Full implementation requires worker connection.\"");
        println!("}}");
    } else {
        println!("Cancel requested for: {}", id);
        println!("  Reason: {}", reason.as_str());
        println!();
        println!("Note: Full cancellation requires active worker connection.");
        println!("      This command will be fully functional when integrated with run execution.");
    }

    // Exit with success - the cancel request was acknowledged
    // In full implementation, exit code depends on cancel result
    process::exit(0);
}
