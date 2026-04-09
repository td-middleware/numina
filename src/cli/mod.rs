use anyhow::Result;
use clap::Subcommand;

pub mod chat;
pub mod session;
pub mod plan;
pub mod agent;
pub mod model;
pub mod mcp;
pub mod collaborate;
pub mod config;

#[derive(Subcommand)]
pub enum Commands {
    /// Start interactive chat with Numina agent
    Chat(chat::ChatArgs),

    /// Plan management - create, execute, and manage plans
    Plan(plan::PlanArgs),

    /// Agent operations - manage agents
    Agent(agent::AgentArgs),

    /// Model configuration and management
    Model(model::ModelArgs),

    /// MCP (Model Context Protocol) server management
    Mcp(mcp::McpArgs),

    /// Multi-agent collaboration
    Collaborate(collaborate::CollaborateArgs),

    /// Configuration management
    Config(config::ConfigArgs),

    /// Show Numina status and diagnostics
    Status,
}

impl Commands {
    pub async fn execute(&self) -> Result<()> {
        match self {
            Commands::Chat(args) => chat::execute(args).await,
            Commands::Plan(args) => plan::execute(args).await,
            Commands::Agent(args) => agent::execute(args).await,
            Commands::Model(args) => model::execute(args).await,
            Commands::Mcp(args) => mcp::execute(args).await,
            Commands::Collaborate(args) => collaborate::execute(args).await,
            Commands::Config(args) => config::execute(args).await,
            Commands::Status => {
                let models = crate::config::ModelsConfig::load().unwrap_or_default();
                let mcp = crate::config::McpFileConfig::load().unwrap_or_default();
                let enabled = mcp.servers.iter().filter(|s| s.enabled).count();
                println!("Numina Status:");
                println!("  Version      : 0.1.0");
                println!("  State        : Running");
                println!("  Active Model : {}", models.active_model());
                println!("  Models       : {} configured  ({})",
                    models.models.len(),
                    crate::config::ModelsConfig::config_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
                println!("  MCP Servers  : {} configured, {} enabled  ({})",
                    mcp.servers.len(), enabled,
                    crate::config::McpFileConfig::config_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
                Ok(())
            }
        }
    }
}
