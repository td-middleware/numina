use anyhow::Result;
use clap::{Parser, Subcommand};
use std::borrow::Cow;
use std::io::Write;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Cmd, Context, Helper, Movement};

use crate::config::{McpFileConfig, McpServerEntry, ModelEntry, ModelsConfig};
use crate::core::chat::{ChatEngine, ChatSession};

// ─────────────────────────────────────────────
// 补全器：支持 / 斜杠命令 + @ 文件路径（含隐藏文件）
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

// 补全列表背景色（深蓝灰，区别于聊天背景）
const COMPLETION_BG: &str = "\x1b[48;5;238m";
const COMPLETION_FG: &str = "\x1b[38;5;255m";
const COMPLETION_DIR_FG: &str = "\x1b[38;5;117m"; // 目录用蓝色

struct ChatCompleter;

impl ChatCompleter {
    fn new() -> Self { Self }

    /// 自定义文件路径补全（支持 ~ 展开、隐藏文件）
    fn complete_path(path_input: &str) -> Vec<Pair> {
        use std::path::Path;

        // 展开 ~ 为 home 目录
        let expanded = if path_input.starts_with("~/") || path_input == "~" {
            if let Some(home) = dirs::home_dir() {
                let rest = &path_input[1..];
                format!("{}{}", home.display(), rest)
            } else {
                path_input.to_string()
            }
        } else {
            path_input.to_string()
        };

        // 分离目录部分和文件名前缀
        let (dir_path, file_prefix, display_prefix) = if expanded.ends_with('/') {
            // 输入以 / 结尾：列出该目录内容
            (expanded.clone(), String::new(), path_input.to_string())
        } else {
            let p = Path::new(&expanded);
            let parent = p.parent().map(|d| {
                let s = d.to_string_lossy().to_string();
                if s.is_empty() { ".".to_string() } else { s }
            }).unwrap_or_else(|| ".".to_string());
            let fname = p.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            // display_prefix：用于构造 replacement 时还原 ~ 前缀
            let disp = if path_input.starts_with("~/") || path_input == "~" {
                let home_str = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
                let parent_disp = parent.replacen(&home_str, "~", 1);
                if parent_disp == "~" { "~/".to_string() } else { format!("{}/", parent_disp) }
            } else if parent == "." {
                String::new()
            } else if parent == "/" {
                // 修复：避免绝对路径父目录产生 "//" 前缀
                "/".to_string()
            } else {
                format!("{}/", parent)
            };
            (parent, fname, disp)
        };

        let dir = Path::new(&dir_path);
        let entries = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return vec![],
        };

        let mut pairs: Vec<Pair> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                // 匹配前缀（大小写敏感）
                name_str.starts_with(file_prefix.as_str())
            })
            .map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy().to_string();
                let is_dir = e.path().is_dir();
                let suffix = if is_dir { "/" } else { "" };
                // replacement：@后面的完整路径
                let replacement = format!("{}{}{}", display_prefix, name_str, suffix);
                // display：纯文本（highlight_candidate 会加颜色）
                let display = format!("{}{}", name_str, suffix);
                Pair { display, replacement }
            })
            .collect();

        // 排序：目录优先，然后按名称
        pairs.sort_by(|a, b| {
            let a_dir = a.replacement.ends_with('/');
            let b_dir = b.replacement.ends_with('/');
            b_dir.cmp(&a_dir).then(a.replacement.cmp(&b.replacement))
        });

        pairs
    }
}

impl Helper for ChatCompleter {}

impl Completer for ChatCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let word = &line[..pos];

        // @ 文件路径补全：找到最后一个 @ 的位置
        if let Some(at_pos) = word.rfind('@') {
            let path_part = &word[at_pos + 1..];
            let candidates = Self::complete_path(path_part);
            return Ok((at_pos + 1, candidates));
        }

        // / 斜杠处理：行首 / 时优先匹配内置命令，无匹配则回退到绝对路径文件补全
        if word.starts_with('/') {
            let cmd_matches: Vec<Pair> = SLASH_COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(word))
                .map(|(cmd, desc)| Pair {
                    // display：纯文本，highlight_candidate 会加颜色
                    display: format!("{:<14} {}", cmd, desc),
                    replacement: cmd.to_string(),
                })
                .collect();

            if !cmd_matches.is_empty() {
                return Ok((0, cmd_matches));
            }

            // 不匹配任何内置命令 → 绝对路径文件系统补全（如 /home/、/etc/）
            let file_candidates = Self::complete_path(word);
            if !file_candidates.is_empty() {
                return Ok((0, file_candidates));
            }
        }

        Ok((pos, vec![]))
    }
}

impl Hinter for ChatCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if pos < line.len() || !line.starts_with('/') {
            return None;
        }
        SLASH_COMMANDS
            .iter()
            .find(|(cmd, _)| cmd.starts_with(line) && *cmd != line)
            .map(|(cmd, _)| {
                // 灰色提示后缀
                format!("\x1b[38;5;244m{}\x1b[0m", &cmd[line.len()..])
            })
    }
}

impl Highlighter for ChatCompleter {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Borrowed(hint)
    }

    /// 给补全候选项加背景色（rustyline List 模式下通过此方法渲染颜色）
    fn highlight_candidate<'c>(
        &self,
        candidate: &'c str,
        _completion: rustyline::CompletionType,
    ) -> Cow<'c, str> {
        // candidate 是 display 字段（纯文本），在这里加颜色
        if candidate.ends_with('/') {
            // 目录：蓝色 + 背景
            Cow::Owned(format!(
                "{}{}  {}{}",
                COMPLETION_BG, COMPLETION_DIR_FG, candidate, "\x1b[0m"
            ))
        } else if candidate.starts_with('/') {
            // 斜杠命令：白色命令 + 灰色描述
            let parts: Vec<&str> = candidate.splitn(2, ' ').collect();
            if parts.len() == 2 {
                Cow::Owned(format!(
                    "{}{}  {}{:<14}{}  {}{}{}\x1b[0m",
                    COMPLETION_BG, COMPLETION_FG,
                    BOLD, parts[0].trim(), RESET,
                    COMPLETION_BG, "\x1b[38;5;244m", parts[1].trim()
                ))
            } else {
                Cow::Owned(format!("{}{}  {}\x1b[0m", COMPLETION_BG, COMPLETION_FG, candidate))
            }
        } else {
            // 普通文件：白色 + 背景
            Cow::Owned(format!(
                "{}{}  {}{}",
                COMPLETION_BG, COMPLETION_FG, candidate, "\x1b[0m"
            ))
        }
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Borrowed(line)
    }
}

impl Validator for ChatCompleter {}

// ─────────────────────────────────────────────
// 交互式竖向下拉补全（crossterm + Tab 键触发）
// ─────────────────────────────────────────────

const MAX_COMPLETION_DISPLAY: usize = 10;

/// 根据当前输入 word 计算补全候选和起始字节偏移
fn compute_candidates_for_str(word: &str) -> (Vec<Pair>, usize) {
    // @ 文件路径补全
    if let Some(at_pos) = word.rfind('@') {
        let path_part = &word[at_pos + 1..];
        let candidates = ChatCompleter::complete_path(path_part);
        return (candidates, at_pos + 1);
    }
    // / 斜杠命令或绝对路径补全
    if word.starts_with('/') {
        let cmd_matches: Vec<Pair> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(word))
            .map(|(cmd, desc)| Pair {
                display: format!("{:<14} {}", cmd, desc),
                replacement: cmd.to_string(),
            })
            .collect();
        if !cmd_matches.is_empty() {
            return (cmd_matches, 0);
        }
        let file_candidates = ChatCompleter::complete_path(word);
        if !file_candidates.is_empty() {
            return (file_candidates, 0);
        }
    }
    (vec![], 0)
}

/// 渲染单个候选项（带 ANSI 颜色和选中高亮）
fn render_one_candidate(c: &Pair, selected: bool) -> String {
    let bg = if selected { "\x1b[48;5;24m" } else { "\x1b[48;5;238m" };
    let indicator = if selected { "\x1b[97m▶\x1b[0m" } else { " " };
    if c.display.ends_with('/') {
        // 目录：蓝绿色
        format!("{} {}  \x1b[38;5;117m{}\x1b[0m", bg, indicator, c.display)
    } else {
        let trimmed = c.display.trim_end();
        // 斜杠命令：display 格式 "/cmd           desc"（多空格分隔）
        if let Some(sep) = trimmed.find("   ") {
            let cmd_part = trimmed[..sep].trim();
            let desc_part = trimmed[sep..].trim();
            format!(
                "{} {}  \x1b[97m\x1b[1m{:<14}\x1b[0m{}\x1b[38;5;244m {}\x1b[0m",
                bg, indicator, cmd_part, bg, desc_part
            )
        } else {
            // 普通文件
            format!("{} {}  \x1b[38;5;255m{}\x1b[0m", bg, indicator, c.display)
        }
    }
}

/// 在终端底部渲染竖向交互式候选列表。
/// 上下键选择，Enter/Tab 确认，Esc/Ctrl+C 取消。
fn show_interactive_list(candidates: &[Pair]) -> std::io::Result<Option<String>> {
    use crossterm::cursor::{MoveTo, position as cursor_pos};
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{Clear, ClearType, size as term_size};
    use crossterm::execute;
    use std::io::{stdout, Write};

    let (term_w, term_h) = term_size()?;
    let display_count = candidates.len().min(MAX_COMPLETION_DISPLAY);
    let extra_line = usize::from(candidates.len() > display_count);
    // 上分隔线 + 候选行 + 可选 more 行 + 下分隔线
    let list_height = (display_count + 2 + extra_line) as u16;
    // 固定到终端底部，不遮挡输入行上方内容
    let list_start = term_h.saturating_sub(list_height + 1);
    let mut selected = 0usize;
    let mut out = stdout();
    let sep = "─".repeat((term_w as usize).min(54));

    // 记录输入行光标位置
    let (orig_col, orig_row) = cursor_pos()?;

    // 绘制完整候选列表
    let draw = |out: &mut std::io::Stdout, sel: usize| -> std::io::Result<()> {
        execute!(out, MoveTo(0, list_start), Clear(ClearType::CurrentLine))?;
        execute!(out, Print(format!("\x1b[38;5;244m{}\x1b[0m", sep)))?;
        for i in 0..display_count {
            execute!(out, MoveTo(0, list_start + 1 + i as u16), Clear(ClearType::CurrentLine))?;
            execute!(out, Print(render_one_candidate(&candidates[i], i == sel)))?;
        }
        if extra_line > 0 {
            execute!(out, MoveTo(0, list_start + 1 + display_count as u16), Clear(ClearType::CurrentLine))?;
            execute!(out, Print(format!(
                "\x1b[48;5;238m\x1b[38;5;244m    … {} more results\x1b[0m",
                candidates.len() - display_count
            )))?;
        }
        execute!(out, MoveTo(0, list_start + list_height - 1), Clear(ClearType::CurrentLine))?;
        execute!(out, Print(format!("\x1b[38;5;244m{}\x1b[0m", sep)))?;
        execute!(out, MoveTo(orig_col, orig_row))?;
        out.flush()
    };

    draw(&mut out, selected)?;

    // 交互循环
    let result = loop {
        match ev_read()? {
            Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. }) => {
                let old = selected;
                selected = if selected == 0 { display_count - 1 } else { selected - 1 };
                execute!(out,
                    MoveTo(0, list_start + 1 + old as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[old], false))
                )?;
                execute!(out,
                    MoveTo(0, list_start + 1 + selected as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[selected], true))
                )?;
                execute!(out, MoveTo(orig_col, orig_row))?;
                out.flush()?;
            }
            Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. }) => {
                let old = selected;
                selected = (selected + 1) % display_count;
                execute!(out,
                    MoveTo(0, list_start + 1 + old as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[old], false))
                )?;
                execute!(out,
                    MoveTo(0, list_start + 1 + selected as u16),
                    Clear(ClearType::CurrentLine),
                    Print(render_one_candidate(&candidates[selected], true))
                )?;
                execute!(out, MoveTo(orig_col, orig_row))?;
                out.flush()?;
            }
            Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. })
            | Event::Key(KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::NONE, .. }) => {
                break Some(candidates[selected].replacement.clone());
            }
            Event::Key(KeyEvent { code: KeyCode::Esc, .. })
            | Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                break None;
            }
            _ => {}
        }
    };

    // 清除候选列表
    for i in 0..list_height {
        execute!(out, MoveTo(0, list_start + i), Clear(ClearType::CurrentLine))?;
    }
    execute!(out, MoveTo(orig_col, orig_row))?;
    out.flush()?;

    Ok(result)
}

/// Tab 键的交互式补全事件处理器
struct InteractiveCompleteHandler;

impl rustyline::ConditionalEventHandler for InteractiveCompleteHandler {
    fn handle(
        &self,
        _evt: &rustyline::Event,
        _n: rustyline::RepeatCount,
        _positive: bool,
        ctx: &rustyline::EventContext<'_>,
    ) -> Option<Cmd> {
        let line = ctx.line();
        let pos = ctx.pos();
        let word = &line[..pos];

        let (candidates, byte_start) = compute_candidates_for_str(word);
        if candidates.is_empty() {
            return None; // 无候选，rustyline 默认处理
        }

        match show_interactive_list(&candidates) {
            Ok(Some(replacement)) => {
                // 计算从 byte_start 到当前光标之间的字符数（Unicode 安全）
                let chars_to_delete = line[byte_start..pos].chars().count();
                // BackwardChar(n) 向后删除 n 个字符，然后插入 replacement
                Some(Cmd::Replace(Movement::BackwardChar(chars_to_delete), Some(replacement)))
            }
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────
// crossterm 实时交互式 readline（完全替代 rustyline 的输入读取）
// 支持：实时候选下拉、输入行下方显示、上下键选择、历史记录、行编辑
// ─────────────────────────────────────────────

enum ReadLine {
    Line(String),
    Interrupted,
    Eof,
}

/// 计算字符串中可见字符列数（跳过 ANSI 转义码，正确处理 Unicode 宽字符）
fn visible_columns(s: &str) -> usize {
    let mut cols = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // 跳过 ANSI 转义序列：\x1b[ ... 字母  或  \x1b 字母
            if chars.peek() == Some(&'[') {
                chars.next(); // 消耗 '['
                // 消耗直到遇到字母（终止符）
                for inner in chars.by_ref() {
                    if inner.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                // 简单转义：\x1b + 单个字符
                chars.next();
            }
        } else {
            // 使用 unicode_width 规则：CJK 等宽字符占 2 列，其余占 1 列
            // 这里用简单规则：U+2E80..U+9FFF 及部分范围为宽字符
            let w = unicode_char_width(c);
            cols += w;
        }
    }
    cols
}

/// 返回单个 Unicode 字符的终端显示宽度（0、1 或 2）
fn unicode_char_width(c: char) -> usize {
    // 控制字符宽度为 0
    if c < ' ' || c == '\x7f' {
        return 0;
    }
    let cp = c as u32;
    // 宽字符范围（CJK、全角符号等）
    if matches!(cp,
        0x1100..=0x115F  // Hangul Jamo
        | 0x2E80..=0x303E  // CJK Radicals
        | 0x3041..=0x33BF  // Hiragana/Katakana/CJK
        | 0x33FF..=0x33FF
        | 0x3400..=0x4DBF  // CJK Extension A
        | 0x4E00..=0x9FFF  // CJK Unified
        | 0xA000..=0xA4CF  // Yi
        | 0xAC00..=0xD7AF  // Hangul Syllables
        | 0xF900..=0xFAFF  // CJK Compatibility
        | 0xFE10..=0xFE1F  // Vertical Forms
        | 0xFE30..=0xFE4F  // CJK Compatibility Forms
        | 0xFF01..=0xFF60  // Fullwidth Forms
        | 0xFFE0..=0xFFE6  // Fullwidth Signs
        | 0x1B000..=0x1B0FF // Kana Supplement
        | 0x1F004..=0x1F0CF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F // Misc Symbols & Emoticons
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD // CJK Extension B-F
        | 0x30000..=0x3FFFD
    ) {
        2
    } else {
        1
    }
}

/// 将候选项应用到输入行（chars[offset..cursor] 替换为 replacement）
fn apply_candidate(chars: &mut Vec<char>, cursor: &mut usize, candidate: &Pair, offset: usize) {
    let rep: Vec<char> = candidate.replacement.chars().collect();
    chars.drain(offset..*cursor);
    for (i, &c) in rep.iter().enumerate() {
        chars.insert(offset + i, c);
    }
    *cursor = offset + rep.len();
}

/// 根据当前输入更新候选列表
fn update_completion(
    chars: &[char],
    cursor: usize,
    candidates: &mut Vec<Pair>,
    selected: &mut Option<usize>,
    offset: &mut usize,
) {
    let word: String = chars[..cursor].iter().collect();

    // @ 文件路径补全
    if let Some(at_pos) = word.rfind('@') {
        let path_part = &word[at_pos + 1..];
        let cands = ChatCompleter::complete_path(path_part);
        if !cands.is_empty() {
            *candidates = cands;
            *selected = None;
            // offset = @ 之后的字符位置
            *offset = word[..at_pos + 1].chars().count();
            return;
        }
    }

    // / 斜杠：行首才处理
    if word.starts_with('/') {
        // 先匹配内置命令
        let cmd_matches: Vec<Pair> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(word.as_str()))
            .map(|(cmd, desc)| Pair {
                display: format!("{:<14} {}", cmd, desc),
                replacement: cmd.to_string(),
            })
            .collect();
        if !cmd_matches.is_empty() {
            *candidates = cmd_matches;
            *selected = None;
            *offset = 0;
            return;
        }
        // 无内置命令匹配 → 文件路径补全
        let file_cands = ChatCompleter::complete_path(&word);
        if !file_cands.is_empty() {
            *candidates = file_cands;
            *selected = None;
            *offset = 0;
            return;
        }
    }

    candidates.clear();
    *selected = None;
    *offset = 0;
}

/// 重绘输入行 + 候选列表（在输入行下方）
/// 返回候选区行数（含上下分隔线）
/// 计算 chars[..n] 的实际终端显示列数（中文等宽字符占2列）
fn chars_display_cols(chars: &[char], n: usize) -> usize {
    chars[..n].iter().map(|&c| unicode_char_width(c)).sum()
}

fn redraw_input_line(
    out: &mut std::io::Stdout,
    prompt: &str,
    chars: &[char],
    cursor: usize,
    candidates: &[Pair],
    selected: Option<usize>,
) -> std::io::Result<()> {
    use crossterm::cursor::MoveToColumn;
    use crossterm::cursor::MoveUp;
    use crossterm::style::Print;
    use crossterm::terminal::{Clear, ClearType, size as term_size};
    use crossterm::execute;

    let display_count = candidates.len().min(MAX_COMPLETION_DISPLAY);
    let extra = usize::from(candidates.len() > display_count);
    let cand_lines = if candidates.is_empty() { 0 } else { display_count + 2 + extra };

    // 从当前位置清除到屏幕底部（清除旧内容）
    execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;

    // 打印 prompt + 输入内容
    execute!(out, Print(prompt))?;
    let input_str: String = chars.iter().collect();
    execute!(out, Print(&input_str))?;

    // 候选列表（输入行下方）
    if !candidates.is_empty() {
        let term_w = term_size().map(|(w, _)| w as usize).unwrap_or(80);
        let sep = "─".repeat(term_w.min(54));
        execute!(out, Print(format!("\r\n\x1b[38;5;244m{}\x1b[0m", sep)))?;
        for i in 0..display_count {
            execute!(out, Print(format!(
                "\r\n{}",
                render_one_candidate(&candidates[i], Some(i) == selected)
            )))?;
        }
        if extra > 0 {
            execute!(out, Print(format!(
                "\r\n\x1b[48;5;238m\x1b[38;5;244m    … {} more results\x1b[0m",
                candidates.len() - display_count
            )))?;
        }
        execute!(out, Print(format!("\r\n\x1b[38;5;244m{}\x1b[0m", sep)))?;
        // 移回输入行
        execute!(out, MoveUp(cand_lines as u16))?;
    }

    // 移到正确的光标列
    // 注意：cursor 是字符索引，中文等宽字符占2列，必须用 chars_display_cols 计算实际列数
    let col = (visible_columns(prompt) + chars_display_cols(chars, cursor)) as u16;
    execute!(out, MoveToColumn(col))?;
    out.flush()?;
    Ok(())
}

/// 交互式读取一行输入（crossterm raw mode）
/// - 每次字符输入实时更新候选列表
/// - 候选在输入行下方竖向显示
/// - ↑↓ 方向键在候选/历史间导航，Tab 确认候选，Enter 提交
fn interactive_readline(
    prompt: &str,
    history: &mut Vec<String>,
) -> std::io::Result<ReadLine> {
    use crossterm::cursor::MoveToColumn;
    use crossterm::event::{read as ev_read, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use crossterm::execute;
    use std::io::stdout;

    enable_raw_mode()?;
    let mut out = stdout();

    let mut chars: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut history_idx: Option<usize> = None;
    let mut history_saved = String::new();
    let mut candidates: Vec<Pair> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut offset: usize = 0; // 候选起始字符偏移

    // 初始渲染 prompt
    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);

    let result = loop {
        let event = match ev_read() {
            Ok(e) => e,
            Err(e) => { let _ = disable_raw_mode(); return Err(e); }
        };

        match event {
            // ── Ctrl+C 中断 ──
            Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. }) => {
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                let _ = execute!(out, Print("\r\n"));
                break ReadLine::Interrupted;
            }
            // ── Ctrl+D EOF（仅在输入为空时） ──
            Event::Key(KeyEvent { code: KeyCode::Char('d'), modifiers: KeyModifiers::CONTROL, .. }) => {
                if chars.is_empty() {
                    let _ = execute!(out, Print("\r\n"));
                    break ReadLine::Eof;
                }
            }
            // ── Enter 提交 / 确认候选 ──
            Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE, .. }) => {
                if let Some(idx) = selected {
                    // 确认选中候选
                    apply_candidate(&mut chars, &mut cursor, &candidates[idx], offset);
                    selected = None;
                    candidates.clear();
                    offset = 0;
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                } else {
                    // 提交输入
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                    let _ = execute!(out, Print("\r\n"));
                    let line: String = chars.iter().collect();
                    break ReadLine::Line(line);
                }
            }
            // ── Tab 选择/确认候选 ──
            Event::Key(KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::NONE, .. }) => {
                if !candidates.is_empty() {
                    if let Some(idx) = selected {
                        // 确认当前选中
                        apply_candidate(&mut chars, &mut cursor, &candidates[idx], offset);
                        selected = None;
                        candidates.clear();
                        offset = 0;
                        update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    } else {
                        selected = Some(0);
                    }
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── Esc 关闭候选 ──
            Event::Key(KeyEvent { code: KeyCode::Esc, .. }) => {
                if !candidates.is_empty() {
                    candidates.clear();
                    selected = None;
                    offset = 0;
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &[], None);
                }
            }
            // ── ↑ 候选上移 / 历史向上 ──
            Event::Key(KeyEvent { code: KeyCode::Up, modifiers: KeyModifiers::NONE, .. }) => {
                if !candidates.is_empty() {
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None | Some(0) => n.saturating_sub(1),
                        Some(i) => i - 1,
                    });
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                } else if !history.is_empty() {
                    if history_idx.is_none() { history_saved = chars.iter().collect(); }
                    let idx = match history_idx {
                        None => history.len() - 1,
                        Some(0) => 0,
                        Some(i) => i - 1,
                    };
                    history_idx = Some(idx);
                    chars = history[idx].chars().collect();
                    cursor = chars.len();
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── ↓ 候选下移 / 历史向下 ──
            Event::Key(KeyEvent { code: KeyCode::Down, modifiers: KeyModifiers::NONE, .. }) => {
                if !candidates.is_empty() {
                    let n = candidates.len().min(MAX_COMPLETION_DISPLAY);
                    selected = Some(match selected {
                        None => 0,
                        Some(i) => (i + 1) % n,
                    });
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                } else if let Some(idx) = history_idx {
                    if idx + 1 < history.len() {
                        history_idx = Some(idx + 1);
                        chars = history[idx + 1].chars().collect();
                    } else {
                        history_idx = None;
                        chars = history_saved.chars().collect();
                    }
                    cursor = chars.len();
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── ← 光标左移 ──
            Event::Key(KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                    let _ = execute!(out, MoveToColumn(col));
                    let _ = out.flush();
                }
            }
            // ── → 光标右移 ──
            Event::Key(KeyEvent { code: KeyCode::Right, modifiers: KeyModifiers::NONE, .. }) => {
                if cursor < chars.len() {
                    cursor += 1;
                    let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                    let _ = execute!(out, MoveToColumn(col));
                    let _ = out.flush();
                }
            }
            // ── Ctrl+A 行首 ──
            Event::Key(KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = 0;
                let col = visible_columns(prompt) as u16;
                let _ = execute!(out, MoveToColumn(col));
                let _ = out.flush();
            }
            // ── Ctrl+E 行尾 ──
            Event::Key(KeyEvent { code: KeyCode::Char('e'), modifiers: KeyModifiers::CONTROL, .. }) => {
                cursor = chars.len();
                let col = (visible_columns(prompt) + chars_display_cols(&chars, cursor)) as u16;
                let _ = execute!(out, MoveToColumn(col));
                let _ = out.flush();
            }
            // ── Ctrl+U 清除行 ──
            Event::Key(KeyEvent { code: KeyCode::Char('u'), modifiers: KeyModifiers::CONTROL, .. }) => {
                chars.clear();
                cursor = 0;
                history_idx = None;
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
            }
            // ── Backspace ──
            Event::Key(KeyEvent { code: KeyCode::Backspace, .. }) => {
                if cursor > 0 {
                    cursor -= 1;
                    chars.remove(cursor);
                    history_idx = None;
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── Delete（前向删除） ──
            Event::Key(KeyEvent { code: KeyCode::Delete, .. }) => {
                if cursor < chars.len() {
                    chars.remove(cursor);
                    update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                    let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
                }
            }
            // ── 普通字符 ──
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            }) if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT => {
                chars.insert(cursor, c);
                cursor += 1;
                history_idx = None;
                update_completion(&chars, cursor, &mut candidates, &mut selected, &mut offset);
                let _ = redraw_input_line(&mut out, prompt, &chars, cursor, &candidates, selected);
            }
            _ => {}
        }
    };

    let _ = disable_raw_mode();
    Ok(result)
}

// ─────────────────────────────────────────────
// @ 文件注入：解析消息中的 @path，替换为文件内容
// ─────────────────────────────────────────────

/// 解析消息中所有 @path 引用，将文件/文件夹内容注入到消息末尾
/// 返回 (处理后的消息, 注入的文件数量)
fn expand_at_references(input: &str) -> (String, usize) {
    use std::path::Path;

    // 找出所有 @token（以空格或行首分隔）
    let mut paths: Vec<String> = Vec::new();
    for token in input.split_whitespace() {
        if let Some(path_str) = token.strip_prefix('@') {
            if !path_str.is_empty() {
                paths.push(path_str.to_string());
            }
        }
    }

    if paths.is_empty() {
        return (input.to_string(), 0);
    }

    let mut injected = String::new();
    let mut count = 0usize;

    for path_str in &paths {
        let path = Path::new(path_str);
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let lang = ext_to_lang(ext);
                    injected.push_str(&format!(
                        "\n\n--- File: {} ---\n```{}\n{}\n```",
                        path_str, lang, content.trim_end()
                    ));
                    count += 1;
                }
                Err(e) => {
                    injected.push_str(&format!("\n\n--- File: {} (read error: {}) ---", path_str, e));
                }
            }
        } else if path.is_dir() {
            // 文件夹：列出目录树（最多 2 层，最多 50 个文件）
            let listing = list_dir_tree(path, 2, 50);
            injected.push_str(&format!(
                "\n\n--- Directory: {} ---\n```\n{}\n```",
                path_str, listing
            ));
            count += 1;
        } else {
            injected.push_str(&format!("\n\n--- @{}: not found ---", path_str));
        }
    }

    if injected.is_empty() {
        (input.to_string(), 0)
    } else {
        (format!("{}{}", input, injected), count)
    }
}

/// 文件扩展名 → 代码块语言标识
fn ext_to_lang(ext: &str) -> &str {
    match ext {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "js" | "mjs" => "javascript",
        "ts" => "typescript",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" => "markdown",
        "sh" | "bash" => "bash",
        "c" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "java" => "java",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        "xml" => "xml",
        _ => "",
    }
}

/// 递归列出目录树，限制深度和文件数
fn list_dir_tree(dir: &std::path::Path, max_depth: usize, max_files: usize) -> String {
    let mut lines = Vec::new();
    let mut count = 0usize;
    list_dir_recursive(dir, "", max_depth, 0, &mut lines, &mut count, max_files);
    lines.join("\n")
}

fn list_dir_recursive(
    dir: &std::path::Path,
    prefix: &str,
    max_depth: usize,
    depth: usize,
    lines: &mut Vec<String>,
    count: &mut usize,
    max_files: usize,
) {
    if depth > max_depth || *count >= max_files {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    // 排序：目录优先，然后按名称
    entries.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        b_is_dir.cmp(&a_is_dir).then(a.file_name().cmp(&b.file_name()))
    });
    // 过滤隐藏文件和常见忽略目录
    let entries: Vec<_> = entries.into_iter().filter(|e| {
        let name = e.file_name();
        let name_str = name.to_string_lossy();
        !name_str.starts_with('.') && name_str != "target" && name_str != "node_modules"
    }).collect();

    let total = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if *count >= max_files {
            lines.push(format!("{}  ... (truncated)", prefix));
            break;
        }
        let is_last = i == total - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        if path.is_dir() {
            lines.push(format!("{}{}{}/", prefix, connector, name_str));
            let new_prefix = format!("{}{}  ", prefix, if is_last { " " } else { "│" });
            list_dir_recursive(&path, &new_prefix, max_depth, depth + 1, lines, count, max_files);
        } else {
            lines.push(format!("{}{}{}", prefix, connector, name_str));
            *count += 1;
        }
    }
}

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
// 代码块背景色（深灰背景 + 浅灰前景，类似 Claude Code 风格）
const CODE_BG: &str = "\x1b[48;5;236m"; // 深灰背景
const CODE_FG: &str = "\x1b[38;5;252m"; // 浅灰前景

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

    // 交互式模式：自动恢复上次 session（除非用户指定了 --session 或 --new）
    let restored_session = if args.session.is_none() {
        load_last_session_id()
    } else {
        None
    };

    // 构造带恢复 session 的 args（借用 restored_session）
    let effective_session = args.session.clone().or(restored_session.clone());

    print_welcome(&model_name, skill_count, effective_session.as_deref(), true);

    if let Some(ref sid) = restored_session {
        println!("  {}↩  Resumed session {}{}{}", GRAY, BOLD, &sid[..8.min(sid.len())], RESET);
        println!("  {}    Use /new to start a fresh conversation.{}", DIM, RESET);
        println!();

        // 显示已恢复 session 的上下文使用情况
        if let Ok(session) = ChatEngine::get_session(sid) {
            let used_chars: usize = session.turns.iter().map(|t| t.content.len()).sum();
            let used_tokens = used_chars / 4; // 粗估：4字符≈1 token
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

// ─────────────────────────────────────────────
// 欢迎界面（Numina 风格）
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
        // 命令提示
        println!("  {}Type a message to start chatting.{}", DIM, RESET);
        println!("  {}Commands:{} {}  /help  /new  /session  /sessions  /model  /skills  /quit{}", 
            DIM, RESET, GRAY, RESET);
        println!();
    }
}

/// 估算模型上下文窗口大小（k tokens），优先从 ModelsConfig 读取 max_tokens
fn estimate_context_size(_provider: &str, model: &str) -> String {
    // 优先从配置文件读取 max_tokens
    if let Ok(mc) = ModelsConfig::load() {
        if let Some(m) = mc.models.iter().find(|m| m.name == model) {
            if let Some(max_tok) = m.max_tokens {
                return format!("{}", max_tok / 1000);
            }
        }
    }
    // 按模型名称推断
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

// ─────────────────────────────────────────────
// Session 持久化记忆（last_session）
// ─────────────────────────────────────────────

/// 读取上次退出时的 session ID（~/.numina/last_session）
fn load_last_session_id() -> Option<String> {
    let path = dirs::home_dir()?.join(".numina").join("last_session");
    let sid = std::fs::read_to_string(path).ok()?.trim().to_string();
    if sid.is_empty() {
        None
    } else {
        // 验证 session 文件确实存在
        ChatEngine::get_session(&sid).ok().map(|_| sid)
    }
}

/// 保存当前 session ID 到 ~/.numina/last_session
fn save_last_session_id(sid: &str) {
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".numina")) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("last_session"), sid);
    }
}

/// 清除 last_session（/new 时调用）
fn clear_last_session_id() {
    if let Some(path) = dirs::home_dir().map(|h| h.join(".numina").join("last_session")) {
        let _ = std::fs::write(path, "");
    }
}

async fn run_interactive_with_session(
    engine: &ChatEngine,
    args: &ChatArgs,
    initial_session: Option<String>,
) -> Result<()> {
    let model_override = args.model.as_deref();
    let mut current_session: Option<String> = initial_session.clone();
    let mut turn_count = 0usize;

    // 初始化累计 token 数：从已有 session 历史读取，保证 context bar 连续显示
    let mut accumulated_tokens: usize = if let Some(ref sid) = initial_session {
        ChatEngine::get_session(sid)
            .map(|s| s.turns.iter().map(|t| t.content.len()).sum::<usize>() / 4)
            .unwrap_or(0)
    } else {
        0
    };

    // 加载历史记录（从文件读取到 Vec<String>，用于 interactive_readline）
    let history_path = dirs::home_dir()
        .map(|h| h.join(".numina").join("chat_history"))
        .unwrap_or_else(|| std::path::PathBuf::from(".numina_history"));
    let mut chat_history: Vec<String> = if let Ok(content) = std::fs::read_to_string(&history_path) {
        content.lines().filter(|s| !s.is_empty()).map(str::to_string).collect()
    } else {
        Vec::new()
    };

    loop {
        // prompt 必须是纯文本（不含 ANSI 转义码），否则 visible_columns()
        // 计算出的列数会偏大，导致光标停在错误位置（偏左）。
        // 颜色通过在 readline 之前打印一个空行来保持视觉效果。
        let prompt = "❯ ";

        let input = match interactive_readline(prompt, &mut chat_history) {
            Ok(ReadLine::Line(line)) => {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    chat_history.push(trimmed.clone());
                    // 追加保存到历史文件，防止强制退出丢失历史
                    if let Some(parent) = history_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&history_path)
                        .and_then(|mut f| {
                            use std::io::Write;
                            writeln!(f, "{}", trimmed)
                        });
                }
                trimmed
            }
            Ok(ReadLine::Interrupted) => {
                // Ctrl+C：取消当前输入，继续循环
                println!();
                continue;
            }
            Ok(ReadLine::Eof) => {
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
                accumulated_tokens = 0; // 新会话重置 context bar
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

        // 展开 @文件 引用
        let (expanded_input, at_count) = expand_at_references(input);
        if at_count > 0 {
            println!("  {}📎 Attached {} file(s){}", GRAY, at_count, RESET);
        }
        let input = expanded_input.as_str();

        // 发送消息（使用 ReAct 工具调用模式）
        match engine
            .chat_react(input, model_override, current_session.as_deref())
            .await
        {
            Ok((mut rx, sid, sent_tokens, ctx_window)) => {
                println!();

                // 等待动画：在模型思考时显示旋转动画
                // 用一个 flag channel 通知动画停止
                let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
                let spinner_handle = tokio::spawn(async move {
                    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let mut i = 0usize;
                    loop {
                        // 检查是否需要停止
                        if stop_rx.try_recv().is_ok() {
                            // 清除动画行
                            print!("\r\x1b[2K");
                            std::io::stdout().flush().ok();
                            break;
                        }
                        print!("\r  \x1b[36m{}\x1b[0m \x1b[2mthinking…\x1b[0m", frames[i % frames.len()]);
                        std::io::stdout().flush().ok();
                        i += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                    }
                });

                let mut full_response = String::new();
                let mut in_code_block = false;
                let mut line_buf = String::new();
                let mut stop_tx_opt = Some(stop_tx);
                // 用于在工具执行完毕后重新启动 thinking 动画（用 abort 停止）
                let mut thinking_task: Option<tokio::task::JoinHandle<()>> = None;

                // 停止当前 thinking 动画的辅助闭包（inline）
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
                    // 收到第一个事件时停止初始动画（只发送一次）
                    if let Some(tx) = stop_tx_opt.take() {
                        let _ = tx.send(());
                        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                        print!("\r\x1b[2K");
                        std::io::stdout().flush().ok();
                    }

                    if event == "\x00D" {
                        // 完成信号：停止任何残留的 thinking 动画
                        stop_thinking!();
                        break;
                    } else if event == "\x00W" {
                        // 工具执行完毕，等待模型下一轮响应：重新显示 thinking 动画
                        // 先停止旧的（如果有）
                        stop_thinking!();
                        // 启动新的 thinking 动画（用 abort 停止，不需要 channel）
                        let h = tokio::spawn(async {
                            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            let mut i = 0usize;
                            loop {
                                print!("\r  \x1b[36m{}\x1b[0m \x1b[2mthinking…\x1b[0m", frames[i % frames.len()]);
                                std::io::stdout().flush().ok();
                                i += 1;
                                tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
                            }
                        });
                        thinking_task = Some(h);
                    } else if let Some(tool_info) = event.strip_prefix("\x00T") {
                        // 停止 thinking 动画（如果有）
                        stop_thinking!();
                        // 工具调用事件：格式 "tool_name|params_preview"
                        let parts: Vec<&str> = tool_info.splitn(2, '|').collect();
                        let tool_name = parts.first().copied().unwrap_or("?");
                        let params = parts.get(1).copied().unwrap_or("");
                        println!();
                        if params.is_empty() {
                            println!("  {}🔧 {}{}{}…{}", GRAY, BOLD, tool_name, RESET, RESET);
                        } else {
                            println!("  {}🔧 {}{}{} {}{}{}",
                                GRAY, BOLD, tool_name, RESET,
                                DIM, params, RESET);
                        }
                        std::io::stdout().flush()?;
                    } else if let Some(result) = event.strip_prefix("\x00R") {
                        // 工具结果事件
                        let preview: String = result.chars().take(300).collect();
                        let ellipsis = if result.len() > 300 { "…" } else { "" };
                        println!("  {}┌─ result{}", GRAY, RESET);
                        for line in preview.lines() {
                            println!("  {}│{} {}", GRAY, RESET, line);
                        }
                        if !ellipsis.is_empty() {
                            println!("  {}│ (truncated…){}", GRAY, RESET);
                        }
                        println!("  {}└─{}", GRAY, RESET);
                        println!();
                        print!("{}{}Numina{} ", BOLD, CYAN, RESET);
                        std::io::stdout().flush()?;
                    } else if let Some(text) = event.strip_prefix("\x00C") {
                        // 普通文本输出（带代码块渲染）
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

                // 本轮新增 token 数（发送 + 回复）
                let this_turn_tokens = sent_tokens + full_response.len() / 4;
                // 累加到历史总量（保证 context bar 连续递增，不从 0 重置）
                accumulated_tokens += this_turn_tokens;
                print_context_bar(accumulated_tokens, ctx_window);

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
    println!("  {}      Use {}@path{} to attach a file or directory to your message.",
        GRAY, BOLD, RESET);
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
// /model 选择器（简单文本菜单，无 raw mode）
// ─────────────────────────────────────────────

/// 列出模型让用户输入编号选择，返回 Some(name) 表示已切换，None 表示取消
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

/// MCP 浏览器：列出 server，输入编号查看 tools
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
                            // 截断长描述
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
