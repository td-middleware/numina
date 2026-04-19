use crate::config::models::ModelsConfig;
use crate::cli::session::commands::{load_lark_user_cache, lark_avatar_cache_path};

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

    // 飞书用户信息（如果已登录）
    if let Some(user) = load_lark_user_cache() {
        let email_hint = if !user.email.is_empty() {
            format!("  {}<{}>{}", GRAY, user.email, RESET)
        } else {
            String::new()
        };

        if is_iterm2() {
            // iTerm2：inline image 显示真实头像图片
            let avatar_display = if let Some(path) = lark_avatar_cache_path() {
                let s = iterm2_inline_image_from_file(&path, 2);
                if s.is_empty() { lark_avatar_block(&user.name) } else { s }
            } else {
                lark_avatar_block(&user.name)
            };
            println!("  {}Lark     {} {}{}{}{}{}", 
                GRAY, avatar_display,
                BOLD, BRIGHT_WHITE, user.name, RESET,
                email_hint
            );
        } else {
            // macOS Terminal.app 及其他终端：彩色首字母方块 + 姓名（不显示字符画）
            println!("  {}Lark     {} {}{}{}{}{}", 
                GRAY, lark_avatar_block(&user.name),
                BOLD, BRIGHT_WHITE, user.name, RESET,
                email_hint
            );
        }
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

/// 根据用户名生成彩色首字母头像方块
/// 例如："张三" → "\x1b[48;5;33m\x1b[97m 张 \x1b[0m"
pub fn lark_avatar_block(name: &str) -> String {
    // 取第一个字符（支持中文）
    let first_char = name.chars().next().unwrap_or('?');

    // 根据名字哈希选择背景色（从一组好看的颜色中选）
    let colors: &[u8] = &[33, 36, 38, 64, 70, 125, 130, 160, 166, 172, 196, 202, 208];
    let hash = name.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let color = colors[(hash as usize) % colors.len()];

    format!("\x1b[48;5;{}m\x1b[97m {} \x1b[0m", color, first_char)
}

/// 返回用户头像方块字符串（用于嵌入提示符）
/// 格式：彩色背景首字母方块，例如 "\x1b[48;5;33m\x1b[97m 张 \x1b[0m"
pub fn draw_user_badge_top_right() -> String {
    let user = match crate::cli::session::commands::load_lark_user_cache() {
        Some(u) => u,
        None => return String::new(),
    };
    lark_avatar_block(&user.name)
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

/// 获取终端宽度（优先 $COLUMNS，其次 tput cols，默认 80）
pub fn terminal_width() -> usize {
    // 1. 环境变量 COLUMNS
    if let Ok(cols) = std::env::var("COLUMNS") {
        if let Ok(n) = cols.parse::<usize>() {
            if n > 0 { return n; }
        }
    }
    // 2. tput cols（macOS Terminal.app / Linux）
    if let Ok(out) = std::process::Command::new("tput").arg("cols").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Ok(n) = s.trim().parse::<usize>() {
                if n > 0 { return n; }
            }
        }
    }
    80
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

// ─────────────────────────────────────────────
// iTerm2 inline image 协议支持
// ─────────────────────────────────────────────

/// 用 chafa 命令将头像渲染为 Unicode 字符画（适用于所有终端）
/// 返回 Some(字符画字符串) 或 None（chafa 未安装或头像不存在）
/// 渲染尺寸：自动适配终端宽度（最大宽度的一半，高度等比），最小 16x8
pub fn render_avatar_chafa() -> Option<String> {
    let path = lark_avatar_cache_path()?;

    // 检测 chafa 是否可用
    let which = std::process::Command::new("which")
        .arg("chafa")
        .output()
        .ok()?;
    if !which.status.success() {
        return None;
    }

    // 动态计算尺寸：宽度 = 终端宽度的一半（最小16，最大40），高度 = 宽度/2（近似正方形）
    let term_w = terminal_width();
    let avatar_w = (term_w / 2).max(16).min(40);
    let avatar_h = avatar_w / 2;
    let size_arg = format!("--size={}x{}", avatar_w, avatar_h);

    // 调用 chafa 渲染：half symbols（上下各1像素，颜色最丰富），256色
    let output = std::process::Command::new("chafa")
        .args([
            &size_arg,
            "--symbols=half",
            "--colors=256",
            path.to_str()?,
        ])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 检测当前终端是否为 iTerm2
pub fn is_iterm2() -> bool {
    std::env::var("TERM_PROGRAM")
        .map(|v| v == "iTerm.app")
        .unwrap_or(false)
}

/// 将图片文件编码为 iTerm2 inline image 转义序列
/// `cell_height` 为显示高度（单位：字符行数），高度用字符单元（不带 px），随字体大小自动缩放
/// 返回可直接 print! 的字符串；若读取文件失败则返回空字符串
pub fn iterm2_inline_image_from_file(path: &std::path::Path, cell_height: u32) -> String {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() || buf.is_empty() {
        return String::new();
    }
    // Base64 编码（不依赖外部 crate，手写简单实现）
    let b64 = base64_encode(&buf);
    // iTerm2 协议：height=N（字符单元，不带 px），随终端字体大小自动缩放
    // width=2 固定占 2 列，与 readline.rs 的 visible_columns 中 OSC 序列计为 2 列保持一致
    // 这样光标位置计算才能准确
    format!(
        "\x1b]1337;File=inline=1;width=2;height={};preserveAspectRatio=1:{}\x07",
        cell_height,
        b64
    )
}

/// 将头像图片裁剪为圆形 PNG，保存到指定路径
/// 使用 image crate 纯 Rust 实现，不依赖外部工具
/// 圆形效果：在 alpha 通道上应用圆形蒙版
pub fn make_circle_avatar(src: &std::path::Path, dst: &std::path::Path) -> bool {
    use image::{GenericImageView, ImageBuffer, Rgba};

    // 读取源图片（支持 JPEG/PNG）
    let img = match image::open(src) {
        Ok(i) => i,
        Err(_) => return false,
    };

    // 缩放为正方形（128×128），保证圆形效果好看
    let size = 128u32;
    let img = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);

    // 创建带 alpha 通道的 RGBA 图像
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(size, size);
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let r = size as f32 / 2.0;

    for (x, y, pixel) in img.pixels() {
        let dx = x as f32 - cx + 0.5;
        let dy = y as f32 - cy + 0.5;
        let dist = (dx * dx + dy * dy).sqrt();

        // 抗锯齿：在边缘 1px 范围内做平滑过渡
        let alpha = if dist <= r - 1.0 {
            255u8
        } else if dist <= r {
            ((r - dist) * 255.0) as u8
        } else {
            0u8
        };

        let [pr, pg, pb, _] = pixel.0;
        out.put_pixel(x, y, Rgba([pr, pg, pb, alpha]));
    }

    // 保存为 PNG（支持 alpha 通道）
    out.save(dst).is_ok()
}

/// 极简 Base64 编码（RFC 4648，无换行）
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 0x3f) as usize] as char } else { '=' });
    }
    out
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
    println!("  {}/memory{}   {}List all memories{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/memory add <content>{}  {}Add a memory (global scope){}", BOLD, RESET, GRAY, RESET);
    println!("  {}/memory add -p <content>{}  {}Add a project-scoped memory{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/memory forget <id>{}  {}Delete a memory by ID{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/memory search <query>{}  {}Search memories{}", BOLD, RESET, GRAY, RESET);
    println!("  {}/quit{}     {}Exit Numina{}", BOLD, RESET, GRAY, RESET);
    println!();
    println!("  {}Tip:{} Press {}Ctrl+D{} to exit, {}Ctrl+C{} to cancel input.",
        GRAY, RESET, BOLD, RESET, BOLD, RESET);
    println!("  {}      Use {}@path{} to attach a file or directory to your message.",
        GRAY, BOLD, RESET);
    println!();
}
