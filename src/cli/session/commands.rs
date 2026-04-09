use anyhow::Result;
use std::io::Write;

use crate::config::{McpFileConfig, McpServerEntry, ModelsConfig};
use crate::core::chat::ChatEngine;

use super::renderer::{BOLD, BRIGHT_WHITE, CYAN, DIM, GRAY, GREEN, RESET, YELLOW};

// ─────────────────────────────────────────────
// 子命令实现
// ─────────────────────────────────────────────

pub fn cmd_sessions() -> Result<()> {
    let sessions = ChatEngine::list_sessions()?;
    if sessions.is_empty() {
        println!("{}No sessions found.{}", GRAY, RESET);
        return Ok(());
    }
    println!();
    println!("  {}{}Sessions ({} total){}", BOLD, BRIGHT_WHITE, sessions.len(), RESET);
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
    for (i, sid) in sessions.iter().enumerate() {
        if let Ok(s) = ChatEngine::get_session(sid) {
            let turns = s.turns.len();
            let preview = s
                .turns
                .first()
                .map(|t| {
                    let c = t.content.chars().take(45).collect::<String>();
                    if t.content.len() > 45 { format!("{}…", c) } else { c }
                })
                .unwrap_or_else(|| "(empty)".to_string());
            println!(
                "  {}{}{}  {}{}{}  {}{}t{}  {}{}{}",
                GRAY, i + 1, RESET,
                BOLD, &sid[..8], RESET,
                GRAY, turns, RESET,
                DIM, preview, RESET
            );
        } else {
            println!("  {}{}  {}{}", GRAY, i + 1, sid, RESET);
        }
    }
    println!();
    Ok(())
}

/// 列出模型让用户输入编号选择，返回 Some(name) 表示已切换，None 表示取消
pub fn cmd_model_picker() -> Result<Option<String>> {
    let mut cfg = match ModelsConfig::load() {
        Ok(c) => c,
        Err(e) => {
            println!("{}Failed to load models: {}{}", YELLOW, e, RESET);
            return Ok(None);
        }
    };

    if cfg.models.is_empty() {
        println!("{}No models configured. Run 'numina model add' first.{}", GRAY, RESET);
        return Ok(None);
    }

    println!();
    println!("  {}{}Models{} {}(enter number to select · Enter to cancel){}",
        BOLD, BRIGHT_WHITE, RESET, GRAY, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(56), RESET);

    for (i, m) in cfg.models.iter().enumerate() {
        let is_active = m.name == cfg.active;
        let active_dot = if is_active { format!(" {}●{}", "\x1b[32m", RESET) } else { String::new() };
        let ctx_k = m.max_tokens.map(|t| format!("{}k", t / 1000)).unwrap_or_else(|| "?k".to_string());
        println!("  {}{}{}{}. {}{}{}{} {}({}){}  {}({}){} {}{}{}",
            BOLD, BRIGHT_WHITE, i + 1, RESET,
            BOLD, m.name, active_dot, RESET,
            GRAY, m.provider, RESET,
            GRAY, ctx_k, RESET,
            DIM, m.description.as_deref().unwrap_or(""), RESET,
        );
    }
    println!("  {}{}{}", GRAY, "─".repeat(56), RESET);
    print!("  {}Select [1-{}] or Enter to cancel:{} ", GRAY, cfg.models.len(), RESET);
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        println!("{}Cancelled.{}", GRAY, RESET);
        return Ok(None);
    }

    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= cfg.models.len() => {
            let name = cfg.models[n - 1].name.clone();
            cfg.active = name.clone();
            let _ = cfg.save();
            Ok(Some(name))
        }
        _ => {
            println!("{}Invalid selection.{}", YELLOW, RESET);
            Ok(None)
        }
    }
}

// ─────────────────────────────────────────────
// /mcp 交互式浏览器
// ─────────────────────────────────────────────

/// 通过 stdio 子进程调用 MCP server 的 tools/list
pub fn fetch_mcp_tools(srv: &McpServerEntry) -> Vec<(String, String, Vec<(String, String, bool)>)> {
    if srv.server_type != "stdio" {
        return vec![];
    }

    let mut parts = vec![srv.command_or_url.clone()];
    if let Some(args_str) = &srv.args {
        for a in args_str.split_whitespace() {
            parts.push(a.to_string());
        }
    }
    if parts.is_empty() {
        return vec![];
    }

    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"numina","version":"0.1.0"}}}"#;
    let list_msg = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;

    let input = format!("{}\n{}\n", init_msg, list_msg);

    let output = std::process::Command::new(&parts[0])
        .args(&parts[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            if let Some(stdin) = child.stdin.take() {
                let _ = { let mut s = stdin; s.write_all(input.as_bytes()) };
            }
            child.wait_with_output()
        });

    let stdout_bytes = match output {
        Ok(o) => o.stdout,
        Err(_) => return vec![],
    };

    let text = String::from_utf8_lossy(&stdout_bytes);
    let mut tools = vec![];

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if val.get("id") == Some(&serde_json::json!(2)) {
                if let Some(arr) = val.pointer("/result/tools").and_then(|v| v.as_array()) {
                    for tool in arr {
                        let name = tool["name"].as_str().unwrap_or("?").to_string();
                        let desc = tool["description"].as_str().unwrap_or("").to_string();
                        let mut params = vec![];
                        if let Some(props) = tool.pointer("/inputSchema/properties").and_then(|v| v.as_object()) {
                            let required: Vec<&str> = tool.pointer("/inputSchema/required")
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            for (pname, pval) in props {
                                let ptype = pval["type"].as_str().unwrap_or("any").to_string();
                                let is_req = required.contains(&pname.as_str());
                                params.push((pname.clone(), ptype, is_req));
                            }
                        }
                        tools.push((name, desc, params));
                    }
                }
                break;
            }
        }
    }
    tools
}

/// MCP 浏览器：列出 server，输入编号查看 tools
pub async fn cmd_mcp_browser() -> Result<()> {
    let cfg = match McpFileConfig::load() {
        Ok(c) => c,
        Err(e) => {
            println!("{}Failed to load MCP config: {}{}", YELLOW, e, RESET);
            return Ok(());
        }
    };

    if cfg.servers.is_empty() {
        println!("{}No MCP servers configured. Use 'numina mcp add' to add one.{}", GRAY, RESET);
        return Ok(());
    }

    let servers = &cfg.servers;

    loop {
        println!();
        println!("  {}{}MCP Servers{} {}(enter number to view tools · Enter to exit){}",
            BOLD, BRIGHT_WHITE, RESET, GRAY, RESET);
        println!("  {}{}{}", GRAY, "─".repeat(60), RESET);

        for (i, srv) in servers.iter().enumerate() {
            let status = if srv.enabled {
                format!("{}●{}", "\x1b[32m", RESET)
            } else {
                format!("{}○{}", GRAY, RESET)
            };
            println!("  {}{}{}{}. {} {}{}{} {}[{}]{}",
                BOLD, BRIGHT_WHITE, i + 1, RESET,
                status,
                BOLD, srv.name, RESET,
                GRAY, srv.server_type, RESET,
            );
        }
        println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
        print!("  {}Select [1-{}] or Enter to exit:{} ", GRAY, servers.len(), RESET);
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            break;
        }

        match input.parse::<usize>() {
            Ok(n) if n >= 1 && n <= servers.len() => {
                let srv = &servers[n - 1];
                println!();
                println!("  {}⏳ Fetching tools from {}...{}", GRAY, srv.name, RESET);
                let tools = fetch_mcp_tools(srv);

                println!();
                println!("  {}{}Tools for: {}{}{}", BOLD, BRIGHT_WHITE, "\x1b[96m", srv.name, RESET);
                println!("  {}{}{}", GRAY, "─".repeat(60), RESET);

                if tools.is_empty() {
                    println!("  {}  (no tools found or server not reachable){}", GRAY, RESET);
                } else {
                    for (tname, tdesc, tparams) in &tools {
                        println!("  {}◆ {}{}{}{}", "\x1b[33m", RESET, BOLD, tname, RESET);
                        if !tdesc.is_empty() {
                            let preview: String = tdesc.chars().take(80).collect();
                            let ellipsis = if tdesc.len() > 80 { "..." } else { "" };
                            println!("     {}  {}{}{}", GRAY, preview, ellipsis, RESET);
                        }
                        for (pname, ptype, req) in tparams {
                            let req_mark = if *req {
                                format!("{}*{}", "\x1b[31m", RESET)
                            } else {
                                format!("{}?{}", GRAY, RESET)
                            };
                            println!("     {}  {} {}{}{}: {}{}{}",
                                DIM, req_mark,
                                "\x1b[96m", pname, RESET,
                                GRAY, ptype, RESET);
                        }
                    }
                }
                println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
            }
            _ => {
                println!("{}Invalid selection.{}", YELLOW, RESET);
            }
        }
    }

    Ok(())
}

pub fn cmd_mcp_list() {
    match McpFileConfig::load() {
        Ok(cfg) => {
            if cfg.servers.is_empty() {
                println!("{}No MCP servers configured.{}", GRAY, RESET);
                println!("{}Use 'numina mcp add' to add a server.{}", DIM, RESET);
                return;
            }
            println!();
            println!("  {}{}MCP Servers ({} total){}", BOLD, BRIGHT_WHITE, cfg.servers.len(), RESET);
            println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
            for (i, srv) in cfg.servers.iter().enumerate() {
                let _ = i;
                let status = if srv.enabled {
                    format!("{}●{}", "\x1b[32m", RESET)
                } else {
                    format!("{}○{}", GRAY, RESET)
                };
                let type_label = match srv.server_type.as_str() {
                    "http"      => "http     ",
                    "websocket" => "ws       ",
                    _           => "stdio    ",
                };
                println!(
                    "  {} {}{}{}{}  {}{}{}  {}{}{}",
                    status,
                    BOLD, BRIGHT_WHITE, srv.name, RESET,
                    GRAY, type_label, RESET,
                    DIM, srv.command_or_url, RESET,
                );
                if let Some(desc) = &srv.description {
                    if !desc.is_empty() {
                        println!("       {}  {}{}", GRAY, desc, RESET);
                    }
                }
            }
            println!();
        }
        Err(e) => {
            println!("{}Failed to load MCP config: {}{}", YELLOW, e, RESET);
        }
    }
}

pub fn cmd_show(session_id: &str) -> Result<()> {
    use crate::core::chat::ChatSession;
    let session: ChatSession = ChatEngine::get_session(session_id)?;
    println!();
    println!("  {}{}Session: {}{}", BOLD, BRIGHT_WHITE, session.id, RESET);
    println!("  {}Model:   {}{}", GRAY, session.model, RESET);
    println!("  {}Created: {}{}", GRAY, session.created_at, RESET);
    println!("  {}Turns:   {}{}", GRAY, session.turns.len(), RESET);
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
    println!();

    for turn in &session.turns {
        let (label, color) = match turn.role.as_str() {
            "assistant" => ("Numina", CYAN),
            _ => ("You", GREEN),
        };
        println!("  {}{}{}{} {}{}{}",
            BOLD, color, label, RESET,
            GRAY, turn.timestamp, RESET
        );
        println!("  {}", turn.content);
        println!();
    }
    Ok(())
}
