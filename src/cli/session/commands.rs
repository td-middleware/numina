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

/// 计算每页可显示的工具数（统一逻辑，避免调用方和渲染函数不一致）
fn tools_page_size(term_h: usize) -> usize {
    let rows_per_tool = 2usize;
    // 标题(1) + 上分隔(1) + 下分隔(1) + 页码提示(1) + 安全余量(4) = 8
    // 安全余量确保列表不触发终端滚动
    let header_lines = 8usize;
    let available = term_h.saturating_sub(header_lines);
    (available / rows_per_tool).max(3)
}

/// 根据 selected 计算 page_start（保证 selected 在当前页内）
fn tools_page_start(selected: usize, tools_len: usize, term_h: usize) -> usize {
    if tools_len == 0 { return 0; }
    let page_size = tools_page_size(term_h);
    (selected / page_size) * page_size
}

/// 渲染 tools 列表（cooked mode 下用 println! 渲染），返回实际输出行数
/// 调用方负责在调用前用 \x1b[{}A\x1b[0J 清除上一次的输出
fn render_tools_list(
    srv_name: &str,
    tools: &[ToolDisplay],
    selected: usize,
    page_start: usize,
    term_h: usize,
) -> usize {
    let page_size = tools_page_size(term_h);
    let page_end = (page_start + page_size).min(tools.len());
    let page_tools = if tools.is_empty() { &tools[..] } else { &tools[page_start..page_end] };

    let sep = "─".repeat(60);
    println!("  {}{}{}\x1b[0m  {}› {} tools{}",
        BOLD, BRIGHT_WHITE, srv_name, GRAY, tools.len(), RESET);
    println!("  {}{}{}", GRAY, sep, RESET);

    let mut lines = 3usize; // 标题 + 上分隔 + 下分隔（预计入）

    if tools.is_empty() {
        println!("  {}(no tools found or server not reachable){}", GRAY, RESET);
        lines += 1;
    } else {
        for (rel_i, (tname, tdesc, tparams)) in page_tools.iter().enumerate() {
            let abs_i = page_start + rel_i;
            let is_sel  = abs_i == selected;
            let sel_bg  = if is_sel { "\x1b[48;5;24m" } else { "" };
            let sel_rst = if is_sel { "\x1b[0m" } else { "" };
            let arrow   = if is_sel { "\x1b[97m▶\x1b[0m" } else { " " };

            let param_info = if tparams.is_empty() {
                format!("{}no params{}", GRAY, RESET)
            } else {
                format!("{}{} params{}", GRAY, tparams.len(), RESET)
            };

            println!("  {} {}\x1b[33m◆\x1b[0m{} {}{}{}  {}",
                arrow, sel_bg, sel_rst, BOLD, tname, RESET, param_info);
            lines += 1;
            // 取描述的第一行再截断，确保不含 \n，不引起 wrap → lines 计数准确
            let first_line = tdesc.lines().next().unwrap_or("");
            if !first_line.is_empty() {
                let preview: String = first_line.chars().take(68).collect();
                let ellipsis = if first_line.chars().count() > 68 || tdesc.contains('\n') { "…" } else { "" };
                println!("       {}{}{}{}", DIM, preview, ellipsis, RESET);
            } else {
                println!();
            }
            lines += 1;
        }
        // 页码提示（多页时显示）
        let total_pages = (tools.len() + page_size - 1) / page_size;
        let cur_page = page_start / page_size + 1;
        if total_pages > 1 {
            println!("  {}  {}/{} pages  ({}-{} of {})  ↑↓ navigate{}",
                GRAY, cur_page, total_pages, page_start + 1, page_end, tools.len(), RESET);
            lines += 1;
        }
    }
    println!("  {}{}{}", GRAY, sep, RESET);
    lines
}

/// 渲染 tool 详情（cooked mode 下用 println! 渲染），返回实际输出行数
/// 调用方负责在调用前用 \x1b[{}A\x1b[0J 清除上一次的输出
fn render_tool_detail(
    srv_name: &str,
    tname: &str,
    tdesc: &str,
    params: &[(String, String, bool, String)],
) -> usize {
    let sep = "─".repeat(60);
    println!("  {}{}{}\x1b[0m  {}›\x1b[0m  {}{}{}\x1b[0m  {}Esc back{}",
        BOLD, BRIGHT_WHITE, srv_name, GRAY, BOLD, "\x1b[33m", tname, GRAY, RESET);
    println!("  {}{}{}", GRAY, sep, RESET);

    let mut lines = 3usize; // 标题 + 上分隔 + 下分隔

    if !tdesc.is_empty() {
        // 描述可能含 \n，按行拆分，每行单独打印并截断，确保 lines 计数准确
        let desc_lines: Vec<&str> = tdesc.lines().collect();
        let show_n = desc_lines.len().min(6);  // 最多显示 6 行描述
        for l in desc_lines.iter().take(show_n) {
            let s: String = l.chars().take(76).collect();
            let e = if l.chars().count() > 76 { "…" } else { "" };
            println!("  {}{}{}{}", DIM, s, e, RESET);
            lines += 1;
        }
        if desc_lines.len() > show_n {
            println!("  {}…{}", DIM, RESET);
            lines += 1;
        }
        println!();  // 空行
        lines += 1;
    }

    if params.is_empty() {
        println!("  {}(no parameters){}", GRAY, RESET);
        lines += 1;
    } else {
        println!("  {}Parameters:{}", BOLD, RESET);
        lines += 1;
        for (pname, ptype, req, pdesc) in params {
            let req_label = if *req {
                "\x1b[31mrequired\x1b[0m".to_string()
            } else {
                format!("{}optional{}", GRAY, RESET)
            };
            // 参数描述取第一行再截断，避免 \n 或 wrap 导致行数不匹配
            let pdesc_line = pdesc.lines().next().unwrap_or("");
            let pdesc_short: String = pdesc_line.chars().take(55).collect();
            let pdesc_e = if pdesc_line.chars().count() > 55 || pdesc.contains('\n') { "…" } else { "" };
            if pdesc_short.is_empty() {
                println!("    \x1b[96m•\x1b[0m \x1b[97m{}\x1b[0m: {}{}\x1b[0m  {}",
                    pname, GRAY, ptype, req_label);
            } else {
                println!("    \x1b[96m•\x1b[0m \x1b[97m{}\x1b[0m: {}{}\x1b[0m  {}  {}{}{}\x1b[0m",
                    pname, GRAY, ptype, req_label, DIM, pdesc_short, pdesc_e);
            }
            lines += 1;
        }
    }
    println!("  {}{}{}", GRAY, sep, RESET);
    lines
}

/// 等待键盘事件（raw mode 下）
fn wait_key() -> Option<crossterm::event::KeyCode> {
    use crossterm::event::{read, Event, KeyEvent};
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
/// 内嵌在当前终端位置渲染，不使用 alternate screen。
/// 技术：先向下打印 tui_reserve 个空行腾出视口空间，再 \x1b[NA 向上复位，
/// 之后所有重绘用相对行数 \x1b[{N}A\x1b[0J 清除，不再依赖绝对坐标。
pub async fn cmd_mcp_browser() -> Result<()> {
    use crossterm::terminal::{enable_raw_mode, disable_raw_mode};
    use crossterm::event::KeyCode;

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

    let term_h = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);

    // ── 预留 TUI 渲染空间 ──
    // 先向下打印 tui_reserve 个空行，使视口下方有足够空间；
    // 再 \x1b[NA 向上移动复位，\x1b[0J 清除从光标到底部（清除预留空行）。
    // 此后无论渲染多少行都不会触发终端滚动，相对行数清除因此可靠工作。
    let tool_max_lines = tools_page_size(term_h) * 2 + 6;
    let srv_max_lines = servers.len() + 3;
    let tui_reserve = tool_max_lines.max(srv_max_lines).min(term_h.saturating_sub(2));
    for _ in 0..tui_reserve { println!(); }
    print!("\x1b[{}A\x1b[0J", tui_reserve);
    std::io::stdout().flush()?;

    // ── 服务列表层 ──
    let mut srv_sel = 0usize;
    let mut srv_lines = render_server_list(&servers, &connected, srv_sel);
    std::io::stdout().flush()?;

    enable_raw_mode()?;

    'srv_loop: loop {
        match wait_key() {
            Some(KeyCode::Esc) | Some(KeyCode::Char('q')) => break 'srv_loop,
            Some(KeyCode::Up) => {
                srv_sel = if srv_sel > 0 { srv_sel - 1 } else { servers.len().saturating_sub(1) };
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", srv_lines);
                srv_lines = render_server_list(&servers, &connected, srv_sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Down) => {
                srv_sel = (srv_sel + 1) % servers.len();
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", srv_lines);
                srv_lines = render_server_list(&servers, &connected, srv_sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Enter) => {
                let srv = &servers[srv_sel];
                disable_raw_mode()?;
                // 清除服务列表，显示加载提示
                print!("\x1b[{}A\x1b[0J", srv_lines);
                print!("  {}⏳ Fetching tools from {}…{}", DIM, srv.name, RESET);
                std::io::stdout().flush()?;

                let srv_clone = srv.clone();
                let tools = tokio::task::spawn_blocking(move || fetch_mcp_tools(&srv_clone))
                    .await
                    .unwrap_or_default();

                let srv_name = srv.name.clone();
                let mut tool_sel = 0usize;
                let th = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
                let mut page_start = tools_page_start(tool_sel, tools.len(), th);

                // 清除加载提示行，渲染工具列表
                print!("\r\x1b[2K");
                let mut tool_lines = render_tools_list(&srv_name, &tools, tool_sel, page_start, th);
                std::io::stdout().flush()?;
                enable_raw_mode()?;

                // ── Tools 列表层 ──
                'tools_loop: loop {
                    match wait_key() {
                        Some(KeyCode::Esc) => {
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            srv_lines = render_server_list(&servers, &connected, srv_sel);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                            break 'tools_loop;
                        }
                        Some(KeyCode::Up) if !tools.is_empty() => {
                            tool_sel = if tool_sel > 0 { tool_sel - 1 } else { tools.len().saturating_sub(1) };
                            let th = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
                            page_start = tools_page_start(tool_sel, tools.len(), th);
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            tool_lines = render_tools_list(&srv_name, &tools, tool_sel, page_start, th);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                        }
                        Some(KeyCode::Down) if !tools.is_empty() => {
                            tool_sel = (tool_sel + 1) % tools.len();
                            let th = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
                            page_start = tools_page_start(tool_sel, tools.len(), th);
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            tool_lines = render_tools_list(&srv_name, &tools, tool_sel, page_start, th);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                        }
                        Some(KeyCode::Enter) if !tools.is_empty() => {
                            let (tname, tdesc, tparams) = &tools[tool_sel];
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", tool_lines);
                            let mut detail_lines = render_tool_detail(&srv_name, tname, tdesc, tparams);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;

                            // ── Tool 详情层：等待 Esc 返回工具列表 ──
                            loop {
                                match wait_key() {
                                    Some(KeyCode::Esc) => {
                                        let th = crossterm::terminal::size().map(|(_, h)| h as usize).unwrap_or(24);
                                        page_start = tools_page_start(tool_sel, tools.len(), th);
                                        disable_raw_mode()?;
                                        print!("\x1b[{}A\x1b[0J", detail_lines);
                                        tool_lines = render_tools_list(&srv_name, &tools, tool_sel, page_start, th);
                                        std::io::stdout().flush()?;
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

// ─────────────────────────────────────────────
// /auth — 交互式授权管理菜单
// ─────────────────────────────────────────────

/// /auth 交互式菜单：渠道选择 → 操作选择
/// 第一层：选择渠道（Lark / 未来可扩展）
/// 第二层：选择操作（登录 / 查看状态 / 退出登录）
pub async fn cmd_auth_browser() -> Result<()> {
    use crossterm::terminal::{enable_raw_mode, disable_raw_mode};
    use crossterm::event::KeyCode;
    use tokio::process::Command;

    // ── 渠道列表 ──
    let channels: &[(&str, &str)] = &[
        ("Lark / 飞书", "通过 lark-cli 管理飞书用户授权"),
    ];

    // ── 操作列表（Lark） ──
    let lark_actions: &[(&str, &str)] = &[
        ("login",         "飞书用户登录授权（lark-cli auth login）"),
        ("login --scope", "按 scope 授权（输入 scope 后执行）"),
        ("login --domain","按业务域授权（输入 domain 后执行）"),
        ("status",        "查看当前授权状态"),
        ("logout",        "退出飞书登录"),
    ];

    // ── 预留渲染空间 ──
    let tui_reserve = 12usize;
    for _ in 0..tui_reserve { println!(); }
    print!("\x1b[{}A\x1b[0J", tui_reserve);
    std::io::stdout().flush()?;

    // ── 渲染渠道列表 ──
    let mut ch_sel = 0usize;
    let mut ch_lines = render_auth_channels(channels, ch_sel);
    std::io::stdout().flush()?;

    enable_raw_mode()?;

    'ch_loop: loop {
        match wait_key() {
            Some(KeyCode::Esc) | Some(KeyCode::Char('q')) => break 'ch_loop,
            Some(KeyCode::Up) => {
                ch_sel = if ch_sel > 0 { ch_sel - 1 } else { channels.len().saturating_sub(1) };
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", ch_lines);
                ch_lines = render_auth_channels(channels, ch_sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Down) => {
                ch_sel = (ch_sel + 1) % channels.len();
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", ch_lines);
                ch_lines = render_auth_channels(channels, ch_sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Enter) => {
                // 进入操作层
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", ch_lines);

                let mut act_sel = 0usize;
                let ch_name = channels[ch_sel].0;
                let mut act_lines = render_auth_actions(ch_name, lark_actions, act_sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;

                'act_loop: loop {
                    match wait_key() {
                        Some(KeyCode::Esc) => {
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", act_lines);
                            ch_lines = render_auth_channels(channels, ch_sel);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                            break 'act_loop;
                        }
                        Some(KeyCode::Up) => {
                            act_sel = if act_sel > 0 { act_sel - 1 } else { lark_actions.len().saturating_sub(1) };
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", act_lines);
                            act_lines = render_auth_actions(ch_name, lark_actions, act_sel);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                        }
                        Some(KeyCode::Down) => {
                            act_sel = (act_sel + 1) % lark_actions.len();
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", act_lines);
                            act_lines = render_auth_actions(ch_name, lark_actions, act_sel);
                            std::io::stdout().flush()?;
                            enable_raw_mode()?;
                        }
                        Some(KeyCode::Enter) => {
                            let action = lark_actions[act_sel].0;
                            disable_raw_mode()?;
                            print!("\x1b[{}A\x1b[0J", act_lines);
                            std::io::stdout().flush()?;

                            // 执行选中的操作
                            match action {
                                "login" => {
                                    cmd_lark_login("").await?;
                                }
                                "login --scope" => {
                                    // 提示用户输入 scope
                                    print!("  {}输入 scope（如 im:message calendar:calendar:readonly）：{} ", GRAY, RESET);
                                    std::io::stdout().flush()?;
                                    let mut scope = String::new();
                                    std::io::stdin().read_line(&mut scope)?;
                                    let scope = scope.trim().to_string();
                                    if !scope.is_empty() {
                                        cmd_lark_login(&format!("--scope {}", scope)).await?;
                                    } else {
                                        println!("  {}已取消{}", GRAY, RESET);
                                    }
                                }
                                "login --domain" => {
                                    // 提示用户输入 domain
                                    print!("  {}输入 domain（如 calendar、im、drive）：{} ", GRAY, RESET);
                                    std::io::stdout().flush()?;
                                    let mut domain = String::new();
                                    std::io::stdin().read_line(&mut domain)?;
                                    let domain = domain.trim().to_string();
                                    if !domain.is_empty() {
                                        cmd_lark_login(&format!("--domain {}", domain)).await?;
                                    } else {
                                        println!("  {}已取消{}", GRAY, RESET);
                                    }
                                }
                                "status" => {
                                    cmd_lark_auth_status().await?;
                                }
                                "logout" => {
                                    cmd_lark_auth_logout().await?;
                                }
                                _ => {}
                            }

                            // 操作完成后退出菜单
                            break 'ch_loop;
                        }
                        None => break 'act_loop,
                        _ => {}
                    }
                }
            }
            None => break 'ch_loop,
            _ => {}
        }
    }

    disable_raw_mode()?;
    println!();
    Ok(())
}

/// 渲染渠道选择列表，返回行数
fn render_auth_channels(channels: &[(&str, &str)], sel: usize) -> usize {
    let sep = "─".repeat(50);
    println!("  {}{}Auth 授权管理{}  {}↑↓ 选择  Enter 进入  Esc 退出{}", BOLD, BRIGHT_WHITE, RESET, GRAY, RESET);
    println!("  {}{}{}", GRAY, sep, RESET);
    let mut lines = 2usize;
    for (i, (name, desc)) in channels.iter().enumerate() {
        if i == sel {
            println!("  {}{} ▶ {}{:<20}{}  {}{}{}", BOLD, CYAN, BRIGHT_WHITE, name, RESET, DIM, desc, RESET);
        } else {
            println!("     {}{:<20}{}  {}{}{}", GRAY, name, RESET, DIM, desc, RESET);
        }
        lines += 1;
    }
    println!("  {}{}{}", GRAY, sep, RESET);
    lines += 1;
    lines
}

/// 渲染操作选择列表，返回行数
fn render_auth_actions(ch_name: &str, actions: &[(&str, &str)], sel: usize) -> usize {
    let sep = "─".repeat(50);
    println!("  {}{}{}{}  {}›{}  {}操作列表{}  {}↑↓ 选择  Enter 执行  Esc 返回{}",
        BOLD, BRIGHT_WHITE, ch_name, RESET, GRAY, RESET, BOLD, RESET, GRAY, RESET);
    println!("  {}{}{}", GRAY, sep, RESET);
    let mut lines = 2usize;
    for (i, (action, desc)) in actions.iter().enumerate() {
        if i == sel {
            println!("  {}{} ▶ {}{:<22}{}  {}{}{}", BOLD, CYAN, BRIGHT_WHITE, action, RESET, DIM, desc, RESET);
        } else {
            println!("     {}{:<22}{}  {}{}{}", GRAY, action, RESET, DIM, desc, RESET);
        }
        lines += 1;
    }
    println!("  {}{}{}", GRAY, sep, RESET);
    lines += 1;
    lines
}

/// 查看飞书授权状态（TUI 内）
async fn cmd_lark_auth_status() -> Result<()> {
    use tokio::process::Command;
    println!();
    println!("  {}🔍 查询飞书授权状态...{}", GRAY, RESET);

    let output = Command::new("lark-cli")
        .args(["contact", "user", "me", "--as", "user"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let name = json.pointer("/data/name").and_then(|v| v.as_str()).unwrap_or("未知");
                let email = json.pointer("/data/email").and_then(|v| v.as_str()).unwrap_or("");
                println!("  {}✅ 已登录：{}{}{}", GRAY, BOLD, name, RESET);
                if !email.is_empty() {
                    println!("  {}   邮箱：{}{}", GRAY, email, RESET);
                }
            } else {
                println!("  {}✅ 飞书授权有效{}", GREEN, RESET);
            }
        }
        _ => {
            println!("  {}❌ 未登录或授权已过期，请运行 /auth 重新授权{}", YELLOW, RESET);
        }
    }
    println!();
    Ok(())
}

/// 退出飞书登录（TUI 内）
async fn cmd_lark_auth_logout() -> Result<()> {
    use tokio::process::Command;
    println!();
    println!("  {}🚪 正在退出飞书登录...{}", GRAY, RESET);

    let status = Command::new("lark-cli")
        .args(["auth", "logout"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            // 清除本地缓存
            if let Some(path) = dirs::home_dir().map(|h| h.join(".numina").join("cache").join("lark_user.json")) {
                let _ = std::fs::remove_file(path);
            }
            println!("  {}✅ 已退出飞书登录{}", GREEN, RESET);
        }
        _ => {
            println!("  {}⚠️  退出登录时遇到问题，请手动清理 lark-cli 凭证{}", YELLOW, RESET);
        }
    }
    println!();
    Ok(())
}

// ─────────────────────────────────────────────
// /login — 飞书 OAuth 用户登录（浏览器扫码/账号登录）
// ─────────────────────────────────────────────

/// 读取飞书 app_id：优先环境变量 LARK_APP_ID，其次 ~/.numina/cache/lark_app.json
fn load_lark_app_id() -> Option<String> {
    // 1. 环境变量
    if let Ok(id) = std::env::var("LARK_APP_ID") {
        if !id.is_empty() { return Some(id); }
    }
    // 2. ~/.numina/cache/lark_app.json: { "app_id": "...", "app_secret": "..." }
    let path = dirs::home_dir()?.join(".numina").join("cache").join("lark_app.json");
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("app_id").and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// 读取飞书 app_secret
fn load_lark_app_secret() -> Option<String> {
    if let Ok(s) = std::env::var("LARK_APP_SECRET") {
        if !s.is_empty() { return Some(s); }
    }
    let path = dirs::home_dir()?.join(".numina").join("cache").join("lark_app.json");
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("app_secret").and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// 飞书 OAuth 用户登录：
/// 1. 打开浏览器到飞书授权页（扫码或账号登录）
/// 2. 本地 HTTP server 接收 OAuth callback（code）
/// 3. 用 code 换取 user_access_token
/// 4. 获取用户信息并缓存
pub async fn cmd_lark_login(_args_str: &str) -> Result<()> {
    const CALLBACK_PORT: u16 = 9527;
    const REDIRECT_URI: &str = "http://localhost:9527/callback";

    // ── 读取 app_id（未配置则引导用户输入）──
    let (app_id, app_secret_configured) = match (load_lark_app_id(), load_lark_app_secret()) {
        (Some(id), Some(secret)) => (id, secret),
        _ => {
            // 引导用户配置
            println!();
            println!("  {}⚙️  首次使用飞书登录，需要配置飞书应用凭证{}", BOLD, RESET);
            println!("  {}{}{}", GRAY, "─".repeat(52), RESET);
            println!("  {}请前往飞书开放平台创建应用并获取凭证：{}", DIM, RESET);
            println!("  {}  https://open.feishu.cn/app{}", CYAN, RESET);
            println!("  {}创建「自建应用」后，在「凭证与基础信息」中获取{}", DIM, RESET);
            println!("  {}App ID（cli_xxx）和 App Secret{}", DIM, RESET);
            println!("  {}并在「安全设置」中添加重定向 URL：{}", DIM, RESET);
            println!("  {}  http://localhost:9527/callback{}", CYAN, RESET);
            println!("  {}{}{}", GRAY, "─".repeat(52), RESET);
            println!();

            // 输入 App ID
            print!("  {}App ID（cli_xxx）：{} ", GRAY, RESET);
            std::io::stdout().flush()?;
            let mut input_id = String::new();
            std::io::stdin().read_line(&mut input_id)?;
            let input_id = input_id.trim().to_string();
            if input_id.is_empty() {
                println!("  {}已取消{}", GRAY, RESET);
                println!();
                return Ok(());
            }

            // 输入 App Secret
            print!("  {}App Secret：{} ", GRAY, RESET);
            std::io::stdout().flush()?;
            let mut input_secret = String::new();
            std::io::stdin().read_line(&mut input_secret)?;
            let input_secret = input_secret.trim().to_string();
            if input_secret.is_empty() {
                println!("  {}已取消{}", GRAY, RESET);
                println!();
                return Ok(());
            }

            // 保存到 ~/.numina/cache/lark_app.json
            if let Some(dir) = dirs::home_dir().map(|h| h.join(".numina").join("cache")) {
                let _ = std::fs::create_dir_all(&dir);
                let json = serde_json::json!({
                    "app_id": input_id,
                    "app_secret": input_secret,
                });
                if let Ok(text) = serde_json::to_string_pretty(&json) {
                    let _ = std::fs::write(dir.join("lark_app.json"), text);
                    println!("  {}✅ 已保存到 ~/.numina/cache/lark_app.json{}", GREEN, RESET);
                }
            }
            println!();

            (input_id, input_secret)
        }
    };
    let _ = app_secret_configured; // 后续通过 load_lark_app_secret() 读取

    // ── 生成 state 防 CSRF ──
    let state = uuid::Uuid::new_v4().to_string();

    // ── 构造飞书 OAuth 授权 URL ──
    let auth_url = format!(
        "https://open.feishu.cn/open-apis/authen/v1/authorize?app_id={}&redirect_uri={}&state={}",
        app_id,
        urlencoding_simple(REDIRECT_URI),
        state,
    );

    println!();
    println!("  {}🔐 飞书用户登录{}", BOLD, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(52), RESET);
    println!("  {}⚠️  登录前请确认已完成以下配置：{}", YELLOW, RESET);
    println!("  {}1. 前往飞书开放平台应用后台：{}", DIM, RESET);
    println!("  {}   https://open.feishu.cn/app/{}{}", CYAN, app_id, RESET);
    println!("  {}2. 进入「安全设置」→「重定向 URL」{}", DIM, RESET);
    println!("  {}3. 添加以下 URL（完整复制，不要有空格）：{}", DIM, RESET);
    println!("  {}   http://localhost:9527/callback{}", GREEN, RESET);
    println!("  {}4. 保存后再点击浏览器中的授权按钮{}", DIM, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(52), RESET);
    println!("  {}正在打开浏览器...{}", GRAY, RESET);
    println!();

    // ── 打开浏览器 ──
    let open_result = tokio::process::Command::new("open")
        .arg(&auth_url)
        .status()
        .await;

    if open_result.is_err() {
        // macOS open 失败，尝试 xdg-open（Linux）
        let _ = tokio::process::Command::new("xdg-open")
            .arg(&auth_url)
            .status()
            .await;
    }

    println!("  {}如果浏览器未自动打开，请手动访问：{}", GRAY, RESET);
    println!("  {}{}{}", DIM, auth_url, RESET);
    println!();

    // ── 启动本地 HTTP server 等待 OAuth callback ──
    let code = wait_for_oauth_callback(CALLBACK_PORT, &state).await?;

    if code.is_empty() {
        println!("  {}⚠️  未收到授权码，登录已取消{}", YELLOW, RESET);
        println!();
        return Ok(());
    }

    println!("  {}✅ 收到授权码，正在获取用户信息...{}", GRAY, RESET);

    // ── 用 code 换取 user_access_token ──
    let app_secret = load_lark_app_secret().unwrap_or_default();
    match exchange_code_for_token(&app_id, &app_secret, &code, REDIRECT_URI).await {
        Ok(token_info) => {
            // 保存 token 和用户信息
            save_lark_token_cache(&token_info);
            if let Some(user) = token_info.user_info() {
                save_lark_user_cache(&user);
                // 后台下载头像（静默，失败不影响登录）
                if !user.avatar_url.is_empty() {
                    download_lark_avatar(&user.avatar_url).await;
                }
                println!("  {}✅ 登录成功！{}", GREEN, RESET);
                println!("  {}👤 {}{}{}", GRAY, BOLD, user.name, RESET);
                if !user.email.is_empty() {
                    println!("  {}   {}{}", DIM, user.email, RESET);
                }
            } else {
                println!("  {}✅ 登录成功！{}", GREEN, RESET);
            }
            println!("  {}现在可以使用飞书相关功能{}", DIM, RESET);
        }
        Err(e) => {
            println!("  {}⚠️  获取用户信息失败：{}{}", YELLOW, e, RESET);
            println!("  {}请检查 app_id / app_secret 是否正确{}", DIM, RESET);
        }
    }
    println!();

    Ok(())
}

/// 简单 URL 编码（只处理常见字符，避免引入额外依赖）
fn urlencoding_simple(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 本地 HTTP server，等待飞书 OAuth callback，返回 code
async fn wait_for_oauth_callback(port: u16, expected_state: &str) -> Result<String> {
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await
        .map_err(|e| anyhow::anyhow!("无法监听端口 {}：{}", port, e))?;

    println!("  {}⏳ 等待浏览器授权回调（端口 {}）...{}", GRAY, port, RESET);

    // 超时 120 秒
    let timeout = tokio::time::Duration::from_secs(120);
    let result = tokio::time::timeout(timeout, async {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await?;
            let request = String::from_utf8_lossy(&buf[..n]);

            // 解析 GET /callback?code=xxx&state=yyy
            if let Some(code) = parse_oauth_code(&request, expected_state) {
                // 返回成功页面
                let html = "<html><body style='font-family:sans-serif;text-align:center;padding:60px'>\
                    <h2>✅ 授权成功！</h2><p>请返回 Numina 继续操作。</p>\
                    <script>setTimeout(()=>window.close(),2000)</script></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(), html
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return Ok::<String, anyhow::Error>(code);
            } else {
                // 非 callback 请求，返回等待页
                let html = "<html><body style='font-family:sans-serif;text-align:center;padding:60px'>\
                    <h2>⏳ 等待授权...</h2></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(), html
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        }
    }).await;

    match result {
        Ok(Ok(code)) => Ok(code),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            println!("  {}⏰ 等待超时（120秒），登录已取消{}", YELLOW, RESET);
            Ok(String::new())
        }
    }
}

/// 从 HTTP 请求中解析 OAuth code
fn parse_oauth_code(request: &str, expected_state: &str) -> Option<String> {
    // 找到请求行：GET /callback?code=xxx&state=yyy HTTP/1.1
    let first_line = request.lines().next()?;
    if !first_line.contains("/callback") {
        return None;
    }

    // 提取 query string
    let query = first_line.split('?').nth(1)?.split(' ').next()?;

    let mut code = None;
    let mut state = None;

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let k = kv.next()?;
        let v = kv.next().unwrap_or("");
        match k {
            "code"  => code  = Some(url_decode(v)),
            "state" => state = Some(url_decode(v)),
            _ => {}
        }
    }

    // 验证 state
    if state.as_deref() != Some(expected_state) {
        return None;
    }

    code.filter(|c| !c.is_empty())
}

/// 简单 URL 解码
fn url_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// 飞书 token 信息
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LarkTokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub name: String,
    pub en_name: String,
    pub avatar_url: String,
    pub email: String,
    pub open_id: String,
    pub union_id: String,
}

impl LarkTokenInfo {
    pub fn user_info(&self) -> Option<LarkUserInfo> {
        if self.name.is_empty() && self.open_id.is_empty() {
            return None;
        }
        Some(LarkUserInfo {
            name: self.name.clone(),
            en_name: self.en_name.clone(),
            avatar_url: self.avatar_url.clone(),
            email: self.email.clone(),
            open_id: self.open_id.clone(),
        })
    }
}

/// 用 code 换取 user_access_token，再调用 user_info 接口获取用户信息
async fn exchange_code_for_token(
    app_id: &str,
    app_secret: &str,
    code: &str,
    _redirect_uri: &str,
) -> Result<LarkTokenInfo> {
    let client = reqwest::Client::new();

    // Step 1: 用 code 换取 user_access_token
    let app_token = get_app_access_token(app_id, app_secret).await?;
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
    });

    let resp = client
        .post("https://open.feishu.cn/open-apis/authen/v1/oidc/access_token")
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Authorization", format!("Bearer {}", app_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("请求飞书 API 失败：{}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("解析响应失败：{}", e))?;

    let code_val = json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code_val != 0 {
        let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Err(anyhow::anyhow!("飞书 API 错误 {}: {}", code_val, msg));
    }

    let data = json.get("data").ok_or_else(|| anyhow::anyhow!("响应缺少 data 字段"))?;
    let user_access_token = data.get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh_token = data.get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_in = data.get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(7200);

    // Step 2: 用 user_access_token 调用 authen/v1/user_info 获取用户信息
    let user_resp = client
        .get("https://open.feishu.cn/open-apis/authen/v1/user_info")
        .header("Authorization", format!("Bearer {}", user_access_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("获取用户信息失败：{}", e))?;

    let user_json: serde_json::Value = user_resp.json().await
        .map_err(|e| anyhow::anyhow!("解析用户信息失败：{}", e))?;

    let u = user_json.get("data").unwrap_or(&serde_json::Value::Null);

    let token_info = LarkTokenInfo {
        access_token:  user_access_token,
        refresh_token,
        token_type:    "Bearer".to_string(),
        expires_in,
        name:      u.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        en_name:   u.get("en_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        avatar_url: u.get("avatar_url").and_then(|v| v.as_str())
                     .or_else(|| u.pointer("/avatar/avatar_origin").and_then(|v| v.as_str()))
                     .unwrap_or("").to_string(),
        email:     u.get("email").and_then(|v| v.as_str())
                    .or_else(|| u.get("enterprise_email").and_then(|v| v.as_str()))
                    .unwrap_or("").to_string(),
        open_id:   u.get("open_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        union_id:  u.get("union_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    };

    Ok(token_info)
}

/// 获取 app_access_token（tenant_access_token 或 app_access_token）
async fn get_app_access_token(app_id: &str, app_secret: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "app_id": app_id,
        "app_secret": app_secret,
    });

    let resp = client
        .post("https://open.feishu.cn/open-apis/auth/v3/app_access_token/internal")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("获取 app_access_token 失败：{}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("解析 app_access_token 响应失败：{}", e))?;

    let code_val = json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code_val != 0 {
        let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown");
        return Err(anyhow::anyhow!("获取 app_access_token 失败 {}: {}", code_val, msg));
    }

    json.get("app_access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("响应中缺少 app_access_token"))
}

/// 保存 token 到 ~/.numina/cache/lark_token.json
pub fn save_lark_token_cache(info: &LarkTokenInfo) {
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".numina").join("cache")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(info) {
            let _ = std::fs::write(dir.join("lark_token.json"), json);
        }
    }
}

// ─────────────────────────────────────────────
// 飞书用户信息缓存（~/.numina/cache/lark_user.json）
// ─────────────────────────────────────────────

/// 飞书用户信息（缓存结构）
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LarkUserInfo {
    pub name: String,
    pub en_name: String,
    pub avatar_url: String,
    pub email: String,
    pub open_id: String,
}

/// 通过 lark-cli 获取当前登录用户信息
pub async fn fetch_lark_user_info() -> Option<LarkUserInfo> {
    use tokio::process::Command;
    let output = Command::new("lark-cli")
        .args(["contact", "user", "me", "--as", "user"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;

    // lark-cli 返回格式：{ "data": { "name": "...", "avatar": { "avatar_origin": "..." }, ... } }
    let data = json.get("data")?;

    let name = data.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let en_name = data.get("en_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let avatar_url = data.get("avatar")
        .and_then(|a| a.get("avatar_origin"))
        .and_then(|v| v.as_str())
        .or_else(|| data.get("avatar_url").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let email = data.get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let open_id = data.get("open_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if name.is_empty() {
        return None;
    }

    Some(LarkUserInfo { name, en_name, avatar_url, email, open_id })
}

/// 保存用户信息到 ~/.numina/cache/lark_user.json
pub fn save_lark_user_cache(info: &LarkUserInfo) {
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".numina").join("cache")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(info) {
            let _ = std::fs::write(dir.join("lark_user.json"), json);
        }
    }
}

/// 读取缓存的用户信息
pub fn load_lark_user_cache() -> Option<LarkUserInfo> {
    let path = dirs::home_dir()?.join(".numina").join("cache").join("lark_user.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 下载飞书头像到 ~/.numina/cache/lark_avatar.png
/// 下载成功后自动生成圆形版本 lark_avatar_circle.png
/// 如果 URL 为空或下载失败则静默忽略
pub async fn download_lark_avatar(url: &str) {
    if url.is_empty() { return; }
    let cache_dir = match dirs::home_dir().map(|h| h.join(".numina").join("cache")) {
        Some(d) => d,
        None => return,
    };
    let _ = std::fs::create_dir_all(&cache_dir);
    let dest = cache_dir.join("lark_avatar.png");

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Ok(resp) = client.get(url).send().await {
        if let Ok(bytes) = resp.bytes().await {
            if std::fs::write(&dest, &bytes).is_ok() {
                // 下载成功后，生成圆形头像
                let circle_dest = cache_dir.join("lark_avatar_circle.png");
                crate::cli::session::renderer::make_circle_avatar(&dest, &circle_dest);
            }
        }
    }
}

/// 读取本地缓存的头像路径（~/.numina/cache/lark_avatar.png）
pub fn lark_avatar_cache_path() -> Option<std::path::PathBuf> {
    let path = dirs::home_dir()?.join(".numina").join("cache").join("lark_avatar.png");
    if path.exists() { Some(path) } else { None }
}

/// 读取本地缓存的圆形头像路径（~/.numina/cache/lark_avatar_circle.png）
/// 若圆形版本不存在，尝试从原图实时生成；若原图也不存在则返回 None
pub fn lark_avatar_circle_path() -> Option<std::path::PathBuf> {
    let cache_dir = dirs::home_dir()?.join(".numina").join("cache");
    let circle = cache_dir.join("lark_avatar_circle.png");
    if circle.exists() {
        return Some(circle);
    }
    // 圆形版本不存在，尝试从原图实时生成
    let src = cache_dir.join("lark_avatar.png");
    if src.exists() {
        if crate::cli::session::renderer::make_circle_avatar(&src, &circle) {
            return Some(circle);
        }
    }
    None
}

// ─────────────────────────────────────────────
// 飞书登录过期检测
// ─────────────────────────────────────────────

/// 检查飞书登录是否已过期
///
/// 优先使用 token 文件中的 `expires_in` 字段精确判断：
///   - 文件修改时间 + expires_in 秒 < 当前时间 → 已过期
/// 兜底：如果无法解析 expires_in，则用文件修改时间超过 8 小时判断
///
/// 返回 Some(hours) 表示已过期多少小时，None 表示未登录或未过期
pub fn check_lark_login_expiry() -> Option<u64> {
    let token_path = dirs::home_dir()?.join(".numina").join("cache").join("lark_token.json");
    if !token_path.exists() {
        return None; // 未登录，不提示
    }
    let meta = std::fs::metadata(&token_path).ok()?;
    let modified = meta.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    let elapsed_secs = elapsed.as_secs();

    // 尝试读取 token 文件中的 expires_in 字段
    if let Ok(content) = std::fs::read_to_string(&token_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(expires_in) = json.get("expires_in").and_then(|v| v.as_i64()) {
                if expires_in > 0 {
                    let expires_in_u = expires_in as u64;
                    if elapsed_secs >= expires_in_u {
                        // 已过期：计算超过了多少小时
                        let overdue_secs = elapsed_secs.saturating_sub(expires_in_u);
                        let overdue_hours = overdue_secs / 3600;
                        // 至少显示 1 小时（避免显示 0 小时）
                        return Some(overdue_hours.max(1));
                    } else {
                        return None; // 未过期
                    }
                }
            }
        }
    }

    // 兜底：文件修改时间超过 8 小时
    let hours = elapsed_secs / 3600;
    if hours >= 8 {
        Some(hours)
    } else {
        None
    }
}

// ─────────────────────────────────────────────
// /login — 统一用户登录入口（平台选择 + 确认）
// ─────────────────────────────────────────────

/// 平台定义
struct LoginPlatform {
    name: &'static str,
    desc: &'static str,
    available: bool,
}

/// 渲染平台选择列表，返回行数
fn render_login_platforms(platforms: &[LoginPlatform], sel: usize) -> usize {
    let sep = "─".repeat(52);
    println!("  {}{}登录授权{}  {}↑↓ 选择平台  Enter 确认  Esc 退出{}",
        BOLD, BRIGHT_WHITE, RESET, GRAY, RESET);
    println!("  {}{}{}", GRAY, sep, RESET);
    let mut lines = 2usize;
    for (i, p) in platforms.iter().enumerate() {
        if i == sel {
            if p.available {
                println!("  {}{} ▶ {}{:<20}{}  {}{}{}", BOLD, CYAN, BRIGHT_WHITE, p.name, RESET, DIM, p.desc, RESET);
            } else {
                println!("  {}{} ▶ {}{:<20}{}  {}{}（即将支持）{}", BOLD, CYAN, GRAY, p.name, RESET, DIM, GRAY, RESET);
            }
        } else if p.available {
            println!("     {}{:<20}{}  {}{}{}", GRAY, p.name, RESET, DIM, p.desc, RESET);
        } else {
            println!("     {}{:<20}{}  {}（即将支持）{}", "\x1b[38;5;240m", p.name, RESET, "\x1b[38;5;240m", RESET);
        }
        lines += 1;
    }
    println!("  {}{}{}", GRAY, sep, RESET);
    lines += 1;
    lines
}

/// /login 统一登录入口：平台选择 → 直接登录（无需二次确认）
pub async fn cmd_login_browser() -> Result<()> {
    use crossterm::terminal::{enable_raw_mode, disable_raw_mode};
    use crossterm::event::KeyCode;

    let platforms = vec![
        LoginPlatform { name: "Lark / 飞书",  desc: "扫码或账号登录飞书", available: true  },
        LoginPlatform { name: "微信",          desc: "微信扫码登录",       available: false },
        LoginPlatform { name: "企业微信",      desc: "企业微信账号登录",   available: false },
    ];

    // 预留渲染空间
    let tui_reserve = 10usize;
    for _ in 0..tui_reserve { println!(); }
    print!("\x1b[{}A\x1b[0J", tui_reserve);
    std::io::stdout().flush()?;

    let mut sel = 0usize;
    let mut list_lines = render_login_platforms(&platforms, sel);
    std::io::stdout().flush()?;

    enable_raw_mode()?;

    loop {
        match wait_key() {
            Some(KeyCode::Esc) | Some(KeyCode::Char('q')) => {
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", list_lines);
                std::io::stdout().flush()?;
                println!("  {}已取消{}", GRAY, RESET);
                println!();
                return Ok(());
            }
            Some(KeyCode::Up) => {
                sel = if sel > 0 { sel - 1 } else { platforms.len() - 1 };
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", list_lines);
                list_lines = render_login_platforms(&platforms, sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Down) => {
                sel = (sel + 1) % platforms.len();
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", list_lines);
                list_lines = render_login_platforms(&platforms, sel);
                std::io::stdout().flush()?;
                enable_raw_mode()?;
            }
            Some(KeyCode::Enter) => {
                let platform = &platforms[sel];
                disable_raw_mode()?;
                print!("\x1b[{}A\x1b[0J", list_lines);
                std::io::stdout().flush()?;

                if !platform.available {
                    println!("  {}「{}」暂未支持，敬请期待！{}", YELLOW, platform.name, RESET);
                    println!();
                    return Ok(());
                }

                // 直接执行登录，无需二次确认
                match sel {
                    0 => cmd_lark_login("").await?,
                    _ => {
                        println!("  {}「{}」暂未支持，敬请期待！{}", YELLOW, platform.name, RESET);
                        println!();
                    }
                }
                return Ok(());
            }
            None => break,
            _ => {}
        }
    }

    disable_raw_mode()?;
    Ok(())
}
