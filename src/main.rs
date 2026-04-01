use clap::Parser;
use tracing::{info, error};

// Top-level modules
// CLI interface
mod cli;
// Core runtime (agents, models, tools, chat, etc.)
mod core;
// Configuration loading & persistence
mod config;
// Shared utilities (logger, fs helpers, etc.)
mod utils;

#[derive(Parser)]
#[command(name = "numina")]
#[command(about = "Numina - AI Intelligent Agent CLI with MCP and Multi-Agent Collaboration", long_about = None)]
#[command(version = "0.1.0")]
struct NuminaCli {
    #[command(subcommand)]
    command: cli::Commands,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    let cli = NuminaCli::parse();

    info!("Numina starting...");

    match cli.command.execute().await {
        Ok(_) => {
            info!("Command executed successfully");
            Ok(())
        }
        Err(e) => {
            error!("Command failed: {}", e);
            Err(e)
        }
    }
}
