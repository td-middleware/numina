use anyhow::Result;
use std::io::Write;

use crate::config::ModelsConfig;
use crate::core::chat::ChatEngine;

use super::commands::{cmd_mcp_browser, cmd_model_picker, cmd_sessions};
use super::file_ref::expand_at_references;
use super::permission::read_permission_choice_interactive;
use super::readline::{interactive_readline, ReadLine};
use super::renderer::{
    estimate_context_size, print_context_bar, print_help, print_welcome,
    BOLD, CODE_BG, CODE_FG, CYAN, DIM, GRAY, GREEN, RESET, YELLOW,
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
fn render_json_block(json: &str) {
    // 最多显示 20 行，超出折叠
    let lines: Vec<&str> = json.lines().collect();
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
                        read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                    })
                        .await
                        .unwrap_or(3);
                    let reply = match decision {
                        1 => format!("{}|allow", perm_id),
                        2 => format!("{}|allow_session", perm_id),
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
                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                    in_code_block = false;
                                } else {
                                    in_code_block = true;
                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                }
                            } else if in_code_block {
                                print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                            } else {
                                print!("{}\n", trimmed);
                            }
                            std::io::stdout().flush()?;
                            line_buf.clear();
                        }
                    }
                    if !line_buf.is_empty() {
                        if in_code_block {
                            print!("{}{}{}", CODE_BG, CODE_FG, line_buf);
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
                if !trimmed.is_empty() {
                    chat_history.push(trimmed.clone());
                    if let Some(parent) = history_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&history_path)
                        .and_then(|mut f| writeln!(f, "{}", trimmed));
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
            "/clear" => {
                print!("\x1b[2J\x1b[H");
                std::io::stdout().flush()?;
                let model = engine.default_model();
                print_welcome(&model, engine.skill_count(), current_session.as_deref(), true);
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
                                        read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                                    }).await.unwrap_or(3);
                                    let reply = match decision {
                                        1 => format!("{}|allow", perm_id),
                                        2 => format!("{}|allow_session", perm_id),
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
                                                if in_code_block { print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET); in_code_block = false; }
                                                else { in_code_block = true; print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET); }
                                            } else if in_code_block { print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET); }
                                            else { print!("{}\n", trimmed); }
                                            std::io::stdout().flush()?;
                                            line_buf.clear();
                                        }
                                    }
                                    if !line_buf.is_empty() {
                                        if in_code_block { print!("{}{}{}", CODE_BG, CODE_FG, line_buf); }
                                        else { print!("{}", line_buf); }
                                        std::io::stdout().flush()?;
                                        line_buf.clear();
                                    }
                                    full_response.push_str(text);
                                }
                            }
                            if in_code_block { print!("{}", RESET); }
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
                            read_permission_choice_interactive(&tool_name_owned, &cmd_owned)
                        })
                            .await
                            .unwrap_or(3);
                        let reply = match decision {
                            1 => format!("{}|allow", perm_id),
                            2 => format!("{}|allow_session", perm_id),
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
                                        print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                        in_code_block = false;
                                    } else {
                                        in_code_block = true;
                                        print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                    }
                                } else if in_code_block {
                                    print!("{}{}{}{}\n", CODE_BG, CODE_FG, trimmed, RESET);
                                } else {
                                    print!("{}\n", trimmed);
                                }
                                std::io::stdout().flush()?;
                                line_buf.clear();
                            }
                        }
                        if !line_buf.is_empty() {
                            if in_code_block {
                                print!("{}{}{}", CODE_BG, CODE_FG, line_buf);
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

    if let Some(ref sid) = restored_session {
        println!("  {}↩  Resumed session {}{}{}", GRAY, BOLD, &sid[..8.min(sid.len())], RESET);
        println!("  {}    Use /new to start a fresh conversation.{}", DIM, RESET);
        println!();

        if let Ok(session) = ChatEngine::get_session(sid) {
            let used_chars: usize = session.turns.iter().map(|t| t.content.len()).sum();
            let used_tokens = used_chars / 4;
            let ctx_window = {
                let provider = ModelsConfig::load()
                    .ok()
                    .and_then(|mc| mc.models.iter().find(|m| m.name == model_name).map(|m| m.provider.clone()))
                    .unwrap_or_else(|| "openai".to_string());
                let ctx_k: usize = estimate_context_size(&provider, &model_name).parse().unwrap_or(128);
                ctx_k * 1000
            };
            if used_tokens > 0 {
                print_context_bar(used_tokens, ctx_window);
            }
        }
    }

    run_interactive_with_session(&engine, args, effective_session).await
}
