use anyhow::Result;
use std::io::Write;
use std::collections::HashMap;

use crate::config::{McpFileConfig, McpServerEntry, ModelsConfig};
use crate::core::chat::ChatEngine;
use crate::core::mcp::{McpToolInfo, fetch_tools_with_timeout, fetch_tools_http_with_timeout, check_http_reachable};

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
// /mcp 内联展示（类 Claude Code 风格）
// ─────────────────────────────────────────────

/// 展开环境变量（$HOME, $PATH, ${VAR} 等）
fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    // 展开 ${VAR} 格式
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            result = format!("{}{}{}", &result[..start], value, &result[start + end + 1..]);
        } else {
            break;
        }
    }
    // 展开 $VAR 格式（不含花括号）
    let mut out = String::new();
    let mut chars = result.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            let var: String = chars.by_ref()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !var.is_empty() {
                out.push_str(&std::env::var(&var).unwrap_or_default());
            } else {
                out.push('$');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 将 McpServerEntry 的 env 列表解析为 HashMap（支持环境变量展开）
fn parse_env(srv: &McpServerEntry) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in &srv.env {
        if let Some(eq) = entry.find('=') {
            let key = entry[..eq].trim().to_string();
            let val = expand_env_vars(entry[eq + 1..].trim());
            map.insert(key, val);
        }
    }
    map
}

/// 判断是否是 HTTP/HTTPS 类型
fn is_http_type(srv: &McpServerEntry) -> bool {
    let t = srv.server_type.to_lowercase();
    t == "http" || t == "https"
        || srv.command_or_url.starts_with("http://")
        || srv.command_or_url.starts_with("https://")
}

/// 将 McpToolInfo 转换为显示格式
/// 返回 (tool_name, tool_desc, params)
/// params 元素：(param_name, param_type, is_required, param_description)
fn mcp_tool_to_display(
    tool: &McpToolInfo,
) -> (String, String, Vec<(String, String, bool, String)>) {
    let name = tool.name.clone();
    let desc = tool.description.clone().unwrap_or_default();
    let mut params = vec![];

    if let Some(schema) = &tool.input_schema {
        if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
            let required: Vec<&str> = schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            for (pname, pval) in props {
                let ptype = pval.get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("any")
                    .to_string();
                let is_req = required.contains(&pname.as_str());
                let pdesc = pval.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                params.push((pname.clone(), ptype, is_req, pdesc));
            }
            // 必填参数优先，同级按名称排序
            params.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        }
    }

    (name, desc, params)
}

// 工具显示类型别名：(tool_name, tool_desc, params)
// params 元素：(param_name, param_type, is_required, param_description)
type ToolDisplay = (String, String, Vec<(String, String, bool, String)>);

/// 异步获取 MCP tools（支持 stdio 和 http/https）
async fn fetch_mcp_tools_async(srv: &McpServerEntry) -> Vec<ToolDisplay> {
    let env = parse_env(srv);

    if is_http_type(srv) {
        let url = &srv.command_or_url;
        let tools = fetch_tools_http_with_timeout(&srv.name, url, &env, 10).await;
        return tools.into_iter().map(|t| mcp_tool_to_display(&t)).collect();
    }

    let command = expand_env_vars(&srv.command_or_url);
    let args: Vec<String> = srv.args
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| expand_env_vars(s))
        .collect();

    let tools = fetch_tools_with_timeout(&srv.name, &command, &args, &env, 10).await;
    tools.into_iter().map(|t| mcp_tool_to_display(&t)).collect()
}

/// 同步包装（用于 spawn_blocking）
pub fn fetch_mcp_tools(srv: &McpServerEntry) -> Vec<ToolDisplay> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    match rt {
        Ok(rt) => rt.block_on(fetch_mcp_tools_async(srv)),
        Err(_) => vec![],
    }
}

/// 检测服务器连接状态（异步）
async fn check_server_connected(srv: &McpServerEntry) -> bool {
    if is_http_type(srv) {
        let env = parse_env(srv);
        check_http_reachable(&srv.command_or_url, &env, 3).await
    } else {
        // stdio: 在 blocking 线程中检查命令是否存在
        let cmd = expand_env_vars(&srv.command_or_url);
        let cmd_name = cmd.split_whitespace().next().unwrap_or("").to_string();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("which")
                .arg(&cmd_name)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }
}

// ─────────────────────────────────────────────
// 内联 TUI 渲染辅助
// ─────────────────────────────────────────────

/// 渲染服务列表（内联，不清屏），返回实际输出行数
fn render_server_list(
    servers: &[McpServerEntry],
    connected: &[bool],
    selected: usize,
) -> usize {
    // 标题行 + 上分隔线 + 每个server + 下分隔线 = servers.len() + 3
    println!("  {}{}MCP Servers{} {}({} total)  {}↑↓ navigate · Enter view tools · Esc exit{}",
        BOLD, BRIGHT_WHITE, RESET, GRAY, servers.len(), GRAY, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);

    for (i, srv) in servers.iter().enumerate() {
        let is_sel = i == selected;
        let sel_bg  = if is_sel { "\x1b[48;5;24m" } else { "" };
        let sel_rst = if is_sel { "\x1b[0m" } else { "" };
        let arrow   = if is_sel { "\x1b[97m▶\x1b[0m" } else { " " };

        let conn_icon = if !srv.enabled {
            format!("{}⏸{}", GRAY, RESET)
        } else if connected[i] {
            "\x1b[32m✅\x1b[0m".to_string()
        } else {
            "\x1b[31m✗\x1b[0m".to_string()
        };

        let type_label = match srv.server_type.to_lowercase().as_str() {
            "http" | "https" => "http ",
            "websocket"      => "ws   ",
            _                => "stdio",
        };

        let url_preview: String = srv.command_or_url.chars().take(40).collect();
        let url_ellipsis = if srv.command_or_url.len() > 40 { "…" } else { "" };

        println!(
            "  {} {}{} {}{}{}{}  \x1b[38;5;240m[{}]\x1b[0m{}  \x1b[38;5;244m{}{}\x1b[0m{}",
            arrow,
            sel_bg, conn_icon,
            BOLD, srv.name, sel_rst, RESET,
            type_label, RESET,
            url_preview, url_ellipsis, RESET,
        );
    }
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
    servers.len() + 3  // 标题 + 上分隔 + 每个server + 下分隔
}

/// 渲染 tools 列表（内联），返回实际输出行数
fn render_tools_list(
    srv_name: &str,
    tools: &[ToolDisplay],
    selected: usize,
) -> usize {
    println!("  {}{}{}\x1b[0m  {}› {} tools  {}↑↓ navigate · Enter detail · Esc back{}",
        BOLD, BRIGHT_WHITE, srv_name,
        GRAY, tools.len(), GRAY, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);

    let mut lines = 3usize; // 标题 + 上分隔 + 下分隔

    if tools.is_empty() {
        println!("  {}(no tools found or server not reachable){}", GRAY, RESET);
        lines += 1;
    } else {
        for (i, (tname, tdesc, tparams)) in tools.iter().enumerate() {
            let is_sel  = i == selected;
            let sel_bg  = if is_sel { "\x1b[48;5;24m" } else { "" };
            let sel_rst = if is_sel { "\x1b[0m" } else { "" };
            let arrow   = if is_sel { "\x1b[97m▶\x1b[0m" } else { " " };

            let param_info = if tparams.is_empty() {
                format!("{}no params{}", GRAY, RESET)
            } else {
                format!("{}{} params{}", GRAY, tparams.len(), RESET)
            };

            println!(
                "  {} {}\x1b[33m◆\x1b[0m{} {}{}{}  {}",
                arrow, sel_bg, sel_rst,
                BOLD, tname, RESET,
                param_info,
            );
            lines += 1;
            if !tdesc.is_empty() {
                let preview: String = tdesc.chars().take(68).collect();
                let ellipsis = if tdesc.len() > 68 { "…" } else { "" };
                println!("       {}{}{}{}", DIM, preview, ellipsis, RESET);
                lines += 1;
            }
        }
    }
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
    lines
}

/// 渲染 tool 详情（内联），返回实际输出行数
/// params 元素：(param_name, param_type, is_required, param_description)
fn render_tool_detail(
    srv_name: &str,
    tname: &str,
    tdesc: &str,
    params: &[(String, String, bool, String)],
) -> usize {
    // 空行 + 标题行 + 上分隔线 = 3
    let mut lines = 3usize;
    println!();
    println!("  {}{}{}\x1b[0m  {}›\x1b[0m  {}{}{}\x1b[0m  {}Esc back{}",
        BOLD, BRIGHT_WHITE, srv_name,
        GRAY,
        BOLD, "\x1b[33m", tname,
        GRAY, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);

    if !tdesc.is_empty() {
        println!("  {}", tdesc);
        println!();
        lines += 2; // 描述行 + 空行
    }

    if params.is_empty() {
        println!("  {}(no parameters){}", GRAY, RESET);
        lines += 1;
    } else {
        println!("  {}Parameters:{}", BOLD, RESET);
        lines += 1; // "Parameters:" 标题行
        for (pname, ptype, req, pdesc) in params {
            let req_label = if *req {
                "\x1b[31mrequired\x1b[0m".to_string()
            } else {
                format!("{}optional{}", GRAY, RESET)
            };
            if pdesc.is_empty() {
                println!("    \x1b[96m•\x1b[0m \x1b[97m{}\x1b[0m: {}{}\x1b[0m  {}",
                    pname, GRAY, ptype, req_label);
            } else {
                println!("    \x1b[96m•\x1b[0m \x1b[97m{}\x1b[0m: {}{}\x1b[0m  {}  {}{}\x1b[0m",
                    pname, GRAY, ptype, req_label, DIM, pdesc);
            }
            lines += 1;
        }
    }
    println!("  {}{}{}", GRAY, "─".repeat(60), RESET);
    lines + 1 // +1 for 下分隔线
}

/// 等待键盘事件（raw mode 下）
fn wait_key() -> Option<crossterm::event::KeyCode> {
    use crossterm::event::{read, Event, KeyEvent, KeyModifiers};
    loop {
        match read() {
            Ok(Event::Key(KeyEvent { code, modifiers, .. })) => {
                if code == crossterm::event::KeyCode::Char('c')
                    && modifiers == crossterm::event::KeyModifiers::CONTROL
                {
                    return Some(crossterm::event::KeyCode::Esc);
                }
                return Some(code);
            }
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

/// /mcp 内联交互式浏览器：↑↓ 导航，Enter 进入，Esc 返回
pub async fn cmd_mcp_browser() -> Result<()> {
    use crossterm::terminal::{enable_raw_mode, disable_raw_mode};

    let cfg = match McpFileConfig::load() {
        Ok(c) => c,
        Err(e) => {
            println!("{}Failed to load MCP config: {}{}", YELLOW, e, RESET);
            return Ok(());
        }
    };

    if cfg.servers.is_empty() {
        println!("{}No MCP servers configured.{}", GRAY, RESET);
        println!("{}Use 'numina mcp add' to add a server.{}", DIM, RESET);
        return Ok(());
    }

    let servers = cfg.servers.clone();

    // ── 检测连接状态 ──
    print!("  {}Checking connections…{}", DIM, RESET);
    std::io::stdout().flush()?;

    let mut connected: Vec<bool> = Vec::with_capacity(servers.len());
    for srv in servers.iter() {
        connected.push(if srv.enabled { check_server_connected(srv).await } else { false });
    }
    print!("\r\x1b[2K");
    std::io::stdout().flush()?;

    // ── 服务列表层 ──
    let mut srv_sel = 0usize;
    let mut srv_lines = render_server_list(&servers, &connected, srv_sel);

    enable_raw_mode()?;

    'srv_loop: loop {
        match wait_key() {
            Some(crossterm::event::KeyCode::Esc) | Some(crossterm::event::KeyCode::Char('q')) => {
                break 'srv_loop;
            }
            Some(crossterm::event::KeyCode::Up) => {
                if srv_sel > 0 { srv_sel -= 1; } else { srv_sel = servers.len().saturating_sub(1); }
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", srv_lines);
                std::io::stdout().flush()?;
                srv_lines = render_server_list(&servers, &connected, srv_sel);
                enable_raw_mode()?;
            }
            Some(crossterm::event::KeyCode::Down) => {
                srv_sel = (srv_sel + 1) % servers.len();
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", srv_lines);
                std::io::stdout().flush()?;
                srv_lines = render_server_list(&servers, &connected, srv_sel);
                enable_raw_mode()?;
            }
            Some(crossterm::event::KeyCode::Enter) => {
                disable_raw_mode()?;

                let srv = &servers[srv_sel];
                println!();
                print!("  {}⏳ Fetching tools from {}…{}", DIM, srv.name, RESET);
                std::io::stdout().flush()?;

                let srv_clone = srv.clone();
                let tools = tokio::task::spawn_blocking(move || fetch_mcp_tools(&srv_clone))
                    .await
                    .unwrap_or_default();

                // 清除加载行（⏳ 那一行 + 上面的空行）
                print!("\r\x1b[2K\x1b[1A\r\x1b[2K");
                std::io::stdout().flush()?;

                // 清除服务列表，切换到 tools 列表
                print!("\x1b[{}A\x1b[0J", srv_lines);
                std::io::stdout().flush()?;

                let srv_name = srv.name.clone();
                let mut tool_sel = 0usize;
                let mut tool_lines = render_tools_list(&srv_name, &tools, tool_sel);

                enable_raw_mode()?;

                // ── Tools 列表层 ──
                'tools_loop: loop {
                    match wait_key() {
                        Some(crossterm::event::KeyCode::Esc) => {
                            disable_raw_mode()?;
                            // 清除 tools 列表，重绘服务列表
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            std::io::stdout().flush()?;
                            srv_lines = render_server_list(&servers, &connected, srv_sel);
                            enable_raw_mode()?;
                            break 'tools_loop;
                        }
                        Some(crossterm::event::KeyCode::Up) if !tools.is_empty() => {
                            if tool_sel > 0 { tool_sel -= 1; } else { tool_sel = tools.len().saturating_sub(1); }
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            std::io::stdout().flush()?;
                            tool_lines = render_tools_list(&srv_name, &tools, tool_sel);
                            enable_raw_mode()?;
                        }
                        Some(crossterm::event::KeyCode::Down) if !tools.is_empty() => {
                            tool_sel = (tool_sel + 1) % tools.len();
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            std::io::stdout().flush()?;
                            tool_lines = render_tools_list(&srv_name, &tools, tool_sel);
                            enable_raw_mode()?;
                        }
                        Some(crossterm::event::KeyCode::Enter) if !tools.is_empty() => {
                            disable_raw_mode()?;
                            let (tname, tdesc, tparams) = &tools[tool_sel];
                            // 清除 tools 列表，显示详情
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            std::io::stdout().flush()?;
                            let detail_lines = render_tool_detail(&srv_name, tname, tdesc, tparams);

                            enable_raw_mode()?;

                            // ── Tool 详情层：等待 Esc 返回 ──
                            loop {
                                match wait_key() {
                                    Some(crossterm::event::KeyCode::Esc) => {
                                        disable_raw_mode()?;
                                        // 清除详情，重绘 tools 列表
                                        print!("\x1b[{}A\x1b[0J", detail_lines);
                                        std::io::stdout().flush()?;
                                        tool_lines = render_tools_list(&srv_name, &tools, tool_sel);
                                        enable_raw_mode()?;
                                        break;
                                    }
                                    None => break,
                                    _ => {}
                                }
                            }
                        }
                        None => break 'tools_loop,
                        _ => {}
                    }
                }
            }
            None => break 'srv_loop,
            _ => {}
        }
    }

    disable_raw_mode()?;
    println!();
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
                let type_label = match srv.server_type.to_lowercase().as_str() {
                    "http" | "https" => "http     ",
                    "websocket"      => "ws       ",
                    _                => "stdio    ",
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
