use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::{ModelEntry, ModelsConfig};

#[derive(Parser)]
pub struct ModelArgs {
    #[command(subcommand)]
    command: Option<ModelCommands>,
}

#[derive(Subcommand)]
pub enum ModelCommands {
    /// List all configured models
    List,

    /// Add a new model configuration
    Add {
        /// Model name/ID (e.g. gpt-4o, claude-3-5-sonnet-20241022)
        name: String,
        /// Provider: openai | anthropic | local
        #[arg(short, long)]
        provider: String,
        /// API endpoint (optional, for OpenAI-compatible APIs)
        #[arg(short = 'e', long)]
        endpoint: Option<String>,
        /// API key (optional; prefer env vars OPENAI_API_KEY / ANTHROPIC_API_KEY)
        #[arg(short = 'k', long)]
        api_key: Option<String>,
        /// Set as default/active model
        #[arg(long)]
        default: bool,
        /// Model description
        #[arg(short = 'd', long)]
        description: Option<String>,
        /// Override temperature for this model
        #[arg(long)]
        temperature: Option<f32>,
        /// Override max_tokens for this model
        #[arg(long)]
        max_tokens: Option<usize>,
    },

    /// Show model details
    Show {
        /// Model name
        name: String,
    },

    /// Switch (use) a model as the active default
    Use {
        /// Model name to activate
        name: String,
    },

    /// Set default model (alias for 'use')
    SetDefault {
        /// Model name
        name: String,
    },

    /// Remove a model configuration
    Remove {
        /// Model name
        name: String,
    },

    /// Test model connection (checks config and API key availability)
    Test {
        /// Model name (uses active model if omitted)
        name: Option<String>,
    },

    /// Import models from a JSON file
    /// Supports: array of ModelEntry, single ModelEntry object
    Import {
        /// Path to JSON file containing model configurations
        file: String,
        /// Overwrite existing models with same name
        #[arg(long)]
        overwrite: bool,
    },

    /// Export current model configurations to a JSON file
    Export {
        /// Output JSON file path (default: ./numina-models.json)
        #[arg(short, long, default_value = "numina-models.json")]
        output: String,
    },

    /// Show the path of the models config file
    Path,
}

pub async fn execute(args: &ModelArgs) -> Result<()> {
    match &args.command {
        Some(ModelCommands::List) => {
            let mc = ModelsConfig::load()?;
            let path = ModelsConfig::config_path()?;
            if mc.models.is_empty() {
                println!("🧠 No models configured yet.");
                println!("   Config file: {}", path.display());
                println!();
                println!("   Add via command:");
                println!("     numina model add <name> --provider <openai|anthropic|local>");
                println!("   Or edit the JSON file directly:");
                println!("     {}", path.display());
            } else {
                println!("🧠 Configured Models  ({})", path.display());
                println!();
                for m in &mc.models {
                    let active_tag = if mc.active == m.name { " ◀ active" } else { "" };
                    let key_tag = if m.api_key.is_some() { " (key: ***)" } else { " (key: env)" };
                    let ep_tag = m.endpoint.as_deref()
                        .map(|e| format!(" endpoint={}", e))
                        .unwrap_or_default();
                    let desc_tag = m.description.as_deref()
                        .map(|d| format!(" - {}", d))
                        .unwrap_or_default();
                    println!("  - {} ({}){}{}{}{}",
                        m.name, m.provider, active_tag, key_tag, ep_tag, desc_tag);
                }
                println!();
                println!("  Active model: {}", mc.active_model());
            }
        }

        Some(ModelCommands::Add {
            name, provider, endpoint, api_key, default,
            description, temperature, max_tokens,
        }) => {
            let mut mc = ModelsConfig::load()?;

            if mc.models.iter().any(|m| m.name == *name) {
                println!("⚠️  Model '{}' already exists. Use 'remove' first to replace it.", name);
                return Ok(());
            }

            if *default {
                mc.active = name.clone();
            }

            mc.models.push(ModelEntry {
                name: name.clone(),
                provider: provider.clone(),
                endpoint: endpoint.clone(),
                api_key: api_key.clone(),
                description: description.clone(),
                temperature: *temperature,
                max_tokens: *max_tokens,
            });

            mc.save()?;

            println!("🧠 Model added: {}", name);
            println!("   Provider : {}", provider);
            if let Some(ep) = endpoint { println!("   Endpoint : {}", ep); }
            if api_key.is_some() {
                println!("   API Key  : *** (stored in config)");
            } else {
                println!("   API Key  : (will use env var at runtime)");
            }
            if let Some(desc) = description { println!("   Desc     : {}", desc); }
            if let Some(t) = temperature { println!("   Temp     : {}", t); }
            if let Some(mt) = max_tokens { println!("   MaxTokens: {}", mt); }
            if *default { println!("   Active   : ✅ set as active model"); }
            println!("✅ Saved to {}", ModelsConfig::config_path()?.display());
        }

        Some(ModelCommands::Show { name }) => {
            let mc = ModelsConfig::load()?;
            match mc.models.iter().find(|m| m.name == *name) {
                Some(m) => {
                    let active_tag = if mc.active == m.name { " ◀ active" } else { "" };
                    println!("🧠 Model: {}{}", m.name, active_tag);
                    println!("   Provider : {}", m.provider);
                    println!("   Endpoint : {}", m.endpoint.as_deref().unwrap_or("(default)"));
                    println!("   API Key  : {}", if m.api_key.is_some() { "*** (stored)" } else { "(env var)" });
                    if let Some(desc) = &m.description { println!("   Desc     : {}", desc); }
                    if let Some(t) = m.temperature { println!("   Temp     : {}", t); }
                    if let Some(mt) = m.max_tokens { println!("   MaxTokens: {}", mt); }
                }
                None => {
                    println!("❌ Model '{}' not found.", name);
                    println!("   Run 'numina model list' to see configured models.");
                }
            }
        }

        Some(ModelCommands::Use { name }) | Some(ModelCommands::SetDefault { name }) => {
            let mut mc = ModelsConfig::load()?;
            if !mc.models.iter().any(|m| m.name == *name) {
                println!("❌ Model '{}' not found.", name);
                println!("   Run 'numina model list' to see available models.");
                return Ok(());
            }
            mc.active = name.clone();
            mc.save()?;
            println!("✅ Switched to model: {}", name);
            println!("   This model will be used for all subsequent commands.");
            println!("   Saved to {}", ModelsConfig::config_path()?.display());
        }

        Some(ModelCommands::Remove { name }) => {
            let mut mc = ModelsConfig::load()?;
            let before = mc.models.len();
            mc.models.retain(|m| m.name != *name);
            if mc.models.len() == before {
                println!("❌ Model '{}' not found.", name);
            } else {
                if mc.active == *name {
                    if let Some(first) = mc.models.first() {
                        mc.active = first.name.clone();
                        println!("⚠️  Removed active model. New active: {}", first.name);
                    } else {
                        mc.active = String::new();
                    }
                }
                mc.save()?;
                println!("🗑️  Model '{}' removed.", name);
            }
        }

        Some(ModelCommands::Test { name }) => {
            let mc = ModelsConfig::load()?;
            let model_name = name.as_deref().unwrap_or_else(|| mc.active_model());

            match mc.models.iter().find(|m| m.name == model_name) {
                Some(m) => {
                    println!("🧠 Testing model: {}", m.name);
                    println!("   Provider : {}", m.provider);
                    println!("   Endpoint : {}", m.endpoint.as_deref().unwrap_or("(default)"));

                    let has_key = m.api_key.is_some()
                        || match m.provider.as_str() {
                            "anthropic" => std::env::var("ANTHROPIC_API_KEY").is_ok(),
                            "local" => true,
                            _ => std::env::var("OPENAI_API_KEY").is_ok(),
                        };

                    if has_key {
                        println!("   API Key  : ✅ found");
                        println!("✅ Model config looks good!");
                    } else {
                        let env_var = match m.provider.as_str() {
                            "anthropic" => "ANTHROPIC_API_KEY",
                            _ => "OPENAI_API_KEY",
                        };
                        println!("   API Key  : ❌ not found");
                        println!("⚠️  Set the {} environment variable to use this model.", env_var);
                    }
                }
                None => {
                    println!("❌ Model '{}' not found in config.", model_name);
                    println!("   Run 'numina model add {}' to register it.", model_name);
                }
            }
        }

        Some(ModelCommands::Import { file, overwrite }) => {
            let content = std::fs::read_to_string(file)
                .map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", file, e))?;

            // 支持两种 JSON 格式：
            // 1. ModelEntry 数组
            // 2. 单个 ModelEntry 对象
            let entries: Vec<ModelEntry> = if content.trim_start().starts_with('[') {
                serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("Invalid JSON array format: {}", e))?
            } else {
                let single: ModelEntry = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("Invalid JSON object format: {}", e))?;
                vec![single]
            };

            let mut mc = ModelsConfig::load()?;
            let mut added = 0usize;
            let mut skipped = 0usize;
            let mut updated = 0usize;

            for entry in entries {
                let existing_idx = mc.models.iter().position(|m| m.name == entry.name);
                match existing_idx {
                    Some(idx) if *overwrite => {
                        println!("  🔄 Updated: {}", entry.name);
                        mc.models[idx] = entry;
                        updated += 1;
                    }
                    Some(_) => {
                        println!("  ⏭️  Skipped (exists): {} (use --overwrite to replace)", entry.name);
                        skipped += 1;
                    }
                    None => {
                        println!("  ✅ Added: {} [{}]", entry.name, entry.provider);
                        mc.models.push(entry);
                        added += 1;
                    }
                }
            }

            // 如果还没有 active model，设置第一个
            if mc.active.is_empty() {
                if let Some(first) = mc.models.first() {
                    mc.active = first.name.clone();
                }
            }

            mc.save()?;
            println!("\n📥 Import complete: {} added, {} updated, {} skipped", added, updated, skipped);
            println!("   Saved to {}", ModelsConfig::config_path()?.display());
        }

        Some(ModelCommands::Export { output }) => {
            let mc = ModelsConfig::load()?;
            if mc.models.is_empty() {
                println!("⚠️  No models configured. Nothing to export.");
                return Ok(());
            }

            // 导出时隐藏 api_key
            let export_models: Vec<serde_json::Value> = mc.models.iter().map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "provider": m.provider,
                    "endpoint": m.endpoint,
                    "api_key": null,
                    "description": m.description,
                    "temperature": m.temperature,
                    "max_tokens": m.max_tokens,
                })
            }).collect();

            let json = serde_json::to_string_pretty(&export_models)?;
            std::fs::write(output, &json)
                .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", output, e))?;

            println!("📤 Exported {} model(s) to: {}", mc.models.len(), output);
            println!("   Note: API keys are NOT exported for security reasons.");
        }

        Some(ModelCommands::Path) => {
            let path = ModelsConfig::config_path()?;
            println!("📁 Models config file: {}", path.display());
            println!("   You can edit this JSON file directly to add/modify/remove models.");
            println!("   Changes take effect immediately on next command run.");
        }

        None => {
            let mc = ModelsConfig::load().unwrap_or_default();
            let path = ModelsConfig::config_path().unwrap();
            println!("🧠 Model Management");
            println!("   Active model : {}", mc.active_model());
            println!("   Config file  : {}", path.display());
            println!();
            println!("Commands:");
            println!("  list                    List all configured models");
            println!("  add <name> --provider   Add a new model");
            println!("  use <name>              Switch to a model (set as active)");
            println!("  show <name>             Show model details");
            println!("  remove <name>           Remove a model");
            println!("  test [name]             Test model configuration");
            println!("  import <file.json>      Import models from JSON file");
            println!("  export [-o file.json]   Export models to JSON file");
            println!("  path                    Show config file path");
            println!();
            println!("  You can also edit the JSON config file directly:");
            println!("  {}", path.display());
        }
    }
    Ok(())
}
