use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::{McpFileConfig, McpServerEntry};

#[derive(Parser)]
pub struct McpArgs {
    #[command(subcommand)]
    command: Option<McpCommands>,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// List all configured MCP servers
    List,

    /// Add an MCP server configuration
    Add {
        /// MCP server name
        name: String,
        /// Server type: stdio | http | websocket
        #[arg(short = 't', long, default_value = "stdio")]
        server_type: String,
        /// Command (for stdio) or URL (for http/websocket)
        #[arg(short = 'c', long)]
        command_or_url: String,
        /// Arguments for stdio server (space-separated, e.g. "-y @pkg/server /path")
        #[arg(short = 'a', long, allow_hyphen_values = true)]
        args: Option<String>,
        /// Server description
        #[arg(short = 'd', long)]
        description: Option<String>,
        /// Environment variables (key=value, can be specified multiple times)
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
        /// Disable this server (enabled by default)
        #[arg(long)]
        disabled: bool,
    },

    /// Show MCP server details
    Show {
        /// MCP server name
        name: String,
    },

    /// Enable an MCP server
    Enable {
        /// MCP server name
        name: String,
    },

    /// Disable an MCP server
    Disable {
        /// MCP server name
        name: String,
    },

    /// Remove an MCP server configuration
    Remove {
        /// MCP server name
        name: String,
    },

    /// Test MCP server connection
    Test {
        /// MCP server name
        name: String,
    },

    /// List available tools from MCP servers
    ListTools {
        /// MCP server name (lists all if omitted)
        name: Option<String>,
    },

    /// Start MCP server
    Start {
        /// MCP server name
        name: String,
    },

    /// Stop MCP server
    Stop {
        /// MCP server name
        name: String,
    },

    /// Import MCP server configurations from a JSON file
    /// Supports: numina array format, single object, VSCode mcpServers format
    Import {
        /// Path to JSON file containing MCP server configurations
        file: String,
        /// Overwrite existing servers with same name
        #[arg(long)]
        overwrite: bool,
    },

    /// Export MCP server configurations to a JSON file
    Export {
        /// Output JSON file path (default: ./numina-mcp.json)
        #[arg(short, long, default_value = "numina-mcp.json")]
        output: String,
    },

    /// Show the path of the MCP config file
    Path,
}

pub async fn execute(args: &McpArgs) -> Result<()> {
    match &args.command {
        Some(McpCommands::List) => {
            let mc = McpFileConfig::load()?;
            let path = McpFileConfig::config_path()?;
            if mc.servers.is_empty() {
                println!("🔧 No MCP servers configured yet.");
                println!("   Config file: {}", path.display());
                println!();
                println!("   Add via command:");
                println!("     numina mcp add <name> -c <command>");
                println!("   Or edit the JSON file directly:");
                println!("     {}", path.display());
            } else {
                println!("🔧 Configured MCP Servers  ({})", path.display());
                println!();
                for s in &mc.servers {
                    let status = if s.enabled { "✅" } else { "⏸️ " };
                    let desc_tag = s.description.as_deref()
                        .map(|d| format!(" - {}", d))
                        .unwrap_or_default();
                    println!("  {} {} [{}] {}{}",
                        status, s.name, s.server_type, s.command_or_url, desc_tag);
                    if let Some(a) = &s.args {
                        println!("       args: {}", a);
                    }
                    if !s.env.is_empty() {
                        println!("       env:  {}", s.env.join(", "));
                    }
                }
                let enabled = mc.servers.iter().filter(|s| s.enabled).count();
                println!();
                println!("  Total: {} server(s), {} enabled", mc.servers.len(), enabled);
            }
        }

        Some(McpCommands::Add {
            name, server_type, command_or_url, args,
            description, env, disabled,
        }) => {
            let mut mc = McpFileConfig::load()?;

            if mc.servers.iter().any(|s| s.name == *name) {
                println!("⚠️  MCP server '{}' already exists. Use 'remove' first to replace it.", name);
                return Ok(());
            }

            mc.servers.push(McpServerEntry {
                name: name.clone(),
                server_type: server_type.clone(),
                command_or_url: command_or_url.clone(),
                args: args.clone(),
                enabled: !disabled,
                description: description.clone(),
                env: env.clone(),
            });

            mc.save()?;

            println!("🔧 MCP server added: {}", name);
            println!("   Type    : {}", server_type);
            println!("   Command : {}", command_or_url);
            if let Some(a) = args { println!("   Args    : {}", a); }
            if let Some(desc) = description { println!("   Desc    : {}", desc); }
            if !env.is_empty() { println!("   Env     : {}", env.join(", ")); }
            println!("   Enabled : {}", if !disabled { "yes ✅" } else { "no ⏸️" });
            println!("✅ Saved to {}", McpFileConfig::config_path()?.display());
        }

        Some(McpCommands::Show { name }) => {
            let mc = McpFileConfig::load()?;
            match mc.servers.iter().find(|s| s.name == *name) {
                Some(s) => {
                    println!("🔧 MCP Server: {}", s.name);
                    println!("   Type    : {}", s.server_type);
                    println!("   Command : {}", s.command_or_url);
                    if let Some(a) = &s.args { println!("   Args    : {}", a); }
                    println!("   Enabled : {}", if s.enabled { "yes ✅" } else { "no ⏸️" });
                    if let Some(desc) = &s.description { println!("   Desc    : {}", desc); }
                    if !s.env.is_empty() { println!("   Env     : {}", s.env.join(", ")); }
                }
                None => {
                    println!("❌ MCP server '{}' not found.", name);
                    println!("   Run 'numina mcp list' to see configured servers.");
                }
            }
        }

        Some(McpCommands::Enable { name }) => {
            let mut mc = McpFileConfig::load()?;
            match mc.servers.iter_mut().find(|s| s.name == *name) {
                Some(s) => {
                    s.enabled = true;
                    mc.save()?;
                    println!("✅ MCP server '{}' enabled.", name);
                }
                None => println!("❌ MCP server '{}' not found.", name),
            }
        }

        Some(McpCommands::Disable { name }) => {
            let mut mc = McpFileConfig::load()?;
            match mc.servers.iter_mut().find(|s| s.name == *name) {
                Some(s) => {
                    s.enabled = false;
                    mc.save()?;
                    println!("⏸️  MCP server '{}' disabled.", name);
                }
                None => println!("❌ MCP server '{}' not found.", name),
            }
        }

        Some(McpCommands::Remove { name }) => {
            let mut mc = McpFileConfig::load()?;
            let before = mc.servers.len();
            mc.servers.retain(|s| s.name != *name);
            if mc.servers.len() == before {
                println!("❌ MCP server '{}' not found.", name);
            } else {
                mc.save()?;
                println!("🗑️  MCP server '{}' removed.", name);
            }
        }

        Some(McpCommands::Test { name }) => {
            let mc = McpFileConfig::load()?;
            match mc.servers.iter().find(|s| s.name == *name) {
                Some(s) => {
                    println!("🔧 Testing MCP server: {}", s.name);
                    println!("   Type    : {}", s.server_type);
                    println!("   Command : {}", s.command_or_url);
                    if !s.enabled {
                        println!("⚠️  Server is disabled. Enable it first: numina mcp enable {}", name);
                        return Ok(());
                    }
                    match s.server_type.as_str() {
                        "stdio" => {
                            let cmd = s.command_or_url.split_whitespace().next().unwrap_or("");
                            let exists = std::process::Command::new("which")
                                .arg(cmd)
                                .output()
                                .map(|o| o.status.success())
                                .unwrap_or(false);
                            if exists {
                                println!("   Command : ✅ found in PATH");
                                println!("✅ MCP server config looks good!");
                            } else {
                                println!("   Command : ⚠️  '{}' not found in PATH", cmd);
                                println!("   Make sure the command is installed and accessible.");
                            }
                        }
                        "http" | "websocket" => {
                            println!("   URL     : {}", s.command_or_url);
                            println!("✅ MCP server config looks good! (connection test not yet implemented)");
                        }
                        _ => println!("⚠️  Unknown server type: {}", s.server_type),
                    }
                }
                None => {
                    println!("❌ MCP server '{}' not found.", name);
                    println!("   Run 'numina mcp list' to see configured servers.");
                }
            }
        }

        Some(McpCommands::ListTools { name }) => {
            let mc = McpFileConfig::load()?;
            match name {
                Some(n) => {
                    match mc.servers.iter().find(|s| s.name == *n) {
                        Some(s) => {
                            println!("🔧 Tools from MCP server '{}':", s.name);
                            println!("   (Tool discovery requires active server connection)");
                            println!("   Server: {} [{}]", s.command_or_url, s.server_type);
                        }
                        None => println!("❌ MCP server '{}' not found.", n),
                    }
                }
                None => {
                    if mc.servers.is_empty() {
                        println!("🔧 No MCP servers configured.");
                    } else {
                        println!("🔧 MCP Servers and Tools:");
                        for s in mc.servers.iter().filter(|s| s.enabled) {
                            println!("  📦 {} [{}] - {}", s.name, s.server_type, s.command_or_url);
                            println!("     (Tool discovery requires active server connection)");
                        }
                        let disabled = mc.servers.iter().filter(|s| !s.enabled).count();
                        if disabled > 0 {
                            println!("  ({} disabled server(s) not shown)", disabled);
                        }
                    }
                }
            }
        }

        Some(McpCommands::Start { name }) => {
            let mc = McpFileConfig::load()?;
            match mc.servers.iter().find(|s| s.name == *name) {
                Some(s) => {
                    if !s.enabled {
                        println!("⚠️  Server '{}' is disabled. Enable it first.", name);
                        return Ok(());
                    }
                    println!("▶️  Starting MCP server: {}", s.name);
                    println!("   Type    : {}", s.server_type);
                    println!("   Command : {}", s.command_or_url);
                    println!("✅ MCP server start requested (background process management not yet implemented)");
                }
                None => println!("❌ MCP server '{}' not found.", name),
            }
        }

        Some(McpCommands::Stop { name }) => {
            let mc = McpFileConfig::load()?;
            match mc.servers.iter().find(|s| s.name == *name) {
                Some(_) => {
                    println!("⏹️  Stopping MCP server: {}", name);
                    println!("✅ MCP server stop requested (background process management not yet implemented)");
                }
                None => println!("❌ MCP server '{}' not found.", name),
            }
        }

        Some(McpCommands::Import { file, overwrite }) => {
            let content = std::fs::read_to_string(file)
                .map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", file, e))?;

            let mut mc = McpFileConfig::load()?;
            let (added, updated, skipped);

            if content.trim_start().starts_with('[') {
                // 格式1：McpServerEntry 数组
                let entries: Vec<McpServerEntry> = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("Invalid JSON array format: {}", e))?;
                let result = merge_entries(&mut mc, entries, *overwrite);
                added = result.0; updated = result.1; skipped = result.2;
            } else {
                let val: serde_json::Value = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("Invalid JSON format: {}", e))?;

                if let Some(mcp_servers) = val.get("mcpServers").and_then(|v| v.as_object()) {
                    // 格式3：VSCode-style { "mcpServers": { ... } }
                    let result = mc.merge_from_vscode_format(mcp_servers, *overwrite)?;
                    added = result.0; updated = result.1; skipped = result.2;
                    // 打印每个条目的状态
                    for (name, _) in mcp_servers {
                        if updated > 0 {
                            println!("  🔄 Updated: {}", name);
                        } else if skipped > 0 {
                            println!("  ⏭️  Skipped: {}", name);
                        } else {
                            println!("  ✅ Added: {}", name);
                        }
                    }
                } else {
                    // 格式2：单个 McpServerEntry 对象
                    let single: McpServerEntry = serde_json::from_value(val)
                        .map_err(|e| anyhow::anyhow!("Invalid JSON object format: {}", e))?;
                    let result = merge_entries(&mut mc, vec![single], *overwrite);
                    added = result.0; updated = result.1; skipped = result.2;
                }
            }

            mc.save()?;
            println!("\n📥 Import complete: {} added, {} updated, {} skipped", added, updated, skipped);
            println!("   Saved to {}", McpFileConfig::config_path()?.display());
        }

        Some(McpCommands::Export { output }) => {
            let mc = McpFileConfig::load()?;
            if mc.servers.is_empty() {
                println!("⚠️  No MCP servers configured. Nothing to export.");
                return Ok(());
            }
            let json = serde_json::to_string_pretty(&mc.servers)?;
            std::fs::write(output, &json)
                .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", output, e))?;
            println!("📤 Exported {} MCP server(s) to: {}", mc.servers.len(), output);
        }

        Some(McpCommands::Path) => {
            let path = McpFileConfig::config_path()?;
            println!("📁 MCP config file: {}", path.display());
            println!("   You can edit this JSON file directly to add/modify/remove MCP servers.");
            println!("   Changes take effect immediately on next command run.");
        }

        None => {
            let mc = McpFileConfig::load().unwrap_or_default();
            let path = McpFileConfig::config_path().unwrap();
            let enabled = mc.servers.iter().filter(|s| s.enabled).count();
            println!("🔧 MCP (Model Context Protocol) Management");
            println!("   Servers     : {} configured, {} enabled", mc.servers.len(), enabled);
            println!("   Config file : {}", path.display());
            println!();
            println!("Commands:");
            println!("  list                         List all configured MCP servers");
            println!("  add <name> -c <cmd>          Add a new MCP server");
            println!("  show <name>                  Show server details");
            println!("  enable <name>                Enable a server");
            println!("  disable <name>               Disable a server");
            println!("  remove <name>                Remove a server");
            println!("  test <name>                  Test server configuration");
            println!("  list-tools [name]            List tools from server(s)");
            println!("  start <name>                 Start a server");
            println!("  stop <name>                  Stop a server");
            println!("  import <file.json>           Import servers from JSON file");
            println!("  export [-o file.json]        Export servers to JSON file");
            println!("  path                         Show config file path");
            println!();
            println!("  You can also edit the JSON config file directly:");
            println!("  {}", path.display());
        }
    }
    Ok(())
}

/// 合并 McpServerEntry 列表到配置中，返回 (added, updated, skipped)
fn merge_entries(
    mc: &mut McpFileConfig,
    entries: Vec<McpServerEntry>,
    overwrite: bool,
) -> (usize, usize, usize) {
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;

    for entry in entries {
        let existing_idx = mc.servers.iter().position(|s| s.name == entry.name);
        match existing_idx {
            Some(idx) if overwrite => {
                println!("  🔄 Updated: {}", entry.name);
                mc.servers[idx] = entry;
                updated += 1;
            }
            Some(_) => {
                println!("  ⏭️  Skipped (exists): {} (use --overwrite to replace)", entry.name);
                skipped += 1;
            }
            None => {
                println!("  ✅ Added: {} [{}]", entry.name, entry.server_type);
                mc.servers.push(entry);
                added += 1;
            }
        }
    }

    (added, updated, skipped)
}
