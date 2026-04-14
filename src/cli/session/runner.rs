use anyhow::Result;
use std::io::Write;

use crate::config::ModelsConfig;
use crate::core::chat::ChatEngine;

use super::commands::{cmd_mcp_browser, cmd_model_picker, cmd_sessions};
use super::file_ref::expand_at_references;
use super::permission::{read_permission_choice_interactive, read_permission_choice_mcp};
use super::readline::{interactive_readline, ReadLine};
use super::renderer::{
    estimate_context_size, print_context_bar, print_help, print_welcome,
    BOLD, BRIGHT_WHITE, CODE_BG, CODE_FG, CYAN, DIM, GRAY, GREEN, RESET, YELLOW,
};

use crate::cli::chat::{ChatArgs, ChatCommand};
use super::completer::register_skill_completions;

// ─────────────────────────────────────────────
// 工具调用渲染辅助
// ─────────────────────────────────────────────

/// 渲染工具调用行（带 JSON 底色展示参数）
/// tool_info 格式："{tool_name}|{params}"
/// params 可能是：
///   - 普通字符串（shell 命令、文件路径等）
///   - 格式化 JSON（其他工具）
///   - "url\x01json_body"（http_post 专用格式）
fn render_tool_call(tool_info: &str) {
    let parts: Vec<&str> = tool_info.splitn(2, '|').collect();
    let tool_name = parts.first().copied().unwrap_or("?");
    let params = parts.get(1).copied().unwrap_or("");

    if params.is_empty() {
        println!("  {}●{} {}{}{}", CYAN, RESET, BOLD, tool_name, RESET);
        return;
    }

    // http_post 特殊格式：url\x01json_body
    if let Some(sep_pos) = params.find('\x01') {
        let url = &params[..sep_pos];
        let json_body = &params[sep_pos + 1..];
        println!("  {}●{} {}{}{}  {}POST {}{}", CYAN, RESET, BOLD, tool_name, RESET, DIM, url, RESET);
        // 渲染 JSON 带底色
        render_json_block(json_body);
        return;
    }

    // 判断是否是 JSON（以 { 或 [ 开头）
    let trimmed = params.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        println!("  {}●{} {}{}{}", CYAN, RESET, BOLD, tool_name, RESET);
        render_json_block(trimmed);
    } else {
        // 普通字符串（命令、路径等）：单行显示
        let preview: String = params.chars().take(80).collect();
        let ellipsis = if params.len() > 80 { "…" } else { "" };
        println!("  {}●{} {}{}{}  {}{}{}{}",
            CYAN, RESET,
            BOLD, tool_name, RESET,
            DIM, preview, ellipsis, RESET);
    }
}

/// 渲染 JSON 内容块（带深色背景，缩进对齐）
/// 对 body 字段的值（JSON 对象/数组）压缩成单行，减少行数
fn render_json_block(json: &str) {
    // 先尝试解析 JSON，对 body 字段做单行压缩
    let display_json = if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(obj) = val.as_object_mut() {
            // 把 body 字段（如果是对象或数组）压缩成单行字符串
            if let Some(body_val) = obj.get("body").cloned() {
                if body_val.is_object() || body_val.is_array() {
                    let compact = serde_json::to_string(&body_val).unwrap_or_default();
                    obj.insert("body".to_string(), serde_json::Value::String(compact));
                }
            }
        }
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| json.to_string())
    } else {
        json.to_string()
    };

    // 最多显示 20 行，超出折叠
    let lines: Vec<&str> = display_json.lines().collect();
    let max_lines = 20usize;
    let show_lines = lines.len().min(max_lines);
    for line in &lines[..show_lines] {
        println!("  {}{}  {}{}", CODE_BG, CODE_FG, line, RESET);
    }
    if lines.len() > max_lines {
        println!("  {}  … {} more lines{}", DIM, lines.len() - max_lines, RESET);
    }
}

/// 工具结果折叠阈值（超过此行数或字符数则折叠）
const RESULT_FOLD_LINES: usize = 5;
const RESULT_FOLD_CHARS: usize = 300;

/// 展开一条折叠的工具结果（分页显示，每屏 40 行）
fn expand_tool_result(content: &str) {
    use crossterm::terminal::size as term_size;
    let term_h = term_size().map(|(_, h)| h as usize).unwrap_or(40);
    let page_size = term_h.saturating_sub(4).max(10);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    if total <= page_size {
        // 内容不多，直接全部显示
        println!("  {}┌─ Result ({} lines) ─────────────────────────{}", DIM, total, RESET);
        for line in &lines {
            println!("  {}│{} {}", DIM, RESET, line);
        }
        println!("  {}└────────────────────────────────────────────{}", DIM, RESET);
    } else {
        // 分页：先显示前 page_size 行，提示还有多少行
        println!("  {}┌─ Result ({} lines, showing first {}) ───────{}", DIM, total, page_size, RESET);
        for line in &lines[..page_size] {
            println!("  {}│{} {}", DIM, RESET, line);
        }
        println!("  {}│  … {} more lines (use shell/read_file to view full content){}", DIM, total - page_size, RESET);
        println!("  {}└────────────────────────────────────────────{}", DIM, RESET);
    }
    std::io::stdout().flush().ok();
}

/// 非阻塞检测 ctrl+o 并展开最后一条折叠结果
/// 返回是否消费了一个键盘事件
fn poll_expand_key(expandable: &mut Vec<String>) -> bool {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;
    if expandable.is_empty() {
        return false;
    }
    if poll(Duration::ZERO).unwrap_or(false) {
        if let Ok(Event::Key(KeyEvent {
            code: KeyCode::Char('o'),
            modifiers: KeyModifiers::CONTROL,
            ..
        })) = read() {
            if let Some(content) = expandable.pop() {
                println!();
                expand_tool_result(&content);
                println!();
                std::io::stdout().flush().ok();
                return true;
            }
        }
    }
    false
}

// ─────────────────────────────────────────────
// MCP 权限辅助：从 cmd 中解析 MCP server/tool/args
// ─────────────────────────────────────────────

/// 对于 http_post/http_get 工具，cmd 格式为 "url\x01json_body"
/// 从 mcp.json 中按 URL 前缀匹配 server 名称，
/// 从 json_body 中提取 JSON-RPC method 和 params
/// 返回 (server_name, mcp_tool_name, args_json_pretty)
fn resolve_mcp_info(tool_name: &str, cmd: &str) -> Option<(String, String, String)> {
    // 只处理 http_post / http_get
    if tool_name != "http_post" && tool_name != "http_get" {
        return None;
    }

    // 解析 url 和 json_body
    let (url, json_body) = if let Some(sep) = cmd.find('\x01') {
        (&cmd[..sep], &cmd[sep + 1..])
    } else {
        // http_get 没有 body，cmd 就是 url
        (cmd, "")
    };

    // 从 mcp.json 中查找匹配的 server
    let server_name = crate::config::mcp::McpConfig::load()
        .ok()
        .and_then(|cfg| {
            cfg.servers.into_iter().find(|s| {
                s.enabled && url.starts_with(&s.command_or_url)
            })
        })
        .map(|s| s.name)
        .unwrap_or_else(|| {
            // 无法匹配时，从 URL 中提取 host 作为名称
            url.trim_start_matches("http://")
               .trim_start_matches("https://")
               .split('/')
               .next()
               .unwrap_or("unknown")
               .to_string()
        });

    // 从 JSON-RPC body 中提取 method 和 params
    let (mcp_tool, args_pretty) = if json_body.is_empty() {
        (url.split('/').last().unwrap_or("request").to_string(), String::new())
    } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_body) {
        let method = val.get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("call")
            .to_string();
        // 优先取 params，否则取整个 body
        let args = val.get("params")
            .cloned()
            .unwrap_or_else(|| val.clone());
        // 单行紧凑格式显示（省行数，能显示完整参数），实际请求内容不受影响
        let pretty = serde_json::to_string(&args).unwrap_or_default();
        (method, pretty)
    } else {
        // 非 JSON body，直接显示
        (url.split('/').last().unwrap_or("request").to_string(), json_body.to_string())
    };

    Some((server_name, mcp_tool, args_pretty))
}

// ─────────────────────────────────────────────
// Session 持久化记忆（last_session）
// ─────────────────────────────────────────────

/// 读取上次退出时的 session ID（~/.numina/last_session）
pub fn load_last_session_id() -> Option<String> {
    let path = dirs::home_dir()?.join(".numina").join("last_session");
    let sid = std::fs::read_to_string(path).ok()?.trim().to_string();
    if sid.is_empty() {
        None
    } else {
        ChatEngine::get_session(&sid).ok().map(|_| sid)
    }
}

/// 保存当前 session ID 到 ~/.numina/last_session
pub fn save_last_session_id(sid: &str) {
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".numina")) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("last_session"), sid);
    }
}

/// 清除 last_session（/new 时调用）
pub fn clear_last_session_id() {
    if let Some(path) = dirs::home_dir().map(|h| h.join(".numina").join("last_session")) {
        let _ = std::fs::write(path, "");
    }
}

// ─────────────────────────────────────────────
// 单次消息
// ─────────────────────────────────────────────

pub async fn run_single_message(engine: &ChatEngine, msg: &str, args: &ChatArgs) -> Result<()> {
    println!("{}{}You{} {}", BOLD, GREEN, RESET, msg);
    println!();

    let model_override = args.model.as_deref();
    let session_id = args.session.as_deref();

    match engine.chat_react(msg, model_override, session_id).await {
        Ok((mut rx, _perm_tx, sid, sent_tokens, ctx_window)) => {
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
            let _spinner = tokio::spawn(async move {
                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let mut i = 0usize;
                loop {
                    if stop_rx.try_recv().is_ok() {
                        print!("\r\x1b[2K");
                        std::io::stdout().flush().ok();
                        break;
                    }
                    print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                    std::io::stdout().flush().ok();
                    i += 1;
                    tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                }
            });

            let mut full_response = String::new();
            let mut in_code_block = false;
            let mut code_block_buf = String::new();
            let mut line_buf = String::new();
            let mut stop_tx_opt = Some(stop_tx);
            let mut thinking_task: Option<tokio::task::JoinHandle<()>> = None;
            // 可展开的工具结果队列（ctrl+o 展开最后一条）
            let mut expandable_results: Vec<String> = Vec::new();

            macro_rules! stop_thinking {
                () => {
                    if let Some(h) = thinking_task.take() {
                        h.abort();
                        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                        print!("\r\x1b[2K");
                        std::io::stdout().flush().ok();
                    }
                };
            }

            while let Some(event) = rx.recv().await {
                if let Some(tx) = stop_tx_opt.take() {
                    let _ = tx.send(());
                    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                    print!("\r\x1b[2K");
                    std::io::stdout().flush().ok();
                }

                // 非阻塞检测 ctrl+o 展开折叠结果
                poll_expand_key(&mut expandable_results);

                if event == "\x00D" {
                    stop_thinking!();
                    break;
                } else if event == "\x00W" {
                    stop_thinking!();
                    let h = tokio::spawn(async {
                        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                        let mut i = 0usize;
                        loop {
                            print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                            std::io::stdout().flush().ok();
                            i += 1;
                            tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                        }
                    });
                    thinking_task = Some(h);
                } else if let Some(thinking_text) = event.strip_prefix("\x00H") {
                    stop_thinking!();
                    let preview: String = thinking_text.chars().take(60).collect();
                    let ellipsis = if thinking_text.len() > 60 { "…" } else { "" };
                    println!("  {}∴ {}{}{} {}(thinking){}", DIM, preview, ellipsis, RESET, DIM, RESET);
                    std::io::stdout().flush()?;
                } else if let Some(summary) = event.strip_prefix("\x00S") {
                    stop_thinking!();
                    println!("  {}⏺ {}{}  {}(ctrl+o to expand){}", DIM, summary, RESET, DIM, RESET);
                    std::io::stdout().flush()?;
                } else if let Some(perm_info) = event.strip_prefix("\x00K") {
                    stop_thinking!();
                    let parts: Vec<&str> = perm_info.splitn(4, '|').collect();
                    let perm_id = parts.first().copied().unwrap_or("");
                    let tool_name = parts.get(1).copied().unwrap_or("?");
                    let cmd = parts.get(2).copied().unwrap_or("");

                    println!();
                    let tool_name_owned = tool_name.to_string();
                    let cmd_owned = cmd.to_string();
                    let decision = tokio::task::spawn_blocking(move || {
                        if let Some((server_name, mcp_tool, args_json)) =
                            resolve_mcp_info(&tool_name_owned, &cmd_owned)
                        {
                            read_permission_choice_mcp(&server_name, &mcp_tool, &args_json)
                        } else {
                            read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                        }
                    })
                        .await
                        .unwrap_or(3);
                    let reply = match decision {
                        1 => format!("{}|allow", perm_id),
                        2 => format!("{}|allow_session", perm_id),
                        0 => format!("{}|deny_abort", perm_id), // Esc = 强制中止整个 agent loop
                        _ => format!("{}|deny", perm_id),
                    };
                    println!();
                    let _ = _perm_tx.send(reply).await;
                } else if let Some(tool_info) = event.strip_prefix("\x00T") {
                    stop_thinking!();
                    render_tool_call(tool_info);
                    std::io::stdout().flush()?;
                } else if let Some(result) = event.strip_prefix("\x00R") {
                    let line_count = result.lines().count();
                    let char_count = result.len();
                    let should_fold = line_count > RESULT_FOLD_LINES || char_count > RESULT_FOLD_CHARS;
                    if should_fold {
                        // 折叠显示：只显示摘要，提示 ctrl+o 展开
                        println!("  {}  └─ {} line{}, {} chars  {}(ctrl+o to expand){}",
                            DIM,
                            line_count, if line_count != 1 { "s" } else { "" },
                            char_count,
                            RESET, RESET);
                        expandable_results.push(result.to_string());
                    } else {
                        // 内容较短，直接显示
                        println!("  {}  └─ {} line{}, {} chars{}",
                            DIM,
                            line_count, if line_count != 1 { "s" } else { "" },
                            char_count,
                            RESET);
                        for line in result.lines().take(RESULT_FOLD_LINES) {
                            println!("  {}     {}{}", DIM, line, RESET);
                        }
                    }
                    std::io::stdout().flush()?;
                } else if let Some(text) = event.strip_prefix("\x00C") {
                    if full_response.is_empty() {
                        println!();
                        print!("{}{}Numina{} ", BOLD, CYAN, RESET);
                        std::io::stdout().flush()?;
                    }
                    for ch in text.chars() {
                        line_buf.push(ch);
                        if ch == '\n' {
                            let trimmed = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                            if trimmed.starts_with("```") {
                                if in_code_block {
                                    // 代码块结束：检查是否需要折叠
                                    let code_lines: Vec<&str> = code_block_buf.lines().collect();
                                    const CODE_FOLD_LINES: usize = 20;
                                    if code_lines.len() > CODE_FOLD_LINES {
                                        // 只显示前 CODE_FOLD_LINES 行，其余折叠
                                        for line in &code_lines[..CODE_FOLD_LINES] {
                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                        }
                                        println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                                    } else {
                                        for line in &code_lines {
                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                        }
                                    }
                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                    code_block_buf.clear();
                                    in_code_block = false;
                                } else {
                                    in_code_block = true;
                                    code_block_buf.clear();
                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                }
                            } else if in_code_block {
                                // 代码块内容先缓存，等结束时统一折叠判断
                                code_block_buf.push_str(trimmed);
                                code_block_buf.push('\n');
                            } else {
                                print!("{}\n", trimmed);
                            }
                            std::io::stdout().flush()?;
                            line_buf.clear();
                        }
                    }
                    if !line_buf.is_empty() {
                        if in_code_block {
                            // 代码块未结束，缓存内容（不立即打印）
                            code_block_buf.push_str(&line_buf);
                        } else {
                            print!("{}", line_buf);
                        }
                        std::io::stdout().flush()?;
                        line_buf.clear();
                    }
                    full_response.push_str(text);
                }
            }

            if in_code_block {
                // 代码块未正常结束（模型截断），把缓存内容全部打印出来
                if !code_block_buf.is_empty() {
                    let code_lines: Vec<&str> = code_block_buf.lines().collect();
                    const CODE_FOLD_LINES: usize = 20;
                    if code_lines.len() > CODE_FOLD_LINES {
                        for line in &code_lines[..CODE_FOLD_LINES] {
                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                        }
                        println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                    } else {
                        for line in &code_lines {
                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                        }
                    }
                }
                print!("{}", RESET);
            }
            println!();
            println!();

            let current_tokens = ChatEngine::get_session(&sid)
                .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
                .unwrap_or_else(|_| sent_tokens + full_response.len() / 4);
            print_context_bar(current_tokens, ctx_window);
        }
        Err(e) => {
            eprintln!("\n{}❌ Error: {}{}\n", YELLOW, e, RESET);
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// 交互式循环
// ─────────────────────────────────────────────

pub async fn run_interactive_with_session(
    engine: &ChatEngine,
    args: &ChatArgs,
    initial_session: Option<String>,
) -> Result<()> {
    use std::io::Write;

    let model_override = args.model.as_deref();
    let mut current_session: Option<String> = initial_session.clone();
    let mut turn_count = 0usize;

    let mut accumulated_tokens: usize = if let Some(ref sid) = initial_session {
        ChatEngine::get_session(sid)
            .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
            .unwrap_or(0)
    } else {
        0
    };

    let history_path = dirs::home_dir()
        .map(|h| h.join(".numina").join("chat_history"))
        .unwrap_or_else(|| std::path::PathBuf::from(".numina_history"));
    let mut chat_history: Vec<String> = if let Ok(content) = std::fs::read_to_string(&history_path) {
        content.lines().filter(|s| !s.is_empty()).map(str::to_string).collect()
    } else {
        Vec::new()
    };

    loop {
        let prompt = "❯ ";

        let input = match interactive_readline(prompt, &mut chat_history) {
            Ok(ReadLine::Line(line)) => {
                let trimmed = line.trim().to_string();
                // readline 已在内部将序列化历史格式推入 chat_history，
                // 此处只需把最后一条（序列化格式）追加写到历史文件
                if !trimmed.is_empty() {
                    if let Some(serialized) = chat_history.last() {
                        let serialized = serialized.clone();
                        if let Some(parent) = history_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::OpenOptions::new()
                            .append(true)
                            .create(true)
                            .open(&history_path)
                            .and_then(|mut f| writeln!(f, "{}", serialized));
                    }
                }
                trimmed
            }
            Ok(ReadLine::Interrupted) => {
                println!();
                continue;
            }
            Ok(ReadLine::Eof) => {
                println!();
                println!("{}Goodbye! 👋{}", DIM, RESET);
                break;
            }
            Err(e) => {
                eprintln!("{}❌ Input error: {}{}", YELLOW, e, RESET);
                break;
            }
        };

        if input.is_empty() {
            continue;
        }

        let input = input.as_str();

        match input {
            "/quit" | "/exit" | "/q" => {
                println!();
                println!("{}Goodbye! 👋{}", DIM, RESET);
                break;
            }
            "/help" | "/h" => {
                print_help();
                continue;
            }
            "/session" => {
                match &current_session {
                    Some(sid) => println!("{}Current session: {}{}", GRAY, sid, RESET),
                    None => println!("{}No active session yet.{}", GRAY, RESET),
                }
                continue;
            }
            "/sessions" => {
                cmd_sessions()?;
                continue;
            }
            "/new" => {
                current_session = None;
                turn_count = 0;
                accumulated_tokens = 0;
                clear_last_session_id();
                println!("{}✅ Started a new session.{}", GREEN, RESET);
                println!();
                continue;
            }
            "/skills" => {
                let skill_list = engine.skill_names();
                if skill_list.is_empty() {
                    println!("{}No skills loaded. Add a claude.md or SKILL.md to your workspace.{}", GRAY, RESET);
                } else {
                    println!("{}Loaded {} skill(s):{}", GRAY, skill_list.len(), RESET);
                    for (name, desc) in &skill_list {
                        println!("  {}  /{:<20}{}  {}{}", BOLD, name, RESET, GRAY, desc);
                        print!("{}", RESET);
                    }
                }
                println!();
                continue;
            }
            "/model" => {
                if let Some(selected) = cmd_model_picker()? {
                    println!("{}✅ Switched to model: {}{}{}", GREEN, BOLD, selected, RESET);
                    println!();
                }
                continue;
            }
            "/mcp" => {
                cmd_mcp_browser().await?;
                continue;
            }
            "/memory" => {
                // 列出所有记忆
                let all = crate::memory::MemoryStore::load_all();
                if all.is_empty() {
                    println!("  {}No memories yet. Use /memory add <content> to add one.{}", GRAY, RESET);
                } else {
                    println!();
                    println!("  {}{}Memories ({} total){}", BOLD, BRIGHT_WHITE, all.len(), RESET);
                    println!("  {}{}{}", GRAY, "─".repeat(40), RESET);
                    for entry in &all {
                        let scope_color = match entry.scope {
                            crate::memory::MemoryScope::Global => CYAN,
                            crate::memory::MemoryScope::Project => GREEN,
                        };
                        let source_tag = match entry.source {
                            crate::memory::MemorySource::User => "user",
                            crate::memory::MemorySource::Auto => "auto",
                        };
                        let scope_tag = match entry.scope {
                            crate::memory::MemoryScope::Global => "global",
                            crate::memory::MemoryScope::Project => "project",
                        };
                        println!("  {}[{}]{} {}{}{}  {}{}[{}/{}]{}",
                            scope_color, entry.id, RESET,
                            BOLD, entry.content, RESET,
                            DIM, GRAY, scope_tag, source_tag, RESET);
                    }
                }
                println!();
                continue;
            }
            _ if input.starts_with("/memory ") => {
                let rest = input.trim_start_matches("/memory ").trim();
                if rest.starts_with("add ") {
                    let content_part = rest.trim_start_matches("add ").trim();
                    // 支持 -p 标志表示项目级记忆
                    let (scope, content) = if content_part.starts_with("-p ") {
                        (crate::memory::MemoryScope::Project, content_part.trim_start_matches("-p ").trim())
                    } else {
                        (crate::memory::MemoryScope::Global, content_part)
                    };
                    if content.is_empty() {
                        println!("  {}Usage: /memory add [-p] <content>{}", YELLOW, RESET);
                    } else {
                        let entry = crate::memory::MemoryEntry::new(
                            content,
                            crate::memory::MemorySource::User,
                            scope,
                        );
                        let id = entry.id.clone();
                        match crate::memory::MemoryStore::add(entry) {
                            Ok(_) => println!("  {}✅ Memory saved [{}]: {}{}", GREEN, id, content, RESET),
                            Err(e) => println!("  {}❌ Failed to save memory: {}{}", YELLOW, e, RESET),
                        }
                    }
                } else if rest.starts_with("forget ") {
                    let id = rest.trim_start_matches("forget ").trim();
                    if id.is_empty() {
                        println!("  {}Usage: /memory forget <id>{}", YELLOW, RESET);
                    } else {
                        match crate::memory::MemoryStore::remove(id) {
                            Ok(true) => println!("  {}✅ Memory [{}] deleted.{}", GREEN, id, RESET),
                            Ok(false) => println!("  {}No memory found with id: {}{}", YELLOW, id, RESET),
                            Err(e) => println!("  {}❌ Failed to delete memory: {}{}", YELLOW, e, RESET),
                        }
                    }
                } else if rest.starts_with("search ") {
                    let query = rest.trim_start_matches("search ").trim();
                    let results = crate::memory::search_memories(query, 10);
                    if results.is_empty() {
                        println!("  {}No memories found for: \"{}\"{}", GRAY, query, RESET);
                    } else {
                        println!();
                        println!("  {}{}Search results for \"{}\" ({} found){}", BOLD, BRIGHT_WHITE, query, results.len(), RESET);
                        println!("  {}{}{}", GRAY, "─".repeat(40), RESET);
                        for entry in &results {
                            let scope_color = match entry.scope {
                                crate::memory::MemoryScope::Global => CYAN,
                                crate::memory::MemoryScope::Project => GREEN,
                            };
                            println!("  {}[{}]{} {}{}{}", scope_color, entry.id, RESET, BOLD, entry.content, RESET);
                        }
                    }
                } else {
                    println!("  {}Unknown memory command. Try: /memory add <content> | /memory forget <id> | /memory search <query>{}", YELLOW, RESET);
                }
                println!();
                continue;
            }
            "/clear" => {
                // 清屏 + 重置 session（上下文归零，下次对话重新开始）
                current_session = None;
                turn_count = 0;
                accumulated_tokens = 0;
                clear_last_session_id();
                print!("\x1b[2J\x1b[H");
                std::io::stdout().flush()?;
                let model = engine.default_model();
                // current_session 已为 None，显示 0k/context_window
                print_welcome(&model, engine.skill_count(), None, true);
                // 显示上下文归零状态：0k / Xk
                let model_provider = crate::config::ModelsConfig::load()
                    .ok()
                    .and_then(|mc| mc.models.iter().find(|m| m.name == model).map(|m| m.provider.clone()))
                    .unwrap_or_else(|| "openai".to_string());
                let ctx_size_str = estimate_context_size(&model_provider, &model);
                let ctx_window: usize = ctx_size_str.parse::<usize>().unwrap_or(128) * 1000;
                print_context_bar(0, ctx_window);
                continue;
            }
            _ if input.starts_with('/') => {
                // 先检查是否是 skill 斜杠命令
                if let Some(expanded) = engine.expand_skill_command(input) {
                    // 是 skill 命令：用展开后的 prompt 发送给模型
                    let skill_name = &input[1..input.find(' ').unwrap_or(input.len())];
                    println!("  {}▶ Skill: /{}{}", DIM, skill_name, RESET);
                    println!();
                    // 直接跳到对话处理，使用展开后的 prompt
                    match engine
                        .chat_react(&expanded, model_override, current_session.as_deref())
                        .await
                    {
                        Ok((mut rx, perm_tx, sid, sent_tokens, ctx_window)) => {
                            println!();
                            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
                            let _spinner_handle = tokio::spawn(async move {
                                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                                let mut i = 0usize;
                                loop {
                                    if stop_rx.try_recv().is_ok() {
                                        print!("\r\x1b[2K");
                                        std::io::stdout().flush().ok();
                                        break;
                                    }
                                    print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                                    std::io::stdout().flush().ok();
                                    i += 1;
                                    tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                                }
                            });
                            let mut full_response = String::new();
                            let mut in_code_block = false;
                            let mut code_block_buf = String::new();
                            let mut line_buf = String::new();
                            let mut stop_tx_opt = Some(stop_tx);
                            let mut thinking_task: Option<tokio::task::JoinHandle<()>> = None;
                            macro_rules! stop_thinking_skill {
                                () => {
                                    if let Some(h) = thinking_task.take() {
                                        h.abort();
                                        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                                        print!("\r\x1b[2K");
                                        std::io::stdout().flush().ok();
                                    }
                                };
                            }
                            while let Some(event) = rx.recv().await {
                                if let Some(tx) = stop_tx_opt.take() {
                                    let _ = tx.send(());
                                    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                                    print!("\r\x1b[2K");
                                    std::io::stdout().flush().ok();
                                }
                                if event == "\x00D" { stop_thinking_skill!(); break; }
                                else if event == "\x00W" {
                                    stop_thinking_skill!();
                                    let h = tokio::spawn(async {
                                        let frames = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
                                        let mut i = 0usize;
                                        loop {
                                            print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                                            std::io::stdout().flush().ok();
                                            i += 1;
                                            tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                                        }
                                    });
                                    thinking_task = Some(h);
                                } else if let Some(summary) = event.strip_prefix("\x00S") {
                                    stop_thinking_skill!();
                                    println!("  {}⏺ {}{}", DIM, summary, RESET);
                                } else if let Some(perm_info) = event.strip_prefix("\x00K") {
                                    stop_thinking_skill!();
                                    let parts: Vec<&str> = perm_info.splitn(4, '|').collect();
                                    let perm_id = parts.first().copied().unwrap_or("");
                                    let tool_name = parts.get(1).copied().unwrap_or("?");
                                    let cmd = parts.get(2).copied().unwrap_or("");
                                    println!();
                                    let tool_name_owned = tool_name.to_string();
                                    let cmd_owned = cmd.to_string();
                                    let decision = tokio::task::spawn_blocking(move || {
                                        if let Some((server_name, mcp_tool, args_json)) =
                                            resolve_mcp_info(&tool_name_owned, &cmd_owned)
                                        {
                                            read_permission_choice_mcp(&server_name, &mcp_tool, &args_json)
                                        } else {
                                            read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                                        }
                                    }).await.unwrap_or(3);
                                    let reply = match decision {
                                        1 => format!("{}|allow", perm_id),
                                        2 => format!("{}|allow_session", perm_id),
                                        0 => format!("{}|deny_abort", perm_id), // Esc = 强制中止整个 agent loop
                                        _ => format!("{}|deny", perm_id),
                                    };
                                    println!();
                                    let _ = perm_tx.send(reply).await;
                                } else if let Some(tool_info) = event.strip_prefix("\x00T") {
                                    stop_thinking_skill!();
                                    render_tool_call(tool_info);
                                    std::io::stdout().flush()?;
                                } else if let Some(result) = event.strip_prefix("\x00R") {
                                    let lc = result.lines().count();
                                    println!("  {}  └─ {} line{}, {} chars{}", DIM, lc, if lc != 1 { "s" } else { "" }, result.len(), RESET);
                                } else if let Some(text) = event.strip_prefix("\x00C") {
                                    if full_response.is_empty() {
                                        println!();
                                        print!("{}{}Numina{} ", BOLD, CYAN, RESET);
                                        std::io::stdout().flush()?;
                                    }
                                    for ch in text.chars() {
                                        line_buf.push(ch);
                                        if ch == '\n' {
                                            let trimmed = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                                            if trimmed.starts_with("```") {
                                                if in_code_block {
                                                    // 代码块结束：检查是否需要折叠
                                                    let code_lines: Vec<&str> = code_block_buf.lines().collect();
                                                    const CODE_FOLD_LINES: usize = 20;
                                                    if code_lines.len() > CODE_FOLD_LINES {
                                                        for line in &code_lines[..CODE_FOLD_LINES] {
                                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                                        }
                                                        println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                                                    } else {
                                                        for line in &code_lines {
                                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                                        }
                                                    }
                                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                                    code_block_buf.clear();
                                                    in_code_block = false;
                                                } else {
                                                    in_code_block = true;
                                                    code_block_buf.clear();
                                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                                }
                                            } else if in_code_block {
                                                // 代码块内容先缓存，等结束时统一折叠判断
                                                code_block_buf.push_str(trimmed);
                                                code_block_buf.push('\n');
                                            } else {
                                                print!("{}\n", trimmed);
                                            }
                                            std::io::stdout().flush()?;
                                            line_buf.clear();
                                        }
                                    }
                                    if !line_buf.is_empty() {
                                        if in_code_block {
                                            // 代码块未结束，缓存内容（不立即打印）
                                            code_block_buf.push_str(&line_buf);
                                        } else {
                                            print!("{}", line_buf);
                                        }
                                        std::io::stdout().flush()?;
                                        line_buf.clear();
                                    }
                                    full_response.push_str(text);
                                }
                            }
                            if in_code_block {
                                // 代码块未正常结束，把缓存内容全部打印出来
                                if !code_block_buf.is_empty() {
                                    let code_lines: Vec<&str> = code_block_buf.lines().collect();
                                    const CODE_FOLD_LINES: usize = 20;
                                    if code_lines.len() > CODE_FOLD_LINES {
                                        for line in &code_lines[..CODE_FOLD_LINES] {
                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                        }
                                        println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                                    } else {
                                        for line in &code_lines {
                                            print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                        }
                                    }
                                }
                                print!("{}", RESET);
                            }
                            println!();
                            println!();
                            let current_tokens = ChatEngine::get_session(&sid)
                                .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
                                .unwrap_or_else(|_| sent_tokens + full_response.len() / 4);
                            accumulated_tokens = current_tokens;
                            print_context_bar(current_tokens, ctx_window);
                            current_session = Some(sid.clone());
                            save_last_session_id(&sid);
                        }
                        Err(e) => {
                            eprintln!("\n{}❌ Error: {}{}\n", YELLOW, e, RESET);
                        }
                    }
                } else {
                    println!("{}Unknown command: {}. Type /help for available commands.{}", YELLOW, input, RESET);
                }
                continue;
            }
            _ => {}
        }

        turn_count += 1;
        let _ = turn_count; // suppress unused warning

        let (expanded_input, at_count) = expand_at_references(input);
        if at_count > 0 {
            println!("  {}📎 Attached {} file(s){}", GRAY, at_count, RESET);
        }
        let input = expanded_input.as_str();

        match engine
            .chat_react(input, model_override, current_session.as_deref())
            .await
        {
            Ok((mut rx, perm_tx, sid, sent_tokens, ctx_window)) => {
                println!();

                let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
                let _spinner_handle = tokio::spawn(async move {
                    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let mut i = 0usize;
                    loop {
                        if stop_rx.try_recv().is_ok() {
                            print!("\r\x1b[2K");
                            std::io::stdout().flush().ok();
                            break;
                        }
                        print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                        std::io::stdout().flush().ok();
                        i += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                    }
                });

                let mut full_response = String::new();
                let mut in_code_block = false;
                let mut code_block_buf = String::new();
                let mut line_buf = String::new();
                let mut stop_tx_opt = Some(stop_tx);
                let mut thinking_task: Option<tokio::task::JoinHandle<()>> = None;
                // 可展开的工具结果队列（ctrl+o 展开最后一条）
                let mut expandable_results: Vec<String> = Vec::new();

                macro_rules! stop_thinking {
                    () => {
                        if let Some(h) = thinking_task.take() {
                            h.abort();
                            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                            print!("\r\x1b[2K");
                            std::io::stdout().flush().ok();
                        }
                    };
                }

                while let Some(event) = rx.recv().await {
                    if let Some(tx) = stop_tx_opt.take() {
                        let _ = tx.send(());
                        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                        print!("\r\x1b[2K");
                        std::io::stdout().flush().ok();
                    }

                    // 非阻塞检测 ctrl+o 展开折叠结果
                    poll_expand_key(&mut expandable_results);

                    if event == "\x00D" {
                        stop_thinking!();
                        break;
                    } else if event == "\x00W" {
                        stop_thinking!();
                        let h = tokio::spawn(async {
                            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            let mut i = 0usize;
                            loop {
                                print!("\r  \x1b[36m{}\x1b[0m \x1b[2m∴ Thinking…\x1b[0m", frames[i % frames.len()]);
                                std::io::stdout().flush().ok();
                                i += 1;
                                tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                            }
                        });
                        thinking_task = Some(h);
                    } else if let Some(thinking_text) = event.strip_prefix("\x00H") {
                        stop_thinking!();
                        let preview: String = thinking_text.chars().take(60).collect();
                        let ellipsis = if thinking_text.len() > 60 { "…" } else { "" };
                        println!("  {}∴ {}{}{} {}(thinking){}", DIM, preview, ellipsis, RESET, DIM, RESET);
                        std::io::stdout().flush()?;
                    } else if let Some(summary) = event.strip_prefix("\x00S") {
                        stop_thinking!();
                        println!("  {}⏺ {}{}  {}(ctrl+o to expand){}", DIM, summary, RESET, DIM, RESET);
                        std::io::stdout().flush()?;
                    } else if let Some(perm_info) = event.strip_prefix("\x00K") {
                        stop_thinking!();
                        let parts: Vec<&str> = perm_info.splitn(4, '|').collect();
                        let perm_id = parts.first().copied().unwrap_or("");
                        let tool_name = parts.get(1).copied().unwrap_or("?");
                        let cmd = parts.get(2).copied().unwrap_or("");

                        println!();
                        let tool_name_owned = tool_name.to_string();
                        let cmd_owned = cmd.to_string();
                        let decision = tokio::task::spawn_blocking(move || {
                            if let Some((server_name, mcp_tool, args_json)) =
                                resolve_mcp_info(&tool_name_owned, &cmd_owned)
                            {
                                read_permission_choice_mcp(&server_name, &mcp_tool, &args_json)
                            } else {
                                read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                            }
                        })
                            .await
                            .unwrap_or(3);
                        let reply = match decision {
                            1 => format!("{}|allow", perm_id),
                            2 => format!("{}|allow_session", perm_id),
                            0 => format!("{}|deny_abort", perm_id), // Esc = 强制中止整个 agent loop
                            _ => format!("{}|deny", perm_id),
                        };
                        println!();
                        let _ = perm_tx.send(reply).await;
                    } else if let Some(tool_info) = event.strip_prefix("\x00T") {
                        stop_thinking!();
                        render_tool_call(tool_info);
                        std::io::stdout().flush()?;
                    } else if let Some(result) = event.strip_prefix("\x00R") {
                        let line_count = result.lines().count();
                        let char_count = result.len();
                        let should_fold = line_count > RESULT_FOLD_LINES || char_count > RESULT_FOLD_CHARS;
                        if should_fold {
                            println!("  {}  └─ {} line{}, {} chars  {}(ctrl+o to expand){}",
                                DIM,
                                line_count, if line_count != 1 { "s" } else { "" },
                                char_count,
                                RESET, RESET);
                            expandable_results.push(result.to_string());
                        } else {
                            println!("  {}  └─ {} line{}, {} chars{}",
                                DIM,
                                line_count, if line_count != 1 { "s" } else { "" },
                                char_count,
                                RESET);
                            for line in result.lines().take(RESULT_FOLD_LINES) {
                                println!("  {}     {}{}", DIM, line, RESET);
                            }
                        }
                        std::io::stdout().flush()?;
                    } else if let Some(text) = event.strip_prefix("\x00C") {
                        if full_response.is_empty() {
                            println!();
                            print!("{}{}Numina{} ", BOLD, CYAN, RESET);
                            std::io::stdout().flush()?;
                        }
                        for ch in text.chars() {
                            line_buf.push(ch);
                            if ch == '\n' {
                                let trimmed = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                                if trimmed.starts_with("```") {
                                    if in_code_block {
                                        // 代码块结束：检查是否需要折叠
                                        let code_lines: Vec<&str> = code_block_buf.lines().collect();
                                        const CODE_FOLD_LINES: usize = 20;
                                        if code_lines.len() > CODE_FOLD_LINES {
                                            for line in &code_lines[..CODE_FOLD_LINES] {
                                                print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                            }
                                            println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                                        } else {
                                            for line in &code_lines {
                                                print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                                            }
                                        }
                                        print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                        code_block_buf.clear();
                                        in_code_block = false;
                                    } else {
                                        in_code_block = true;
                                        code_block_buf.clear();
                                        print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                    }
                                } else if in_code_block {
                                    // 代码块内容先缓存，等结束时统一折叠判断
                                    code_block_buf.push_str(trimmed);
                                    code_block_buf.push('\n');
                                } else {
                                    print!("{}\n", trimmed);
                                }
                                std::io::stdout().flush()?;
                                line_buf.clear();
                            }
                        }
                        if !line_buf.is_empty() {
                            if in_code_block {
                                // 代码块未结束，缓存内容（不立即打印）
                                code_block_buf.push_str(&line_buf);
                            } else {
                                print!("{}", line_buf);
                            }
                            std::io::stdout().flush()?;
                            line_buf.clear();
                        }
                        full_response.push_str(text);
                    }
                }

                if in_code_block {
                    // 代码块未正常结束，把缓存内容全部打印出来
                    if !code_block_buf.is_empty() {
                        let code_lines: Vec<&str> = code_block_buf.lines().collect();
                        const CODE_FOLD_LINES: usize = 20;
                        if code_lines.len() > CODE_FOLD_LINES {
                            for line in &code_lines[..CODE_FOLD_LINES] {
                                print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                            }
                            println!("  {}  … {} more lines (folded){}", DIM, code_lines.len() - CODE_FOLD_LINES, RESET);
                        } else {
                            for line in &code_lines {
                                print!("{}{}{}{}\n", CODE_BG, CODE_FG, line, RESET);
                            }
                        }
                    }
                    print!("{}", RESET);
                }
                println!();
                println!();

                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let current_tokens = ChatEngine::get_session(&sid)
                    .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
                    .unwrap_or_else(|_| sent_tokens + full_response.len() / 4);
                accumulated_tokens = current_tokens;
                let _ = accumulated_tokens;
                print_context_bar(current_tokens, ctx_window);

                current_session = Some(sid.clone());
                save_last_session_id(&sid);
            }
            Err(e) => {
                eprintln!("\n{}❌ Error: {}{}\n", YELLOW, e, RESET);
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// 入口函数
// ─────────────────────────────────────────────

pub async fn execute(args: &ChatArgs) -> Result<()> {
    use super::commands::{cmd_sessions, cmd_show};

    if let Some(cmd) = &args.command {
        return match cmd {
            ChatCommand::Sessions => cmd_sessions(),
            ChatCommand::Show { session_id } => cmd_show(session_id),
        };
    }

    let engine = match ChatEngine::new() {
        Ok(e) => e,
        Err(err) => {
            eprintln!("{}⚠️  Failed to initialize ChatEngine: {}{}", YELLOW, err, RESET);
            eprintln!("   Run {}numina config init{} to set up your workspace.", BOLD, RESET);
            return Err(err);
        }
    };

    let model_name = engine.default_model();
    let skill_count = engine.skill_count();

    // 注册 skills 到补全器（Tab 补全时动态显示 skill 命令）
    register_skill_completions(engine.skill_names());

    if let Some(msg) = &args.message {
        print_welcome(&model_name, skill_count, args.session.as_deref(), false);
        run_single_message(&engine, msg, args).await?;
        return Ok(());
    }

    let restored_session = if args.session.is_none() {
        load_last_session_id()
    } else {
        None
    };

    let effective_session = args.session.clone().or(restored_session.clone());

    print_welcome(&model_name, skill_count, effective_session.as_deref(), true);

    // 计算上下文窗口大小（用于显示 context bar）
    let ctx_window = {
        let provider = ModelsConfig::load()
            .ok()
            .and_then(|mc| mc.models.iter().find(|m| m.name == model_name).map(|m| m.provider.clone()))
            .unwrap_or_else(|| "openai".to_string());
        let ctx_k: usize = estimate_context_size(&provider, &model_name).parse().unwrap_or(128);
        ctx_k * 1000
    };

    if let Some(ref sid) = restored_session {
        println!("  {}↩  Resumed session {}{}{}", GRAY, BOLD, &sid[..8.min(sid.len())], RESET);
        println!("  {}    Use /new to start a fresh conversation.{}", DIM, RESET);
        println!();
    }

    // 显示上下文使用情况：
    // - 有 session（自动恢复或 --session 指定）→ 读取历史累计 token 用量
    // - 全新 session（/new、/clear 后或首次启动无历史）→ 显示 0k
    let init_used_tokens = if let Some(ref sid) = effective_session {
        ChatEngine::get_session(sid)
            .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
            .unwrap_or(0)
    } else {
        0
    };
    print_context_bar(init_used_tokens, ctx_window);

    run_interactive_with_session(&engine, args, effective_session).await
}
