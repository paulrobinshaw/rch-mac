//! RCH Xcode Lane CLI
//!
//! Entry point for the `rch-xcode` command-line tool.

use clap::{Parser, Subcommand};
use rch_xcode_lane::worker::Capabilities;
use rch_xcode_lane::{
    Action, Classifier, ClassifierConfig, Pipeline, PipelineConfig, RepoConfig, RpcRequest,
    WorkerInventory, execute_tail,
};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::time::Duration;

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

    /// Execute verify actions from config (build + test)
    Run {
        /// Path to repo config file (default: .rch/xcode.toml)
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,

        /// Path to workers inventory file
        #[arg(long, short = 'i')]
        inventory: Option<PathBuf>,

        /// Output directory for artifacts and state
        #[arg(long, short = 'o', default_value = ".rch/artifacts")]
        output: PathBuf,

        /// Execute specific action (build or test) instead of full verify
        #[arg(long)]
        action: Option<String>,

        /// Continue on failure (process all steps even if one fails)
        #[arg(long)]
        continue_on_failure: bool,

        /// Overall timeout in seconds (default: 1800)
        #[arg(long, default_value = "1800")]
        timeout: u64,

        /// Idle timeout in seconds - no output for this long triggers timeout (default: 300)
        #[arg(long, default_value = "300")]
        idle_timeout: u64,

        /// Verbose output
        #[arg(long, short = 'v')]
        verbose: bool,

        /// Output JSON summary at the end
        #[arg(long)]
        json: bool,

        /// Dry-run mode: print plan without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Stream logs from a running or completed job
    Tail {
        /// Job ID or Run ID to tail
        id: String,

        /// Path to workers inventory file
        #[arg(long, short = 'i')]
        inventory: Option<PathBuf>,

        /// Follow mode - keep streaming until job completes
        #[arg(long, short = 'f')]
        follow: bool,

        /// Verbose output
        #[arg(long, short = 'v')]
        verbose: bool,
    },

    /// Show artifact paths for a run or job
    Artifacts {
        /// Run ID or Job ID to inspect
        id: String,

        /// Path to artifacts directory (default: .rch/artifacts)
        #[arg(long, short = 'o', default_value = ".rch/artifacts")]
        artifacts_dir: PathBuf,

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
        Commands::Run {
            config,
            inventory,
            output,
            action,
            continue_on_failure,
            timeout,
            idle_timeout,
            verbose,
            json,
            dry_run,
        } => {
            run_pipeline(
                config,
                inventory,
                output,
                action,
                continue_on_failure,
                timeout,
                idle_timeout,
                dry_run,
                verbose,
                json,
            );
        }
        Commands::Tail {
            id,
            inventory,
            follow,
            verbose,
        } => {
            run_tail(&id, inventory, follow, verbose);
        }
        Commands::Artifacts {
            id,
            artifacts_dir,
            json,
        } => {
            run_artifacts(&id, &artifacts_dir, json);
        }
    }
}

fn run_explain(human: bool, config_path: Option<PathBuf>, cmd: Vec<String>) {
    let classifier_config = match load_classifier_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            process::exit(1);
        }
    };

    let classifier = Classifier::new(classifier_config);
    let explanation = classifier.explain(&cmd);

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
        Ok(ClassifierConfig::default())
    }
}

fn run_workers_list(tags: Option<Vec<String>>, inventory_path: Option<PathBuf>, json_output: bool) {
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

    let workers: Vec<_> = if let Some(ref tag_list) = tags {
        let tag_refs: Vec<&str> = tag_list.iter().map(|s| s.as_str()).collect();
        inventory.filter_by_tags(&tag_refs)
    } else {
        inventory.workers.iter().collect()
    };

    let mut workers = workers;
    workers.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.name.cmp(&b.name)));

    if json_output {
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

fn run_workers_probe(
    worker_name: &str,
    inventory_path: Option<PathBuf>,
    json_output: bool,
    save: bool,
) {
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

    let worker = match inventory.get(worker_name) {
        Some(w) => w,
        None => {
            eprintln!("Worker '{}' not found in inventory.", worker_name);
            eprintln!(
                "Available workers: {}",
                inventory
                    .workers
                    .iter()
                    .map(|w| w.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            process::exit(1);
        }
    };

    let mut ssh_args = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=30".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
    ];

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

    if worker.port != 22 {
        ssh_args.push("-p".to_string());
        ssh_args.push(worker.port.to_string());
    }

    ssh_args.push(format!("{}@{}", worker.user, worker.host));

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
            process::exit(20);
        }
    };

    if let Some(ref mut stdin) = child.stdin {
        if let Err(e) = writeln!(stdin, "{}", request_json) {
            eprintln!("Failed to write to SSH stdin: {}", e);
            process::exit(20);
        }
    }
    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("stdout was piped");
    let mut reader = BufReader::new(stdout);
    let mut response_line = String::new();

    match reader.read_line(&mut response_line) {
        Ok(0) => {
            eprintln!("No response from worker");
            if let Some(mut stderr) = child.stderr.take() {
                let mut err_output = String::new();
                if std::io::Read::read_to_string(&mut stderr, &mut err_output).is_ok()
                    && !err_output.is_empty()
                {
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

    let status = child.wait().unwrap_or_else(|e| {
        eprintln!("Failed to wait for SSH process: {}", e);
        process::exit(20);
    });

    if !status.success() {
        eprintln!("SSH command failed with status: {}", status);
        process::exit(20);
    }

    let response: serde_json::Value = match serde_json::from_str(&response_line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse probe response: {}", e);
            eprintln!("Raw response: {}", response_line);
            process::exit(1);
        }
    };

    let capabilities_json = match response.get("payload") {
        Some(payload) => payload,
        None => {
            eprintln!("Probe response missing payload");
            eprintln!(
                "Response: {}",
                serde_json::to_string_pretty(&response).unwrap_or_default()
            );
            process::exit(1);
        }
    };

    let capabilities: Capabilities = match serde_json::from_value(capabilities_json.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: Could not parse capabilities struct: {}", e);
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(capabilities_json).unwrap()
                );
            } else {
                println!(
                    "Raw capabilities:\n{}",
                    serde_json::to_string_pretty(capabilities_json).unwrap()
                );
            }
            process::exit(0);
        }
    };

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

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:016x}{:08x}", now.as_nanos(), std::process::id())
}

fn run_cancel(id: &str, reason_str: &str, json_output: bool) {
    use rch_xcode_lane::host::rpc::CancelReason;

    let reason = match reason_str.to_lowercase().as_str() {
        "user" => CancelReason::User,
        "signal" => CancelReason::Signal,
        "timeout" | "timeout_overall" => CancelReason::TimeoutOverall,
        "timeout_idle" => CancelReason::TimeoutIdle,
        _ => {
            eprintln!(
                "Invalid reason '{}'. Valid: user, signal, timeout, timeout_idle",
                reason_str
            );
            process::exit(1);
        }
    };

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

    process::exit(0);
}

fn run_pipeline(
    config_path: Option<PathBuf>,
    inventory_path: Option<PathBuf>,
    artifacts_dir: PathBuf,
    action_str: Option<String>,
    continue_on_failure: bool,
    timeout_secs: u64,
    idle_timeout_secs: u64,
    dry_run: bool,
    verbose: bool,
    json_output: bool,
) {
    let action = match action_str {
        Some(ref s) => match s.to_lowercase().as_str() {
            "build" => Some(Action::Build),
            "test" => Some(Action::Test),
            _ => {
                eprintln!("Invalid action '{}'. Valid: build, test", s);
                process::exit(1);
            }
        },
        None => None,
    };

    let repo_config_path = config_path.unwrap_or_else(|| PathBuf::from(".rch/xcode.toml"));

    let pipeline_config = PipelineConfig {
        repo_config_path,
        inventory_path,
        artifacts_dir,
        overall_timeout_seconds: timeout_secs,
        idle_timeout_seconds: idle_timeout_secs,
        continue_on_failure,
        tail_poll_interval: Duration::from_secs(1),
        verbose,
        dry_run,
    };

    let mut pipeline = Pipeline::new(pipeline_config);

    // Dry-run mode: print plan without executing
    if dry_run {
        match pipeline.dry_run(action) {
            Ok(plan) => {
                if json_output {
                    match serde_json::to_string_pretty(&plan) {
                        Ok(json) => println!("{}", json),
                        Err(e) => {
                            eprintln!("Error serializing plan: {}", e);
                            process::exit(1);
                        }
                    }
                } else {
                    println!("{}", plan);
                }
                process::exit(0);
            }
            Err(e) => {
                if json_output {
                    eprintln!("{{\"error\": \"{}\"}}", e);
                } else {
                    eprintln!("Dry-run error: {}", e);
                }
                process::exit(e.exit_code());
            }
        }
    }

    let result = match action {
        Some(a) => pipeline.execute_action(a),
        None => pipeline.execute_verify(),
    };

    match result {
        Ok(summary) => {
            if json_output {
                match serde_json::to_string_pretty(&summary) {
                    Ok(json) => println!("{}", json),
                    Err(e) => {
                        eprintln!("Error serializing summary: {}", e);
                        process::exit(1);
                    }
                }
            } else {
                println!("\n=== Run Summary ===");
                println!("Run ID: {}", summary.run_id);
                println!("Status: {:?}", summary.status);
                println!(
                    "Steps: {} total, {} succeeded, {} failed",
                    summary.step_count,
                    summary.steps_succeeded,
                    summary.steps_failed,
                );
                if summary.steps_skipped > 0 {
                    println!("  Skipped: {}", summary.steps_skipped);
                }
                if summary.steps_rejected > 0 {
                    println!("  Rejected: {}", summary.steps_rejected);
                }
                println!("Duration: {:.1}s", summary.duration_ms as f64 / 1000.0);
            }

            process::exit(summary.exit_code);
        }
        Err(e) => {
            if json_output {
                eprintln!("{{\"error\": \"{}\"}}", e);
            } else {
                eprintln!("Pipeline error: {}", e);
            }
            process::exit(e.exit_code());
        }
    }
}

fn run_tail(id: &str, inventory_path: Option<PathBuf>, follow: bool, verbose: bool) {
    match execute_tail(id, inventory_path, follow, verbose) {
        Ok(()) => process::exit(0),
        Err(e) => {
            eprintln!("Error tailing {}: {}", id, e);
            process::exit(e.exit_code());
        }
    }
}

#[derive(serde::Serialize)]
struct ArtifactInfo {
    id: String,
    id_type: String,
    path: PathBuf,
    files: Vec<ArtifactFile>,
}

#[derive(serde::Serialize)]
struct ArtifactFile {
    name: String,
    path: PathBuf,
    size_bytes: u64,
    file_type: String,
}

fn run_artifacts(id: &str, artifacts_dir: &PathBuf, json_output: bool) {
    let run_path = artifacts_dir.join(id);

    if run_path.exists() && run_path.is_dir() {
        let files = collect_artifact_files(&run_path);
        let info = ArtifactInfo {
            id: id.to_string(),
            id_type: "run".to_string(),
            path: run_path.clone(),
            files,
        };
        output_artifact_info(&info, json_output);
        return;
    }

    if let Some(job_path) = find_job_path(artifacts_dir, id) {
        let files = collect_artifact_files(&job_path);
        let info = ArtifactInfo {
            id: id.to_string(),
            id_type: "job".to_string(),
            path: job_path,
            files,
        };
        output_artifact_info(&info, json_output);
        return;
    }

    eprintln!("No artifacts found for ID: {}", id);
    eprintln!("Searched in: {}", artifacts_dir.display());
    process::exit(1);
}

fn find_job_path(artifacts_dir: &PathBuf, job_id: &str) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(artifacts_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let run_dir = entry.path();
        if !run_dir.is_dir() {
            continue;
        }

        let steps_dir = run_dir.join("steps");
        if !steps_dir.exists() {
            continue;
        }

        if let Ok(step_entries) = std::fs::read_dir(&steps_dir) {
            for step_entry in step_entries.flatten() {
                let step_dir = step_entry.path();
                if !step_dir.is_dir() {
                    continue;
                }

                let job_dir = step_dir.join(job_id);
                if job_dir.exists() && job_dir.is_dir() {
                    return Some(job_dir);
                }
            }
        }
    }

    None
}

fn collect_artifact_files(path: &PathBuf) -> Vec<ArtifactFile> {
    let mut files = Vec::new();

    let key_files = [
        ("run_summary.json", "summary"),
        ("run_state.json", "state"),
        ("job_state.json", "state"),
        ("job_summary.json", "summary"),
        ("build.log", "log"),
        ("test.log", "log"),
        ("stdout.log", "log"),
        ("stderr.log", "log"),
    ];

    for (name, file_type) in &key_files {
        let file_path = path.join(name);
        if file_path.exists() {
            if let Ok(metadata) = std::fs::metadata(&file_path) {
                files.push(ArtifactFile {
                    name: name.to_string(),
                    path: file_path,
                    size_bytes: metadata.len(),
                    file_type: file_type.to_string(),
                });
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension() {
                if ext == "xcresult" {
                    let name = entry_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if let Ok(metadata) = std::fs::metadata(&entry_path) {
                        files.push(ArtifactFile {
                            name,
                            path: entry_path,
                            size_bytes: if metadata.is_dir() {
                                dir_size(&entry.path()).unwrap_or(0)
                            } else {
                                metadata.len()
                            },
                            file_type: "xcresult".to_string(),
                        });
                    }
                }
            }
        }
    }

    let steps_dir = path.join("steps");
    if steps_dir.exists() && steps_dir.is_dir() {
        if let Ok(step_entries) = std::fs::read_dir(&steps_dir) {
            for step_entry in step_entries.flatten() {
                let step_path = step_entry.path();
                if step_path.is_dir() {
                    if let Ok(job_entries) = std::fs::read_dir(&step_path) {
                        for job_entry in job_entries.flatten() {
                            let job_path = job_entry.path();
                            if job_path.is_dir() {
                                let mut job_files = collect_artifact_files(&job_path);
                                for file in &mut job_files {
                                    let rel_path = file.path.strip_prefix(path).unwrap_or(&file.path);
                                    file.name = rel_path.to_string_lossy().to_string();
                                }
                                files.extend(job_files);
                            }
                        }
                    }
                }
            }
        }
    }

    files
}

fn dir_size(path: &PathBuf) -> std::io::Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                total += dir_size(&path)?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}

fn output_artifact_info(info: &ArtifactInfo, json_output: bool) {
    if json_output {
        match serde_json::to_string_pretty(info) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing output: {}", e);
                process::exit(1);
            }
        }
    } else {
        println!("Artifacts for {} ({})", info.id, info.id_type);
        println!("  Path: {}", info.path.display());
        println!();

        if info.files.is_empty() {
            println!("  No key files found.");
        } else {
            println!("  Files:");
            for file in &info.files {
                let size = format_size(file.size_bytes);
                println!("    {} ({}) - {}", file.name, file.file_type, size);
            }
        }
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
