use crate::config::models::ModelsConfig;

// ─────────────────────────────────────────────
// 终端颜色/样式常量（ANSI escape codes）
// ─────────────────────────────────────────────

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const CYAN: &str = "\x1b[36m";
pub const BRIGHT_CYAN: &str = "\x1b[96m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BRIGHT_WHITE: &str = "\x1b[97m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const GRAY: &str = "\x1b[90m";
/// 代码块背景色（深灰背景 + 浅灰前景，类似 Claude Code 风格）
pub const CODE_BG: &str = "\x1b[48;5;236m";
pub const CODE_FG: &str = "\x1b[38;5;252m";

// ─────────────────────────────────────────────
// 欢迎界面（Numina 风格）
// ─────────────────────────────────────────────

pub fn print_welcome(model: &str, skill_count: usize, session: Option<&str>, interactive: bool) {
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
    println!("  {}Context  {} {}{} k tokens{}", 
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
        println!("  {}Type a message to start chatting.{}", DIM, RESET);
        println!("  {}Commands:{} {}  /help  /new  /session  /sessions  /model  /skills  /quit{}", 
            DIM, RESET, GRAY, RESET);
        println!();
    }
}

/// 估算模型上下文窗口大小（k tokens），优先从 ModelsConfig 读取 max_tokens
pub fn estimate_context_size(_provider: &str, model: &str) -> String {
    if let Ok(mc) = ModelsConfig::load() {
        if let Some(m) = mc.models.iter().find(|m| m.name == model) {
            if let Some(max_tok) = m.max_tokens {
                return format!("{}", max_tok / 1000);
            }
        }
    }
    let model_lower = model.to_lowercase();
    if model_lower.contains("claude-3-5") || model_lower.contains("claude-3.5") {
        "200".to_string()
    } else if model_lower.contains("claude-3") {
        "200".to_string()
    } else if model_lower.contains("gpt-4o") {
        "128".to_string()
    } else if model_lower.contains("gpt-4-turbo") {
        "128".to_string()
    } else if model_lower.contains("gpt-4") {
        "8".to_string()
    } else if model_lower.contains("gpt-3.5") {
        "16".to_string()
    } else if model_lower.contains("o1") || model_lower.contains("o3") {
        "200".to_string()
    } else {
        "128".to_string()
    }
}

/// 获取终端宽度
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80)
}

/// 打印上下文使用情况状态栏
pub fn print_context_bar(used_tokens: usize, ctx_window: usize) {
    if ctx_window == 0 {
        return;
    }
    let pct = (used_tokens * 100) / ctx_window;

    let bar_len = 16usize;
    let filled = (pct.min(100) * bar_len / 100).min(bar_len);
    let empty = bar_len - filled;
    let bar: String = "▓".repeat(filled) + &"░".repeat(empty);

    let color = if pct >= 100 {
        "\x1b[1;31m"
    } else if pct >= 80 {
        "\x1b[31m"
    } else if pct >= 50 {
        "\x1b[33m"
    } else {
        "\x1b[32m"
    };

    let used_k = used_tokens as f64 / 1000.0;
    let ctx_k = ctx_window as f64 / 1000.0;

    if pct >= 100 {
        println!(
            "  {}context  {}{}{} {}{:.1}k{} / {}{:.1}k{}  {}({}% ⚠ context full){}",
            GRAY,
            color, bar, RESET,
            color, used_k, RESET,
            GRAY, ctx_k, RESET,
            color, pct, RESET
        );
    } else {
        println!(
            "  {}context  {}{}{} {}{:.1}k{} / {}{:.1}k{}  {}({}%){}",
            GRAY,
            color, bar, RESET,
            BRIGHT_WHITE, used_k, RESET,
            GRAY, ctx_k, RESET,
            DIM, pct, RESET
        );
    }
    println!();
}

pub fn print_help() {
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
    println!("  {}      Use {}@path{} to attach a file or directory to your message.",
        GRAY, BOLD, RESET);
    println!();
}
