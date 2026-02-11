//! RCH Xcode Lane CLI
//!
//! Entry point for the `rch-xcode` command-line tool.

use clap::{Parser, Subcommand};
use rch_xcode_lane::{Classifier, ClassifierConfig, RepoConfig};
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
