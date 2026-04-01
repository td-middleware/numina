use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::Write;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, ClearType},
};

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor, Helper};

use crate::config::{McpFileConfig, McpServerEntry, ModelEntry, ModelsConfig};
use crate::core::chat::{ChatEngine, ChatSession};

// ─────────────────────────────────────────────
// 斜杠命令补全器
// ─────────────────────────────────────────────

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help",     "显示帮助信息"),
    ("/new",      "开始新会话"),
    ("/session",  "显示当前会话 ID"),
    ("/sessions", "列出所有历史会话"),
    ("/model",    "显示当前模型"),
    ("/mcp",      "列出已配置的 MCP 服务"),
    ("/skills",   "显示已加载的 skills"),
    ("/clear",    "清屏"),
    ("/quit",     "退出 Numina"),
];

struct SlashCompleter;

impl Helper for SlashCompleter {}

impl Completer for SlashCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // 只在行首输入 / 时触发补全
        let word = &line[..pos];
        if !word.starts_with('/') {
            return Ok((pos, vec![]));
        }

        let matches: Vec<Pair> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(word))
            .map(|(cmd, desc)| Pair {
                display: format!("{:<12} {}", cmd, desc),
                replacement: cmd.to_string(),
            })
            .collect();

        Ok((0, matches))
    }
}

impl Hinter for SlashCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if pos < line.len() || !line.starts_with('/') {
            return None;
        }
        // 找到第一个匹配的命令，给出灰色提示后缀
        SLASH_COMMANDS
            .iter()
            .find(|(cmd, _)| cmd.starts_with(line) && *cmd != line)
            .map(|(cmd, _)| cmd[line.len()..].to_string())
    }
}

impl Highlighter for SlashCompleter {}
impl Validator for SlashCompleter {}

// ─────────────────────────────────────────────
// CLI 参数定义
// ─────────────────────────────────────────────

#[derive(Parser)]
pub struct ChatArgs {
    #[command(subcommand)]
    command: Option<ChatCommand>,

    /// 直接发送一条消息（非交互式）
    #[arg(short = 'M', long)]
    message: Option<String>,

    /// 覆盖默认模型（如 gpt-4o、claude-3-5-sonnet-20241022）
    #[arg(short = 'o', long)]
    model: Option<String>,

    /// 继续已有会话（传入 session ID）
    #[arg(short = 's', long)]
    session: Option<String>,

    /// 使用流式输出（逐 token 打印）
    #[arg(long, default_value_t = true)]
    stream: bool,
}

#[derive(Subcommand)]
enum ChatCommand {
    /// 列出所有历史会话
    Sessions,
    /// 查看某个会话的详细记录
    Show {
        /// Session ID
        session_id: String,
    },
}

// ─────────────────────────────────────────────
// 终端颜色/样式常量（ANSI escape codes）
// ─────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const BRIGHT_CYAN: &str = "\x1b[96m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BRIGHT_WHITE: &str = "\x1b[97m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const GRAY: &str = "\x1b[90m";

// ─────────────────────────────────────────────
// 入口
// ─────────────────────────────────────────────

pub async fn execute(args: &ChatArgs) -> Result<()> {
    // 处理子命令
    if let Some(cmd) = &args.command {
        return match cmd {
            ChatCommand::Sessions => cmd_sessions(),
            ChatCommand::Show { session_id } => cmd_show(session_id),
        };
    }

    // 初始化 ChatEngine
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

    // 单次消息模式
    if let Some(msg) = &args.message {
        print_welcome(&model_name, skill_count, args.session.as_deref(), false);
        run_single_message(&engine, msg, args).await?;
        return Ok(());
    }

    // 交互式模式
    print_welcome(&model_name, skill_count, args.session.as_deref(), true);
    run_interactive(&engine, args).await
}

// ─────────────────────────────────────────────
// 欢迎界面（Claude Code 风格）
// ─────────────────────────────────────────────

fn print_welcome(model: &str, skill_count: usize, session: Option<&str>, interactive: bool) {
    // 检测终端宽度
    let term_width = terminal_width();
    let separator = "─".repeat(term_width.min(72));

    println!();

    // ASCII Art 大字标题
    println!("{}{}  ███╗   ██╗██╗   ██╗███╗   ███╗██╗███╗   ██╗ █████╗{}", BOLD, BRIGHT_CYAN, RESET);
    println!("{}{}  ████╗  ██║██║   ██║████╗ ████║██║████╗  ██║██╔══██╗{}", BOLD, BRIGHT_CYAN, RESET);
    println!("{}{}  ██╔██╗ ██║██║   ██║██╔████╔██║██║██╔██╗ ██║███████║{}", BOLD, BRIGHT_CYAN, RESET);
    println!("{}{}  ██║╚██╗██║██║   ██║██║╚██╔╝██║██║██║╚██╗██║██╔══██║{}", BOLD, BRIGHT_CYAN, RESET);
    println!("{}{}  ██║ ╚████║╚██████╔╝██║ ╚═╝ ██║██║██║ ╚████║██║  ██║{}", BOLD, BRIGHT_CYAN, RESET);
    println!("{}{}  ╚═╝  ╚═══╝ ╚═════╝ ╚═╝     ╚═╝╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝{}", BOLD, BRIGHT_CYAN, RESET);
    println!();

    // 副标题
    println!("  {}{}AI Intelligent Agent  ·  v0.1.0{}", DIM, BRIGHT_WHITE, RESET);
    println!();

    // 分隔线
    println!("  {}{}{}", GRAY, separator, RESET);
    println!();

    // 模型信息行
    let model_provider = ModelsConfig::load()
        .ok()
        .and_then(|mc| mc.models.iter().find(|m| m.name == model).map(|m| m.provider.clone()))
        .unwrap_or_else(|| "openai".to_string());

    let provider_icon = match model_provider.as_str() {
        "anthropic" => "◆",
        "openai" => "◇",
        "local" => "◈",
        _ => "◉",
    };

    println!("  {}Model    {} {}{}{} {}({}){}", 
        GRAY,
        provider_icon,
        BOLD, BRIGHT_WHITE, model,
        GRAY, model_provider, RESET
    );

    // 上下文大小（估算）
    let ctx_size = estimate_context_size(&model_provider, model);
    println!("  {}Context  {} {}{}k tokens{}", 
        GRAY,
        "◈",
        BRIGHT_WHITE, ctx_size, RESET
    );

    // Skills
    if skill_count > 0 {
        println!("  {}Skills   {} {}{} loaded{}", 
            GRAY,
            "◆",
            BRIGHT_WHITE, skill_count, RESET
        );
    }

    // Session 信息
    if let Some(sid) = session {
        println!("  {}Session  {} {}{}...{}", 
            GRAY,
            "◈",
            BRIGHT_WHITE, &sid[..sid.len().min(8)], RESET
        );
    }

    println!();
    println!("  {}{}{}", GRAY, separator, RESET);
    println!();

    if interactive {
        // 命令提示
        println!("  {}Type a message to start chatting.{}", DIM, RESET);
        println!("  {}Commands:{} {}  /help  /new  /session  /sessions  /model  /skills  /quit{}", 
            DIM, RESET, GRAY, RESET);
        println!();
    }
}

/// 估算模型上下文窗口大小（k tokens）
fn estimate_context_size(provider: &str, model: &str) -> &'static str {
    let model_lower = model.to_lowercase();
    if model_lower.contains("claude-3-5") || model_lower.contains("claude-3.5") {
        "200"
    } else if model_lower.contains("claude-3") {
        "200"
    } else if model_lower.contains("gpt-4o") {
        "128"
    } else if model_lower.contains("gpt-4-turbo") {
        "128"
    } else if model_lower.contains("gpt-4") {
        "8"
    } else if model_lower.contains("gpt-3.5") {
        "16"
    } else if model_lower.contains("o1") || model_lower.contains("o3") {
        "200"
    } else if provider == "local" {
        "32"
    } else {
        "128"
    }
}

/// 获取终端宽度
fn terminal_width() -> usize {
    // 尝试从环境变量获取，否则默认 80
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80)
}

/// 打印上下文使用情况状态栏
/// 例如：  context  ▓▓▓▓▓▓░░░░░░░░░░  12.3k / 128k  (9%)
fn print_context_bar(used_tokens: usize, ctx_window: usize) {
    if ctx_window == 0 {
        return;
    }
    let pct = (used_tokens * 100) / ctx_window;
    let pct_clamped = pct.min(100);

    // 进度条：16 格
    let bar_len = 16usize;
    let filled = (pct_clamped * bar_len / 100).min(bar_len);
    let empty = bar_len - filled;
    let bar: String = "▓".repeat(filled) + &"░".repeat(empty);

    // 颜色：<50% 绿，50-80% 黄，>80% 红
    let color = if pct_clamped >= 80 {
        "\x1b[31m" // 红
    } else if pct_clamped >= 50 {
        "\x1b[33m" // 黄
    } else {
        "\x1b[32m" // 绿
    };

    // 格式化 token 数量（k 单位，保留一位小数）
    let used_k = used_tokens as f64 / 1000.0;
    let ctx_k = ctx_window as f64 / 1000.0;

    println!(
        "  {}context  {}{}{} {}{:.1}k{} / {}{:.1}k{}  {}({}%){}",
        GRAY,
        color, bar, RESET,
        BRIGHT_WHITE, used_k, RESET,
        GRAY, ctx_k, RESET,
        DIM, pct_clamped, RESET
    );
    println!();
}

// ─────────────────────────────────────────────
// 单次消息
// ─────────────────────────────────────────────

async fn run_single_message(engine: &ChatEngine, msg: &str, args: &ChatArgs) -> Result<()> {
    println!("{}{}You{} {}", BOLD, GREEN, RESET, msg);
    println!();

    let model_override = args.model.as_deref();
    let session_id = args.session.as_deref();

    if args.stream {
        let (mut rx, sid, sent_tokens, ctx_window) = engine
            .chat_stream(msg, model_override, session_id)
            .await?;

        print!("{}{}Numina{} ", BOLD, CYAN, RESET);
        std::io::stdout().flush()?;

        let mut full_response = String::new();
        while let Some(token) = rx.recv().await {
            print!("{}", token);
            std::io::stdout().flush()?;
            full_response.push_str(&token);
        }
        println!();
        println!();

        let used_tokens = sent_tokens + full_response.len() / 4;
        print_context_bar(used_tokens, ctx_window);

        if let Err(e) = ChatEngine::append_assistant_turn(&sid, &full_response) {
            eprintln!("{}⚠️  Failed to save session: {}{}", YELLOW, e, RESET);
        }
    } else {
        let (reply, _sid, used_tokens, ctx_window) = engine
            .chat_once(msg, model_override, session_id)
            .await?;

        println!("{}{}Numina{} {}", BOLD, CYAN, RESET, reply);
        println!();
        print_context_bar(used_tokens, ctx_window);
    }

    Ok(())
}

// ─────────────────────────────────────────────
// 交互式循环
// ─────────────────────────────────────────────

async fn run_interactive(engine: &ChatEngine, args: &ChatArgs) -> Result<()> {
    let model_override = args.model.as_deref();
    let mut current_session: Option<String> = args.session.clone();
    let mut turn_count = 0usize;

    // 初始化 rustyline editor（Tab 补全 + 历史记录）
    let rl_config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut rl: Editor<SlashCompleter, _> = Editor::with_config(rl_config)?;
    rl.set_helper(Some(SlashCompleter));

    // 加载历史记录（忽略错误）
    let history_path = dirs::home_dir()
        .map(|h| h.join(".numina").join("chat_history"))
        .unwrap_or_else(|| std::path::PathBuf::from(".numina_history"));
    let _ = rl.load_history(&history_path);

    loop {
        // 使用 rustyline 读取输入（支持 Tab 补全、上下键历史、左右键移动）
        let prompt = format!("{}{}>{} ", BOLD, GREEN, RESET);
        let readline = rl.readline(&prompt);

        let input = match readline {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    let _ = rl.add_history_entry(&trimmed);
                }
                trimmed
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C：取消当前输入，继续循环
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D：退出
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

        // 内置命令
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
                // 清屏
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

        // 发送消息
        if args.stream {
            let (mut rx, sid, sent_tokens, ctx_window) = match engine
                .chat_stream(input, model_override, current_session.as_deref())
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("\n{}❌ Error: {}{}\n", YELLOW, e, RESET);
                    continue;
                }
            };

            println!();
            print!("{}{}Numina{} ", BOLD, CYAN, RESET);
            std::io::stdout().flush()?;

            let mut full_response = String::new();
            while let Some(token) = rx.recv().await {
                print!("{}", token);
                std::io::stdout().flush()?;
                full_response.push_str(&token);
            }
            println!();
            println!();

            let used_tokens = sent_tokens + full_response.len() / 4;
            print_context_bar(used_tokens, ctx_window);

            if let Err(e) = ChatEngine::append_assistant_turn(&sid, &full_response) {
                eprintln!("{}⚠️  Failed to save session: {}{}", YELLOW, e, RESET);
            }

            current_session = Some(sid);
        } else {
            match engine
                .chat_once(input, model_override, current_session.as_deref())
                .await
            {
                Ok((reply, sid, used_tokens, ctx_window)) => {
                    println!();
                    println!("{}{}Numina{} {}", BOLD, CYAN, RESET, reply);
                    println!();

                    print_context_bar(used_tokens, ctx_window);

                    current_session = Some(sid);
                }
                Err(e) => {
                    eprintln!("\n{}❌ Error: {}{}\n", YELLOW, e, RESET);
                }
            }
        }
    }

    // 保存历史记录
    let _ = rl.save_history(&history_path);

    Ok(())
}

fn print_help() {
    println!();
    println!("  {}{}Available Commands{}", BOLD, BRIGHT_WHITE, RESET);
    println!("  {}{}{}", GRAY, "─".repeat(40), RESET);
    println!("  {}/help{}     {}Show this help message{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/new{}      {}Start a new conversation session{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/session{}  {}Show current session ID{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/sessions{} {}List all saved sessions{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/model{}    {}Show active model info{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/mcp{}      {}List configured MCP servers{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/skills{}   {}Show loaded skills count{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/clear{}    {}Clear screen and show welcome{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/quit{}     {}Exit Numina{}", BOLD, RESET, GRAY, RESET);
    println!();
    println!("  {}Tip:{} Press {}Ctrl+D{} to exit, {}Ctrl+C{} to cancel input.",
        GRAY, RESET, BOLD, RESET, BOLD, RESET);
    println!();
}

// ─────────────────────────────────────────────
// 子命令实现
// ─────────────────────────────────────────────

fn cmd_sessions() -> Result<()> {
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

// ─────────────────────────────────────────────
// /model 交互式选择器
// ─────────────────────────────────────────────

/// 用上下键选择模型，Enter 确认切换，Esc/q 取消
/// 返回 Some(name) 表示已切换，None 表示取消
fn cmd_model_picker() -> Result<Option<String>> {
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

    // 找到当前 active 的索引
    let active_idx = cfg.models.iter().position(|m| m.name == cfg.active).unwrap_or(0);
    let mut selected = active_idx;
    let count = cfg.models.len();

    // 进入 raw mode
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();

    // 渲染函数（用 lines_drawn 追踪行数，每次重绘先 MoveUp 回到起始行）
    let render_models = |stdout: &mut std::io::Stdout,
                         models: &[ModelEntry],
                         sel: usize,
                         active: &str,
                         lines_drawn: &mut u16| -> Result<()> {
        if *lines_drawn > 0 {
            execute!(stdout, cursor::MoveUp(*lines_drawn))?;
        }
        execute!(stdout, cursor::MoveToColumn(0), terminal::Clear(ClearType::FromCursorDown))?;

        let mut drawn: u16 = 0;
        macro_rules! pline {
            ($s:expr) => {{
                execute!(stdout, Print($s))?;
                drawn += 1;
            }};
        }

        pline!(format!("\r\n  {}{}Models{} {}(↑↓ navigate · Enter select · Esc cancel){}\r\n",
            BOLD, BRIGHT_WHITE, RESET, GRAY, RESET));
        pline!(format!("  {}{}\r\n", GRAY, "─".repeat(56)));

        for (i, m) in models.iter().enumerate() {
            let is_sel = i == sel;
            let is_active = m.name == active;
            let cursor_str = if is_sel { "❯ " } else { "  " };
            let active_dot = if is_active { format!(" {}●{}", "\x1b[32m", RESET) } else { String::new() };
            let ctx_k = m.max_tokens.map(|t| format!("{}k", t / 1000)).unwrap_or_else(|| "?k".to_string());
            if is_sel {
                pline!(format!("  {}{}{}{}{}{} {}{}{} {}({}){}  {}{}{}\r\n",
                    BOLD, "\x1b[96m", cursor_str, m.name, active_dot, RESET,
                    GRAY, m.provider, RESET,
                    GRAY, ctx_k, RESET,
                    DIM, m.description.as_deref().unwrap_or(""), RESET,
                ));
            } else {
                pline!(format!("  {}{}{} {}{}{}{} {}({}){}  {}{}{}\r\n",
                    GRAY, cursor_str, RESET,
                    BOLD, m.name, active_dot, RESET,
                    GRAY, m.provider, RESET,
                    DIM, m.description.as_deref().unwrap_or(""), RESET,
                ));
            }
        }
        pline!(format!("  {}{}\r\n", GRAY, "─".repeat(56)));
        stdout.flush()?;
        *lines_drawn = drawn;
        Ok(())
    };

    let mut lines_drawn: u16 = 0;
    let active_clone = cfg.active.clone();
    render_models(&mut stdout, &cfg.models, selected, &active_clone, &mut lines_drawn)?;

    let result = loop {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 { selected -= 1; } else { selected = count - 1; }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1) % count;
                }
                KeyCode::Enter => {
                    let name = cfg.models[selected].name.clone();
                    cfg.active = name.clone();
                    let _ = cfg.save();
                    break Some(name);
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    break None;
                }
                _ => {}
            }
            let active_clone = cfg.active.clone();
            render_models(&mut stdout, &cfg.models, selected, &active_clone, &mut lines_drawn)?;
        }
    };

    terminal::disable_raw_mode()?;
    if lines_drawn > 0 {
        execute!(stdout, cursor::MoveUp(lines_drawn))?;
    }
    execute!(stdout, cursor::MoveToColumn(0), terminal::Clear(ClearType::FromCursorDown))?;
    println!();

    Ok(result)
}

// ─────────────────────────────────────────────
// /mcp 交互式浏览器
// ─────────────────────────────────────────────

/// 通过 stdio 子进程调用 MCP server 的 tools/list
fn fetch_mcp_tools(srv: &McpServerEntry) -> Vec<(String, String, Vec<(String, String, bool)>)> {
    // 只支持 stdio 类型
    if srv.server_type != "stdio" {
        return vec![];
    }

    // 构建命令
    let mut parts = vec![srv.command_or_url.clone()];
    if let Some(args_str) = &srv.args {
        for a in args_str.split_whitespace() {
            parts.push(a.to_string());
        }
    }
    if parts.is_empty() {
        return vec![];
    }

    // JSON-RPC initialize + tools/list
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

    // 解析每行 JSON，找 tools/list 的响应
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

/// MCP 浏览器：上下键选 server，Enter 展开查看 tools，Esc 返回
async fn cmd_mcp_browser() -> Result<()> {
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

    let servers = cfg.servers;
    let count = servers.len();
    let mut selected = 0usize;
    // expanded[i] = Some(tools) 表示已展开，None 表示未展开
    let mut expanded: Vec<Option<Vec<(String, String, Vec<(String, String, bool)>)>>> = vec![None; count];

    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();

    // 计算渲染行数（用于回滚光标）
    let count_render_lines = |expanded: &[Option<Vec<(String, String, Vec<(String, String, bool)>)>>],
                               servers: &[McpServerEntry]| -> u16 {
        let mut lines: u16 = 4; // header(2) + separator(2)
        for (i, _) in servers.iter().enumerate() {
            lines += 1; // server 行
            if let Some(tools) = &expanded[i] {
                if tools.is_empty() {
                    lines += 1;
                } else {
                    for (_, _, params) in tools {
                        lines += 1; // tool name
                        if true { lines += 1; } // desc（保守估计）
                        lines += params.len() as u16;
                    }
                }
            }
        }
        lines += 1; // bottom separator
        lines
    };

    let render_servers = |stdout: &mut std::io::Stdout,
                          servers: &[McpServerEntry],
                          expanded: &[Option<Vec<(String, String, Vec<(String, String, bool)>)>>],
                          sel: usize,
                          lines_drawn: &mut u16| -> Result<()> {
        // 回到起始行
        if *lines_drawn > 0 {
            execute!(stdout, cursor::MoveUp(*lines_drawn))?;
        }
        execute!(stdout, cursor::MoveToColumn(0), terminal::Clear(ClearType::FromCursorDown))?;

        let mut drawn: u16 = 0;
        let print_line = |stdout: &mut std::io::Stdout, s: String, cnt: &mut u16| -> Result<()> {
            execute!(stdout, Print(s))?;
            *cnt += 1;
            Ok(())
        };

        print_line(stdout, format!("\r\n  {}{}MCP Servers{} {}(↑↓ navigate · Enter expand/collapse · Esc close){}\r\n",
            BOLD, BRIGHT_WHITE, RESET, GRAY, RESET), &mut drawn)?;
        print_line(stdout, format!("  {}{}\r\n", GRAY, "─".repeat(60)), &mut drawn)?;

        for (i, srv) in servers.iter().enumerate() {
            let is_sel = i == sel;
            let cursor_str = if is_sel { "❯ " } else { "  " };
            let status = if srv.enabled { format!("{}●{}", "\x1b[32m", RESET) } else { format!("{}○{}", GRAY, RESET) };

            if is_sel {
                print_line(stdout, format!("  {}{}{}{} {} {}{}{} {}[{}]{}\r\n",
                    BOLD, "\x1b[96m", cursor_str, RESET,
                    status,
                    BOLD, srv.name, RESET,
                    GRAY, srv.server_type, RESET,
                ), &mut drawn)?;
            } else {
                print_line(stdout, format!("  {}{}{} {} {}{}{} {}[{}]{}\r\n",
                    GRAY, cursor_str, RESET,
                    status,
                    BOLD, srv.name, RESET,
                    GRAY, srv.server_type, RESET,
                ), &mut drawn)?;
            }

            if let Some(tools) = &expanded[i] {
                if tools.is_empty() {
                    print_line(stdout, format!("       {}  (no tools found or server not reachable){}\r\n", GRAY, RESET), &mut drawn)?;
                } else {
                    for (tname, tdesc, tparams) in tools {
                        print_line(stdout, format!("       {}◆ {}{}{}{}\r\n",
                            "\x1b[33m", RESET, BOLD, tname, RESET), &mut drawn)?;
                        if !tdesc.is_empty() {
                            print_line(stdout, format!("         {}  {}{}\r\n", GRAY, tdesc, RESET), &mut drawn)?;
                        }
                        for (pname, ptype, req) in tparams {
                            let req_mark = if *req { format!("{}*{}", "\x1b[31m", RESET) } else { format!("{}?{}", GRAY, RESET) };
                            print_line(stdout, format!("         {}  {} {}{}{}: {}{}{}\r\n",
                                DIM, req_mark,
                                "\x1b[96m", pname, RESET,
                                GRAY, ptype, RESET), &mut drawn)?;
                        }
                    }
                }
            }
        }
        print_line(stdout, format!("  {}{}\r\n", GRAY, "─".repeat(60)), &mut drawn)?;
        stdout.flush()?;
        *lines_drawn = drawn;
        Ok(())
    };

    let mut lines_drawn: u16 = 0;
    render_servers(&mut stdout, &servers, &expanded, selected, &mut lines_drawn)?;

    loop {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 { selected -= 1; } else { selected = count - 1; }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1) % count;
                }
                KeyCode::Enter => {
                    if expanded[selected].is_some() {
                        expanded[selected] = None;
                    } else {
                        // 显示 loading 提示
                        if lines_drawn > 0 {
                            execute!(stdout, cursor::MoveUp(lines_drawn))?;
                        }
                        execute!(stdout, cursor::MoveToColumn(0), terminal::Clear(ClearType::FromCursorDown))?;
                        execute!(stdout, Print(format!("  {}⏳ Fetching tools from {}...{}\r\n",
                            GRAY, servers[selected].name, RESET)))?;
                        stdout.flush()?;
                        lines_drawn = 1;

                        terminal::disable_raw_mode()?;
                        let tools = fetch_mcp_tools(&servers[selected]);
                        terminal::enable_raw_mode()?;
                        expanded[selected] = Some(tools);
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    break;
                }
                _ => {}
            }
            render_servers(&mut stdout, &servers, &expanded, selected, &mut lines_drawn)?;
        }
    }

    terminal::disable_raw_mode()?;
    if lines_drawn > 0 {
        execute!(stdout, cursor::MoveUp(lines_drawn))?;
    }
    execute!(stdout, cursor::MoveToColumn(0), terminal::Clear(ClearType::FromCursorDown))?;
    println!();
    Ok(())
}

fn cmd_mcp_list() {
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
                let status = if srv.enabled {
                    format!("{}●{}", "\x1b[32m", RESET) // 绿点
                } else {
                    format!("{}○{}", GRAY, RESET)       // 灰圈
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

fn cmd_show(session_id: &str) -> Result<()> {
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
