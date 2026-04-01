use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::NuminaConfig;

#[derive(Parser)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommands>,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Initialize configuration (creates ~/.numina/config.toml and workspace)
    Init,

    /// Show current configuration
    Show,

    /// Set a configuration value (key=dot.path, e.g. model.default_model)
    Set {
        /// Configuration key (e.g. model.default_model)
        key: String,
        /// Configuration value
        value: String,
    },

    /// Get a configuration value
    Get {
        /// Configuration key (e.g. model.default_model)
        key: String,
    },

    /// Open configuration file in $EDITOR
    Edit,

    /// Reset configuration to defaults
    Reset,
}

fn config_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".numina").join("config.toml"))
}

fn workspace_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".numina").join("workspace"))
}

pub async fn execute(args: &ConfigArgs) -> Result<()> {
    match &args.command {
        Some(ConfigCommands::Init) => {
            let cfg_path = config_path()?;
            let ws_path = workspace_path()?;

            // 创建 workspace 子目录
            for sub in &["sessions", "memory", "cache", "logs"] {
                std::fs::create_dir_all(ws_path.join(sub))?;
            }

            if cfg_path.exists() {
                println!("⚙️  Config already exists at {}", cfg_path.display());
            } else {
                let config = NuminaConfig::default();
                config.save()?;
                println!("⚙️  Config created at {}", cfg_path.display());
            }

            // 生成默认 claude.md（如果不存在）
            let claude_md = ws_path.join("claude.md");
            if !claude_md.exists() {
                let default_skills = include_str!("../../examples/claude.md");
                std::fs::write(&claude_md, default_skills)?;
                println!("🎯 Default skills (claude.md) created at {}", claude_md.display());
            }

            println!("✅ Numina workspace initialized at {}", ws_path.display());
            println!("\nNext steps:");
            println!("  1. Add a model:  numina model add gpt-4o --provider openai --default");
            println!("  2. Set API key:  export OPENAI_API_KEY=sk-...");
            println!("  3. Start chat:   numina chat");
        }

        Some(ConfigCommands::Show) => {
            let config = NuminaConfig::load()?;
            let cfg_path = config_path()?;
            println!("⚙️  Numina Configuration ({})", cfg_path.display());
            println!();
            println!("[general]");
            println!("  version   = {}", config.general.version);
            println!("  log_level = {}", config.general.log_level);
            println!();
            println!("[model]");
            println!("  default_model = {}", config.model.default_model);
            println!("  temperature   = {}", config.model.temperature);
            println!("  max_tokens    = {}", config.model.max_tokens);
            println!();
            println!("[workspace]");
            println!("  path         = {}", config.workspace.path);
            println!("  max_memory_mb = {}", config.workspace.max_memory_mb);
            println!();
            println!("[mcp]");
            println!("  auto_connect    = {}", config.mcp_global.auto_connect);
            println!();
            let models_cfg = crate::config::ModelsConfig::load().unwrap_or_default();
            if models_cfg.models.is_empty() {
                println!("[models] (none configured)");
                println!("  Edit: {}", crate::config::ModelsConfig::config_path().unwrap().display());
            } else {
                println!("[models]  ({})", crate::config::ModelsConfig::config_path().unwrap().display());
                for m in &models_cfg.models {
                    let active_tag = if models_cfg.active == m.name { " *active*" } else { "" };
                    println!("  - {} ({}){}", m.name, m.provider, active_tag);
                }
            }
            let mcp_cfg = crate::config::McpFileConfig::load().unwrap_or_default();
            if mcp_cfg.servers.is_empty() {
                println!("[mcp_servers] (none configured)");
                println!("  Edit: {}", crate::config::McpFileConfig::config_path().unwrap().display());
            } else {
                println!("[mcp_servers]  ({})", crate::config::McpFileConfig::config_path().unwrap().display());
                for s in &mcp_cfg.servers {
                    let status = if s.enabled { "enabled" } else { "disabled" };
                    println!("  - {} [{}] {}", s.name, s.server_type, status);
                }
            }
        }

        Some(ConfigCommands::Set { key, value }) => {
            let mut config = NuminaConfig::load()?;
            match key.as_str() {
                "model.default_model" => config.model.default_model = value.clone(),
                "model.temperature" => {
                    config.model.temperature = value.parse()
                        .map_err(|_| anyhow::anyhow!("temperature must be a float"))?;
                }
                "model.max_tokens" => {
                    config.model.max_tokens = value.parse()
                        .map_err(|_| anyhow::anyhow!("max_tokens must be an integer"))?;
                }
                "general.log_level" => config.general.log_level = value.clone(),
                "mcp.auto_connect" => {
                    config.mcp_global.auto_connect = value.parse()
                        .map_err(|_| anyhow::anyhow!("auto_connect must be true or false"))?;
                }
                "workspace.path" => config.workspace.path = value.clone(),
                _ => {
                    println!("❌ Unknown config key: {}", key);
                    println!("   Supported keys: model.default_model, model.temperature,");
                    println!("   model.max_tokens, general.log_level, mcp.auto_connect, workspace.path");
                    return Ok(());
                }
            }
            config.save()?;
            println!("✅ {} = {}", key, value);
        }

        Some(ConfigCommands::Get { key }) => {
            let config = NuminaConfig::load()?;
            let val = match key.as_str() {
                "model.default_model" => config.model.default_model.clone(),
                "model.temperature" => config.model.temperature.to_string(),
                "model.max_tokens" => config.model.max_tokens.to_string(),
                "general.log_level" => config.general.log_level.clone(),
                "mcp.auto_connect" => config.mcp_global.auto_connect.to_string(),
                "workspace.path" => config.workspace.path.clone(),
                _ => {
                    println!("❌ Unknown config key: {}", key);
                    return Ok(());
                }
            };
            println!("{} = {}", key, val);
        }

        Some(ConfigCommands::Edit) => {
            let cfg_path = config_path()?;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            println!("⚙️  Opening {} with {}...", cfg_path.display(), editor);
            std::process::Command::new(&editor)
                .arg(&cfg_path)
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to open editor '{}': {}", editor, e))?;
        }

        Some(ConfigCommands::Reset) => {
            let config = NuminaConfig::default();
            config.save()?;
            println!("⚠️  Configuration reset to defaults.");
            println!("✅ Saved to {}", config_path()?.display());
        }

        None => {
            println!("⚙️  Configuration Management");
            println!("Use --help to see available commands");
        }
    }
    Ok(())
}
