use std::borrow::Cow;
use std::cell::RefCell;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Cmd, Context, Helper, Movement};

use super::renderer::{BOLD, RESET};

// ─────────────────────────────────────────────
// 补全器：支持 / 斜杠命令 + @ 文件路径（含隐藏文件）
// ─────────────────────────────────────────────

pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help",     "显示帮助信息"),
    ("/new",      "开始新会话"),
    ("/session",  "显示当前会话 ID"),
    ("/sessions", "列出所有历史会话"),
    ("/model",    "显示当前模型"),
    ("/mcp",      "列出已配置的 MCP 服务"),
    ("/skills",   "显示已加载的 skills"),
    ("/memory",   "列出/管理记忆"),
    ("/clear",    "清屏"),
    ("/quit",     "退出 Numina"),
];

// 线程局部变量：存储动态加载的 skill 名称和描述
thread_local! {
    static SKILL_COMMANDS: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
}

/// 注册已加载的 skills 到补全器（在 ChatEngine 初始化后调用）
pub fn register_skill_completions(skills: Vec<(String, String)>) {
    SKILL_COMMANDS.with(|sc| {
        *sc.borrow_mut() = skills;
    });
}

/// 获取所有斜杠命令（内置 + 动态 skills）
fn all_slash_commands() -> Vec<(String, String)> {
    let mut cmds: Vec<(String, String)> = SLASH_COMMANDS
        .iter()
        .map(|(cmd, desc)| (cmd.to_string(), desc.to_string()))
        .collect();

    SKILL_COMMANDS.with(|sc| {
        for (name, desc) in sc.borrow().iter() {
            cmds.push((format!("/{}", name), desc.clone()));
        }
    });

    cmds
}

// 补全列表背景色（深蓝灰，区别于聊天背景）
const COMPLETION_BG: &str = "\x1b[48;5;238m";
const COMPLETION_FG: &str = "\x1b[38;5;255m";
const COMPLETION_DIR_FG: &str = "\x1b[38;5;117m"; // 目录用蓝色

pub struct ChatCompleter;

impl ChatCompleter {
    pub fn new() -> Self { Self }

    /// 自定义文件路径补全（支持 ~ 展开、隐藏文件）
    pub fn complete_path(path_input: &str) -> Vec<Pair> {
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

        // / 斜杠处理：行首 / 时优先匹配内置命令 + skills，无匹配则回退到绝对路径文件补全
        if word.starts_with('/') {
            let all_cmds = all_slash_commands();
            let cmd_matches: Vec<Pair> = all_cmds
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(word))
                .map(|(cmd, desc)| Pair {
                    // display：纯文本，highlight_candidate 会加颜色
                    display: format!("{:<20} {}", cmd, desc),
                    replacement: cmd.clone(),
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
// Tab 键的交互式补全事件处理器
// ─────────────────────────────────────────────

/// 根据当前输入 word 计算补全候选和起始字节偏移
pub fn compute_candidates_for_str(word: &str) -> (Vec<Pair>, usize) {
    // @ 文件路径补全
    if let Some(at_pos) = word.rfind('@') {
        let path_part = &word[at_pos + 1..];
        let candidates = ChatCompleter::complete_path(path_part);
        return (candidates, at_pos + 1);
    }
    // / 斜杠命令或绝对路径补全（包含动态 skills）
    if word.starts_with('/') {
        let all_cmds = all_slash_commands();
        let cmd_matches: Vec<Pair> = all_cmds
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(word))
            .map(|(cmd, desc)| Pair {
                display: format!("{:<20} {}", cmd, desc),
                replacement: cmd.clone(),
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

pub struct InteractiveCompleteHandler;

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

        match super::readline::show_interactive_list(&candidates) {
            Ok(Some(replacement)) => {
                // 计算从 byte_start 到当前光标之间的字符数（Unicode 安全）
                let chars_to_delete = line[byte_start..pos].chars().count();
                Some(Cmd::Replace(Movement::BackwardChar(chars_to_delete), Some(replacement)))
            }
            _ => None,
        }
    }
}
