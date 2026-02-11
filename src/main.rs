//! RCH Xcode Lane CLI
//!
//! Entry point for the `rch-xcode` command-line tool.

use clap::{Parser, Subcommand};
use rch_xcode_lane::{Classifier, ClassifierConfig, RepoConfig, WorkerInventory};
use std::path::PathBuf;
use std::process;

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
        },
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
