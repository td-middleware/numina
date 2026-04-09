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
                    let parts: Vec<&str> = tool_info.splitn(2, '|').collect();
                    let tool_name = parts.first().copied().unwrap_or("?");
                    let params = parts.get(1).copied().unwrap_or("");
                    if params.is_empty() {
                        println!("  {}●{} {}{}{}", CYAN, RESET, BOLD, tool_name, RESET);
                    } else {
                        let params_preview: String = params.chars().take(60).collect();
                        let ellipsis = if params.len() > 60 { "…" } else { "" };
                        println!("  {}●{} {}{}{}({}{}{}{}){}",
                            CYAN, RESET,
                            BOLD, tool_name, RESET,
                            DIM, params_preview, ellipsis, RESET,
                            RESET);
                    }
                    std::io::stdout().flush()?;
                } else if let Some(result) = event.strip_prefix("\x00R") {
                    let line_count = result.lines().count();
                    let char_count = result.len();
                    println!("  {}  └─ {} line{}, {} chars{}",
                        DIM,
                        line_count, if line_count != 1 { "s" } else { "" },
                        char_count,
                        RESET);
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
                let skills = engine.skill_count();
                if skills == 0 {
                    println!("{}No skills loaded. Add a claude.md to your workspace.{}", GRAY, RESET);
                } else {
                    println!("{}Loaded {} skill(s). See ~/.numina/workspace/claude.md{}", GRAY, skills, RESET);
                }
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
                println!("{}Unknown command: {}. Type /help for available commands.{}", YELLOW, input, RESET);
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
                        let parts: Vec<&str> = tool_info.splitn(2, '|').collect();
                        let tool_name = parts.first().copied().unwrap_or("?");
                        let params = parts.get(1).copied().unwrap_or("");
                        if params.is_empty() {
                            println!("  {}●{} {}{}{}", CYAN, RESET, BOLD, tool_name, RESET);
                        } else {
                            let params_preview: String = params.chars().take(60).collect();
                            let ellipsis = if params.len() > 60 { "…" } else { "" };
                            println!("  {}●{} {}{}{}({}{}{}{}){}",
                                CYAN, RESET,
                                BOLD, tool_name, RESET,
                                DIM, params_preview, ellipsis, RESET,
                                RESET);
                        }
                        std::io::stdout().flush()?;
                    } else if let Some(result) = event.strip_prefix("\x00R") {
                        let line_count = result.lines().count();
                        let char_count = result.len();
                        println!("  {}  └─ {} line{}, {} chars{}",
                            DIM,
                            line_count, if line_count != 1 { "s" } else { "" },
                            char_count,
                            RESET);
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
